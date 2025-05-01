use axum::{
    extract::{Path, State},
    response::IntoResponse,
    Json,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use crate::{api::ErrorResponse, room::{manager::RoomsManager, presence::PresenceLike, ClientIdLike, RoomIdLike}, server::ChatManager};
use crate::room::storage::{StorageLike, YrsStorage};

#[derive(Debug, Serialize, Deserialize)]
pub struct StorageDocument<Storage: StorageLike> {
    pub data: serde_json::Value,
    pub version: Storage::Version,
}

/// GET /rooms/:room_id/storage
/// Get storage document
pub async fn get_storage<RoomId: RoomIdLike, ClientId: ClientIdLike, Presence: PresenceLike, Storage: StorageLike>(
    State(manager): State<Arc<RoomsManager<RoomId, ClientId, Presence, Storage>>>,
    Path(room_id): Path<String>,
) -> impl IntoResponse {
    match manager.get_room_details(&room_id.into()).await {
        Ok(details) => {
            let storage = details.storage.clone();
            Json(StorageDocument::<Storage> {
                data: serde_json::to_value(storage.clone()).unwrap_or_default(),
                version: storage.version(),
            }).into_response()
        },
        Err(e) => (
            axum::http::StatusCode::NOT_FOUND,
            Json(ErrorResponse::new("RoomNotFound", &e.to_string())),
        ).into_response(),
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct InitializeStorageRequest {
    pub capacity: Option<usize>,
}

/// POST /rooms/:room_id/storage
/// Initialize storage document
pub async fn initialize_storage<RoomId: RoomIdLike, ClientId: ClientIdLike, Presence: PresenceLike, Storage: StorageLike>(
    State(manager): State<Arc<RoomsManager<RoomId, ClientId, Presence, Storage>>>,
    Path(room_id): Path<String>,
    Json(payload): Json<InitializeStorageRequest>,
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

/// DELETE /rooms/:room_id/storage
/// Delete storage document
pub async fn delete_storage<RoomId: RoomIdLike, ClientId: ClientIdLike, Presence: PresenceLike, Storage: StorageLike>(
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