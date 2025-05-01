use axum::{
    extract::{Path, State},
    response::IntoResponse,
    Json,
};
use serde::{Deserialize, Serialize};
use ts_rs::TS;
use uuid::Uuid;
use std::sync::Arc;
use crate::{api::ErrorResponse, room::{manager::{RoomDetails, RoomsManager}, presence::{cursor_presence::CursorPresence, PresenceLike}, storage::{SharedList, StorageLike}, ClientIdLike, RoomIdLike}, server::{ChatManager, ClientId, RoomId}};



#[derive(Debug, Serialize, Deserialize)]
pub struct CreateRoomRequest {
    capacity: Option<usize>
}





#[derive(Debug, Serialize, TS)]
#[ts(concrete(RoomId = String, ClientId = Uuid, Presence = CursorPresence, Storage = SharedList<String>))]
#[ts(export)]
pub struct ListRoomsResponse<RoomId: RoomIdLike, ClientId: ClientIdLike, Presence: PresenceLike, Storage: StorageLike> {
    pub rooms: Vec<RoomDetails<RoomId, ClientId, Presence, Storage>>
}

/// GET /rooms
/// List all rooms
pub async fn list_rooms<RoomId: RoomIdLike, ClientId: ClientIdLike, Presence: PresenceLike, Storage: StorageLike>(
    State(manager): State<Arc<RoomsManager<RoomId, ClientId, Presence, Storage>>>,
) -> impl IntoResponse {
    let rooms = manager.list_rooms_with_details().await;
    Json(ListRoomsResponse { rooms })
}



#[derive(Debug, Serialize, TS)]
#[ts(concrete(RoomId = String, ClientId = Uuid, Presence = CursorPresence, Storage = SharedList<String>))]
#[ts(export)]
pub struct GetRoomResponse<RoomId: RoomIdLike, ClientId: ClientIdLike, Presence: PresenceLike, Storage: StorageLike> {
    pub room: RoomDetails<RoomId, ClientId, Presence, Storage>
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