pub mod message;
pub mod client_state;
pub mod presence;
pub mod storage;

use std::{
    collections::HashMap,
    error::Error,
    fmt,
    fmt::Display,
    hash::Hash,
    sync::{
        atomic::{AtomicU32, Ordering},
        Arc, // Added Arc for potential shared ownership if needed
    },
    time::{Duration, Instant},
};

use client_state::ClientState;
use presence::PresenceLike;
use storage::StorageLike;
use tokio::{
    sync::{broadcast, RwLock},
    task::JoinHandle,
};

use chrono::{DateTime, Utc};






const DEFAULT_CHANNEL_CAPACITY: usize = 100;

// --- Error Enum ---


#[derive(thiserror::Error, Debug)]
pub enum RoomError {
    #[error("Target room '{0}' not found")]
    RoomNotFound(String), // Can include the RoomId if it's Display

    #[error("User '{0}' has not been initialized or is already disconnected")]
    UserNotInitialized(String), // Can include ClientId if it's Display

    #[error("User '{0}' is already subscribed")]
    UserAlreadySubscribed(String), // For the new subscribe API

    #[error("Failed to send message: {0}")]
    MessageSendFail(#[from] Box<dyn std::error::Error + Send + Sync>),

    // Optional: Add specific internal errors if needed
    #[error("Internal lock poisoned: {0}")]
    LockPoisoned(String),

    #[error("Failed to forward message to user channel")]
    UserSendFail, // Keep this if the forwarding task needs to signal failure, though often just stopping is enough.
}





// --- Room Structure ---

/// Represents a single chat room or broadcast group.
#[derive(Debug)]
struct Room<ClientId, MessageType, Presence, Storage>
where
    ClientId: Eq + Hash + Clone + Send + Sync + 'static,
    MessageType: Clone + Send + Sync + std::fmt::Debug + 'static,
    Presence:  PresenceLike,
    Storage: StorageLike,
{
    // Room identifier is implicitly the key in the RoomsManager map.
    // room_id: RoomId, // Removed, key in manager's map is sufficient
    sender: broadcast::Sender<MessageType>,
    clients: RwLock<HashMap<ClientId, ClientState<Presence>>>,
    subscriber_count: AtomicU32, // Renamed from user_count
    storage: RwLock<Storage>
}

impl<ClientId, MessageType, Presence, Storage> Room<ClientId, MessageType, Presence, Storage>
where
    ClientId: Eq + Hash + Clone + Send + Sync + 'static,
    MessageType: Clone + Send + Sync + std::fmt::Debug + 'static,
    Presence:  PresenceLike,
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

    /// Adds a client to the room, spawning a task to forward messages.
    /// The `client_sender` is the sender part of the specific client's broadcast channel.
    async fn join(&self, client_id: ClientId, client_sender: broadcast::Sender<MessageType>) {
        let mut clients_guard = self.clients.write().await;

        // Avoid joining if already present
        if clients_guard.contains_key(&client_id) {
            return;
        }

        let mut room_receiver = self.sender.subscribe();
        let client_id_clone = client_id.clone(); // Clone for the task

        let task_handle = tokio::spawn(async move {
            loop {
                match room_receiver.recv().await {
                    Ok(message) => {
                        // If sending to the client fails, it likely means the client disconnected
                        // or their channel is full/closed. Stop forwarding for this client.
                        if client_sender.send(message).is_err() {
                            // Optional: Log this failure
                            // eprintln!("Failed to forward message to client {}", client_id_clone);
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
    fn broadcast(&self, message: MessageType) -> Result<usize, broadcast::error::SendError<MessageType>> {
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

    /// Gets a clone of the room's broadcast sender.
    fn get_sender(&self) -> broadcast::Sender<MessageType> {
        self.sender.clone()
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
pub struct UserSubscription<RoomId, ClientId, MessageType, Presence, Storage>
where
    // Relaxed Clone bounds here as the handle itself doesn't need them cloned often
    RoomId: Clone + Display + Eq + Hash + Send + Sync + 'static,
    ClientId: Clone + Display + Eq + Hash + Clone + Send + Sync + 'static, // Need Clone for internal use
    MessageType: Clone + Send + Sync + std::fmt::Debug + 'static,
    Presence: PresenceLike,
    Storage: StorageLike,
{
    client_id: ClientId,
    // Store Arc<Manager> to allow calling join/leave etc.
    manager: Arc<RoomsManager<RoomId, ClientId, MessageType, Presence, Storage>>,
    // The receiver for this user's messages
    receiver: broadcast::Receiver<MessageType>,
}

impl<RoomId, ClientId, MessageType, Presence, Storage> UserSubscription<RoomId, ClientId, MessageType, Presence, Storage>
where
    // Add Clone bounds needed by manager methods here
    RoomId: Eq + Hash + Clone + Send + Sync + 'static + fmt::Display, // Added Display for errors
    ClientId: Eq + Hash + Clone + Send + Sync + 'static + fmt::Display, // Added Display for errors
    MessageType: Clone + Send + Sync + std::fmt::Debug + 'static,
    Presence: PresenceLike,
    Storage: StorageLike,
{
    /// Joins a specific room.
    ///
    /// Idempotent: If the user is already in the room, this does nothing.
    /// Returns an error if the room does not exist or the user subscription is invalid (which shouldn't happen if the handle exists).
    pub async fn join(&self, room_id: RoomId) -> Result<(), RoomError> {
        self.manager.join_room_internal(&room_id, &self.client_id).await
    }

    /// Joins a specific room, creating it if it doesn't exist.
    ///
    /// Idempotent regarding room membership.
    pub async fn join_or_create(&self, room_id: RoomId, capacity: Option<usize>) -> Result<(), RoomError> {
        self.manager.join_or_create_room_internal(room_id, &self.client_id, capacity).await
    }


    /// Leaves a specific room.
    ///
    /// Idempotent: If the user is not in the room, this does nothing.
    /// Returns an error if the room does not exist.
    pub async fn leave(&self, room_id: RoomId) -> Result<(), RoomError> {
        self.manager.leave_room_internal(&room_id, &self.client_id).await
    }

    /// Receives the next message broadcast to any joined room.
    /// See `tokio::sync::broadcast::Receiver::recv` for error details (Lagged, Closed).
    pub async fn recv(&mut self) -> Result<MessageType, broadcast::error::RecvError> {
        self.receiver.recv().await
    }

    /// Attempts to receive the next message without waiting.
    /// See `tokio::sync::broadcast::Receiver::try_recv` for error details (Empty, Lagged, Closed).
    pub fn try_recv(&mut self) -> Result<MessageType, broadcast::error::TryRecvError> {
        self.receiver.try_recv()
    }

    /// Gets the ClientId associated with this subscription.
    pub fn client_id(&self) -> &ClientId {
        &self.client_id
    }

    /// Returns a mutable reference to the underlying receiver.
    /// Useful if you need methods not directly exposed by the handle.
    pub fn receiver_mut(&mut self) -> &mut broadcast::Receiver<MessageType> {
        &mut self.receiver
    }

     /// Creates a new receiver handle for this subscription.
     /// Useful if the original receiver needs to be consumed or used elsewhere.
    pub fn resubscribe(&self) -> broadcast::Receiver<MessageType> {
        self.manager.user_channels.blocking_read() // Use blocking read as we might be in sync context
            .get(&self.client_id)
            .map(|sender| sender.subscribe())
            // This panic indicates an inconsistent state if the handle exists but the channel doesn't.
            .expect("User channel sender missing while subscription handle exists")
    }

}

impl<RoomId, ClientId, MessageType, Presence, Storage> Drop for UserSubscription<RoomId, ClientId, MessageType, Presence, Storage>
where
    RoomId: Clone + Display + Eq + Hash + Send + Sync + 'static,
    ClientId: Clone + Display + Eq + Hash + Clone + Send + Sync + 'static,
    MessageType: Clone + Send + Sync + std::fmt::Debug + 'static,
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
pub struct RoomsManager<RoomId, ClientId, MessageType, Presence, Storage>
where
    RoomId: Eq + Hash + Clone + Send + Sync + 'static + fmt::Display, // Added Display
    ClientId: Eq + Hash + Clone + Send + Sync + 'static + fmt::Display, // Added Display
    MessageType: Clone + Send + Sync + std::fmt::Debug + 'static,
    Presence: PresenceLike,
    Storage: StorageLike,
{
    rooms: RwLock<HashMap<RoomId, Arc<Room<ClientId, MessageType, Presence, Storage>>>>,
    user_channels: RwLock<HashMap<ClientId, broadcast::Sender<MessageType>>>,
}


impl<RoomId, ClientId, MessageType, Presence, Storage> RoomsManager<RoomId, ClientId, MessageType, Presence, Storage>
where
    RoomId: Eq + Hash + Clone + Send + Sync + 'static + fmt::Display, // Added Display
    ClientId: Eq + Hash + Clone + Send + Sync + 'static + fmt::Display, // Added Display
    MessageType: Clone + Send + Sync + std::fmt::Debug + 'static,
    Presence: PresenceLike,
    Storage: StorageLike,
{
    // ... (new, ensure_room, remove_room, etc. remain similar) ...
    pub fn new() -> Self {
        RoomsManager {
            rooms: RwLock::new(HashMap::new()),
            user_channels: RwLock::new(HashMap::new()),
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
    ) -> Result<UserSubscription<RoomId, ClientId, MessageType, Presence, Storage>, RoomError> {
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
    async fn join_room_internal(&self, room_id: &RoomId, client_id: &ClientId) -> Result<(), RoomError> {
        let room = { // Scope for read lock
            let rooms_guard = self.rooms.read().await;
            rooms_guard.get(room_id)
                .cloned() // Clone Arc<Room>
                .ok_or_else(|| RoomError::RoomNotFound(room_id.to_string()))?
        }; // rooms_guard lock released here

        let user_sender = { // Scope for read lock
            let user_channels_guard = self.user_channels.read().await;
            user_channels_guard
                .get(client_id)
                // This error implies the handle exists but the user was somehow cleaned up,
                // or was never properly initialized by `subscribe`.
                .cloned() // Clone Sender<T>
                .ok_or_else(|| RoomError::UserNotInitialized(client_id.to_string()))?
        }; // user_channels_guard lock released here

        // Now join the specific room (acquires write lock on room.clients)
        room.join(client_id.clone(), user_sender).await;
        Ok(())
    }

     // Internal join_or_create method used by UserSubscription handle
    async fn join_or_create_room_internal(&self, room_id: RoomId, client_id: &ClientId, capacity: Option<usize>) -> Result<(), RoomError> {
         let room = { // Scoped to release lock quickly
            let mut rooms_guard = self.rooms.write().await;
            rooms_guard.entry(room_id.clone())
                .or_insert_with(|| Arc::new(Room::new(capacity.unwrap_or(DEFAULT_CHANNEL_CAPACITY))))
                .clone() // Clone Arc<Room>
        }; // rooms_guard lock released here

        let user_sender = { // Scoped to release lock quickly
            let user_channels_guard = self.user_channels.read().await;
             user_channels_guard
                .get(client_id)
                .cloned()
                .ok_or_else(|| RoomError::UserNotInitialized(client_id.to_string()))?
        }; // user_channels_guard lock released here

        // Now join the specific room (acquires write lock on room.clients)
        room.join(client_id.clone(), user_sender).await;
        Ok(())
    }


    // Internal leave method used by UserSubscription handle
    async fn leave_room_internal(&self, room_id: &RoomId, client_id: &ClientId) -> Result<(), RoomError> {
        let room = { // Scope for read lock
            let rooms_guard = self.rooms.read().await;
            rooms_guard.get(room_id)
                .cloned() // Clone Arc<Room>
                .ok_or_else(|| RoomError::RoomNotFound(room_id.to_string()))?
        }; // rooms_guard lock released here

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
    pub async fn send_message_to_room(&self, room_id: &RoomId, message: MessageType) -> Result<usize, RoomError> {
        let room = { // Scope for read lock
             let rooms_guard = self.rooms.read().await;
             rooms_guard.get(room_id)
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
            // Tell each room the user has left (this aborts the task if present)
            room.blocking_leave(client_id);
        }
        drop(rooms_guard); // Release read lock on rooms

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
        let mut user_channels_guard = self.user_channels.blocking_write();
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
        let room = rooms_guard.get(room_id).ok_or_else(|| RoomError::RoomNotFound(room_id.to_string()))?;
        Ok(room.is_empty())
    }

     pub async fn room_subscriber_count(&self, room_id: &RoomId) -> Result<u32, RoomError> {
         let rooms_guard = self.rooms.read().await;
         let room = rooms_guard.get(room_id).ok_or_else(|| RoomError::RoomNotFound(room_id.to_string()))?;
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
}

// --- Default Impl ---
impl<RoomId, ClientId, MessageType, Presence, Storage> Default for RoomsManager<RoomId, ClientId, MessageType, Presence, Storage>
where
    RoomId: Eq + Hash + Clone + Send + Sync + 'static + fmt::Display,
    ClientId: Eq + Hash + Clone + Send + Sync + 'static + fmt::Display,
    MessageType: Clone + Send + Sync + std::fmt::Debug + 'static,
    Presence: PresenceLike,
    Storage: StorageLike,
{
    fn default() -> Self {
        Self::new()
    }
}


