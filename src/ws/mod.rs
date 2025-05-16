use crate::{
    room::{
        ClientIdLike,
        RoomIdLike,
        error::RoomError, // Make sure RoomError is in scope
        manager::RoomsManager,
        message::ClientMessageType,
        presence::PresenceLike,
        storage::StorageLike,
        subscription::UserSubscription,
    },
    // server::{ChatManager, ChatSubscription}, // Assuming these are not directly needed here
};
use axum::{
    extract::{
        Path, State,
        ws::{Message, WebSocket, WebSocketUpgrade},
    },
    response::IntoResponse,
};
use futures_util::{
    SinkExt,
    StreamExt, // Explicit imports for clarity
};
use std::{sync::Arc, time::Duration}; // Add required traits for generics
use tokio::{
    sync::Mutex,
    task::JoinHandle,          // Add JoinHandle
    time::{Instant, interval, sleep_until}, // Add Interval
};
use uuid::Uuid; // Assuming ws_rooms is in scope

mod fsm;
use fsm::{FsmManager, Event};

// --- Configuration Constants (Server-side) ---
const SERVER_HEARTBEAT_INTERVAL_SECONDS: u64 = 30; // How often server sends a Ping
const SERVER_HEARTBEAT_TIMEOUT_SECONDS: u64 = 60; // How long server waits for Pong after its Ping

/// Axum WebSocket handler (mostly unchanged)
pub async fn ws_handler<
    RoomId: RoomIdLike + for<'a> serde::Deserialize<'a> + Send + Sync + 'static, // Add Send + Sync + 'static
    ClientId: ClientIdLike + for<'a> serde::Deserialize<'a> + Send + Sync + 'static, // Add Send + Sync + 'static
    Presence: PresenceLike + for<'a> serde::Deserialize<'a> + Send + Sync + 'static, // Add Send + Sync + 'static
    Storage: StorageLike + for<'a> serde::Deserialize<'a> + Send + Sync + 'static, // Add Send + Sync + 'static
>(
    ws: WebSocketUpgrade,
    State(manager): State<Arc<RoomsManager<RoomId, ClientId, Presence, Storage>>>,
    Path(room_id_str): Path<String>, // Extract room_id as String from path
) -> impl IntoResponse {
    // Attempt to convert the room_id string into the actual RoomId type
    let room_id: RoomId = match room_id_str.clone().try_into() {
        Ok(id) => id,
        Err(_) => {
            // Handle potential conversion errors (adjust error type if needed)
            tracing::error!(room_id = %room_id_str, "Failed to parse room_id from path");
            // Consider returning a specific Axum error response here
            return RoomError::RoomNotFound(room_id_str).into_response();
        }
    };

    // Generate a ClientId (assuming Uuid -> ClientId conversion exists via From/Into)
    let client_id: ClientId = Uuid::new_v4().into();
    tracing::info!(%client_id, %room_id, "New WebSocket connection attempt");

    // Subscribe the user
    let sub_handle = match manager.subscribe(client_id, None).await {
        Ok(handle) => handle,
        Err(e) => {
            tracing::error!(%client_id, %room_id, "Failed to subscribe user: {}", e);
            return e.into_response();
        }
    };

    // Join the specific room
    // Clone room_id again as join_or_create might consume it depending on impl
    if let Err(e) = sub_handle.join_or_create(room_id.clone(), None).await {
        tracing::error!(%client_id, %room_id, "Failed to join room: {}", e);
        // Note: If join fails, the subscription handle will be dropped, triggering cleanup_user
        return e.into_response();
    }

    // Upgrade the connection and pass necessary data to the handler
    ws.on_upgrade(move |socket| handle_socket(socket, manager, sub_handle, room_id))
}

/// Handles the actual WebSocket communication for a single client with FSM integration
async fn handle_socket<RoomId, ClientId, Presence, Storage>(
    socket: WebSocket,
    manager: Arc<RoomsManager<RoomId, ClientId, Presence, Storage>>,
    mut subscription: UserSubscription<RoomId, ClientId, Presence, Storage>,
    room_id: RoomId,
) where
    RoomId: RoomIdLike + for<'a> serde::Deserialize<'a> + Send + Sync + 'static,
    ClientId: ClientIdLike + for<'a> serde::Deserialize<'a> + Send + Sync + 'static,
    Presence: PresenceLike + for<'a> serde::Deserialize<'a> + Send + Sync + 'static,
    Storage: StorageLike + for<'a> serde::Deserialize<'a> + Send + Sync + 'static,
{
    let client_id = *subscription.client_id();
    let owned_room_id = room_id.clone();

    tracing::info!(%client_id, room_id = %owned_room_id, "WebSocket connection established");

    let (ws_sender, ws_receiver) = socket.split();
    let ws_sender = Arc::new(Mutex::new(ws_sender));

    // Initialize FSM with heartbeat configuration
    let fsm = FsmManager::new(
        Duration::from_secs(SERVER_HEARTBEAT_INTERVAL_SECONDS),
        Duration::from_secs(SERVER_HEARTBEAT_TIMEOUT_SECONDS),
    );

    // Start in connecting state
    fsm.update_state(Event::Connect).await;

    // --- Task 1: Receive messages from the subscription and send to WebSocket ---
    let recv_task_ws_sender = Arc::clone(&ws_sender);
    let recv_task_client_id = client_id;
    let recv_task_room_id = owned_room_id.clone();
    let mut recv_task: JoinHandle<()> = tokio::spawn(async move {
        while let Ok(msg) = subscription.recv().await {
            let mut sender_guard = recv_task_ws_sender.lock().await;

            let msg_text = match serde_json::to_string(&msg) {
                Ok(text) => text,
                Err(e) => {
                    tracing::error!(client_id = %recv_task_client_id, room_id = %recv_task_room_id, "Failed to serialize server message: {}", e);
                    continue;
                }
            };

            tracing::trace!(client_id = %recv_task_client_id, room_id = %recv_task_room_id, "Sending message to WebSocket: {}", msg_text);
            if sender_guard.send(Message::Text(msg_text.into())).await.is_err() {
                tracing::warn!(client_id = %recv_task_client_id, room_id = %recv_task_room_id, "WS Send (recv_task): Failed to send message, client disconnected?");
                break;
            }
            drop(sender_guard);
        }
        tracing::debug!(client_id = %recv_task_client_id, room_id = %recv_task_room_id, "Subscription receive task loop finished.");
    });

    // --- Task 2: Receive messages from WebSocket and send to Room Manager / Handle Pong ---
    let send_task_manager = Arc::clone(&manager);
    let send_task_client_id = client_id;
    let send_task_room_id = owned_room_id.clone();
    let fsm_clone = fsm.clone();
    let mut send_task: JoinHandle<()> = tokio::spawn(async move {
        let mut receiver = ws_receiver;
        while let Some(msg_result) = receiver.next().await {
            match msg_result {
                Ok(msg) => {
                    match msg {
                        Message::Text(text) => {
                            match serde_json::from_str::<ClientMessageType<RoomId, ClientId, Presence, Storage>>(&text) {
                                Ok(client_msg) => {
                                    if let Err(e) = send_task_manager.handle_client_message(&send_task_client_id, client_msg).await {
                                        tracing::error!(client_id = %send_task_client_id, room_id = %send_task_room_id, "Failed to handle client message: {}", e);
                                    }
                                }
                                Err(e) => {
                                    tracing::warn!(client_id = %send_task_client_id, room_id = %send_task_room_id, "Failed to parse client message: {}. Raw: '{}'", e, text);
                                }
                            }
                        }
                        Message::Binary(bin) => {
                            tracing::warn!(client_id = %send_task_client_id, room_id = %send_task_room_id, "Received unexpected binary message ({} bytes)", bin.len());
                        }
                        Message::Ping(ping) => {
                            tracing::trace!(client_id = %send_task_client_id, room_id = %send_task_room_id, "Received Ping from client");
                        }
                        Message::Pong(_) => {
                            tracing::trace!(client_id = %send_task_client_id, room_id = %send_task_room_id, "Received Pong from client");
                            fsm_clone.handle_pong().await;
                        }
                        Message::Close(close_frame) => {
                            tracing::info!(client_id = %send_task_client_id, room_id = %send_task_room_id, "Received Close frame: {:?}", close_frame);
                            fsm_clone.update_state(Event::Close).await;
                            break;
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!(client_id = %send_task_client_id, room_id = %send_task_room_id, "WebSocket receive error: {}", e);
                    fsm_clone.update_state(Event::Failure).await;
                    break;
                }
            }
        }
        tracing::debug!(client_id = %send_task_client_id, room_id = %send_task_room_id, "WebSocket receive task loop finished.");
    });

    // --- Task 3: FSM-driven Heartbeat and State Management ---
    let heartbeat_ws_sender = Arc::clone(&ws_sender);
    let heartbeat_client_id = client_id;
    let heartbeat_room_id = owned_room_id.clone();
    let mut heartbeat_task: JoinHandle<()> = tokio::spawn(async move {
        let mut interval = interval(Duration::from_secs(SERVER_HEARTBEAT_INTERVAL_SECONDS));

        loop {
            tokio::select! {
                _ = interval.tick() => {
                    if !fsm.check_heartbeat().await {
                        tracing::warn!(client_id = %heartbeat_client_id, room_id = %heartbeat_room_id, "Heartbeat check failed");
                        break;
                    }
                    fsm.update_state(Event::AfterKeepaliveInterval).await;
                }
                _ = async {
                    if let Some(deadline) = fsm.needs_backoff().await {
                        sleep_until(deadline).await;
                    }
                }, if fsm.needs_backoff().await.is_some() => {
                    fsm.update_state(Event::AfterBackoffDelay).await;
                }
            }

            // Handle state-specific actions
            if fsm.is_awaiting_pong().await {
                let mut sender_guard = heartbeat_ws_sender.lock().await;
                if sender_guard.send(Message::Ping(Vec::new().into())).await.is_err() {
                    tracing::warn!(client_id = %heartbeat_client_id, room_id = %heartbeat_room_id, "Failed to send ping");
                    break;
                }
            }

            if fsm.is_failed().await {
                let mut sender_guard = heartbeat_ws_sender.lock().await;
                let _ = sender_guard.send(Message::Close(None)).await;
                break;
            }
        }
    });

    // --- Wait for any task to complete ---
    tokio::select! {
        res = &mut recv_task => {
            match res {
                Ok(()) => tracing::debug!(%client_id, room_id = %owned_room_id, "Subscription receive task completed."),
                Err(e) => tracing::error!(%client_id, room_id = %owned_room_id, "Subscription receive task panicked: {}", e),
            }
            send_task.abort();
            heartbeat_task.abort();
        }
        res = &mut send_task => {
            match res {
                Ok(()) => tracing::debug!(%client_id, room_id = %owned_room_id, "WebSocket receive task completed."),
                Err(e) => tracing::error!(%client_id, room_id = %owned_room_id, "WebSocket receive task panicked: {}", e),
            }
            recv_task.abort();
            heartbeat_task.abort();
        }
        res = &mut heartbeat_task => {
            match res {
                Ok(()) => tracing::debug!(%client_id, room_id = %owned_room_id, "Heartbeat task completed."),
                Err(e) => tracing::error!(%client_id, room_id = %owned_room_id, "Heartbeat task panicked: {}", e),
            }
            recv_task.abort();
            send_task.abort();
        }
    }

    // --- Cleanup ---
    tracing::info!(%client_id, room_id = %owned_room_id, "WebSocket connection handler finishing. Cleaning up resources.");

    // Attempt graceful close
    {
        let mut sender_guard = ws_sender.lock().await;
        if let Err(e) = sender_guard.close().await {
            tracing::debug!(%client_id, room_id = %owned_room_id, "Ignoring error closing WebSocket sender: {}", e);
        }
    }

    // Explicitly leave the room
    match manager.leave_room(&owned_room_id, &client_id).await {
        Ok(()) => {
            tracing::debug!(%client_id, room_id = %owned_room_id, "Explicitly left room during cleanup.");
        }
        Err(RoomError::UserNotInRoom(_))
        | Err(RoomError::RoomNotFound(_))
        | Err(RoomError::UserNotInitialized(_)) => {
            tracing::debug!(%client_id, room_id = %owned_room_id, "Explicit leave_room call found user/room already gone.");
        }
        Err(e) => {
            tracing::warn!(%client_id, room_id = %owned_room_id, "Error during explicit room leave in cleanup: {}", e);
        }
    }

    tracing::info!(%client_id, room_id = %owned_room_id, "Client disconnected and cleanup initiated.");
}
