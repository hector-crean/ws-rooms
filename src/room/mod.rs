pub mod client_state;
pub mod message;
pub mod presence;
pub mod storage;

use std::{
    collections::HashMap,
    fmt::{self, Debug, Display},
    hash::Hash,
    sync::{
        atomic::{AtomicU32, Ordering}, Arc
    },
};

use client_state::ClientState;
use message::{ClientMessageType, ServerMessageType};
use presence::{PresenceError, PresenceLike};
use serde::{Serialize, de::DeserializeOwned};
use storage::{StorageError, StorageLike};
use tokio::sync::{RwLock, broadcast};
use chrono::Utc;
use uuid::Uuid;

const DEFAULT_CHANNEL_CAPACITY: usize = 100;

// --- Error Enum ---

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

pub trait RoomIdLike:
    Eq + Hash + Clone + Send + Sync + 'static + fmt::Display + Serialize + Debug
{
}
pub trait ClientIdLike:
    Eq + Hash + Clone + Send + Sync + 'static + fmt::Display + Serialize + Debug
{
}


impl RoomIdLike for String {}
impl ClientIdLike for Uuid {}


// --- Room Structure ---

/// Represents a single chat room or broadcast group.
#[derive(Debug)]
struct Room<RoomId, ClientId, Presence, Storage>
where
    RoomId: RoomIdLike,
    ClientId: ClientIdLike,
    Presence: PresenceLike,
    Storage: StorageLike,
{
    // Room identifier is implicitly the key in the RoomsManager map.
    // room_id: RoomId, // Removed, key in manager's map is sufficient
    // For broadcasting server messages TO clients
    sender: broadcast::Sender<ServerMessageType<RoomId, ClientId, Presence, Storage>>,
    clients: RwLock<HashMap<ClientId, ClientState<Presence>>>,
    subscriber_count: AtomicU32, // Renamed from user_count
    storage: RwLock<Storage>,
}

impl<RoomId, ClientId, Presence, Storage> Room<RoomId, ClientId, Presence, Storage>
where
    RoomId: RoomIdLike,
    ClientId: ClientIdLike,
    Presence: PresenceLike,
    Storage: StorageLike,
{
    /// Creates a new room with a specific broadcast channel capacity.
    fn new(capacity: usize) -> Self {
        let (sender, _) = broadcast::channel(capacity);
        Room {
            sender,
            clients: RwLock::new(HashMap::new()),
            subscriber_count: AtomicU32::new(0),
            storage: RwLock::new(Storage::default()),
        }
    }

    async fn handle_storage_update(
        &self,
        version: Storage::Version,
        operations: Vec<Storage::Operation>,
    ) -> Result<(), StorageError> {
        let mut storage_guard = self.storage.write().await;
        for op in operations {
            storage_guard.apply_operation(op)?;
        }
        Ok(())
    }

    async fn handle_presence_update(
        &self,
        client_id: ClientId,
        update: Presence::Update,
    ) -> Result<(), PresenceError> {
        let mut clients_guard = self.clients.write().await;
        if let Some(client_state) = clients_guard.get_mut(&client_id) {
            client_state.presence_mut().apply_update(update)?;
        }
        Ok(())
    }

    /// Adds a client to the room, spawning a task to forward messages.
    /// The `client_sender` is the sender part of the specific client's broadcast channel.
    async fn join(
        &self,
        client_id: ClientId,
        client_sender: broadcast::Sender<ServerMessageType<RoomId, ClientId, Presence, Storage>>,
    ) {
        let mut clients_guard = self.clients.write().await;

        // Avoid joining if already present
        if clients_guard.contains_key(&client_id) {
            return;
        }

        let mut room_receiver = self.sender.subscribe();

        let task_handle = tokio::spawn(async move {
            loop {
                match room_receiver.recv().await {
                    Ok(server_msg) => {
                        // If sending to the client fails, it likely means the client disconnected
                        // or their channel is full/closed. Stop forwarding for this client.

                        if client_sender.send(server_msg).is_err() {
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => {
                        // Optional: Log lagging if necessary
                        // eprintln!("Client {} lagged in room", client_id_clone);
                        continue; // Try to catch up
                    }
                    Err(broadcast::error::RecvError::Closed) => {
                        // Room channel closed, task can exit.
                        break;
                    }
                }
            }
            // Optional: Log task exit
            // println!("Forwarding task for client {} in room shutting down.", client_id_clone);
        });

        clients_guard.insert(client_id, ClientState::new(task_handle));
        self.subscriber_count.fetch_add(1, Ordering::Relaxed); // Relaxed is sufficient here
    }

    /// Removes a client from the room and aborts their forwarding task.
    async fn leave(&self, client_id: &ClientId) {
        let mut clients_guard = self.clients.write().await;
        if let Some(client_state) = clients_guard.remove(client_id) {
            client_state.forwarder().abort();
            self.subscriber_count.fetch_sub(1, Ordering::Relaxed); // Relaxed is sufficient
        }
    }

    /// Blocking version of `leave` for use in synchronous contexts like `Drop`.
    fn blocking_leave(&self, client_id: &ClientId) {
        // Use blocking_write for sync context
        let mut clients_guard = self.clients.blocking_write();
        if let Some(client_state) = clients_guard.remove(client_id) {
            client_state.forwarder().abort();
            self.subscriber_count.fetch_sub(1, Ordering::Relaxed);
        }
    }

    /// Aborts all client forwarding tasks and clears the client list.
    /// Used when removing the entire room.
    async fn clear(&self) {
        let mut clients_guard = self.clients.write().await;
        for (_, client_state) in clients_guard.drain() {
            client_state.forwarder().abort();
        }
        self.subscriber_count.store(0, Ordering::Relaxed);
    }

    /// Sends a message to all clients currently subscribed to this room.
    fn broadcast(
        &self,
        message: ServerMessageType<RoomId, ClientId, Presence, Storage>,
    ) -> Result<
        usize,
        broadcast::error::SendError<ServerMessageType<RoomId, ClientId, Presence, Storage>>,
    > {
        self.sender.send(message)
    }

    /// Checks if the room has no subscribers.
    fn is_empty(&self) -> bool {
        self.subscriber_count.load(Ordering::Relaxed) == 0
        // Alternative: Check map length (requires read lock)
        // self.clients.blocking_read().is_empty() // If called from sync context
        // Or async: self.clients.read().await.is_empty()
    }

    /// Returns the number of clients currently subscribed to the room.
    fn subscriber_count(&self) -> u32 {
        self.subscriber_count.load(Ordering::Relaxed)
    }



    // New method to handle incoming client messages
    async fn process_client_message(
        &self,
        client_id: &ClientId,
        msg: ClientMessageType<RoomId, ClientId, Presence, Storage>,
    ) -> Result<Option<ServerMessageType<RoomId, ClientId, Presence, Storage>>, RoomError> {
        match msg {
            ClientMessageType::UpdatedPresence { presence } => {
                // Handle presence update
                self.handle_presence_update(client_id.clone(), presence.clone()).await?;
                
                Ok(Some(ServerMessageType::PresenceUpdated {
                    client_id: client_id.clone(),
                    timestamp: Utc::now(),
                    presence,
                }))
            },
            
            ClientMessageType::UpdatedStorage { operations } => {
                // Get current storage version before applying operations
                let mut storage = self.storage.write().await;
                let mut applied_ops = Vec::new();
                
                // Apply each operation and collect successful ones
                for op in operations {
                    if let Ok(version) = storage.apply_operation(op.clone()) {
                        applied_ops.push(op);
                    }
                }
                
                // Only broadcast if we have successful operations
                if !applied_ops.is_empty() {
                    Ok(Some(ServerMessageType::StorageUpdated {
                        version: storage.version(),
                        operations: applied_ops,
                    }))
                } else {
                    Ok(None)
                }
            },
            
            ClientMessageType::JoinRoom { room_id } => {
                Ok(Some(ServerMessageType::RoomJoined {
                    room_id,
                    client_id: client_id.clone(),
                }))
            },
            
            ClientMessageType::LeaveRoom => {
                // Note: This requires access to the RoomsManager, so we should move this logic
                // to the RoomsManager's handle_client_message method instead
                Err(RoomError::InvalidMessage)
            },
            
            ClientMessageType::_Phantom(_) => unreachable!(),
        }
    }
}

// --- Redesigned UserSubscription Handle ---

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
    // Add Clone bounds needed by manager methods here
    RoomId: RoomIdLike,
    ClientId: ClientIdLike,
    Presence: PresenceLike,
    Storage: StorageLike,
{
    /// Joins a specific room.
    ///
    /// Idempotent: If the user is already in the room, this does nothing.
    /// Returns an error if the room does not exist or the user subscription is invalid (which shouldn't happen if the handle exists).
    pub async fn join(&self, room_id: RoomId) -> Result<(), RoomError> {
        self.manager
            .join_room_internal(&room_id, &self.client_id)
            .await
    }

    /// Joins a specific room, creating it if it doesn't exist.
    ///
    /// Idempotent regarding room membership.
    pub async fn join_or_create(
        &self,
        room_id: RoomId,
        capacity: Option<usize>,
    ) -> Result<(), RoomError> {
        self.manager
            .join_or_create_room_internal(room_id, &self.client_id, capacity)
            .await
    }

    /// Leaves a specific room.
    ///
    /// Idempotent: If the user is not in the room, this does nothing.
    /// Returns an error if the room does not exist.
    pub async fn leave(&self, room_id: RoomId) -> Result<(), RoomError> {
        self.manager
            .leave_room_internal(&room_id, &self.client_id)
            .await
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
            .user_channels
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

// --- Updated RoomsManager ---

#[derive(Debug)]
pub struct RoomsManager<RoomId, ClientId, Presence, Storage>
where
    RoomId: RoomIdLike,
    ClientId: ClientIdLike,
    Presence: PresenceLike,
    Storage: StorageLike,
{
    rooms: RwLock<HashMap<RoomId, Arc<Room<RoomId, ClientId, Presence, Storage>>>>,
    user_channels: RwLock<
        HashMap<
            ClientId,
            broadcast::Sender<ServerMessageType<RoomId, ClientId, Presence, Storage>>,
        >
    >,
    client_rooms: RwLock<HashMap<ClientId, RoomId>>,
}

impl<RoomId, ClientId, Presence, Storage> RoomsManager<RoomId, ClientId, Presence, Storage>
where
    RoomId: RoomIdLike,
    ClientId: ClientIdLike,
    Presence: PresenceLike,
    Storage: StorageLike,
{
    pub fn new() -> Self {
        RoomsManager {
            rooms: RwLock::new(HashMap::new()),
            user_channels: RwLock::new(HashMap::new()),
            client_rooms: RwLock::new(HashMap::new()),
        }
    }

    async fn handle_presence_update(
        &self,
        client_id: &ClientId,
        update: Presence::Update,
    ) -> Result<(), RoomError> {
        // First get the client's current room
        let room_id = {
            let client_rooms_guard = self.client_rooms.read().await;
            client_rooms_guard
                .get(client_id)
                .cloned()
                .ok_or_else(|| {
                    tracing::error!(%client_id, "Client not in any room when updating presence");
                    RoomError::UserNotInRoom(client_id.to_string())
                })?
        };

        // Then get the room and apply the update
        let room = {
            let rooms_guard = self.rooms.read().await;
            rooms_guard
                .get(&room_id)
                .cloned()
                .ok_or_else(|| {
                    tracing::error!(%client_id, %room_id, "Room not found when updating presence");
                    RoomError::RoomNotFound(room_id.to_string())
                })?
        };

        // Apply the presence update
        room.handle_presence_update(client_id.clone(), update.clone()).await?;

        // Broadcast the update to all clients in the room
        tracing::debug!(%client_id, %room_id, ?update, "Broadcasting presence update");
        room.broadcast(ServerMessageType::PresenceUpdated {
            client_id: client_id.clone(),
            timestamp: Utc::now(),
            presence: update,
        })
        .map_err(|e| {
            tracing::error!(%client_id, %room_id, error = %e, "Failed to broadcast presence update");
            RoomError::MessageSendFail(Box::new(e))
        })?;

        Ok(())
    }

    async fn handle_storage_update(
        &self,
        client_id: &ClientId,
        operations: Vec<Storage::Operation>,
    ) -> Result<(), RoomError> {
        // First get the client's current room
        let room_id = {
            let client_rooms_guard = self.client_rooms.read().await;
            client_rooms_guard
                .get(client_id)
                .cloned()
                .ok_or_else(|| {
                    tracing::error!(%client_id, "Client not in any room when updating storage");
                    RoomError::UserNotInRoom(client_id.to_string())
                })?
        };

        // Then get the room and apply the update
        let room = {
            let rooms_guard = self.rooms.read().await;
            rooms_guard
                .get(&room_id)
                .cloned()
                .ok_or_else(|| {
                    tracing::error!(%client_id, %room_id, "Room not found when updating storage");
                    RoomError::RoomNotFound(room_id.to_string())
                })?
        };

        // Apply storage operations and collect successful ones
        let mut storage = room.storage.write().await;
        let mut applied_ops = Vec::new();
        
        for op in operations {
            match storage.apply_operation(op.clone()) {
                Ok(_) => {
                    applied_ops.push(op);
                }
                Err(e) => {
                    tracing::warn!(%client_id, %room_id, ?op, error = %e, "Failed to apply storage operation");
                }
            }
        }

        // Only broadcast if we have successful operations
        if !applied_ops.is_empty() {
            tracing::debug!(%client_id, %room_id, operation_count = applied_ops.len(), "Broadcasting storage update");
            room.broadcast(ServerMessageType::StorageUpdated {
                version: storage.version(),
                operations: applied_ops,
            })
            .map_err(|e| {
                tracing::error!(%client_id, %room_id, error = %e, "Failed to broadcast storage update");
                RoomError::MessageSendFail(Box::new(e))
            })?;
        }

        Ok(())
    }

    pub async fn handle_client_message(
        &self,
        client_id: &ClientId,
        message: ClientMessageType<RoomId, ClientId, Presence, Storage>,
    ) -> Result<(), RoomError> {
        match message {
            ClientMessageType::JoinRoom { room_id } => {
                tracing::info!(%client_id, %room_id, "Client joining room");
                self.join_room_internal(&room_id, client_id).await
            },
            ClientMessageType::LeaveRoom => {
                // Get the client's current room
                let room_id = {
                    let client_rooms_guard = self.client_rooms.read().await;
                    client_rooms_guard
                        .get(client_id)
                        .cloned()
                        .ok_or_else(|| RoomError::UserNotInRoom(client_id.to_string()))?
                };
                
                tracing::info!(%client_id, %room_id, "Client leaving room");
                self.leave_room_internal(&room_id, client_id).await
            },
            ClientMessageType::UpdatedPresence { presence } => {
                tracing::debug!(%client_id, ?presence, "Client updating presence");
                self.handle_presence_update(client_id, presence).await?;
                Ok(())
            },
            ClientMessageType::UpdatedStorage { operations } => {
                tracing::debug!(%client_id, operation_count = operations.len(), "Client updating storage");
                self.handle_storage_update(client_id, operations).await?;
                Ok(())
            },
            _ => {
                // For other messages, get the client's current room
                let room_id = {
                    let client_rooms_guard = self.client_rooms.read().await;
                    client_rooms_guard
                        .get(client_id)
                        .cloned()
                        .ok_or_else(|| {
                            tracing::error!(%client_id, "Client not in any room");
                            RoomError::UserNotInRoom(client_id.to_string())
                        })?
                };

                let room = {
                    let rooms_guard = self.rooms.read().await;
                    rooms_guard
                        .get(&room_id)
                        .cloned()
                        .ok_or_else(|| {
                            tracing::error!(%client_id, %room_id, "Room not found");
                            RoomError::RoomNotFound(room_id.to_string())
                        })?
                };

                tracing::debug!(%client_id, %room_id, ?message, "Processing client message");
                // Process the message
                if let Some(server_msg) = room.process_client_message(client_id, message).await? {
                    tracing::debug!(%client_id, %room_id, ?server_msg, "Broadcasting server message");
                    room.broadcast(server_msg)
                        .map_err(|e| {
                            tracing::error!(%client_id, %room_id, error = %e, "Failed to broadcast message");
                            RoomError::MessageSendFail(Box::new(e))
                        })?;
                }

                Ok(())
            }
        }
    }


    /// Creates a new room with the given ID or returns the existing one.
    /// If created, uses the specified capacity or a default.
    /// Returns `true` if the room was newly created, `false` otherwise.
    pub async fn ensure_room(&self, room_id: &RoomId, capacity: Option<usize>) -> bool {
        let mut rooms_guard = self.rooms.write().await;
        if rooms_guard.contains_key(room_id) {
            false // Room already exists
        } else {
            let room = Room::new(capacity.unwrap_or(DEFAULT_CHANNEL_CAPACITY));
            rooms_guard.insert(room_id.clone(), Arc::new(room));
            true // Room was created
        }
    }

    /// Removes a room and cleans up its resources (stops forwarding tasks).
    /// Returns `true` if the room existed and was removed, `false` otherwise.
    pub async fn remove_room(&self, room_id: &RoomId) -> bool {
        let mut rooms_guard = self.rooms.write().await;
        if let Some(room_arc) = rooms_guard.remove(room_id) {
            room_arc.clear().await;
            true
        } else {
            false
        }
    }

    /// Subscribes a user to the system, providing a handle for interaction.
    ///
    /// This initializes the user's presence and broadcast channel if they aren't already active.
    /// If the user already has an active subscription handle, this will return a new handle
    /// linked to the same underlying channel. Dropping one handle will NOT affect others
    /// until the *last* handle for that user is dropped.
    ///
    /// Use the returned `UserSubscription` handle to join/leave rooms and receive messages.
    pub async fn subscribe(
        self: &Arc<Self>, // Takes Arc<Self> for easy cloning into the handle
        client_id: ClientId,
        capacity: Option<usize>,
    ) -> Result<UserSubscription<RoomId, ClientId, Presence, Storage>, RoomError> {
        let mut user_channels_guard = self.user_channels.write().await;
        let channel_capacity = capacity.unwrap_or(DEFAULT_CHANNEL_CAPACITY);

        let sender = user_channels_guard
            .entry(client_id.clone())
            .or_insert_with(|| {
                // Optional: Log user initialization
                // println!("Initializing user channel for: {}", client_id);
                let (tx, _) = broadcast::channel(channel_capacity);
                tx
            });

        let receiver = sender.subscribe();

        Ok(UserSubscription {
            client_id,
            manager: Arc::clone(self), // Clone the Arc for the handle
            receiver,
        })

        // Note: We previously checked if the user was already present.
        // Now, subscribing again just gives a new handle to the same underlying channel.
        // This might be desirable, or you could add a check/error here if you only
        // want one active "session" handle per client_id at a time.
        // For example:
        // if user_channels_guard.contains_key(&client_id) {
        //      return Err(RoomError::UserAlreadySubscribed(client_id.to_string()));
        // }
        // But the current approach allows multiple independent handles if needed.
    }

    // Internal join method used by UserSubscription handle
    async fn join_room_internal(
        &self,
        room_id: &RoomId,
        client_id: &ClientId,
    ) -> Result<(), RoomError> {
        let room = {
            let rooms_guard = self.rooms.read().await;
            rooms_guard
                .get(room_id)
                .cloned()
                .ok_or_else(|| RoomError::RoomNotFound(room_id.to_string()))?
        };

        let user_sender = {
            let user_channels_guard = self.user_channels.read().await;
            user_channels_guard
                .get(client_id)
                .cloned()
                .ok_or_else(|| RoomError::UserNotInitialized(client_id.to_string()))?
        };

        // Update client_rooms mapping
        {
            let mut client_rooms_guard = self.client_rooms.write().await;
            // Check if client is already in a room
            if let Some(old_room_id) = client_rooms_guard.get(client_id) {
                return Err(RoomError::UserAlreadyInRoom(
                    client_id.to_string(),
                    old_room_id.to_string(),
                ));
            }
            client_rooms_guard.insert(client_id.clone(), room_id.clone());
        }

        room.join(client_id.clone(), user_sender).await;
        Ok(())
    }

    // Internal join_or_create method used by UserSubscription handle
    async fn join_or_create_room_internal(
        &self,
        room_id: RoomId,
        client_id: &ClientId,
        capacity: Option<usize>,
    ) -> Result<(), RoomError> {
        let room = {
            let mut rooms_guard = self.rooms.write().await;
            rooms_guard
                .entry(room_id.clone())
                .or_insert_with(|| {
                    Arc::new(Room::new(capacity.unwrap_or(DEFAULT_CHANNEL_CAPACITY)))
                })
                .clone()
        };

        let user_sender = {
            let user_channels_guard = self.user_channels.read().await;
            user_channels_guard
                .get(client_id)
                .cloned()
                .ok_or_else(|| RoomError::UserNotInitialized(client_id.to_string()))?
        };

        // Add this block to update client_rooms mapping
        {
            let mut client_rooms_guard = self.client_rooms.write().await;
            // Check if client is already in a room
            if let Some(old_room_id) = client_rooms_guard.get(client_id) {
                return Err(RoomError::UserAlreadyInRoom(
                    client_id.to_string(),
                    old_room_id.to_string(),
                ));
            }
            client_rooms_guard.insert(client_id.clone(), room_id.clone());
        }

        room.join(client_id.clone(), user_sender).await;
        Ok(())
    }

    // Internal leave method used by UserSubscription handle
    async fn leave_room_internal(
        &self,
        room_id: &RoomId,
        client_id: &ClientId,
    ) -> Result<(), RoomError> {
        let room = {
            let rooms_guard = self.rooms.read().await;
            rooms_guard
                .get(room_id)
                .cloned() // Clone Arc<Room>
                .ok_or_else(|| RoomError::RoomNotFound(room_id.to_string()))?
        }; // rooms_guard lock released here

        // Remove from client_rooms mapping
        {
            let mut client_rooms_guard = self.client_rooms.write().await;
            client_rooms_guard.remove(client_id);
        }

        // Check if user channel still exists, though it should if the handle is valid
        if !self.user_channels.read().await.contains_key(client_id) {
            return Err(RoomError::UserNotInitialized(client_id.to_string()));
        }

        // Leave the specific room (acquires write lock on room.clients)
        room.leave(client_id).await;
        Ok(())
    }

    /// Sends a message to all clients in a specific room.
    /// Fails if the room does not exist.
    pub async fn send_message_to_room(
        &self,
        room_id: &RoomId,
        message: ServerMessageType<RoomId, ClientId, Presence, Storage>,
    ) -> Result<usize, RoomError> {
        let room = {
            // Scope for read lock
            let rooms_guard = self.rooms.read().await;
            rooms_guard
                .get(room_id)
                .cloned()
                .ok_or_else(|| RoomError::RoomNotFound(room_id.to_string()))?
        }; // rooms_guard lock released here

        // Broadcast the message
        room.broadcast(message)
            .map_err(|e| RoomError::MessageSendFail(Box::new(e)))
    }

    /// Cleans up a user, removing them from all rooms and their user channel map entry *if*
    /// this is the last reference (tracked implicitly by Arc on the Room and Sender).
    /// Called automatically when `UserSubscription` is dropped.
    fn cleanup_user(&self, client_id: &ClientId) {
        // Use blocking operations as this is called from Drop
        let rooms_guard = self.rooms.blocking_read();
        for room in rooms_guard.values() {
            room.blocking_leave(client_id);
        }
        drop(rooms_guard);

        // Clean up client_rooms mapping
        let _ = self.client_rooms.blocking_write().remove(client_id);

        // Remove user channel sender *only if we're sure no other handles exist*.
        // The broadcast channel itself handles receiver cleanup.
        // We need to decide the cleanup strategy:
        // 1. Remove sender immediately: If another handle exists, `resubscribe` fails. Maybe bad.
        // 2. Rely on broadcast channel cleanup: `user_channels` map might grow indefinitely if users subscribe but never fully disconnect all handles.
        // 3. **Use Arc<Sender> or check sender's ref count?** `broadcast::Sender` doesn't easily exposes ref counts.
        // 4. **Track handle count explicitly?** Add an AtomicUsize counter per user? Seems complex.

        // Let's adopt Strategy 2 for now, relying on the broadcast channel's behavior.
        // When the *last* receiver (including the original one stored in Room tasks)
        // AND the Sender in `user_channels` is dropped, the channel resources are freed.
        // We don't strictly *need* to remove the entry from `user_channels` immediately on drop,
        // although it would prevent a user ID from being reused immediately with a fresh channel.

        // Option: If we *really* want to remove from the map, we could try checking `sender.receiver_count()`
        // *after* leaving all rooms, but it's tricky due to timing. Let's omit explicit removal for simplicity.
        let user_channels_guard = self.user_channels.blocking_write();
        if let Some(sender) = user_channels_guard.get(client_id) {
            // If receiver_count is 0, it *might* be safe to remove, but tasks might still hold receivers briefly.
            // println!("User {} cleanup: {} receivers remaining on sender.", client_id, sender.receiver_count());
            // if sender.receiver_count() == 0 {
            //     println!("Removing user {} from channel map.", client_id);
            //     user_channels_guard.remove(client_id);
            // }
        }
        // For now, do NOT remove from user_channels on drop. Allow resubscription.
    }

    // ... (room_exists, is_room_empty, room_count, etc.) ...
    pub async fn room_exists(&self, room_id: &RoomId) -> bool {
        self.rooms.read().await.contains_key(room_id)
    }

    pub async fn is_room_empty(&self, room_id: &RoomId) -> Result<bool, RoomError> {
        let rooms_guard = self.rooms.read().await;
        let room = rooms_guard
            .get(room_id)
            .ok_or_else(|| RoomError::RoomNotFound(room_id.to_string()))?;
        Ok(room.is_empty())
    }

    pub async fn room_subscriber_count(&self, room_id: &RoomId) -> Result<u32, RoomError> {
        let rooms_guard = self.rooms.read().await;
        let room = rooms_guard
            .get(room_id)
            .ok_or_else(|| RoomError::RoomNotFound(room_id.to_string()))?;
        Ok(room.subscriber_count())
    }

    pub async fn room_count(&self) -> usize {
        self.rooms.read().await.len()
    }

    pub async fn user_count(&self) -> usize {
        // This now represents users who have subscribed at least once and whose
        // channels might still exist, even if they have no active handles.
        self.user_channels.read().await.len()
    }

    pub async fn list_rooms(&self) -> Vec<RoomId> {
        self.rooms.read().await.keys().cloned().collect()
    }

    // Add new method to get a client's current room
    pub async fn get_client_room(&self, client_id: &ClientId) -> Option<RoomId> {
        self.client_rooms.read().await.get(client_id).cloned()
    }
}

// --- Default Impl ---
impl<RoomId, ClientId, Presence, Storage> Default
    for RoomsManager<RoomId, ClientId, Presence, Storage>
where
    RoomId: RoomIdLike,
    ClientId: ClientIdLike,
    Presence: PresenceLike,
    Storage: StorageLike,
{
    fn default() -> Self {
        Self::new()
    }
}
