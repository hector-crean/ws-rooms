use crate::room::{manager::RoomsManager, presence::cursor_presence::CursorPresence};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub mod presence;
pub mod rooms;
pub mod storage;

#[derive(Debug, Serialize, Deserialize)]
pub struct ErrorResponse {
    error: String,
    message: String,
}

impl ErrorResponse {
    pub fn new(error: &str, message: &str) -> Self {
        Self {
            error: error.to_string(),
            message: message.to_string(),
        }
    }
}
