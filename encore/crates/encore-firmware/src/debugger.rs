//! Crash capture and hold-mode debugging.
//!
//! Captures subsystem crashes: backtrace, error message, timestamp.
//! Stores ring buffer of last 16 crashes to /lsync/encore/crashes/.
//! Supports DebugMode::Hold — freeze failed subsystem for dashboard inspection.

use encore_common::protocol::{CrashSummary, DebugMode};
use std::collections::VecDeque;
use std::sync::Mutex;
use tracing::{info, warn};

const CRASH_DIR: &str = "/lsync/encore/crashes";
const MAX_CRASHES: usize = 16;

/// Global crash store — accessed by manager and web API.
pub static CRASH_LOG: CrashLog = CrashLog::new();

pub struct CrashLog {
    inner: Mutex<Option<VecDeque<CrashSummary>>>,
}

impl CrashLog {
    const fn new() -> Self {
        Self {
            inner: Mutex::new(None),
        }
    }

    fn ensure_init(&self) -> std::sync::MutexGuard<'_, Option<VecDeque<CrashSummary>>> {
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        if guard.is_none() {
            let mut ring = VecDeque::with_capacity(MAX_CRASHES);
            // Load existing crashes from disk
            if let Ok(entries) = std::fs::read_dir(CRASH_DIR) {
                let mut files: Vec<_> = entries
                    .filter_map(|e| e.ok())
                    .filter(|e| {
                        e.path()
                            .extension()
                            .map(|ext| ext == "json")
                            .unwrap_or(false)
                    })
                    .collect();
                files.sort_by_key(|e| e.file_name());
                for entry in files.iter().rev().take(MAX_CRASHES) {
                    if let Ok(data) = std::fs::read_to_string(entry.path()) {
                        if let Ok(crash) = serde_json::from_str::<CrashSummary>(&data) {
                            ring.push_front(crash);
                        }
                    }
                }
            }
            *guard = Some(ring);
        }
        guard
    }

    /// Record a crash. Writes to disk and stores in ring buffer.
    pub fn record(&self, crash: CrashSummary) {
        // Write to disk
        persist_crash(&crash);

        let mut guard = self.ensure_init();
        let ring = guard.as_mut().unwrap();
        if ring.len() >= MAX_CRASHES {
            ring.pop_front();
        }
        ring.push_back(crash);
    }

    /// Get all recorded crashes (most recent last).
    pub fn crashes(&self) -> Vec<CrashSummary> {
        let guard = self.ensure_init();
        guard.as_ref().map(|r| r.iter().cloned().collect()).unwrap_or_default()
    }

    /// Get the most recent crash for a given subsystem.
    pub fn last_crash(&self, subsystem: &str) -> Option<CrashSummary> {
        let guard = self.ensure_init();
        guard
            .as_ref()
            .and_then(|r| r.iter().rev().find(|c| c.subsystem == subsystem).cloned())
    }
}

fn persist_crash(crash: &CrashSummary) {
    if let Err(e) = std::fs::create_dir_all(CRASH_DIR) {
        warn!("Debugger: failed to create crash dir: {}", e);
        return;
    }

    let filename = format!(
        "{}/{}-{}.json",
        CRASH_DIR, crash.timestamp_secs, crash.subsystem
    );

    match serde_json::to_string_pretty(crash) {
        Ok(json) => {
            if let Err(e) = std::fs::write(&filename, json) {
                warn!("Debugger: failed to write crash file: {}", e);
            } else {
                info!(
                    "Debugger: crash recorded for [{}]: {}",
                    crash.subsystem, crash.message
                );
            }
        }
        Err(e) => {
            warn!("Debugger: failed to serialize crash: {}", e);
        }
    }

    // Prune old crash files beyond MAX_CRASHES
    prune_crash_dir();
}

fn prune_crash_dir() {
    let entries = match std::fs::read_dir(CRASH_DIR) {
        Ok(e) => e,
        Err(_) => return,
    };

    let mut files: Vec<_> = entries
        .filter_map(|e| e.ok())
        .filter(|e| {
            e.path()
                .extension()
                .map(|ext| ext == "json")
                .unwrap_or(false)
        })
        .collect();

    if files.len() <= MAX_CRASHES {
        return;
    }

    // Sort by name (timestamp prefix) — oldest first
    files.sort_by_key(|e| e.file_name());
    let to_remove = files.len() - MAX_CRASHES;
    for entry in files.iter().take(to_remove) {
        std::fs::remove_file(entry.path()).ok();
    }
}

/// Per-subsystem debug mode tracking.
pub struct DebugModes {
    modes: Mutex<std::collections::HashMap<String, DebugMode>>,
}

impl DebugModes {
    pub fn new() -> Self {
        Self {
            modes: Mutex::new(std::collections::HashMap::new()),
        }
    }

    pub fn get(&self, subsystem: &str) -> DebugMode {
        self.modes
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(subsystem)
            .copied()
            .unwrap_or(DebugMode::Production)
    }

    pub fn set(&self, subsystem: &str, mode: DebugMode) {
        self.modes
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(subsystem.to_string(), mode);
        info!("Debugger: [{}] mode set to {:?}", subsystem, mode);
    }
}
