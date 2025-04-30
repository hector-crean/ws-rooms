pub mod cursor_presence;

use std::time::Duration;
use serde::{Deserialize, Serialize};
use std::fmt::Debug;
use std::time::Instant;


#[derive(thiserror::Error, Debug)]
pub enum PresenceError {
    #[error("Invalid presence update: {0}")]
    InvalidUpdate(String),
    #[error("Stale update (older than current)")]
    StaleUpdate,
}

pub trait PresenceLike: Send + Sync + Clone + Debug + Default + 'static {
    /// The data structure for presence updates (e.g., cursor position, status)
    type Update: Serialize + for<'de> Deserialize<'de> + Clone + Debug + Send + Sync;

    /// Apply an update to the presence state
    /// Returns whether the state actually changed
    fn apply_update(&mut self, update: Self::Update) -> Result<bool, PresenceError>;

    /// Get the last time this presence was updated
    fn last_updated(&self) -> Instant;

    /// Check if this presence is still considered active
    fn is_active(&self, timeout: Duration) -> bool {
        self.last_updated().elapsed() < timeout
    }
}