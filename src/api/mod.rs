use serde::{Deserialize, Serialize};

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
