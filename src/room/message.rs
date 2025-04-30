use axum::{
    Router,
    extract::{
        Path, State,
        ws::{Message, Utf8Bytes, WebSocket, WebSocketUpgrade},
    },
    response::IntoResponse,
    routing::get,
};
use futures_util::{
    SinkExt,
    StreamExt,
    stream::{SplitSink, SplitStream}, // Add SplitSink/SplitStream
};
use serde::{Deserialize, Serialize};
use std::{sync::Arc, time::Duration}; // Add Duration
use tokio::{
    sync::Mutex,               // Add tokio::sync::Mutex
    time::{Instant, Interval}, // Add Instant, Interval
};




#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ServerMessageType<RoomId, ClientId> {
    RoomCreated {
        room_id: RoomId,
    },
    RoomDeleted {
        room_id: RoomId,
    },
    RoomJoined {
        room_id: RoomId,
        client_id: ClientId,
    },
    RoomLeft {
        room_id: RoomId,
        client_id: ClientId,
    },
    StorageUpdated,
    CommentCreated,
    CommentEdited,
    CommentDeleted,
    CommentReactionAdded,
    CommentReactionRemoved,
    ThreadCreated,
    ThreadDeleted,
    ThreadMetadataUpdated,
    Notification,
    // event types associated with a specific room type. Liveblocks has quite an interesting pattern of
    // just sating stoage updated, which is just a prompt for the client to fetch the latest state.
}

impl<RoomId: Serialize, ClientId: Serialize> TryInto<Utf8Bytes> for ServerMessageType<RoomId, ClientId> {
    type Error = serde_json::Error;
    fn try_into(self) -> Result<Utf8Bytes, Self::Error> {
        serde_json::to_string(&self).map(Utf8Bytes::from)   
    }
}

impl<RoomId: for<'a> Deserialize<'a>, ClientId: for<'a> Deserialize<'a>> TryFrom<Utf8Bytes> for ServerMessageType<RoomId, ClientId> {
    type Error = serde_json::Error;
    fn try_from(bytes: Utf8Bytes) -> Result<Self, Self::Error> {
        serde_json::from_str(bytes.as_str())
    }
}