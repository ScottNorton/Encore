//! GPIO debug operations with timeouts.
//!
//! Each operation runs in `spawn_blocking` with a 2-second timeout.
//! If exporting a pin pegs the CPU, the timeout fires and the caller
//! gets an error instead of a hung connection.

use crate::mcu::{dsp_gpio::DspGpio, gpio};
use anyhow::{bail, Context, Result};
use serde::Serialize;
use std::sync::{Arc, Mutex};
use std::time::Duration;

const OP_TIMEOUT: Duration = Duration::from_secs(2);

/// Result of reading a single GPIO pin.
#[derive(Serialize)]
pub struct PinInfo {
    pub pin: u32,
    pub exported: bool,
    pub direction: Option<String>,
    pub value: Option<bool>,
}

/// Result of a full GPIO state read.
#[derive(Serialize)]
pub struct GpioStateResponse {
    pub initialized: bool,
    pub pins: Vec<PinInfo>,
}

/// Export all DSP flow control pins (4, 12, 13, 15).
pub async fn init_all(state: Arc<Mutex<Option<DspGpio>>>) -> Result<()> {
    tokio::time::timeout(
        OP_TIMEOUT,
        tokio::task::spawn_blocking(move || {
            let dsp_gpio = DspGpio::init()?;
            let mut guard = state.lock().map_err(|_| anyhow::anyhow!("lock poisoned"))?;
            *guard = Some(dsp_gpio);
            Ok(())
        }),
    )
    .await
    .context("GPIO init timed out (possible CPU peg — pin export may be stuck)")?
    .context("GPIO init task panicked")?
}

/// Unexport all DSP flow control pins.
pub async fn deinit_all(state: Arc<Mutex<Option<DspGpio>>>) -> Result<()> {
    tokio::time::timeout(
        OP_TIMEOUT,
        tokio::task::spawn_blocking(move || {
            let mut guard = state.lock().map_err(|_| anyhow::anyhow!("lock poisoned"))?;
            if let Some(mut g) = guard.take() {
                g.deinit();
            }
            Ok(())
        }),
    )
    .await
    .context("GPIO deinit timed out")?
    .context("GPIO deinit task panicked")?
}

/// Read state of all DSP flow control pins.
pub async fn read_state(state: Arc<Mutex<Option<DspGpio>>>) -> Result<GpioStateResponse> {
    tokio::time::timeout(
        OP_TIMEOUT,
        tokio::task::spawn_blocking(move || {
            let guard = state.lock().map_err(|_| anyhow::anyhow!("lock poisoned"))?;
            match guard.as_ref() {
                Some(g) => {
                    let s = g.read_state();
                    Ok(GpioStateResponse {
                        initialized: true,
                        pins: vec![
                            PinInfo {
                                pin: 4,
                                exported: true,
                                direction: Some("out".into()),
                                value: Some(s.cs),
                            },
                            PinInfo {
                                pin: 12,
                                exported: true,
                                direction: Some("in".into()),
                                value: Some(s.data_ready),
                            },
                            PinInfo {
                                pin: 13,
                                exported: true,
                                direction: Some("out".into()),
                                value: Some(s.arm_ready),
                            },
                            PinInfo {
                                pin: 15,
                                exported: true,
                                direction: Some("in".into()),
                                value: Some(s.dsp_ready),
                            },
                        ],
                    })
                }
                None => Ok(GpioStateResponse {
                    initialized: false,
                    pins: vec![],
                }),
            }
        }),
    )
    .await
    .context("GPIO state read timed out")?
    .context("GPIO state read task panicked")?
}

/// Export a single GPIO pin.
pub async fn export_pin(pin: u32) -> Result<()> {
    validate_pin(pin)?;
    tokio::time::timeout(
        OP_TIMEOUT,
        tokio::task::spawn_blocking(move || gpio::export(pin)),
    )
    .await
    .context("GPIO export timed out (possible CPU peg)")?
    .context("GPIO export task panicked")?
}

/// Unexport a single GPIO pin.
pub async fn unexport_pin(pin: u32) -> Result<()> {
    validate_pin(pin)?;
    tokio::time::timeout(
        OP_TIMEOUT,
        tokio::task::spawn_blocking(move || {
            gpio::unexport(pin);
            Ok(())
        }),
    )
    .await
    .context("GPIO unexport timed out")?
    .context("GPIO unexport task panicked")?
}

/// Read a single GPIO pin value.
pub async fn read_pin(pin: u32) -> Result<PinInfo> {
    validate_pin(pin)?;
    tokio::time::timeout(
        OP_TIMEOUT,
        tokio::task::spawn_blocking(move || {
            let exported = std::path::Path::new(&format!("/sys/class/gpio/gpio{}", pin)).exists();
            if !exported {
                return Ok(PinInfo {
                    pin,
                    exported: false,
                    direction: None,
                    value: None,
                });
            }
            let direction =
                std::fs::read_to_string(format!("/sys/class/gpio/gpio{}/direction", pin))
                    .ok()
                    .map(|s| s.trim().to_string());
            let value = gpio::read_value(pin).ok();
            Ok(PinInfo {
                pin,
                exported: true,
                direction,
                value,
            })
        }),
    )
    .await
    .context("GPIO read timed out")?
    .context("GPIO read task panicked")?
}

/// Only allow known DSP flow control pins (safety guard).
fn validate_pin(pin: u32) -> Result<()> {
    match pin {
        4 | 12 | 13 | 15 => Ok(()),
        _ => bail!("pin {} not in DSP flow control set (4, 12, 13, 15)", pin),
    }
}
