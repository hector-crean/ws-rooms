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