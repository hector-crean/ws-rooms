use super::{PresenceLike, PresenceError};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use ts_rs::TS;
use std::time::Instant;

// Example implementation for cursor presence:
#[derive(Debug, Clone, Serialize, TS, Deserialize)]
pub struct CursorPresence {
    position: Option<Point>,
    #[serde(skip)]
    last_updated: DateTime<Utc>,
}

impl Default for CursorPresence {
    fn default() -> Self {
        let now = std::time::SystemTime::now();
        Self { 
            position: None, 
            last_updated: DateTime::<Utc>::from(now),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
pub struct Point {
    x: f64,
    y: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
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
            let now = std::time::SystemTime::now();
            self.last_updated = DateTime::<Utc>::from(now);
        }
        Ok(changed)
    }

    fn last_updated(&self) -> DateTime<Utc> {
        self.last_updated
    }
}

