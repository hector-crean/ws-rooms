
// --- Redesigned UserSubscription Handle ---

use std::sync::Arc;

use tokio::sync::broadcast;

use super::{error::RoomError, manager::RoomsManager, message::ServerMessageType, presence::PresenceLike, storage::StorageLike, ClientIdLike, RoomIdLike};

/// Represents an active user subscription to the RoomsManager system.
///
/// Obtain this via `RoomsManager::subscribe`.
/// Use methods like `join()` and `leave()` to manage room membership.
/// Use `recv()` or `try_recv()` to get messages from joined rooms.
///
/// When this handle is dropped, the user is automatically removed from all
/// joined rooms and their presence is cleaned up in the manager.
#[derive(Debug)]
#[must_use = "UserSubscription must be kept, otherwise the user is immediately cleaned up"]
pub struct UserSubscription<RoomId, ClientId, Presence, Storage>
where
    // Relaxed Clone bounds here as the handle itself doesn't need them cloned often
    RoomId: RoomIdLike,
    ClientId: ClientIdLike,
    Presence: PresenceLike,
    Storage: StorageLike,
{
    client_id: ClientId,
    // Store Arc<Manager> to allow calling join/leave etc.
    manager: Arc<RoomsManager<RoomId, ClientId, Presence, Storage>>,
    // For receiving server messages FROM server
    receiver: broadcast::Receiver<ServerMessageType<RoomId, ClientId, Presence, Storage>>,
}

impl<RoomId, ClientId, Presence, Storage> UserSubscription<RoomId, ClientId, Presence, Storage>
where
    RoomId: RoomIdLike,
    ClientId: ClientIdLike,
    Presence: PresenceLike,
    Storage: StorageLike,
{
    pub fn new(client_id: ClientId, manager: Arc<RoomsManager<RoomId, ClientId, Presence, Storage>>, receiver: broadcast::Receiver<ServerMessageType<RoomId, ClientId, Presence, Storage>>) -> Self {
        Self {
            client_id,
            manager,
            receiver
        }
    }
    /// Joins a specific room.
    ///
    /// Idempotent: If the user is already in the room, this does nothing.
    /// Returns an error if the room does not exist or the user subscription is invalid (which shouldn't happen if the handle exists).
    pub async fn join(&self, room_id: RoomId) -> Result<(), RoomError> {
        self.manager.join_room(&room_id, &self.client_id).await
    }

    /// Joins a specific room, creating it if it doesn't exist.
    ///
    /// Idempotent regarding room membership.
    pub async fn join_or_create(
        &self,
        room_id: RoomId,
        capacity: Option<usize>,
    ) -> Result<(), RoomError> {
        self.manager.join_or_create_room(room_id, &self.client_id, capacity).await
    }

    /// Leaves a specific room.
    ///
    /// Idempotent: If the user is not in the room, this does nothing.
    /// Returns an error if the room does not exist.
    pub async fn leave(&self, room_id: RoomId) -> Result<(), RoomError> {
        self.manager.leave_room(&room_id, &self.client_id).await
    }

    /// Receives the next message broadcast to any joined room.
    /// See `tokio::sync::broadcast::Receiver::recv` for error details (Lagged, Closed).
    pub async fn recv(&mut self) -> Result<ServerMessageType<RoomId, ClientId, Presence, Storage>, broadcast::error::RecvError> {
        self.receiver.recv().await
    }

    /// Attempts to receive the next message without waiting.
    /// See `tokio::sync::broadcast::Receiver::try_recv` for error details (Empty, Lagged, Closed).
    pub fn try_recv(&mut self) -> Result<ServerMessageType<RoomId, ClientId, Presence, Storage>, broadcast::error::TryRecvError> {
        self.receiver.try_recv()
    }

    /// Gets the ClientId associated with this subscription.
    pub fn client_id(&self) -> &ClientId {
        &self.client_id
    }

    /// Returns a mutable reference to the underlying receiver.
    /// Useful if you need methods not directly exposed by the handle.
    pub fn receiver_mut(&mut self) -> &mut broadcast::Receiver<ServerMessageType<RoomId, ClientId, Presence, Storage>> {
        &mut self.receiver
    }

    /// Creates a new receiver handle for this subscription.
    /// Useful if the original receiver needs to be consumed or used elsewhere.
    pub fn resubscribe(&self) -> broadcast::Receiver<ServerMessageType<RoomId, ClientId, Presence, Storage>> {
        self.manager
            .user_channels()
            .blocking_read() // Use blocking read as we might be in sync context
            .get(&self.client_id)
            .map(|sender| sender.subscribe())
            // This panic indicates an inconsistent state if the handle exists but the channel doesn't.
            .expect("User channel sender missing while subscription handle exists")
    }
}

impl<RoomId, ClientId, Presence, Storage> Drop
    for UserSubscription<RoomId, ClientId , Presence, Storage>
where
    RoomId: RoomIdLike,
    ClientId: ClientIdLike,
    Presence: PresenceLike,
    Storage: StorageLike,
{
    fn drop(&mut self) {
        // Call the cleanup function on the manager
        self.manager.cleanup_user(&self.client_id);
        // Optional: Log cleanup
        // println!("Cleaning up user: {}", self.client_id);
    }
}