//! Subsystem trait and health tracking.
//!
//! Every subsystem (audio, Spotify, Bluetooth, etc.) implements the
//! [`Subsystem`] trait. [`SubsystemHealth`] provides atomic state,
//! heartbeat, and message count tracking for the lifecycle manager.

use anyhow::Result;
use encore_common::protocol::SubsystemState;
use std::sync::atomic::{AtomicU64, AtomicU8, Ordering};
use std::sync::Arc;
use tokio::sync::broadcast;

/// Context provided to each subsystem at startup
pub struct SubsystemContext {
    /// Subsystem name (for logging and status)
    pub name: &'static str,
    /// Shutdown signal -- subsystem should stop when this fires
    pub shutdown: broadcast::Receiver<()>,
    /// Health reporting -- subsystem sends heartbeats here
    pub health: Arc<SubsystemHealth>,
}

/// Atomic health counters -- read by watchdog and web dashboard
pub struct SubsystemHealth {
    pub heartbeat: AtomicU64,
    pub state: AtomicU8,
    pub restart_count: AtomicU8,
    pub msg_count: AtomicU64,
    /// Monotonic instant when the subsystem was first started (for uptime)
    pub started_at: std::time::Instant,
}

impl SubsystemHealth {
    pub fn new() -> Self {
        Self {
            heartbeat: AtomicU64::new(0),
            state: AtomicU8::new(SubsystemState::Starting as u8),
            restart_count: AtomicU8::new(0),
            msg_count: AtomicU64::new(0),
            started_at: std::time::Instant::now(),
        }
    }

    pub fn beat(&self, now: u64) {
        self.heartbeat.store(now, Ordering::Relaxed);
    }

    pub fn set_state(&self, state: SubsystemState) {
        self.state.store(state as u8, Ordering::Relaxed);
    }

    pub fn inc_msg(&self) {
        self.msg_count.fetch_add(1, Ordering::Relaxed);
    }
}

/// The trait every subsystem implements
#[async_trait::async_trait]
pub trait Subsystem: Send {
    /// Human-readable name
    fn name(&self) -> &'static str;

    /// Run the subsystem. Returns when shutdown signal received or on error.
    async fn run(&mut self, ctx: SubsystemContext) -> Result<()>;

    /// Graceful shutdown. Called with a 5-second timeout.
    async fn shutdown(&mut self) -> Result<()> {
        Ok(()) // default: no cleanup needed
    }

    /// Whether this subsystem is vital (never give up restarting)
    fn is_vital(&self) -> bool {
        encore_common::status::is_vital(self.name())
    }
}
