use super::{StorageError, StorageLike};
use serde::{Deserialize, Serialize};
use std::fmt::Debug;


#[derive(Debug, Serialize, Deserialize, Clone)]
pub enum Operation<T> {
    // Atomic operations that can be applied to our storage
    Insert { index: usize, value: T },
    Delete { index: usize },
    Update { index: usize, value: T },
    // Could add more complex operations like:
    Batch(Vec<Operation<T>>),  // Multiple operations as one
    Move { from: usize, to: usize },
}

#[derive(Debug, Clone)]
pub struct SharedList<T> {
    items: Vec<T>,
    version: u64,
}

impl<T: Clone + Debug + Serialize + for<'de> Deserialize<'de> + Send + Sync + 'static> StorageLike for SharedList<T> {
    type Operation = Operation<T>;
    type Snapshot = Vec<T>;
    type Version = u64;

    fn version(&self) -> Self::Version {
        self.version
    }

    fn apply_operation(&mut self, op: Self::Operation) -> Result<Self::Version, StorageError> {
        match op {
            Operation::Insert { index, value } => {
                if index <= self.items.len() {
                    self.items.insert(index, value);
                    self.version += 1;
                    Ok(self.version)
                } else {
                    Err(StorageError::InvalidOperation("Index out of bounds".into()))
                }
            },
            Operation::Delete { index } => {
                if index < self.items.len() {
                    self.items.remove(index);
                    self.version += 1;
                    Ok(self.version)
                } else {
                    Err(StorageError::InvalidOperation("Index out of bounds".into()))
                }
            },
            Operation::Update { index, value } => {
                if index < self.items.len() {
                    self.items[index] = value;
                    self.version += 1;
                    Ok(self.version)
                } else {
                    Err(StorageError::InvalidOperation("Index out of bounds".into()))
                }
            },
            Operation::Batch(ops) => {
                for op in ops {
                    self.apply_operation(op)?;
                }
                Ok(self.version)
            },
            Operation::Move { from, to } => {
                if from < self.items.len() && to < self.items.len() {
                    let item = self.items.remove(from);
                    self.items.insert(to, item);
                    self.version += 1;
                    Ok(self.version)
                } else {
                    Err(StorageError::InvalidOperation("Index out of bounds".into()))
                }
            },
        }
    }
    fn get_snapshot(&self) -> Result<Self::Snapshot, StorageError> {
        Ok(self.items.clone())
    }
    fn from_snapshot(snapshot: Self::Snapshot) -> Result<Self, StorageError> {
        Ok(SharedList { items: snapshot, version: 0 })
    }
    
}

impl<T: Clone + Debug + Serialize + for<'de> Deserialize<'de> + Send + Sync + 'static> Default for SharedList<T> {
    fn default() -> Self {
        Self { items: Vec::new(), version: 0 }
    }
}