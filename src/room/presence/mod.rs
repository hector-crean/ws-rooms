pub mod presentation_presence;

use std::time::Duration;
use chrono::{DateTime, Utc};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use ts_rs::TS;
use std::fmt::Debug;


#[derive(thiserror::Error, Debug)]
pub enum PresenceError {
    #[error("Invalid presence update: {0}")]
    InvalidUpdate(String),
    #[error("Stale update (older than current)")]
    StaleUpdate,
    #[error("Client not found")]
    ClientNotFound,
}

pub trait PresenceLike: Send + Sync + Clone + Debug + Default + 'static + serde::Serialize + TS {
    /// The data structure for presence updates (e.g., cursor position, status)
    type Update: Serialize + DeserializeOwned + Clone + Debug + Send + Sync + TS;

    /// Apply an update to the presence state
    /// Returns whether the state actually changed
    fn apply_update(&mut self, update: Self::Update) -> Result<(bool, Self), PresenceError>;

    /// Get the last time this presence was updated
    fn last_updated(&self) -> DateTime<Utc>;

    /// Check if this presence is still considered active
    fn is_active(&self, timeout: Duration) -> bool {
        Utc::now().signed_duration_since(self.last_updated()) < chrono::Duration::from_std(timeout).unwrap()
    }
}

