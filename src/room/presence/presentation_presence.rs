use super::{PresenceLike, PresenceError};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
pub struct Point {
    x: f64,
    y: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
pub struct CursorState {
    position: Option<Point>,
    color: String,
    is_dragging: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
pub struct UserState {
    name: String,
    avatar_url: Option<String>,
    is_typing: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct PresentationPresence {
    cursor: CursorState,
    user: UserState,
    #[serde(skip)]
    last_updated: DateTime<Utc>,
}

impl Default for PresentationPresence {
    fn default() -> Self {
        let now = std::time::SystemTime::now();
        Self { 
            cursor: CursorState {
                position: None,
                color: "#000000".to_string(),
                is_dragging: false,
            },
            user: UserState {
                name: "Anonymous".to_string(),
                avatar_url: None,
                is_typing: false,
            },
            last_updated: DateTime::<Utc>::from(now),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub enum PresentationPresenceUpdate {
    Move(Point),
    Hide,
    SetColor(String),
    SetDragging(bool),
    SetName(String),
    SetAvatar(String),
    SetTyping(bool),
}

impl PresenceLike for PresentationPresence {
    type Update = PresentationPresenceUpdate;

    fn apply_update(&mut self, update: Self::Update) -> Result<(bool, Self), PresenceError> {
        let changed = match update {
            PresentationPresenceUpdate::Move(new_pos) => {
                if self.cursor.position.as_ref() != Some(&new_pos) {
                    self.cursor.position = Some(new_pos);
                    true
                } else {
                    false
                }
            }
            PresentationPresenceUpdate::Hide => {
                if self.cursor.position.is_some() {
                    self.cursor.position = None;
                    true
                } else {
                    false
                }
            }
            PresentationPresenceUpdate::SetColor(color) => {
                if self.cursor.color != color {
                    self.cursor.color = color;
                    true
                } else {
                    false
                }
            }
            PresentationPresenceUpdate::SetDragging(is_dragging) => {
                if self.cursor.is_dragging != is_dragging {
                    self.cursor.is_dragging = is_dragging;
                    true
                } else {
                    false
                }
            }
            PresentationPresenceUpdate::SetName(name) => {
                if self.user.name != name {
                    self.user.name = name;
                    true
                } else {
                    false
                }
            }
            PresentationPresenceUpdate::SetAvatar(avatar_url) => {
                if self.user.avatar_url.as_deref() != Some(&avatar_url) {
                    self.user.avatar_url = Some(avatar_url);
                    true
                } else {
                    false
                }
            }
            PresentationPresenceUpdate::SetTyping(is_typing) => {
                if self.user.is_typing != is_typing {
                    self.user.is_typing = is_typing;
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
        Ok((changed, self.clone()))
    }

    fn last_updated(&self) -> DateTime<Utc> {
        self.last_updated
    }
}

