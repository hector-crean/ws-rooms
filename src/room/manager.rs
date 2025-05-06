// --- Updated RoomsManager ---

use chrono::Utc;
use serde;
use std::{collections::HashMap, sync::Arc};
use tokio::sync::RwLock;
use tokio::sync::broadcast;
use ts_rs::TS;
use uuid::Uuid;

use super::DEFAULT_CHANNEL_CAPACITY;
use super::error::RoomError;
use super::message::ClientMessageType;
use super::message::ServerMessageType;
use super::storage::SharedPresentation;
use super::subscription::UserSubscription;
use super::{
    ClientIdLike, Room, RoomIdLike,
    presence::{PresenceLike, presentation_presence::PresentationPresence},
    storage::StorageLike,
};

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
        >,
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

    pub fn user_channels(
        &self,
    ) -> &RwLock<
        HashMap<
            ClientId,
            broadcast::Sender<ServerMessageType<RoomId, ClientId, Presence, Storage>>,
        >,
    > {
        &self.user_channels
    }

    async fn handle_presence_update(
        &self,
        client_id: &ClientId,
        update: Presence::Update,
    ) -> Result<(), RoomError> {
        // First get the client's current room
        let room_id = self.get_client_room_id(client_id).await?;

        // Then get the room and apply the update
        let room = self.get_room(&room_id).await?;

        // Apply the presence update
        let updated_presence = room.handle_presence_update(*client_id, update.clone())
            .await?;

        
        let msg = ServerMessageType::PresenceUpdated {
            client_id: *client_id,
            timestamp: Utc::now(),
            presence: updated_presence,
        };


        // Broadcast the update to all clients in the room
        tracing::debug!(%client_id, %room_id, ?msg, "Broadcasting presence update");
        self.broadcast_message(
            &room,
            msg,
            client_id,
            &room_id,
        )
        .await?;

        Ok(())
    }

    async fn handle_storage_update(
        &self,
        client_id: &ClientId,
        operations: Vec<Storage::Operation>,
    ) -> Result<(), RoomError> {
        let room_id = self.get_client_room_id(client_id).await?;
        let room = self.get_room(&room_id).await?;

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
            self.broadcast_message(
                &room,
                ServerMessageType::StorageUpdated {
                    version: storage.version(),
                    operations: applied_ops,
                },
                client_id,
                &room_id,
            )
            .await?;
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
                self.join_room(&room_id, client_id).await
            }
            ClientMessageType::LeaveRoom => {
                let room_id = self.get_client_room_id(client_id).await?;
                tracing::info!(%client_id, %room_id, "Client leaving room");
                self.leave_room(&room_id, client_id).await
            }
            ClientMessageType::UpdatePresence { presence } => {
                tracing::debug!(%client_id, ?presence, "Client updating presence");
                self.handle_presence_update(client_id, presence).await?;
                Ok(())
            }
            ClientMessageType::UpdateStorage { operations } => {
                tracing::debug!(%client_id, operation_count = operations.len(), "Client updating storage");
                self.handle_storage_update(client_id, operations).await?;
                Ok(())
            }
            _ => {
                let room_id = self.get_client_room_id(client_id).await?;
                let room = self.get_room(&room_id).await?;

                tracing::debug!(%client_id, %room_id, ?message, "Processing client message");
                if let Some(server_msg) = room.process_client_message(client_id, message).await? {
                    tracing::debug!(%client_id, %room_id, ?server_msg, "Broadcasting server message");
                    self.broadcast_message(&room, server_msg, client_id, &room_id)
                        .await?;
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
            .entry(client_id)
            .or_insert_with(|| {
                // Optional: Log user initialization
                // println!("Initializing user channel for: {}", client_id);
                let (tx, _) = broadcast::channel(channel_capacity);
                tx
            });

        let receiver = sender.subscribe();

        Ok(UserSubscription::new(client_id, Arc::clone(self), receiver))

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

    /// Sends a message to all clients in a specific room.
    /// Fails if the room does not exist.
    pub async fn send_message_to_room(
        &self,
        room_id: &RoomId,
        message: ServerMessageType<RoomId, ClientId, Presence, Storage>,
    ) -> Result<usize, RoomError> {
        let room = self.get_room(room_id).await?;
        room.broadcast(message)
            .map_err(|e| RoomError::MessageSendFail(Box::new(e)))
    }

    /// Cleans up a user, removing them from all rooms and their user channel map entry *if*
    /// this is the last reference (tracked implicitly by Arc on the Room and Sender).
    /// Called automatically when `UserSubscription` is dropped.
    pub async fn cleanup_user(&self, client_id: &ClientId) {
        // Use async operations instead of blocking ones
        let rooms_guard = self.rooms.read().await;
        for room in rooms_guard.values() {
            room.leave(client_id).await;
        }
        drop(rooms_guard);

        // Clean up client_rooms mapping
        let _ = self.client_rooms.write().await.remove(client_id);

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

    /// Helper method to get a room by ID with proper error handling
    pub async fn get_room(
        &self,
        room_id: &RoomId,
    ) -> Result<Arc<Room<RoomId, ClientId, Presence, Storage>>, RoomError> {
        let rooms_guard = self.rooms.read().await;
        rooms_guard
            .get(room_id)
            .cloned()
            .ok_or_else(|| RoomError::RoomNotFound(room_id.to_string()))
    }

    /// Helper method to get a client's current room ID
    pub async fn get_client_room_id(&self, client_id: &ClientId) -> Result<RoomId, RoomError> {
        let client_rooms_guard = self.client_rooms.read().await;
        client_rooms_guard
            .get(client_id)
            .cloned()
            .ok_or_else(|| RoomError::UserNotInRoom(client_id.to_string()))
    }

    /// Helper method to get a user's sender channel
    pub async fn get_user_sender(
        &self,
        client_id: &ClientId,
    ) -> Result<broadcast::Sender<ServerMessageType<RoomId, ClientId, Presence, Storage>>, RoomError>
    {
        let user_channels_guard = self.user_channels.read().await;
        user_channels_guard
            .get(client_id)
            .cloned()
            .ok_or_else(|| RoomError::UserNotInitialized(client_id.to_string()))
    }

    /// Helper method to broadcast a message with proper error handling
    pub async fn broadcast_message(
        &self,
        room: &Arc<Room<RoomId, ClientId, Presence, Storage>>,
        message: ServerMessageType<RoomId, ClientId, Presence, Storage>,
        client_id: &ClientId,
        room_id: &RoomId,
    ) -> Result<(), RoomError> {
        room.broadcast(message).map_err(|e| {
            tracing::error!(%client_id, %room_id, error = %e, "Failed to broadcast message");
            RoomError::MessageSendFail(Box::new(e))
        })?;
        Ok(())
    }

    pub async fn join_room(&self, room_id: &RoomId, client_id: &ClientId) -> Result<(), RoomError> {
        let room = self.get_room(room_id).await?;
        let user_sender = self.get_user_sender(client_id).await?;

        // Update client_rooms mapping
        {
            let mut client_rooms_guard = self.client_rooms.write().await;
            if let Some(old_room_id) = client_rooms_guard.get(client_id) {
                return Err(RoomError::UserAlreadyInRoom(
                    client_id.to_string(),
                    old_room_id.to_string(),
                ));
            }
            client_rooms_guard.insert(*client_id, room_id.clone());
        }

        room.join(*client_id, user_sender).await;
        Ok(())
    }

    pub async fn leave_room(
        &self,
        room_id: &RoomId,
        client_id: &ClientId,
    ) -> Result<(), RoomError> {
        let room = self.get_room(room_id).await?;

        // Remove from client_rooms mapping
        {
            let mut client_rooms_guard = self.client_rooms.write().await;
            client_rooms_guard.remove(client_id);
        }

        // Check if user channel still exists
        if !self.user_channels.read().await.contains_key(client_id) {
            return Err(RoomError::UserNotInitialized(client_id.to_string()));
        }

        room.leave(client_id).await;
        Ok(())
    }

    pub async fn join_or_create_room(
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

        let user_sender = self.get_user_sender(client_id).await?;

        // Update client_rooms mapping
        {
            let mut client_rooms_guard = self.client_rooms.write().await;
            if let Some(old_room_id) = client_rooms_guard.get(client_id) {
                return Err(RoomError::UserAlreadyInRoom(
                    client_id.to_string(),
                    old_room_id.to_string(),
                ));
            }
            client_rooms_guard.insert(*client_id, room_id.clone());
        }

        room.join(*client_id, user_sender).await;
        Ok(())
    }

    /// Gets detailed information about a specific room.
    /// Returns an error if the room doesn't exist.
    pub async fn get_room_details(
        &self,
        room_id: &RoomId,
    ) -> Result<RoomDetails<RoomId, ClientId, Presence, Storage>, RoomError> {
        let rooms_guard = self.rooms.read().await;
        let room = rooms_guard
            .get(room_id)
            .ok_or_else(|| RoomError::RoomNotFound(room_id.to_string()))?;

        let clients_guard = room.clients.read().await;
        let storage_guard = room.storage.read().await;

        // Collect client IDs and their presences
        let mut client_presences = HashMap::new();
        let mut client_ids = Vec::with_capacity(clients_guard.len());

        for (client_id, client_state) in clients_guard.iter() {
            client_ids.push(*client_id);
            client_presences.insert(*client_id, client_state.presence().clone());
        }

        Ok(RoomDetails {
            room_id: room_id.clone(),
            subscriber_count: room.subscriber_count(),
            client_ids,
            storage: storage_guard.clone(),
            client_presences,
        })
    }

    /// Lists all rooms with their basic information.
    pub async fn list_rooms_with_details(
        &self,
    ) -> Vec<RoomDetails<RoomId, ClientId, Presence, Storage>> {
        let rooms_guard = self.rooms.read().await;
        let mut details = Vec::with_capacity(rooms_guard.len());

        for (room_id, room) in rooms_guard.iter() {
            let clients_guard = room.clients.read().await;
            let storage_guard = room.storage.read().await;

            // Collect client IDs and their presences
            let mut client_presences = HashMap::new();
            let mut client_ids = Vec::with_capacity(clients_guard.len());

            for (client_id, client_state) in clients_guard.iter() {
                client_ids.push(*client_id);
                client_presences.insert(*client_id, client_state.presence().clone());
            }

            details.push(RoomDetails {
                room_id: room_id.clone(),
                subscriber_count: room.subscriber_count(),
                client_ids,
                storage: storage_guard.clone(),
                client_presences,
            });
        }

        details
    }

    /// Deletes a room and all its associated resources.
    /// Returns true if the room was deleted, false if it didn't exist.
    pub async fn delete_room(&self, room_id: &RoomId) -> Result<bool, RoomError> {
        let mut rooms_guard = self.rooms.write().await;
        if let Some(room_arc) = rooms_guard.remove(room_id) {
            // Clear the room's resources
            room_arc.clear().await;

            // Remove all client-room mappings for this room
            let mut client_rooms_guard = self.client_rooms.write().await;
            client_rooms_guard.retain(|_, r| r != room_id);

            Ok(true)
        } else {
            Ok(false)
        }
    }
    pub async fn cleanup_disconnected_clients(&self) {
        // Get all clients that have a room mapping but no active channel
        let disconnected_clients = {
            let client_rooms = self.client_rooms.read().await;
            let user_channels = self.user_channels.read().await;

            client_rooms
                .keys()
                .filter(|client_id| !user_channels.contains_key(*client_id))
                .cloned()
                .collect::<Vec<ClientId>>()
        };

        // Process each disconnected client
        for client_id in disconnected_clients {
            tracing::info!(%client_id, "Cleaning up disconnected client");

            // Get the room this client was in
            let room_id = {
                let client_rooms = self.client_rooms.read().await;
                match client_rooms.get(&client_id) {
                    Some(room_id) => room_id.clone(),
                    None => continue, // Client no longer has a room mapping
                }
            };

            // Get the room and remove the client
            if let Ok(room) = self.get_room(&room_id).await {
                // Remove client from room
                room.leave(&client_id).await;

                // Remove client-room mapping
                {
                    let mut client_rooms_guard = self.client_rooms.write().await;
                    client_rooms_guard.remove(&client_id);
                }

                // Broadcast that the client has left
                let _ = self
                    .broadcast_message(
                        &room,
                        ServerMessageType::RoomLeft {
                            room_id: room_id.clone(),
                            client_id,
                        },
                        &client_id,
                        &room_id,
                    )
                    .await;

                tracing::info!(%client_id, %room_id, "Client removed from room");
            }
        }
    }
}

/// Detailed information about a room
#[derive(Debug, Clone, serde::Serialize, TS)]
#[ts(export)]
#[ts(concrete(RoomId = String, ClientId = Uuid, Presence = PresentationPresence, Storage =  SharedPresentation))]
pub struct RoomDetails<RoomId, ClientId, Presence, Storage>
where
    RoomId: RoomIdLike + serde::Serialize,
    ClientId: ClientIdLike + serde::Serialize,
    Presence: PresenceLike + serde::Serialize,
    Storage: StorageLike + serde::Serialize,
{
    pub room_id: RoomId,
    pub subscriber_count: u32,
    pub client_ids: Vec<ClientId>,
    pub storage: Storage,
    pub client_presences: HashMap<ClientId, Presence>,
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
