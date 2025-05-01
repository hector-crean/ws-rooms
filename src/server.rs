use axum::{
    http::{HeaderName, Method}, routing::{delete, get, post}, Router
};
use tracing::{info, error};
use std::sync::Arc; // Add Duration
use uuid::Uuid;
use crate::{api, room::{
    manager::RoomsManager, presence::cursor_presence::CursorPresence, storage::shared_list::SharedList, subscription::UserSubscription
}, ws::ws_handler}; // Assuming ws_rooms is in scope
use std::net::SocketAddr;
use tower_http::cors::{Any, CorsLayer};


pub type ClientId = Uuid;
pub type RoomId = String;

// Your existing ChatManager/ChatSubscription types
pub type ChatManager = RoomsManager<RoomId, ClientId, CursorPresence, SharedList<String>>;
pub type ChatSubscription = UserSubscription<RoomId, ClientId, CursorPresence, SharedList<String>>;



pub struct App {
    pub manager: Arc<ChatManager>,
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}

impl App {
    pub fn new() -> Self {
        let manager = Arc::new(ChatManager::new());
        Self { manager }
    }
    pub async fn run(&self, port: u16) -> Result<(), Box<dyn std::error::Error>> {


        let cors = CorsLayer::new()
        // allow `GET` and `POST` when accessing the resource
        .allow_methods([Method::GET, Method::POST])
        // allow the Content-Type header
        .allow_headers([HeaderName::from_static("content-type")])
        // allow requests from any origin
        .allow_origin(Any);


        
    
        let app = Router::new()
        // WebSocket route
        .route("/ws/room/{:room_id}", get(ws_handler))
        // Room management routes
        .route("/api/rooms", get(api::rooms::list_rooms))
        .route("/api/rooms/{:room_id}", get(api::rooms::get_room))
        .route("/api/rooms/{:room_id}", post(api::rooms::create_room))
        .route("/api/rooms/{:room_id}", delete(api::rooms::delete_room))
        .route("/api/rooms/{:room_id}/storage", get(api::storage::get_storage))
        .route("/api/rooms/{:room_id}/storage", post(api::storage::initialize_storage))
        .route("/api/rooms/{:room_id}/storage", delete(api::storage::delete_storage))
        .route("/api/rooms/{:room_id}/presence", get(api::presence::get_presence))
        .layer(cors)
        .with_state(self.manager.clone());

        let addr: SocketAddr = SocketAddr::from(([0, 0, 0, 0], port));
        // let addr: SocketAddr = SocketAddr::from(([127, 0, 0, 1], port));

        let listener = tokio::net::TcpListener::bind(addr).await?;

        info!("Server running on http://{}", addr);
        match axum::serve(listener, app).await {
            Ok(_) => info!("Server shut down gracefully"),
            Err(e) => error!("Server error: {}", e),
        }

        Ok(())

    }
}

