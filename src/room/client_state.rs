use std::time::Instant;
use tokio::task::JoinHandle;
use chrono::{DateTime, Utc};
use crate::room::presence::PresenceLike;

#[derive(Debug)]
pub struct ClientState<P: PresenceLike> {
    forwarder: ClientForwarder,
    presence: P,
    last_seen: Instant,
    // Could add more client-specific state:
    connection_info: Option<ConnectionInfo>,
    permissions: ClientPermissions,
}

impl<P: PresenceLike> ClientState<P> {
    pub fn new(join_handle: JoinHandle<()>) -> Self {
        Self {
            forwarder: ClientForwarder { task_handle: join_handle },
            presence: P::default(),
            last_seen: Instant::now(),
            connection_info: None,
            permissions: ClientPermissions::default(),
        }
    }
    pub fn forwarder(&self) -> &ClientForwarder {
        &self.forwarder
    }
    pub fn forwarder_mut(&mut self) -> &mut ClientForwarder {
        &mut self.forwarder
    }
  
    
}

#[derive(Debug, Clone)]
pub struct ConnectionInfo {
    connected_at: DateTime<Utc>,
    client_metadata: Option<serde_json::Value>,
    connection_type: ConnectionType,
}

#[derive(Debug, Clone)]
pub enum ConnectionType {
    WebSocket,
    LongPolling,
    // ... other connection types
}

#[derive(Debug, Clone, Default)]
pub struct ClientPermissions {
    can_write: bool,
    can_admin: bool,
    // ... other permissions
}

#[derive(Debug)]
pub struct ClientForwarder {
    task_handle: JoinHandle<()>,
}

impl ClientForwarder {
    /// Aborts the associated forwarding task.
    pub fn abort(&self) {
        self.task_handle.abort();
    }
}
