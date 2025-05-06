use crate::{
    api::ErrorResponse,
    room::{
        ClientIdLike, RoomIdLike,
        error::RoomError,
        manager::{RoomDetails, RoomsManager},
        presence::{PresenceLike, presentation_presence::PresentationPresence},
        storage::{SharedPresentation, StorageLike},
    },
};
use axum::{
    Json,
    extract::{Path, State},
    response::IntoResponse,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use ts_rs::TS;
use uuid::Uuid;

#[derive(Debug, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct CreateRoomRequest {
    capacity: Option<usize>,
}

#[derive(Debug, Serialize, TS)]
#[ts(concrete(RoomId = String, ClientId = Uuid, Presence = PresentationPresence, Storage = SharedPresentation))]
#[ts(export)]
pub struct ListRoomsResponse<
    RoomId: RoomIdLike,
    ClientId: ClientIdLike,
    Presence: PresenceLike,
    Storage: StorageLike,
> {
    pub rooms: Vec<RoomDetails<RoomId, ClientId, Presence, Storage>>,
}

/// GET /rooms
/// List all rooms
pub async fn list_rooms<
    RoomId: RoomIdLike,
    ClientId: ClientIdLike,
    Presence: PresenceLike,
    Storage: StorageLike,
>(
    State(manager): State<Arc<RoomsManager<RoomId, ClientId, Presence, Storage>>>,
) -> impl IntoResponse {
    let rooms = manager.list_rooms_with_details().await;
    Json(ListRoomsResponse { rooms })
}

#[derive(Debug, Serialize, TS)]
#[ts(concrete(RoomId = String, ClientId = Uuid, Presence = PresentationPresence, Storage = SharedPresentation))]
#[ts(export)]
pub struct GetRoomResponse<
    RoomId: RoomIdLike,
    ClientId: ClientIdLike,
    Presence: PresenceLike,
    Storage: StorageLike,
> {
    pub room: RoomDetails<RoomId, ClientId, Presence, Storage>,
}

/// GET /rooms/:room_id
/// Get room details
pub async fn get_room<
    RoomId: RoomIdLike,
    ClientId: ClientIdLike,
    Presence: PresenceLike,
    Storage: StorageLike,
>(
    State(manager): State<Arc<RoomsManager<RoomId, ClientId, Presence, Storage>>>,
    Path(room_id): Path<String>,
) -> Result<Json<GetRoomResponse<RoomId, ClientId, Presence, Storage>>, RoomError> {
    match manager.get_room_details(&room_id.into()).await {
        Ok(details) => Ok(Json(GetRoomResponse { room: details })),
        Err(e) => Err(e),
    }
}

#[derive(Debug, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct CreateRoomResponse {
    pub success: bool,
}

/// POST /rooms/:room_id
/// Create or update a room
pub async fn create_room<
    RoomId: RoomIdLike,
    ClientId: ClientIdLike,
    Presence: PresenceLike,
    Storage: StorageLike,
>(
    State(manager): State<Arc<RoomsManager<RoomId, ClientId, Presence, Storage>>>,
    Path(room_id): Path<String>,
    Json(payload): Json<CreateRoomRequest>,
) -> Result<Json<CreateRoomResponse>, RoomError> {
    let success = manager.ensure_room(&room_id.into(), payload.capacity).await;
    match success {
        true => Ok(Json(CreateRoomResponse { success: true })),
        false => Err(RoomError::RoomAlreadyExists),
    }
}

/// DELETE /rooms/:room_id
/// Delete a room
pub async fn delete_room<
    RoomId: RoomIdLike,
    ClientId: ClientIdLike,
    Presence: PresenceLike,
    Storage: StorageLike,
>(
    State(manager): State<Arc<RoomsManager<RoomId, ClientId, Presence, Storage>>>,
    Path(room_id): Path<String>,
) -> impl IntoResponse {
    match manager.delete_room(&room_id.into()).await {
        Ok(true) => (
            axum::http::StatusCode::OK,
            Json(serde_json::json!({ "success": true })),
        )
            .into_response(),
        Ok(false) => (
            axum::http::StatusCode::NOT_FOUND,
            Json(ErrorResponse::new("RoomNotFound", "Room not found")),
        )
            .into_response(),
        Err(e) => (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse::new("InternalError", &e.to_string())),
        )
            .into_response(),
    }
}
