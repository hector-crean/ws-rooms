pub mod shared_list;
pub mod yrs_storage;

pub use shared_list::SharedList;
use ts_rs::TS;
pub use yrs_storage::YrsStorage;

use serde::{Deserialize, Serialize};
use std::fmt::Debug;

#[derive(thiserror::Error, Debug)]
pub enum StorageError {
    #[error("Failed to apply operation: {0}")]
    ApplyError(String),
    #[error("Failed to merge states: {0}")]
    MergeError(String),
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

pub trait StorageLike: Default + Clone + Send + Sync + 'static + Serialize + for<'de> Deserialize<'de>+ Debug + TS {
    type Version: Debug + Clone + Send + Sync + 'static + Serialize + for<'de> Deserialize<'de> + TS;
    type Operation: Debug + Clone + Send + Sync + 'static + Serialize + for<'de> Deserialize<'de> + TS;

    fn version(&self) -> Self::Version;
    fn apply_operation(&mut self, operation: Self::Operation) -> Result<Self::Version, StorageError>;
    fn merge(&mut self, other: &Self) -> Result<(), StorageError>;
}





