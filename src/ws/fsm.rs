use std::time::Duration;
use tokio::time::Instant;
use tokio::sync::Mutex;
use std::sync::Arc;
use tracing::{debug, error, info, trace, warn};

// --- FSM Types ---

#[derive(Debug, Clone)]
pub enum ConnectionState {
    Idle(IdleState),
    Auth(AuthState),
    Connecting(ConnectingState),
    Ok(OkState),
}

#[derive(Debug, Clone)]
pub enum IdleState {
    Initial,
    Failed,
}

#[derive(Debug, Clone)]
pub enum AuthState {
    Busy,
    Backoff { retries: u32, until: Instant },
}

#[derive(Debug, Clone)]
pub enum ConnectingState {
    Busy,
    Backoff { retries: u32, until: Instant },
}

#[derive(Debug, Clone)]
pub enum OkState {
    Connected,
    AwaitingPong { since: Instant },
}

#[derive(Debug, Clone)]
pub enum Event {
    Connect,
    Disconnect,
    Reconnect,
    Failure,
    Success,
    Unauthorized,
    AfterBackoffDelay,
    AfterKeepaliveInterval,
    Pong,
    Close,
}

// --- FSM Implementation ---

impl ConnectionState {
    pub fn on_event(self, event: Event) -> ConnectionState {
        match (self, event) {
            // Idle state transitions
            (ConnectionState::Idle(IdleState::Initial), Event::Connect) => ConnectionState::Auth(AuthState::Busy),
            (ConnectionState::Idle(_), Event::Unauthorized) => ConnectionState::Idle(IdleState::Failed),
            (ConnectionState::Idle(_), Event::Disconnect) => ConnectionState::Idle(IdleState::Initial),
            (ConnectionState::Idle(_), Event::Reconnect) => ConnectionState::Auth(AuthState::Busy),

            // Auth state transitions
            (ConnectionState::Auth(AuthState::Busy), Event::Failure) => ConnectionState::Auth(AuthState::Backoff { retries: 1, until: Instant::now() + Duration::from_secs(2) }),
            (ConnectionState::Auth(AuthState::Backoff { retries, .. }), Event::AfterBackoffDelay) => ConnectionState::Auth(AuthState::Busy),
            (ConnectionState::Auth(_), Event::Success) => ConnectionState::Connecting(ConnectingState::Busy),
            (ConnectionState::Auth(_), Event::Unauthorized) => ConnectionState::Idle(IdleState::Failed),

            // Connecting state transitions
            (ConnectionState::Connecting(ConnectingState::Busy), Event::Failure) => ConnectionState::Connecting(ConnectingState::Backoff { retries: 1, until: Instant::now() + Duration::from_secs(2) }),
            (ConnectionState::Connecting(ConnectingState::Backoff { retries, .. }), Event::AfterBackoffDelay) => ConnectionState::Connecting(ConnectingState::Busy),
            (ConnectionState::Connecting(_), Event::Success) => ConnectionState::Ok(OkState::Connected),
            (ConnectionState::Connecting(_), Event::Unauthorized) => ConnectionState::Auth(AuthState::Busy),

            // Ok state transitions
            (ConnectionState::Ok(OkState::Connected), Event::AfterKeepaliveInterval) => ConnectionState::Ok(OkState::AwaitingPong { since: Instant::now() }),
            (ConnectionState::Ok(OkState::AwaitingPong { .. }), Event::Pong) => ConnectionState::Ok(OkState::Connected),
            (ConnectionState::Ok(_), Event::Close) => ConnectionState::Connecting(ConnectingState::Busy),

            // Default: remain in current state
            (state, _) => state,
        }
    }

    pub fn needs_backoff(&self) -> Option<Instant> {
        match self {
            ConnectionState::Auth(AuthState::Backoff { until, .. }) |
            ConnectionState::Connecting(ConnectingState::Backoff { until, .. }) => Some(*until),
            _ => None,
        }
    }

    pub fn is_awaiting_pong(&self) -> bool {
        matches!(self, ConnectionState::Ok(OkState::AwaitingPong { .. }))
    }

    pub fn is_failed(&self) -> bool {
        matches!(self, ConnectionState::Idle(IdleState::Failed))
    }
}

// --- FSM Manager ---

#[derive(Clone)]
pub struct FsmManager {
    state: Arc<Mutex<ConnectionState>>,
    last_pong: Arc<Mutex<Option<Instant>>>,
    heartbeat_interval: Duration,
    heartbeat_timeout: Duration,
}

impl FsmManager {
    pub fn new(heartbeat_interval: Duration, heartbeat_timeout: Duration) -> Self {
        Self {
            state: Arc::new(Mutex::new(ConnectionState::Idle(IdleState::Initial))),
            last_pong: Arc::new(Mutex::new(None)),
            heartbeat_interval,
            heartbeat_timeout,
        }
    }

    pub async fn state(&self) -> ConnectionState {
        self.state.lock().await.clone()
    }

    pub async fn update_state(&self, event: Event) {
        let mut state = self.state.lock().await;
        *state = state.clone().on_event(event);
    }

    pub async fn handle_pong(&self) {
        *self.last_pong.lock().await = Some(Instant::now());
        self.update_state(Event::Pong).await;
    }

    pub async fn check_heartbeat(&self) -> bool {
        if let Some(last_pong) = *self.last_pong.lock().await {
            if last_pong.elapsed() > self.heartbeat_timeout {
                self.update_state(Event::Failure).await;
                return false;
            }
        }
        true
    }

    pub async fn needs_backoff(&self) -> Option<Instant> {
        self.state.lock().await.needs_backoff()
    }

    pub async fn is_awaiting_pong(&self) -> bool {
        self.state.lock().await.is_awaiting_pong()
    }

    pub async fn is_failed(&self) -> bool {
        self.state.lock().await.is_failed()
    }
} 