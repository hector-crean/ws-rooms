use axum::{
    Router,
    extract::{
        ConnectInfo, State,
        ws::{WebSocket, WebSocketUpgrade},
    },
    response::IntoResponse,
    routing::get,
};

use futures_util::{SinkExt, stream::StreamExt};
use serde::{Deserialize, Serialize};
use std::{net::SocketAddr, sync::Arc};
use ws_rooms::room::RoomsManager;

//https://www.shuttle.dev/launchpad/issues/2023-12-09-issue-08-websockets-chat

// Define your message type
#[derive(Clone, Serialize, Deserialize)]
struct ChatMessage {
    room: String,
    user: String,
    message: String,
}

// Create a type alias for your RoomsManager
pub type ChatManager = RoomsManager<String, String, ChatMessage>;

// +----------------+                  +----------------+                  +----------------+
// |                |                  |                |                  |                |
// |  Client        |                  |  Server        |                  |  Room          |
// |  (Browser)     |                  |  (Your Code)   |                  |  (ChatManager) |
// |                |                  |                |                  |                |
// +-------+--------+                  +-------+--------+                  +-------+--------+
//         |                                   |                                   |
//         |                                   |                                   |
//         |                                   |                                   |
//         |  WebSocket message                |                                   |
//         | (e.g. "room1:Hello")              |                                   |
//         | -------------------------->       |                                   |
//         |                                   |                                   |
//         |                                   |  Parse message                    |
//         |                                   |  Create ChatMessage               |
//         |                                   |                                   |
//         |                                   |  send_message_to_room()           |
//         |                                   | -------------------------->       |
//         |                                   |                                   |
//         |                                   |                                   |  Distribute to
//         |                                   |                                   |  all users in room
//         |                                   |                                   |
//         |                                   |  Message via user_receiver        |
//         |                                   | <--------------------------       |
//         |                                   |                                   |
//         |  JSON serialized message          |                                   |
//         | <--------------------------       |                                   |
//         |                                   |                                   |
//         |                                   |                                   |

async fn ws_handler(
    ws: WebSocketUpgrade,
    State(manager): State<Arc<ChatManager>>,
    axum::extract::Path(room_id): axum::extract::Path<String>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_socket(socket, addr, room_id, manager))
}

async fn handle_socket(
    socket: WebSocket,
    addr: SocketAddr,
    room_id: String,
    manager: Arc<ChatManager>,
) {
    // Split the socket into sender and receiver
    let mut room = manager.join_or_create(room_id.clone()).await.unwrap();

    let (mut sender, mut receiver) = socket.split();

    // FLOW 1: CLIENT -> SERVER -> ROOM
    let mut recv_task = tokio::spawn(async move {});

    // FLOW 2: ROOM -> SERVER -> CLIENT
    // Forward messages from rooms to the WebSocket (client browser)
    let mut send_task = tokio::spawn(async move {});

    // Wait for either task to finish
    tokio::select! {
        _ = &mut recv_task => send_task.abort(),
        _ = &mut send_task => recv_task.abort(),
    }

    // The UserReceiverGuard will automatically clean up when dropped
}

#[tokio::main]
async fn main() {
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
    axum::serve(listener, app).await.unwrap();
}
