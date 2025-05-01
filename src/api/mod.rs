use axum::{
    extract::{Path, State},
    response::IntoResponse,
    routing::{get, post, delete},
    Router,
    Json,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use uuid::Uuid;
use crate::room::{
    manager::RoomsManager,
    presence::cursor_presence::CursorPresence,
    storage::shared_list::SharedList,
};

pub mod rooms;
pub mod storage;
pub mod presence;

type ClientId = Uuid;
type RoomId = String;
type ChatManager = RoomsManager<RoomId, ClientId, CursorPresence, SharedList<String>>;

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


