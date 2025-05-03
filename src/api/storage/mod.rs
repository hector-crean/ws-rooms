use crate::room::error::RoomError;
use crate::room::storage::{SharedPresentation, StorageLike};
use crate::{
    api::ErrorResponse,
    room::{ClientIdLike, RoomIdLike, manager::RoomsManager, presence::PresenceLike},
};
use axum::{
    Json,
    extract::{Path, State},
    response::IntoResponse,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use ts_rs::TS;

#[derive(Debug, TS, Serialize)]
#[ts(export)]
#[ts(concrete(Storage = SharedPresentation))]
pub struct StorageDocumentResponse<Storage: StorageLike> {
    pub data: Storage,
    pub version: Storage::Version,
}

impl<'de, Storage: StorageLike> Deserialize<'de> for StorageDocumentResponse<Storage> {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let (data, version) = Deserialize::deserialize(deserializer)?;
        Ok(StorageDocumentResponse { data, version })
    }
}

/// GET /rooms/:room_id/storage
/// Get storage document
pub async fn get_storage<
    RoomId: RoomIdLike,
    ClientId: ClientIdLike,
    Presence: PresenceLike,
    Storage: StorageLike,
>(
    State(manager): State<Arc<RoomsManager<RoomId, ClientId, Presence, Storage>>>,
    Path(room_id): Path<String>,
) -> Result<Json<StorageDocumentResponse<Storage>>, RoomError> {
    match manager.get_room_details(&room_id.into()).await {
        Ok(details) => {
            let storage = details.storage.clone();
            let version = storage.version();
            let document = StorageDocumentResponse::<Storage> {
                data: storage,
                version,
            };
            Ok(Json(document))
        }
        Err(e) => Err(e),
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct InitializeStorageRequest {
    pub capacity: Option<usize>,
}

/// POST /rooms/:room_id/storage
/// Initialize storage document
pub async fn initialize_storage<
    RoomId: RoomIdLike,
    ClientId: ClientIdLike,
    Presence: PresenceLike,
    Storage: StorageLike,
>(
    State(manager): State<Arc<RoomsManager<RoomId, ClientId, Presence, Storage>>>,
    Path(room_id): Path<String>,
    Json(payload): Json<InitializeStorageRequest>,
) -> impl IntoResponse {
    match manager.ensure_room(&room_id.into(), payload.capacity).await {
        true => (
            axum::http::StatusCode::CREATED,
            Json(serde_json::json!({ "success": true })),
        )
            .into_response(),
        false => (
            axum::http::StatusCode::OK,
            Json(serde_json::json!({ "success": true })),
        )
            .into_response(),
    }
}

/// DELETE /rooms/:room_id/storage
/// Delete storage document
pub async fn delete_storage<
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
