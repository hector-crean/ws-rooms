use axum::{
    extract::{Path, State},
    response::IntoResponse,
    Json,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use crate::{api::ErrorResponse, room::{manager::RoomsManager, presence::PresenceLike, storage::StorageLike, ClientIdLike, RoomIdLike}, server::{ChatManager, ClientId, RoomId}};

#[derive(Debug, Serialize, Deserialize)]
pub struct RoomDetails {
    pub id: RoomId,
    pub created_at: u128,
    pub last_connection_at: u128,
    pub metadata: Option<serde_json::Value>,
    pub users: Vec<ClientId>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CreateRoomRequest {
    capacity: Option<usize>
}



/// GET /rooms
/// List all rooms
pub async fn list_rooms<RoomId: RoomIdLike, ClientId: ClientIdLike, Presence: PresenceLike, Storage: StorageLike>(
    State(manager): State<Arc<RoomsManager<RoomId, ClientId, Presence, Storage>>>,
) -> impl IntoResponse {
    let rooms = manager.list_rooms_with_details().await;
    Json(rooms)
}

/// GET /rooms/:room_id
/// Get room details
pub async fn get_room<RoomId: RoomIdLike, ClientId: ClientIdLike, Presence: PresenceLike, Storage: StorageLike>(
    State(manager): State<Arc<RoomsManager<RoomId, ClientId, Presence, Storage>>>,
    Path(room_id): Path<String>,
) -> impl IntoResponse {
    match manager.get_room_details(&room_id.into()).await {
        Ok(details) => Json(details).into_response(),
        Err(e) => (
            axum::http::StatusCode::NOT_FOUND,
            Json(ErrorResponse::new("RoomNotFound", &e.to_string())),
        ).into_response(),
    }
}

/// POST /rooms/:room_id
/// Create or update a room
pub async fn create_room<RoomId: RoomIdLike, ClientId: ClientIdLike, Presence: PresenceLike, Storage: StorageLike>(
    State(manager): State<Arc<RoomsManager<RoomId, ClientId, Presence, Storage>>>,
    Path(room_id): Path<String>,
    Json(payload): Json<CreateRoomRequest>,
) -> impl IntoResponse {
    match manager.ensure_room(&room_id.into(), payload.capacity).await {
        true => (
            axum::http::StatusCode::CREATED,
            Json(serde_json::json!({ "success": true })),
        ).into_response(),
        false => (
            axum::http::StatusCode::OK,
            Json(serde_json::json!({ "success": true })),
        ).into_response(),
    }
}

/// DELETE /rooms/:room_id
/// Delete a room
pub async fn delete_room<RoomId: RoomIdLike, ClientId: ClientIdLike, Presence: PresenceLike, Storage: StorageLike>(
    State(manager): State<Arc<RoomsManager<RoomId, ClientId, Presence, Storage>>>,
    Path(room_id): Path<String>,
) -> impl IntoResponse {
    match manager.delete_room(&room_id.into()).await {
        Ok(true) => (
            axum::http::StatusCode::OK,
            Json(serde_json::json!({ "success": true })),
        ).into_response(),
        Ok(false) => (
            axum::http::StatusCode::NOT_FOUND,
            Json(ErrorResponse::new("RoomNotFound", "Room not found")),
        ).into_response(),
        Err(e) => (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse::new("InternalError", &e.to_string())),
        ).into_response(),
    }
} 