
use super::{PresenceLike, PresenceError};
use serde::{Deserialize, Serialize};
use std::time::Instant;

// Example implementation for cursor presence:
#[derive(Debug, Clone)]
pub struct CursorPresence {
    position: Option<Point>,
    last_updated: Instant,
}

impl Default for CursorPresence {
    fn default() -> Self {
        Self { position: None, last_updated: Instant::now() }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Point {
    x: f64,
    y: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum CursorUpdate {
    Move(Point),
    Hide,
}

impl PresenceLike for CursorPresence {
    type Update = CursorUpdate;

    fn apply_update(&mut self, update: Self::Update) -> Result<bool, PresenceError> {
        let changed = match update {
            CursorUpdate::Move(new_pos) => {
                if self.position.as_ref() != Some(&new_pos) {
                    self.position = Some(new_pos);
                    true
                } else {
                    false
                }
            }
            CursorUpdate::Hide => {
                if self.position.is_some() {
                    self.position = None;
                    true
                } else {
                    false
                }
            }
        };

        if changed {
            self.last_updated = Instant::now();
        }
        Ok(changed)
    }

    fn last_updated(&self) -> Instant {
        self.last_updated
    }
}

