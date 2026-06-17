//! Subsystem lifecycle manager.
//!
//! Runs each subsystem in a supervised task with automatic restart (up to
//! MAX_RESTARTS), crash logging, debug mode (hold/trace), and health
//! telemetry broadcast.

use crate::debugger::{DebugModes, CRASH_LOG};
use crate::subsystem::{Subsystem, SubsystemContext, SubsystemHealth};
use encore_common::protocol::{CrashSummary, DebugMode, SubsystemSnapshot, SubsystemState};
use futures::FutureExt;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{broadcast, mpsc};
use tracing::{error, info, warn};

const MAX_RESTARTS: u8 = 3;
const RESTART_DELAY: Duration = Duration::from_secs(1);
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(5);

pub struct SubsystemManager {
    shutdown_tx: broadcast::Sender<()>,
    status_tx: mpsc::Sender<SubsystemSnapshot>,
    status_rx: mpsc::Receiver<SubsystemSnapshot>,
    handles: Vec<tokio::task::JoinHandle<()>>,
    debug_modes: Arc<DebugModes>,
    /// Tracked subsystem health handles for periodic status broadcasting
    health_handles: Vec<(&'static str, Arc<SubsystemHealth>)>,
    /// WebSocket broadcast for real-time crash notifications
    ws_tx: Option<broadcast::Sender<String>>,
}

impl SubsystemManager {
    pub fn new() -> Self {
        let (shutdown_tx, _) = broadcast::channel(1);
        let (status_tx, status_rx) = mpsc::channel(256);
        Self {
            shutdown_tx,
            status_tx,
            status_rx,
            handles: Vec::new(),
            debug_modes: Arc::new(DebugModes::new()),
            health_handles: Vec::new(),
            ws_tx: None,
        }
    }

    /// Set the WebSocket broadcast channel for real-time crash notifications.
    pub fn set_ws_tx(&mut self, tx: broadcast::Sender<String>) {
        self.ws_tx = Some(tx);
    }

    /// Register and start a subsystem. Returns its health handle.
    pub fn start(&mut self, mut subsystem: Box<dyn Subsystem>) -> Arc<SubsystemHealth> {
        let health = Arc::new(SubsystemHealth::new());
        let health_clone = health.clone();
        let shutdown_tx = self.shutdown_tx.clone();
        let name = subsystem.name();
        let is_vital = subsystem.is_vital();
        let debug_modes = self.debug_modes.clone();
        let ws_tx = self.ws_tx.clone();

        // Track for periodic status broadcasting
        self.health_handles.push((name, health.clone()));

        let handle = tokio::spawn(async move {
            let mut restarts: u8 = 0;
            let max = if is_vital { u8::MAX } else { MAX_RESTARTS };

            loop {
                let ctx = SubsystemContext {
                    name,
                    shutdown: shutdown_tx.subscribe(),
                    health: health_clone.clone(),
                };

                health_clone.set_state(SubsystemState::Running);
                info!("[{}] starting (attempt {})", name, restarts + 1);

                let result = std::panic::AssertUnwindSafe(subsystem.run(ctx))
                    .catch_unwind()
                    .await;

                let error_msg = match &result {
                    Ok(Ok(())) => {
                        info!("[{}] exited cleanly", name);
                        health_clone.set_state(SubsystemState::Stopped);
                        break;
                    }
                    Ok(Err(e)) => {
                        let msg = format!("{:#}", e);
                        error!("[{}] error: {}", name, msg);
                        msg
                    }
                    Err(panic) => {
                        let msg = panic
                            .downcast_ref::<String>()
                            .cloned()
                            .or_else(|| panic.downcast_ref::<&str>().map(|s| s.to_string()))
                            .unwrap_or_else(|| "unknown panic".to_string());
                        error!("[{}] PANIC: {}", name, msg);
                        msg
                    }
                };

                // Record crash
                let timestamp = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs();
                let crash = CrashSummary {
                    subsystem: name.to_string(),
                    message: error_msg,
                    backtrace: String::new(), // Full backtrace capture is expensive on ARM
                    timestamp_secs: timestamp,
                    restart_count: restarts + 1,
                };
                CRASH_LOG.record(crash.clone());

                // Broadcast to dashboard
                if let Some(ref ws) = ws_tx {
                    let msg = encore_common::protocol::ServerMsg::CrashReport(crash);
                    if let Ok(json) = serde_json::to_string(&msg) {
                        let _ = ws.send(json);
                    }
                }

                restarts += 1;
                health_clone
                    .restart_count
                    .store(restarts, std::sync::atomic::Ordering::Relaxed);

                // Check for Hold mode — freeze subsystem for dashboard inspection
                if debug_modes.get(name) == DebugMode::Hold {
                    info!(
                        "[{}] debug mode is Hold — subsystem frozen for inspection",
                        name
                    );
                    health_clone.set_state(SubsystemState::Held);
                    // Block until shutdown — don't restart
                    let mut shutdown = shutdown_tx.subscribe();
                    shutdown.recv().await.ok();
                    break;
                }

                if restarts >= max {
                    error!(
                        "[{}] max restarts ({}) reached -- marking degraded",
                        name, max
                    );
                    health_clone.set_state(SubsystemState::Degraded);
                    break;
                }

                warn!("[{}] restarting in {:?}...", name, RESTART_DELAY);
                health_clone.set_state(SubsystemState::Starting);
                tokio::time::sleep(RESTART_DELAY).await;
            }
        });

        self.handles.push(handle);
        health
    }

    /// Broadcast shutdown signal and wait for all subsystems to exit
    pub async fn shutdown(self) {
        info!("Shutting down all subsystems...");
        let _ = self.shutdown_tx.send(());

        for handle in self.handles {
            let _ = tokio::time::timeout(SHUTDOWN_TIMEOUT, handle).await;
        }
        info!("All subsystems stopped");
    }

    /// Take the status receiver (for the web server to consume).
    /// The sender remains unchanged so the broadcaster can use it.
    pub fn take_status_rx(&mut self) -> mpsc::Receiver<SubsystemSnapshot> {
        let (_dummy_tx, dummy_rx) = mpsc::channel(1);
        std::mem::replace(&mut self.status_rx, dummy_rx)
    }

    /// Spawn a 1Hz task that reads all subsystem health handles and sends
    /// `SubsystemSnapshot` messages through the status channel.
    /// Call this after all subsystems are started.
    pub fn start_status_broadcaster(&self) {
        let handles = self.health_handles.clone();
        let status_tx = self.status_tx.clone();
        let debug_modes = self.debug_modes.clone();
        let mut shutdown = self.shutdown_tx.subscribe();

        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(1));
            loop {
                tokio::select! {
                    _ = interval.tick() => {
                        for (name, health) in &handles {
                            let state_u8 = health.state.load(std::sync::atomic::Ordering::Relaxed);
                            let state = SubsystemState::from_u8(state_u8);
                            let snap = SubsystemSnapshot {
                                name: name.to_string(),
                                state,
                                debug_mode: debug_modes.get(name),
                                restart_count: health.restart_count.load(std::sync::atomic::Ordering::Relaxed),
                                msg_count: health.msg_count.load(std::sync::atomic::Ordering::Relaxed),
                                uptime_secs: health.started_at.elapsed().as_secs(),
                            };
                            let _ = status_tx.try_send(snap);
                        }
                    }
                    _ = shutdown.recv() => break,
                }
            }
        });
    }

    /// Get a clone of the shutdown sender (for signal handlers)
    pub fn shutdown_handle(&self) -> broadcast::Sender<()> {
        self.shutdown_tx.clone()
    }

    /// Get a handle to debug mode settings.
    pub fn debug_modes(&self) -> Arc<DebugModes> {
        self.debug_modes.clone()
    }
}
