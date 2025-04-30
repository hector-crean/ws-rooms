use axum::extract::ws::Utf8Bytes;
use chrono::{DateTime, Utc};
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};

use super::{presence::PresenceLike, storage::StorageLike, ClientIdLike, Room, RoomIdLike};
use crate::room::error::RoomError;
 // Add Duration




#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ClientMessageType<RoomId, ClientId, Presence: PresenceLike, Storage: StorageLike>
where
    RoomId: RoomIdLike,
    ClientId: ClientIdLike,
    Presence::Update: Send + Sync,
    Storage::Operation: Send + Sync,
{
    JoinRoom {
        room_id: RoomId,
    },
    LeaveRoom,
    UpdatedPresence {
        presence: Presence::Update,
    },
    UpdatedStorage {
        operations: Vec<Storage::Operation>,
    },
    // Add a marker variant to use ClientId
    #[serde(skip)]
    _Phantom(std::marker::PhantomData<ClientId>),
}


impl<RoomId: RoomIdLike, ClientId: ClientIdLike, Storage: StorageLike, Presence: PresenceLike> ClientMessageType<RoomId, ClientId, Presence, Storage> {
    pub fn process(&self) -> Result<ServerMessageType<RoomId, ClientId, Presence, Storage>, RoomError> {
     match self {
        
         _ => Err(RoomError::InvalidMessage)
     }
    }
 }



impl<RoomId: RoomIdLike, ClientId: ClientIdLike, Storage: StorageLike, Presence: PresenceLike> 
    TryInto<Utf8Bytes> for ClientMessageType<RoomId, ClientId, Presence, Storage> {
    type Error = serde_json::Error;
    fn try_into(self) -> Result<Utf8Bytes, Self::Error> {
        serde_json::to_string(&self).map(Utf8Bytes::from)   
    }
}

impl<RoomId: RoomIdLike + for<'a> Deserialize<'a>, ClientId: ClientIdLike + for<'a> Deserialize<'a>, Storage: StorageLike, Presence: PresenceLike> 
    TryFrom<Utf8Bytes> for ClientMessageType<RoomId, ClientId, Presence, Storage> {
    type Error = serde_json::Error;
    fn try_from(bytes: Utf8Bytes) -> Result<Self, Self::Error> {
        serde_json::from_str(bytes.as_str())
    }
}





#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ServerMessageType<RoomId, ClientId, Presence: PresenceLike, Storage: StorageLike>
where
    RoomId: Send + Sync,
    ClientId: Send + Sync,
    Presence::Update: Send + Sync,
    Storage::Operation: Send + Sync,
{
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
    StorageUpdated {
        version: Storage::Version,
        operations: Vec<Storage::Operation>,
    },
    PresenceUpdated {
        client_id: ClientId,
        timestamp: DateTime<Utc>,
        presence: Presence::Update,
    },
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



impl<RoomId: Serialize + RoomIdLike, ClientId: Serialize + ClientIdLike, Storage: StorageLike, Presence: PresenceLike> 
    TryInto<Utf8Bytes> for ServerMessageType<RoomId, ClientId, Presence, Storage> {
    type Error = serde_json::Error;
    fn try_into(self) -> Result<Utf8Bytes, Self::Error> {
        serde_json::to_string(&self).map(Utf8Bytes::from)   
    }
}

impl<RoomId: RoomIdLike + for<'a> Deserialize<'a>, ClientId: ClientIdLike + for<'a> Deserialize<'a>, Storage: StorageLike, Presence: PresenceLike> 
    TryFrom<Utf8Bytes> for ServerMessageType<RoomId, ClientId, Presence, Storage> {
    type Error = serde_json::Error;
    fn try_from(bytes: Utf8Bytes) -> Result<Self, Self::Error> {
        serde_json::from_str(bytes.as_str())
    }
}


