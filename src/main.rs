use axum::{
    Router,
    extract::{
        Path, State,
        ws::{Message, Utf8Bytes, WebSocket, WebSocketUpgrade},
    },
    response::IntoResponse,
    routing::get,
};
use futures_util::{
    SinkExt,
    StreamExt,
    stream::{SplitSink, SplitStream}, // Add SplitSink/SplitStream
};
use serde::{Deserialize, Serialize};
use std::{sync::Arc, time::Duration}; // Add Duration
use tokio::{
    sync::Mutex,               // Add tokio::sync::Mutex
    time::{Instant, Interval}, // Add Instant, Interval
};
use uuid::Uuid;
use ws_rooms::room::{message::ServerMessageType, presence::{cursor_presence::CursorPresence}, storage::{shared_list::SharedList}, RoomsManager, UserSubscription}; // Assuming ws_rooms is in scope



// --- Configuration Constants (Server-side) ---
const SERVER_HEARTBEAT_INTERVAL_SECONDS: u64 = 30; // How often server sends a Ping
const SERVER_HEARTBEAT_TIMEOUT_SECONDS: u64 = 60; // How long server waits for Pong after its Ping



type ClientId = Uuid;
type RoomId = String;




// Your existing ChatManager/ChatSubscription types
type ChatManager = RoomsManager<RoomId, ClientId, ServerMessageType<RoomId, ClientId>, CursorPresence, SharedList<String>>;
type ChatSubscription = UserSubscription<RoomId, ClientId, ServerMessageType<RoomId, ClientId>, CursorPresence, SharedList<String>>;

/// Axum WebSocket handler (mostly unchanged)
async fn ws_handler(
    ws: WebSocketUpgrade,
    State(manager): State<Arc<ChatManager>>,
    Path(room_id): Path<String>, // Extract room_id from path
) -> impl IntoResponse {
    let client_id = Uuid::new_v4();
    tracing::info!(%client_id, %room_id, "New WebSocket connection attempt");

    let sub_result = manager.subscribe(client_id, None).await;
    let sub_handle = match sub_result {
        Ok(handle) => handle,
        Err(e) => {
            tracing::error!(%client_id, %room_id, "Failed to subscribe user: {}", e);
            return axum::http::StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };

    if let Err(e) = sub_handle.join_or_create(room_id.clone(), None).await {
        tracing::error!(%client_id, %room_id, "Failed to join room: {}", e);
        // Decide if failure to join should prevent upgrade or not
        // return axum::http::StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }

    ws.on_upgrade(move |socket| handle_socket(socket, manager, sub_handle, room_id))
}

/// Handles the actual WebSocket communication for a single client (Updated for Heartbeat)
async fn handle_socket(
    socket: WebSocket,
    manager: Arc<ChatManager>,
    mut subscription: ChatSubscription, // Takes ownership
    room_id: String,
) {
    let client_id = *subscription.client_id();
    tracing::info!(%client_id, %room_id, "WebSocket connection established");

    let (ws_sender, ws_receiver) = socket.split();

    // Wrap sender in Arc<Mutex> for shared access between tasks
    let ws_sender = Arc::new(Mutex::new(ws_sender));

    // Track last pong received time from *this* client
    let last_pong_received = Arc::new(Mutex::new(Some(Instant::now())));

    let send_task_room_id = room_id.clone();
    let heartbeat_room_id = room_id.clone();
    let final_sub_room_id = room_id.clone();

    // --- Task 1: Receive messages from the subscription and send to WebSocket ---
    let recv_task_ws_sender = ws_sender.clone(); // Clone Arc for task
    let mut recv_task = tokio::spawn(async move {
        while let Ok(msg) = subscription.recv().await {
            let mut sender = recv_task_ws_sender.lock().await;

            let msg_bytes: Result<Utf8Bytes, _> = msg.try_into();
            match msg_bytes {
                Ok(msg) => {
                    tracing::trace!(%client_id, %room_id, "Sending message to WebSocket: {}", msg);
                    if sender.send(Message::Text(msg.into())).await.is_err() {
                        tracing::warn!(%client_id, %room_id, "WS Send (recv_task): Failed to send message, client disconnected?");
                        break;
                    }
                }
                Err(e) => {
                    tracing::error!(%client_id, %room_id, "Failed to convert message to Utf8Bytes: {}", e);
                }
            }

            // Drop lock promptly
            drop(sender);
        }
        subscription // Return subscription handle on exit
    });

    // --- Task 2: Receive messages from WebSocket and send to Room Manager / Handle Pong ---
    let send_task_manager = manager.clone();
    let send_task_last_pong = last_pong_received.clone(); // Clone Arc for task
    let mut send_task = tokio::spawn(async move {
        let mut receiver = ws_receiver; // Take ownership of receiver
        while let Some(Ok(msg)) = receiver.next().await {
            match msg {
                Message::Text(utf8_bytes) => {

                    let msg = ServerMessageType::<RoomId, ClientId>::try_from(utf8_bytes);

                    match msg {
                        Ok(msg) => {
                            tracing::debug!(%client_id, %send_task_room_id, "Received text: {:?}", msg);
                            if let Err(e) = send_task_manager
                                .send_message_to_room(&send_task_room_id, msg)
                                .await
                            {
                                tracing::error!(%client_id, %send_task_room_id, "Failed to send message to room: {}", e);
                            }
                        }
                        Err(e) => {
                            tracing::error!(%client_id, %send_task_room_id, "Failed to convert message to ServerMessageType: {}", e);
                        }
                    }


                   
                }
                Message::Binary(bin) => {
                    tracing::warn!(%client_id, %send_task_room_id, "Received unexpected binary message ({} bytes)", bin.len());
                }
                Message::Ping(ping) => {
                    tracing::trace!(%client_id, %send_task_room_id, "Received Ping from client");
                    // Axum typically handles sending Pong automatically. If explicit needed:
                    // let mut sender = send_task_ws_sender.lock().await;
                    // if sender.send(Message::Pong(ping)).await.is_err() { break; }
                }
                Message::Pong(_) => {
                    tracing::trace!(%client_id, %send_task_room_id, "Received Pong from client");
                    // Update the last pong time for this client
                    *send_task_last_pong.lock().await = Some(Instant::now());
                }
                Message::Close(close_frame) => {
                    tracing::info!(%client_id, %send_task_room_id, "Received Close frame: {:?}", close_frame);
                    break; // Exit the loop, client initiated close
                }
            }
        }
        // Loop exited, receiver is closed or errored
    });

    // --- Task 3: Server-side Heartbeat (Send Pings, Check Pongs) ---
    let heartbeat_ws_sender = ws_sender.clone(); // Clone Arc for task
    let heartbeat_last_pong = last_pong_received.clone(); // Clone Arc for task
    let mut heartbeat_task = tokio::spawn(async move {
        let interval_duration = Duration::from_secs(SERVER_HEARTBEAT_INTERVAL_SECONDS);
        let timeout_duration = Duration::from_secs(SERVER_HEARTBEAT_TIMEOUT_SECONDS);
        let mut interval = tokio::time::interval(interval_duration);

        loop {
            interval.tick().await; // Wait for next interval tick

            // 1. Check if last pong is too old
            let pong_deadline = Instant::now() - timeout_duration;
            let last_pong_time = *heartbeat_last_pong.lock().await;

            if let Some(last_pong) = last_pong_time {
                if last_pong < pong_deadline {
                    tracing::warn!(%client_id, %heartbeat_room_id, "Heartbeat timeout: Client Pong not received recently. Disconnecting.");
                    // Signal disconnection by breaking the loop. The main select! will handle cleanup.
                    return Err("Heartbeat Timeout"); // Return error to signal failure
                }
            } else {
                // Should not happen if initialized correctly, but handle defensively
                tracing::warn!(%client_id, %heartbeat_room_id, "Heartbeat check: No Pong ever received. Disconnecting.");
                return Err("No Pong Received");
            }

            // 2. Send Ping
            tracing::trace!(%client_id, %heartbeat_room_id, "Sending Ping to client");
            let mut sender = heartbeat_ws_sender.lock().await;
            // Use empty payload for standard ping
            if let Err(e) = sender.send(Message::Ping(Vec::new().into())).await {
                tracing::warn!(%client_id, %heartbeat_room_id, "WS Send (heartbeat): Failed to send Ping: {}. Client likely disconnected.", e);
                return Err("Failed to send Ping"); // Return error
            }
            drop(sender); // Drop lock
        }
    });

    // --- Concurrently run all tasks ---
    // Wait for *any* task to finish. If one finishes (error, disconnect, timeout),
    // abort the others and clean up.
    let mut final_sub_handle: Option<ChatSubscription> = None;

    tokio::select! {
        res = &mut recv_task => {
            match res {
                Ok(sub_handle_returned) => {
                    tracing::debug!(%client_id, %final_sub_room_id, "Subscription receive task finished normally.");
                    final_sub_handle = Some(sub_handle_returned); // Keep handle for final drop
                }
                Err(e) => tracing::error!(%client_id, %final_sub_room_id, "Subscription receive task panicked: {}", e),
            }
            send_task.abort();
            heartbeat_task.abort();
        },
        res = &mut send_task => {
             match res {
                 Ok(_) => tracing::debug!(%client_id, %final_sub_room_id, "WebSocket receive task finished."),
                 Err(e) => tracing::error!(%client_id, %final_sub_room_id, "WebSocket receive task panicked: {}", e),
             }
            recv_task.abort();
            heartbeat_task.abort();
        },
        res = &mut heartbeat_task => {
            match res {
                 Ok(Ok(())) => tracing::debug!(%client_id, %final_sub_room_id, "Heartbeat task finished unexpectedly (should loop)."), // Should not happen normally
                 Ok(Err(reason)) => tracing::info!(%client_id, %final_sub_room_id, "Heartbeat task stopped: {}", reason), // Expected on timeout/error
                 Err(e) => tracing::error!(%client_id, %final_sub_room_id, "Heartbeat task panicked: {}", e),
             }
            recv_task.abort();
            send_task.abort();
        }
    }

    // --- Cleanup ---
    tracing::info!(%client_id, %final_sub_room_id, "WebSocket connection handler finishing. Cleaning up subscription.");

    // Attempt graceful close on the sender side
    let mut sender = ws_sender.lock().await;
    if let Err(e) = sender.close().await {
        tracing::warn!(%client_id, %final_sub_room_id, "Error closing WebSocket sender: {}", e);
    }
    drop(sender);

    // Dropping `final_sub_handle` (if it was successfully retrieved) or the original `subscription`
    // (if recv_task panicked before returning it) automatically handles leaving rooms via ws-rooms Drop impl.
    // If recv_task panicked, `subscription` is still in scope and will be dropped here.
    // If recv_task finished normally, `final_sub_handle` holds the subscription and it will be dropped.
    // This ensures cleanup happens regardless of which task finished first.
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::DEBUG) // Use DEBUG for heartbeat traces
        .init();

    // Create the room manager
    let manager = Arc::new(ChatManager::new());

    // Create Axum router
    let app = Router::new()
        .route("/ws/room/{:room_id}", get(ws_handler)) 
        .with_state(manager);

    // Start the server
    let listener = tokio::net::TcpListener::bind("127.0.0.1:3000")
        .await
        .unwrap();
    tracing::info!("Server listening on 127.0.0.1:3000");
    axum::serve(listener, app).await.unwrap();
}
