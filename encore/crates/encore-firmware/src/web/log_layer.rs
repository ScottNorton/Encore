//! Tracing layer that broadcasts log entries to WebSocket clients.
//!
//! The [`WsBroadcastLayer`] captures tracing events and converts them to
//! [`LogEntry`] structs. Because `Layer::on_event` is synchronous, entries
//! are sent through a `std::sync::mpsc` channel to an async batching task
//! spawned via [`spawn_log_broadcaster`]. That task drains the channel every
//! 500ms and sends a single `ServerMsg::LogEntries` batch over the WebSocket
//! broadcast channel.
//!
//! **Important**: The layer only captures INFO+ events and filters out all
//! web/framework targets to prevent feedback loops and excessive CPU usage.

use encore_common::protocol::{LogEntry, ServerMsg};
use std::collections::VecDeque;
use std::sync::{LazyLock, Mutex, mpsc};
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::broadcast;
use tracing::field::{Field, Visit};
use tracing::Subscriber;
use tracing_subscriber::layer::Context;
use tracing_subscriber::Layer;

/// Maximum entries buffered per batch drain cycle.
const MAX_BATCH: usize = 50;

/// Maximum entries kept in the persistent history ring buffer.
const MAX_HISTORY: usize = 1000;

/// In-memory ring buffer of recent log entries for the REST API.
static LOG_HISTORY: LazyLock<Mutex<VecDeque<LogEntry>>> =
    LazyLock::new(|| Mutex::new(VecDeque::with_capacity(MAX_HISTORY)));

/// Return a snapshot of the log history for `GET /api/logs`.
pub fn log_history() -> Vec<LogEntry> {
    LOG_HISTORY
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .iter()
        .cloned()
        .collect()
}

/// Interval between batch flushes.
const FLUSH_INTERVAL_MS: u64 = 500;

/// A tracing layer that captures events and forwards them as [`LogEntry`]
/// through an internal mpsc channel.
pub struct WsBroadcastLayer {
    tx: mpsc::SyncSender<LogEntry>,
}

impl WsBroadcastLayer {
    /// Create the layer and its paired receiver.
    ///
    /// The receiver should be passed to [`spawn_log_broadcaster`].
    pub fn new() -> (Self, mpsc::Receiver<LogEntry>) {
        // Bounded channel: if the broadcaster falls behind, we drop events
        // rather than accumulating unbounded memory.
        let (tx, rx) = mpsc::sync_channel(512);
        (Self { tx }, rx)
    }
}

impl<S: Subscriber> Layer<S> for WsBroadcastLayer {
    fn on_event(&self, event: &tracing::Event<'_>, _ctx: Context<'_, S>) {
        let meta = event.metadata();

        // Only capture INFO and above — TRACE/DEBUG from tokio/hyper/rustls
        // generates thousands of events per second and will pin the CPU.
        if *meta.level() > tracing::Level::INFO {
            return;
        }

        // Skip events from web subsystem to avoid feedback loops:
        // logging → ws_tx.send → tokio/axum events → more logging
        let target = meta.target();
        if target.starts_with("encore_firmware::web") {
            return;
        }
        // Skip framework internals that generate high-volume events
        if target.starts_with("tokio")
            || target.starts_with("hyper")
            || target.starts_with("tungstenite")
            || target.starts_with("rustls")
            || target.starts_with("tower")
            || target.starts_with("mio")
            || target.starts_with("h2")
        {
            return;
        }

        let timestamp_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);

        let level = meta.level().as_str().to_owned();
        let target = target.to_owned();

        let mut visitor = MessageVisitor::default();
        event.record(&mut visitor);

        let entry = LogEntry {
            timestamp_ms,
            level,
            target,
            message: visitor.message,
        };

        // Non-blocking: drop the entry if the channel is full
        let _ = self.tx.try_send(entry);
    }
}

/// Visitor that extracts the `message` field from a tracing event.
#[derive(Default)]
struct MessageVisitor {
    message: String,
}

impl Visit for MessageVisitor {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            self.message = format!("{:?}", value);
        } else if self.message.is_empty() {
            // Fallback: use the first field if no "message" field
            self.message = format!("{:?}", value);
        }
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "message" {
            self.message = value.to_owned();
        } else if self.message.is_empty() {
            self.message = value.to_owned();
        }
    }
}

/// Spawn the async batching task that drains [`LogEntry`] items from the
/// mpsc receiver and broadcasts them as `ServerMsg::LogEntries` over the
/// WebSocket channel.
///
/// This task runs until the mpsc sender is dropped (i.e. the tracing
/// subscriber is dropped, which effectively means process exit).
pub fn spawn_log_broadcaster(
    rx: mpsc::Receiver<LogEntry>,
    ws_tx: broadcast::Sender<String>,
) {
    tokio::spawn(async move {
        let mut interval =
            tokio::time::interval(std::time::Duration::from_millis(FLUSH_INTERVAL_MS));

        loop {
            interval.tick().await;

            // Drain up to MAX_BATCH entries from the sync channel
            let mut batch = Vec::new();
            loop {
                match rx.try_recv() {
                    Ok(entry) => {
                        batch.push(entry);
                        if batch.len() >= MAX_BATCH {
                            break;
                        }
                    }
                    Err(mpsc::TryRecvError::Empty) => break,
                    Err(mpsc::TryRecvError::Disconnected) => return,
                }
            }

            if batch.is_empty() {
                continue;
            }

            // Store in history ring buffer for REST API
            {
                let mut history = LOG_HISTORY.lock().unwrap_or_else(|e| e.into_inner());
                for entry in &batch {
                    history.push_back(entry.clone());
                }
                while history.len() > MAX_HISTORY {
                    history.pop_front();
                }
            }

            let msg = ServerMsg::LogEntries(batch);
            if let Ok(json) = serde_json::to_string(&msg) {
                // Ignore send errors (no active subscribers)
                let _ = ws_tx.send(json);
            }
        }
    });
}
