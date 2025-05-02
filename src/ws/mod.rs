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
        ws::{Message, Utf8Bytes, WebSocket, WebSocketUpgrade},
    },
    response::IntoResponse,
};
use futures_util::{
    SinkExt,
    StreamExt,
    stream::{SplitSink, SplitStream}, // Explicit imports for clarity
};
use std::{fmt::Display, hash::Hash, sync::Arc, time::Duration}; // Add required traits for generics
use tokio::{
    sync::Mutex,
    task::JoinHandle,          // Add JoinHandle
    time::{Instant, Interval}, // Add Interval
};
use uuid::Uuid; // Assuming ws_rooms is in scope

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
    let sub_handle = match manager.subscribe(client_id.clone(), None).await {
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

/// Handles the actual WebSocket communication for a single client (Updated for Robust Cleanup)
async fn handle_socket<RoomId, ClientId, Presence, Storage>(
    socket: WebSocket,
    manager: Arc<RoomsManager<RoomId, ClientId, Presence, Storage>>,
    mut subscription: UserSubscription<RoomId, ClientId, Presence, Storage>, // Keep the subscription handle here initially
    room_id: RoomId, // Pass the typed RoomId
) where
    RoomId: RoomIdLike + for<'a> serde::Deserialize<'a> + Send + Sync + 'static, // Add Send + Sync + 'static
    ClientId: ClientIdLike + for<'a> serde::Deserialize<'a> + Send + Sync + 'static, // Add Send + Sync + 'static
    Presence: PresenceLike + for<'a> serde::Deserialize<'a> + Send + Sync + 'static, // Add Send + Sync + 'static
    Storage: StorageLike + for<'a> serde::Deserialize<'a> + Send + Sync + 'static, // Add Send + Sync + 'static
{
    // Clone client_id and room_id for use throughout the function, especially in logging and cleanup
    let client_id = subscription.client_id().clone();
    let owned_room_id = room_id.clone(); // Clone room_id for logging/cleanup use

    tracing::info!(%client_id, room_id = %owned_room_id, "WebSocket connection established");

    let (ws_sender, ws_receiver) = socket.split();

    // Wrap sender in Arc<Mutex> for shared access between tasks
    let ws_sender = Arc::new(Mutex::new(ws_sender));

    // Track last pong received time from *this* client
    let last_pong_received = Arc::new(Mutex::new(Some(Instant::now())));

    // --- Task 1: Receive messages from the subscription and send to WebSocket ---
    let recv_task_ws_sender = Arc::clone(&ws_sender);
    let recv_task_client_id = client_id.clone();
    let recv_task_room_id = owned_room_id.clone();
    // Move the subscription into this task. It will be dropped if the task is aborted.
    let mut recv_task: JoinHandle<()> = tokio::spawn(async move {
        while let Ok(msg) = subscription.recv().await {
            let mut sender_guard = recv_task_ws_sender.lock().await;

            // Attempt to convert the ServerMessage to WebSocket Message::Text
            let msg_text = match serde_json::to_string(&msg) {
                // Assuming ServerMessageType is Serialize
                Ok(text) => text,
                Err(e) => {
                    tracing::error!(client_id = %recv_task_client_id, room_id = %recv_task_room_id, "Failed to serialize server message: {}", e);
                    continue; // Skip this message
                }
            };

            tracing::trace!(client_id = %recv_task_client_id, room_id = %recv_task_room_id, "Sending message to WebSocket: {}", msg_text);
            if sender_guard
                .send(Message::Text(msg_text.into()))
                .await
                .is_err()
            {
                tracing::warn!(client_id = %recv_task_client_id, room_id = %recv_task_room_id, "WS Send (recv_task): Failed to send message, client disconnected?");
                break; // Exit loop on send failure
            }
            // Drop lock promptly
            drop(sender_guard);
        }
        // When the loop ends (error or subscription closed), the task finishes.
        // The `subscription` owned by this task will be dropped here naturally
        // or when the task is aborted by the select! below.
        tracing::debug!(client_id = %recv_task_client_id, room_id = %recv_task_room_id, "Subscription receive task loop finished.");
    });

    // --- Task 2: Receive messages from WebSocket and send to Room Manager / Handle Pong ---
    let send_task_manager = Arc::clone(&manager);
    let send_task_last_pong = Arc::clone(&last_pong_received);
    let send_task_client_id = client_id.clone();
    let send_task_room_id = owned_room_id.clone(); // Use cloned room_id
    let mut send_task: JoinHandle<()> = tokio::spawn(async move {
        let mut receiver = ws_receiver; // Take ownership of receiver
        while let Some(msg_result) = receiver.next().await {
            match msg_result {
                Ok(msg) => {
                    match msg {
                        Message::Text(text) => {
                            // Try to parse the client message (assuming ClientMessageType is Deserialize)
                            match serde_json::from_str::<
                                ClientMessageType<RoomId, ClientId, Presence, Storage>,
                            >(&text)
                            {
                                Ok(client_msg) => {
                                    // Process the message using the manager
                                    if let Err(e) = send_task_manager
                                        .handle_client_message(&send_task_client_id, client_msg)
                                        .await
                                    {
                                        tracing::error!(client_id = %send_task_client_id, room_id = %send_task_room_id, "Failed to handle client message: {}", e);
                                        // Decide if an error here should terminate the connection
                                        // break;
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
                            // Axum tungsten automatically handles sending Pong responses to Pings.
                            // If manual handling were needed:
                            // let mut sender = send_task_ws_sender.lock().await;
                            // if sender.send(Message::Pong(ping)).await.is_err() { break; }
                        }
                        Message::Pong(_) => {
                            tracing::trace!(client_id = %send_task_client_id, room_id = %send_task_room_id, "Received Pong from client");
                            // Update the last pong time for this client
                            *send_task_last_pong.lock().await = Some(Instant::now());
                        }
                        Message::Close(close_frame) => {
                            tracing::info!(client_id = %send_task_client_id, room_id = %send_task_room_id, "Received Close frame: {:?}", close_frame);
                            break; // Exit the loop, client initiated close
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!(client_id = %send_task_client_id, room_id = %send_task_room_id, "WebSocket receive error: {}", e);
                    break; // Exit loop on receive error
                }
            }
        }
        // Loop exited (client closed, error, etc.)
        tracing::debug!(client_id = %send_task_client_id, room_id = %send_task_room_id, "WebSocket receive task loop finished.");
    });

    // --- Task 3: Server-side Heartbeat (Send Pings, Check Pongs) ---
    let heartbeat_ws_sender = Arc::clone(&ws_sender);
    let heartbeat_last_pong = Arc::clone(&last_pong_received);
    let heartbeat_client_id = client_id.clone();
    let heartbeat_room_id = owned_room_id.clone(); // Use cloned room_id
    let mut heartbeat_task: JoinHandle<Result<(), &'static str>> = tokio::spawn(async move {
        let interval_duration = Duration::from_secs(SERVER_HEARTBEAT_INTERVAL_SECONDS);
        let timeout_duration = Duration::from_secs(SERVER_HEARTBEAT_TIMEOUT_SECONDS);
        let mut interval = tokio::time::interval(interval_duration);

        loop {
            interval.tick().await; // Wait for next interval tick

            // 1. Check if last pong is too old
            let pong_deadline = Instant::now()
                .checked_sub(timeout_duration)
                .unwrap_or_else(Instant::now); // Handle potential subtraction underflow near startup
            let last_pong_time_opt = *heartbeat_last_pong.lock().await;

            if let Some(last_pong_instant) = last_pong_time_opt {
                if last_pong_instant < pong_deadline {
                    tracing::warn!(client_id = %heartbeat_client_id, room_id = %heartbeat_room_id, "Heartbeat timeout: Client Pong not received recently. Disconnecting.");
                    return Err("Heartbeat Timeout"); // Return error to signal failure
                }
            } else {
                // This case (None) might occur if the initial value wasn't set or somehow got cleared.
                // Check against the initial connection time + timeout? For simplicity, treat as timeout if None after first check.
                // Or, more robustly, check if Instant::now() > initial_connection_time + timeout_duration
                if Instant::now() > pong_deadline {
                    // Check if timeout period has passed since connection start even without a pong
                    tracing::warn!(client_id = %heartbeat_client_id, room_id = %heartbeat_room_id, "Heartbeat check: No Pong ever received and timeout expired. Disconnecting.");
                    return Err("Initial Pong Timeout");
                }
            }

            // 2. Send Ping
            tracing::trace!(client_id = %heartbeat_client_id, room_id = %heartbeat_room_id, "Sending Ping to client");
            {
                // Scope the lock guard
                let mut sender_guard = heartbeat_ws_sender.lock().await;
                // Use empty payload for standard ping
                if let Err(e) = sender_guard.send(Message::Ping(Vec::new().into())).await {
                    // No need for .into() on Vec::new()
                    tracing::warn!(client_id = %heartbeat_client_id, room_id = %heartbeat_room_id, "WS Send (heartbeat): Failed to send Ping: {}. Client likely disconnected.", e);
                    return Err("Failed to send Ping"); // Return error
                }
            } // Sender lock guard is dropped here
        }
        // Note: This loop is infinite, so it only exits via `return Err(...)`
    });

    // --- Concurrently run all tasks ---
    // Wait for *any* task to finish. If one finishes (error, disconnect, timeout),
    // abort the others and proceed to cleanup.
    tokio::select! {
        // biased; // Optional: Prioritize checking recv_task slightly if needed, but usually not necessary.
        res = &mut recv_task => {
            match res {
                Ok(()) => tracing::debug!(%client_id, room_id = %owned_room_id, "Subscription receive task completed."),
                Err(e) => tracing::error!(%client_id, room_id = %owned_room_id, "Subscription receive task panicked: {}", e),
            }
            // If recv_task finishes (normally or panic), abort the others.
            send_task.abort();
            heartbeat_task.abort();
        },
        res = &mut send_task => {
             match res {
                 Ok(()) => tracing::debug!(%client_id, room_id = %owned_room_id, "WebSocket receive task completed."),
                 Err(e) => tracing::error!(%client_id, room_id = %owned_room_id, "WebSocket receive task panicked: {}", e),
             }
            // If send_task finishes (normally or panic), abort the others.
            recv_task.abort();
            heartbeat_task.abort();
        },
        res = &mut heartbeat_task => {
            match res {
                 Ok(Ok(())) => tracing::error!(%client_id, room_id = %owned_room_id, "Heartbeat task completed unexpectedly (should loop or Err)."), // Should not happen
                 Ok(Err(reason)) => tracing::info!(%client_id, room_id = %owned_room_id, "Heartbeat task stopped: {}", reason), // Expected on timeout/error
                 Err(e) => tracing::error!(%client_id, room_id = %owned_room_id, "Heartbeat task panicked: {}", e),
             }
            // If heartbeat_task finishes (timeout, error, or panic), abort the others.
            recv_task.abort();
            send_task.abort();
        }
    }

    // --- Cleanup ---
    // This section runs *after* select! has exited because one task finished/errored,
    // and the other tasks have been signalled to abort.
    tracing::info!(%client_id, room_id = %owned_room_id, "WebSocket connection handler finishing. Cleaning up resources.");

    // 1. Attempt graceful close on the sender side (best effort)
    {
        // Scope the lock guard
        let mut sender_guard = ws_sender.lock().await;
        if let Err(e) = sender_guard.close().await {
            // This might often fail if the connection is already broken, log as debug.
            tracing::debug!(%client_id, room_id = %owned_room_id, "Ignoring error closing WebSocket sender (might be expected): {}", e);
        }
        // Sender lock guard is dropped here
    }

    // 2. Explicitly tell the manager the client is leaving THIS room.
    // This is crucial for ensuring prompt removal from the specific room state,
    // regardless of whether the UserSubscription Drop handler also runs.
    // We use the manager, client_id, and owned_room_id captured at the start.
    match manager.leave_room(&owned_room_id, &client_id).await {
        Ok(()) => {
            tracing::debug!(%client_id, room_id = %owned_room_id, "Explicitly left room during cleanup.");
        }
        Err(RoomError::UserNotInRoom(_))
        | Err(RoomError::RoomNotFound(_))
        | Err(RoomError::UserNotInitialized(_)) => {
            // These errors likely mean cleanup already happened (e.g., via UserSubscription::drop), which is acceptable.
            tracing::debug!(%client_id, room_id = %owned_room_id, "Explicit leave_room call found user/room already gone (likely cleaned up via Drop).");
        }
        Err(e) => {
            // Log other unexpected errors during cleanup
            tracing::warn!(%client_id, room_id = %owned_room_id, "Error during explicit room leave in cleanup: {}", e);
        }
    }

    // 3. The UserSubscription instance moved into `recv_task` will be dropped automatically
    //    when `recv_task` finishes or is dropped after being aborted.
    //    Its `Drop` implementation (presumably calling `manager.cleanup_user`) should handle
    //    any further general cleanup for the user across potentially *other* rooms
    //    or removing the user channel if it's the last reference.
    //    Our explicit `manager.leave_room` call above ensures *this specific room* is handled promptly.

    tracing::info!(%client_id, room_id = %owned_room_id, "Client disconnected and cleanup initiated.");
    // Function returns here.
}
