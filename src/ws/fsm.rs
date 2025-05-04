use std::time::Duration;
use tokio::time::Instant;
use tokio::sync::Mutex;
use std::sync::Arc;
use tracing::{debug, error, info, trace, warn};

// --- FSM Types ---

#[derive(Debug, Clone, PartialEq)]
pub enum ConnectionState {
    Idle(IdleState),
    Auth(AuthState),
    Connecting(ConnectingState),
    Ok(OkState),
}

#[derive(Debug, Clone, PartialEq)]
pub enum IdleState {
    Initial,
    Failed,
}

#[derive(Debug, Clone, PartialEq)]
pub enum AuthState {
    Busy,
    Backoff { retries: u32, until: Instant },
}

#[derive(Debug, Clone, PartialEq)]
pub enum ConnectingState {
    Busy,
    Backoff { retries: u32, until: Instant },
}

#[derive(Debug, Clone, PartialEq)]
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
        let new_state = match (self.clone(), event.clone()) {
            // Idle state transitions
            (ConnectionState::Idle(IdleState::Initial), Event::Connect) => {
                info!("🔄 Transition: Initial → Auth (Busy)");
                ConnectionState::Auth(AuthState::Busy)
            },
            (ConnectionState::Idle(_), Event::Unauthorized) => {
                error!("❌ Transition: Idle → Failed (Unauthorized)");
                ConnectionState::Idle(IdleState::Failed)
            },
            (ConnectionState::Idle(_), Event::Disconnect) => {
                info!("🔄 Transition: Idle → Initial");
                ConnectionState::Idle(IdleState::Initial)
            },
            (ConnectionState::Idle(_), Event::Reconnect) => {
                info!("🔄 Transition: Idle → Auth (Busy)");
                ConnectionState::Auth(AuthState::Busy)
            },

            // Auth state transitions
            (ConnectionState::Auth(AuthState::Busy), Event::Failure) => {
                warn!("⚠️ Transition: Auth (Busy) → Auth (Backoff)");
                ConnectionState::Auth(AuthState::Backoff { retries: 1, until: Instant::now() + Duration::from_secs(2) })
            },
            (ConnectionState::Auth(AuthState::Backoff { retries, .. }), Event::AfterBackoffDelay) => {
                info!("🔄 Transition: Auth (Backoff) → Auth (Busy)");
                ConnectionState::Auth(AuthState::Busy)
            },
            (ConnectionState::Auth(_), Event::Success) => {
                info!("✅ Transition: Auth → Connecting (Busy)");
                ConnectionState::Connecting(ConnectingState::Busy)
            },
            (ConnectionState::Auth(_), Event::Unauthorized) => {
                error!("❌ Transition: Auth → Idle (Failed)");
                ConnectionState::Idle(IdleState::Failed)
            },

            // Connecting state transitions
            (ConnectionState::Connecting(ConnectingState::Busy), Event::Failure) => {
                warn!("⚠️ Transition: Connecting (Busy) → Connecting (Backoff)");
                ConnectionState::Connecting(ConnectingState::Backoff { retries: 1, until: Instant::now() + Duration::from_secs(2) })
            },
            (ConnectionState::Connecting(ConnectingState::Backoff { retries, .. }), Event::AfterBackoffDelay) => {
                info!("🔄 Transition: Connecting (Backoff) → Connecting (Busy)");
                ConnectionState::Connecting(ConnectingState::Busy)
            },
            (ConnectionState::Connecting(_), Event::Success) => {
                info!("✅ Transition: Connecting → Ok (Connected)");
                ConnectionState::Ok(OkState::Connected)
            },
            (ConnectionState::Connecting(_), Event::Unauthorized) => {
                warn!("⚠️ Transition: Connecting → Auth (Busy)");
                ConnectionState::Auth(AuthState::Busy)
            },

            // Ok state transitions
            (ConnectionState::Ok(OkState::Connected), Event::AfterKeepaliveInterval) => {
                debug!("💓 Transition: Ok (Connected) → Ok (AwaitingPong)");
                ConnectionState::Ok(OkState::AwaitingPong { since: Instant::now() })
            },
            (ConnectionState::Ok(OkState::AwaitingPong { .. }), Event::Pong) => {
                debug!("💓 Transition: Ok (AwaitingPong) → Ok (Connected)");
                ConnectionState::Ok(OkState::Connected)
            },
            (ConnectionState::Ok(_), Event::Close) => {
                warn!("⚠️ Transition: Ok → Connecting (Busy)");
                ConnectionState::Connecting(ConnectingState::Busy)
            },

            // Default: remain in current state
            (state, _) => state,
        };

        // Log the current state if it's different from the previous state
        if self != new_state {
            match &new_state {
                ConnectionState::Idle(IdleState::Initial) => info!("📱 Current State: Idle (Initial)"),
                ConnectionState::Idle(IdleState::Failed) => error!("❌ Current State: Idle (Failed)"),
                ConnectionState::Auth(AuthState::Busy) => info!("🔑 Current State: Auth (Busy)"),
                ConnectionState::Auth(AuthState::Backoff { retries, until }) => 
                    warn!("⏳ Current State: Auth (Backoff) - Retries: {}, Backoff until: {:?}", retries, until),
                ConnectionState::Connecting(ConnectingState::Busy) => info!("🔌 Current State: Connecting (Busy)"),
                ConnectionState::Connecting(ConnectingState::Backoff { retries, until }) => 
                    warn!("⏳ Current State: Connecting (Backoff) - Retries: {}, Backoff until: {:?}", retries, until),
                ConnectionState::Ok(OkState::Connected) => info!("✅ Current State: Ok (Connected)"),
                ConnectionState::Ok(OkState::AwaitingPong { since }) => 
                    debug!("💓 Current State: Ok (AwaitingPong) - Since: {:?}", since),
            }
        }

        new_state
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
        let old_state = state.clone();
        *state = state.clone().on_event(event);
        
        // Log state changes with additional context
        if old_state != *state {
            debug!("📊 FSM State Change: {:?} → {:?}", old_state, state);
        }
    }

    pub async fn handle_pong(&self) {
        *self.last_pong.lock().await = Some(Instant::now());
        debug!("💓 Received Pong");
        self.update_state(Event::Pong).await;
    }

    pub async fn check_heartbeat(&self) -> bool {
        if let Some(last_pong) = *self.last_pong.lock().await {
            if last_pong.elapsed() > self.heartbeat_timeout {
                error!("💔 Heartbeat timeout - Last pong: {:?}", last_pong);
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