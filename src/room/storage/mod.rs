pub mod shared_list;
use serde::{Deserialize, Serialize};
use std::fmt::Debug;

#[derive(thiserror::Error, Debug)]
pub enum StorageError {
    #[error("Merge conflict: {0}")]
    MergeConflict(String),
    #[error("Failed to apply diff: {0}")]
    ApplyDiffError(String),
    #[error("Serialization error: {0}")]
    SerializationError(#[from] serde_json::Error),
    #[error("Incompatible storage types")]
    IncompatibleTypes,
    #[error("Invalid operation: {0}")]
    InvalidOperation(String),
    #[error("Invalid snapshot: {0}")]
    InvalidSnapshot(String),
    #[error("Invalid from snapshot: {0}")]
    InvalidFromSnapshot(String),
    // Add other specific storage errors as needed
}



pub trait StorageLike: Send + Sync + Clone + Debug + 'static + Default {
    /// The type representing a change/operation to the storage
    type Version: Serialize + for<'de> Deserialize<'de> + Send + Sync + Debug + Clone;
    type Operation: Serialize + for<'de> Deserialize<'de> + Send + Sync + Debug + Clone;
    
    /// The type representing the full state that can be sent to clients
    type Snapshot: Serialize + for<'de> Deserialize<'de> + Send + Sync + Debug + Clone;

    /// Current version of the storage, used for reconciliation
    fn version(&self) -> Self::Version;

    /// Apply an operation and return the new version
    fn apply_operation(&mut self, op: Self::Operation) -> Result<Self::Version, StorageError>;

    /// Get current state as a snapshot (for new clients or full sync)
    fn get_snapshot(&self) -> Result<Self::Snapshot, StorageError>;

    /// Initialize from a snapshot (for client-side initialization)
    fn from_snapshot(snapshot: Self::Snapshot) -> Result<Self, StorageError>
    where
        Self: Sized;
}





