//! UART debug operations with timeouts.
//!
//! Each operation runs in `spawn_blocking` with a timeout.
//! UART is never opened automatically — only via explicit REST call.

use crate::mcu::uart::McuUart;
use anyhow::{Context, Result};
use serde::Serialize;
use std::sync::{Arc, Mutex};
use std::time::Duration;

const PROBE_TIMEOUT: Duration = Duration::from_secs(5);
const SEND_TIMEOUT: Duration = Duration::from_secs(2);

/// Result of a baud probe.
#[derive(Serialize)]
pub struct ProbeResult {
    pub baud: u32,
}

/// Result of a send/recv.
#[derive(Serialize)]
pub struct SendResult {
    pub response: String,
}

/// Result of UART status query.
#[derive(Serialize)]
pub struct UartStatus {
    pub open: bool,
    pub baud: Option<u32>,
}

/// Auto-detect MCU UART baud rate and open the port.
pub async fn probe(state: Arc<Mutex<Option<McuUart>>>) -> Result<ProbeResult> {
    tokio::time::timeout(
        PROBE_TIMEOUT,
        tokio::task::spawn_blocking(move || {
            let (uart, baud) = McuUart::probe_baud()?;
            let mut guard = state.lock().map_err(|_| anyhow::anyhow!("lock poisoned"))?;
            *guard = Some(uart);
            Ok(ProbeResult { baud })
        }),
    )
    .await
    .context("UART probe timed out")?
    .context("UART probe task panicked")?
}

/// Send a command to the MCU and return the response.
pub async fn send(state: Arc<Mutex<Option<McuUart>>>, command: String) -> Result<SendResult> {
    tokio::time::timeout(
        SEND_TIMEOUT,
        tokio::task::spawn_blocking(move || {
            let mut guard = state.lock().map_err(|_| anyhow::anyhow!("lock poisoned"))?;
            match guard.as_mut() {
                Some(uart) => {
                    let response = uart.send_recv(&command)?;
                    Ok(SendResult { response })
                }
                None => anyhow::bail!("UART not open (run probe first)"),
            }
        }),
    )
    .await
    .context("UART send timed out")?
    .context("UART send task panicked")?
}

/// Close the UART connection.
pub async fn close(state: Arc<Mutex<Option<McuUart>>>) -> Result<()> {
    let mut guard = state.lock().map_err(|_| anyhow::anyhow!("lock poisoned"))?;
    *guard = None;
    Ok(())
}

/// Get current UART status.
pub async fn status(state: Arc<Mutex<Option<McuUart>>>) -> UartStatus {
    let guard = state.lock().unwrap_or_else(|e| e.into_inner());
    match guard.as_ref() {
        Some(uart) => UartStatus {
            open: true,
            baud: Some(uart.baud()),
        },
        None => UartStatus {
            open: false,
            baud: None,
        },
    }
}
