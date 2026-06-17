//! Hardware watchdog subsystem.
//!
//! Opens /dev/watchdog and pets it on a **dedicated OS thread** (not a
//! tokio task). This guarantees the watchdog is always fed even if the
//! tokio runtime is overloaded or starved (e.g. WebSocket backpressure,
//! TLS handshakes, heavy serialization).
//!
//! Also sends heartbeat (cmd 0x24) to the MCU every 30s out of caution,
//! though RE confirmed cmd 0x24 is a NO-OP (MCU dispatcher ignores it,
//! MCU WDT_A is stopped at boot). The SoC DesignWare WDT is the only
//! real reset source — fed by /dev/watchdog writes, not MCU heartbeats.
//!
//! The supervisor (encore_supervisor.sh) releases /dev/watchdog before starting
//! Encore, so Encore always opens and owns it. If Encore hangs, it stops petting,
//! and the watchdog reboots the device — real application-level protection.
//! When Encore exits, the supervisor immediately reclaims the watchdog.
//!
//! EBUSY is handled gracefully as a safety net (degrades to MCU-only heartbeats).
//! On graceful shutdown, writes the magic close byte 'V' to disable the timer.

use crate::mcu::Mcu;
use crate::subsystem::{Subsystem, SubsystemContext};
use anyhow::{Context, Result};
use encore_common::protocol::SubsystemState;
use std::fs::OpenOptions;
use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tracing::{info, warn};

const WATCHDOG_DEVICE: &str = "/dev/watchdog";
/// Pet interval: 10s gives a comfortable margin against the typical 30s
/// hardware timeout. Previous value of 30s left zero margin, causing
/// reboots when the tokio scheduler delayed the pet task.
const PET_INTERVAL_SECS: u64 = 10;
/// MCU heartbeat interval in multiples of PET_INTERVAL_SECS.
/// 3 × 10s = 30s. Stock ARM binary used 5s, but cmd 0x24 is a NO-OP on the MCU.
const MCU_HEARTBEAT_EVERY: u32 = 3;

pub struct WatchdogSubsystem {
    running: Arc<AtomicBool>,
    mcu: Option<Arc<Mutex<Mcu>>>,
}

impl WatchdogSubsystem {
    pub fn new() -> Self {
        Self {
            running: Arc::new(AtomicBool::new(false)),
            mcu: None,
        }
    }

    /// Set the MCU reference for heartbeat petting.
    pub fn set_mcu(&mut self, mcu: Arc<Mutex<Mcu>>) {
        self.mcu = Some(mcu);
    }
}

#[async_trait::async_trait]
impl Subsystem for WatchdogSubsystem {
    fn name(&self) -> &'static str {
        "watchdog"
    }

    async fn run(&mut self, mut ctx: SubsystemContext) -> Result<()> {
        // Open /dev/watchdog. The supervisor releases it before starting us,
        // so this should succeed. If EBUSY (safety net), degrade to MCU-only.
        let watchdog_file = match OpenOptions::new().write(true).open(WATCHDOG_DEVICE) {
            Ok(mut f) => {
                info!(
                    "Watchdog: opened {} (interval={}s, dedicated thread)",
                    WATCHDOG_DEVICE, PET_INTERVAL_SECS
                );
                let _ = f.write_all(&[0x00]);
                Some(f)
            }
            Err(e) if e.raw_os_error() == Some(libc::EBUSY) => {
                warn!(
                    "Watchdog: {} held by another process, MCU heartbeat only",
                    WATCHDOG_DEVICE
                );
                None
            }
            Err(e) => return Err(e).context("failed to open /dev/watchdog"),
        };

        ctx.health.set_state(SubsystemState::Running);

        // Move to a dedicated OS thread so it's immune to tokio starvation
        let running = self.running.clone();
        running.store(true, Ordering::Release);
        let health = ctx.health.clone();
        let mcu = self.mcu.clone();

        std::thread::Builder::new()
            .name("encore-watchdog".into())
            .spawn(move || {
                let mut mcu_counter: u32 = 0;
                let mut watchdog_file = watchdog_file;

                while running.load(Ordering::Acquire) {
                    // Pet Linux hardware watchdog if we own it
                    if let Some(ref mut f) = watchdog_file {
                        if let Err(e) = f.write_all(&[0x00]) {
                            warn!("Watchdog: pet failed: {}", e);
                            break;
                        }
                    }

                    // Send MCU heartbeat (cmd 0x24, confirmed NO-OP) every ~30s
                    mcu_counter += 1;
                    if mcu_counter >= MCU_HEARTBEAT_EVERY {
                        mcu_counter = 0;
                        if let Some(ref mcu) = mcu {
                            if let Ok(mut m) = mcu.lock() {
                                if let Err(e) = m.pet_watchdog() {
                                    warn!("Watchdog: MCU heartbeat failed: {}", e);
                                }
                            }
                        }
                    }

                    health.beat(
                        std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_secs(),
                    );
                    std::thread::sleep(std::time::Duration::from_secs(PET_INTERVAL_SECS));
                }

                // Graceful close: write 'V' to disable watchdog (only if we own it)
                if let Some(ref mut f) = watchdog_file {
                    f.write_all(b"V").ok();
                    info!("Watchdog: disabled (magic close)");
                }
            })
            .context("failed to spawn watchdog thread")?;

        // Wait for shutdown signal
        ctx.shutdown.recv().await.ok();
        info!("Watchdog: shutdown signal received");

        Ok(())
    }

    async fn shutdown(&mut self) -> Result<()> {
        // Signal the watchdog thread to stop; it will write 'V' and close the fd
        self.running.store(false, Ordering::Release);
        // Give the thread a moment to write the magic byte
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        Ok(())
    }
}
