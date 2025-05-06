pub mod client_state;
pub mod error;
pub mod manager;
pub mod message;
pub mod presence;
pub mod storage;
pub mod subscription;

use std::{
    collections::HashMap,
    fmt::{self, Debug},
    hash::Hash,
    sync::atomic::{AtomicU32, Ordering},
};

use chrono::Utc;
use client_state::ClientState;
use error::RoomError;
use message::{ClientMessageType, ServerMessageType};
use presence::{PresenceError, PresenceLike};
use serde::Serialize;
use storage::{StorageError, StorageLike};
use tokio::sync::{RwLock, broadcast};
use ts_rs::TS;
use uuid::Uuid;

const DEFAULT_CHANNEL_CAPACITY: usize = 100;

// --- Error Enum ---

pub trait RoomIdLike:
    Eq + Hash + Clone + Send + Sync + 'static + fmt::Display + Serialize + Debug + TS + From<String>
{
}
pub trait ClientIdLike:
    Eq
    + Hash
    + Clone
    + Send
    + Sync
    + 'static
    + fmt::Display
    + Serialize
    + Debug
    + TS
    + From<Uuid>
    + Copy
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
    ) -> Result<Presence, PresenceError> {
        let mut clients_guard = self.clients.write().await;
        match clients_guard.get_mut(&client_id) {
            Some(client_state) => {
                let (_, presence) = client_state.presence_mut().apply_update(update)?;
                return Ok(presence);
            }
            None => {
                return Err(PresenceError::ClientNotFound);
            }
        }
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
            ClientMessageType::UpdatePresence { presence } => {
                // Handle presence update
                let updated_presence = self
                    .handle_presence_update(*client_id, presence.clone())
                    .await?;

                Ok(Some(ServerMessageType::PresenceUpdated {
                    client_id: *client_id,
                    timestamp: Utc::now(),
                    presence: updated_presence,
                }))
            }

            ClientMessageType::UpdateStorage { operations } => {
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
            }

            ClientMessageType::JoinRoom { room_id } => Ok(Some(ServerMessageType::RoomJoined {
                room_id,
                client_id: *client_id,
            })),

            ClientMessageType::LeaveRoom => {
                // Note: This requires access to the RoomsManager, so we should move this logic
                // to the RoomsManager's handle_client_message method instead
                Err(RoomError::InvalidMessage)
            }

            ClientMessageType::_Phantom(_) => unreachable!(),
        }
    }
}
