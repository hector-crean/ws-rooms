use axum::response::IntoResponse;

use super::{presence::PresenceError, storage::StorageError};

#[derive(thiserror::Error, Debug)]
pub enum RoomError {
    #[error("Target room '{0}' not found")]
    RoomNotFound(String),
    #[error("User '{0}' has not been initialized or is already disconnected")]
    UserNotInitialized(String),
    #[error("User '{0}' is already subscribed")]
    UserAlreadySubscribed(String),
    #[error("Failed to send message: {0}")]
    MessageSendFail(#[from] Box<dyn std::error::Error + Send + Sync>),
    #[error("Internal lock poisoned: {0}")]
    LockPoisoned(String),
    #[error("Failed to forward message to user channel")]
    UserSendFail,
    #[error("Invalid message")]
    InvalidMessage,
    #[error("User '{0}' is already in room '{1}'")]
    UserAlreadyInRoom(String, String),
    #[error("User '{0}' is not in room")]
    UserNotInRoom(String),
    #[error("Presence error: {0}")]
    PresenceError(#[from] PresenceError),
    #[error("Storage error: {0}")]
    StorageError(#[from] StorageError),
}

impl IntoResponse for RoomError {
    fn into_response(self) -> axum::response::Response {
        match self {
            Self::RoomNotFound(room_id) => (axum::http::StatusCode::NOT_FOUND, format!("Room '{}' not found", room_id)).into_response(),
            Self::UserNotInitialized(user_id) => (axum::http::StatusCode::UNAUTHORIZED, format!("User '{}' not initialized", user_id)).into_response(),
            Self::UserAlreadySubscribed(user_id) => (axum::http::StatusCode::CONFLICT, format!("User '{}' already subscribed", user_id)).into_response(),
            Self::LockPoisoned(room_id) => (axum::http::StatusCode::INTERNAL_SERVER_ERROR, format!("Internal lock poisoned in room '{}'", room_id)).into_response(),
            Self::UserSendFail => (axum::http::StatusCode::INTERNAL_SERVER_ERROR, "Failed to send message to user channel").into_response(),
            Self::InvalidMessage => (axum::http::StatusCode::BAD_REQUEST, "Invalid message").into_response(),
            Self::UserAlreadyInRoom(room_id, user_id) => (axum::http::StatusCode::CONFLICT, format!("User '{}' already in room '{}'", user_id, room_id)).into_response(),
            Self::UserNotInRoom(user_id) => (axum::http::StatusCode::NOT_FOUND, format!("User '{}' not in room", user_id)).into_response(),
            Self::PresenceError(presence_error) => (axum::http::StatusCode::BAD_REQUEST, format!("Presence error: {}", presence_error)).into_response(),
            Self::StorageError(storage_error) => (axum::http::StatusCode::INTERNAL_SERVER_ERROR, format!("Storage error: {}", storage_error)).into_response(),
            _ => (axum::http::StatusCode::INTERNAL_SERVER_ERROR, "Internal server error").into_response(),
        }
    }
}
