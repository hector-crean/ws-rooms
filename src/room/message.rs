use axum::extract::ws::Utf8Bytes;
use chrono::{DateTime, Utc};
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{
    ClientIdLike, RoomIdLike,
    presence::{PresenceLike, cursor_presence::CursorPresence},
    storage::{SharedPresentation, StorageLike},
};
use crate::room::error::RoomError;
use ts_rs::TS;

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
#[ts(concrete(RoomId = String, ClientId = Uuid, Presence = CursorPresence, Storage = SharedPresentation))]
#[serde(tag = "type", content = "data")]
pub enum ClientMessageType<RoomId, ClientId, Presence, Storage>
where
    RoomId: RoomIdLike,
    ClientId: ClientIdLike,
    Presence: PresenceLike,
    Storage: StorageLike,
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

impl<
    RoomId: RoomIdLike + TS,
    ClientId: ClientIdLike + TS,
    Storage: StorageLike + TS,
    Presence: PresenceLike + TS,
> ClientMessageType<RoomId, ClientId, Presence, Storage>
{
    pub fn process(
        &self,
    ) -> Result<ServerMessageType<RoomId, ClientId, Presence, Storage>, RoomError> {
        Err(RoomError::InvalidMessage)
    }
}

impl<
    RoomId: RoomIdLike + TS,
    ClientId: ClientIdLike + TS,
    Storage: StorageLike + TS,
    Presence: PresenceLike + TS,
> TryInto<Utf8Bytes> for ClientMessageType<RoomId, ClientId, Presence, Storage>
{
    type Error = serde_json::Error;
    fn try_into(self) -> Result<Utf8Bytes, Self::Error> {
        serde_json::to_string(&self).map(Utf8Bytes::from)
    }
}

impl<
    RoomId: RoomIdLike + for<'a> Deserialize<'a> + TS,
    ClientId: ClientIdLike + for<'a> Deserialize<'a> + TS,
    Storage: StorageLike + TS,
    Presence: PresenceLike + TS,
> TryFrom<Utf8Bytes> for ClientMessageType<RoomId, ClientId, Presence, Storage>
{
    type Error = serde_json::Error;
    fn try_from(bytes: Utf8Bytes) -> Result<Self, Self::Error> {
        serde_json::from_str(bytes.as_str())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(tag = "type", content = "data")]
#[ts(export)]
#[ts(concrete(RoomId = String, ClientId = Uuid, Presence = CursorPresence, Storage = SharedPresentation))]
pub enum ServerMessageType<RoomId, ClientId, Presence: PresenceLike, Storage: StorageLike>
where
    RoomId: RoomIdLike + TS,
    ClientId: ClientIdLike + TS,
    Presence: PresenceLike + TS,
    Storage: StorageLike + TS,
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

impl<
    RoomId: RoomIdLike + TS,
    ClientId: ClientIdLike + TS,
    Storage: StorageLike + TS,
    Presence: PresenceLike + TS,
> TryInto<Utf8Bytes> for ServerMessageType<RoomId, ClientId, Presence, Storage>
{
    type Error = serde_json::Error;
    fn try_into(self) -> Result<Utf8Bytes, Self::Error> {
        serde_json::to_string(&self).map(Utf8Bytes::from)
    }
}

impl<
    RoomId: RoomIdLike + for<'a> Deserialize<'a> + TS,
    ClientId: ClientIdLike + for<'a> Deserialize<'a> + TS,
    Storage: StorageLike + TS,
    Presence: PresenceLike + TS,
> TryFrom<Utf8Bytes> for ServerMessageType<RoomId, ClientId, Presence, Storage>
{
    type Error = serde_json::Error;
    fn try_from(bytes: Utf8Bytes) -> Result<Self, Self::Error> {
        serde_json::from_str(bytes.as_str())
    }
}
