use axum::{
    extract::{Path, State},
    response::IntoResponse,
    Json,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use crate::{api::ErrorResponse, room::{manager::RoomsManager, storage::StorageLike, ClientIdLike, RoomIdLike}, server::ChatManager};
use crate::room::presence::PresenceLike;



#[derive(Debug, Serialize, Deserialize)]
pub struct PresenceResponse {
    pub users: Vec<ClientPresence>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ClientPresence {
    pub client_id: String,
    pub presence: serde_json::Value,
    pub last_updated: DateTime<Utc>,
}



/// GET /rooms/:room_id/presence
/// Get presence information for all users in a room
pub async fn get_presence<RoomId: RoomIdLike, ClientId: ClientIdLike, Presence: PresenceLike, Storage: StorageLike>(
    State(manager): State<Arc<RoomsManager<RoomId, ClientId, Presence, Storage>>>,
    Path(room_id): Path<String>,
) -> impl IntoResponse {
    match manager.get_room_details(&room_id.into()).await {
        Ok(details) => {
            let users = details.client_presences
                .into_iter()
                .map(|(client_id, presence)| ClientPresence {
                    client_id: client_id.to_string(),
                    presence: serde_json::to_value(presence).unwrap_or_default(),
                    last_updated: DateTime::<Utc>::from(std::time::SystemTime::now()),
                })
                .collect();

            Json(PresenceResponse { users }).into_response()
        },
        Err(e) => (
            axum::http::StatusCode::NOT_FOUND,
            Json(ErrorResponse::new("RoomNotFound", &e.to_string())),
        ).into_response(),
    }
} 