//! Hardware debug tools — GPIO and UART exposed over REST API.
//!
//! These are dangerous, low-level hardware probing tools. GPIO pin export
//! has previously pegged a CPU core and frozen the MCU. UART is an unverified
//! debug interface. Both are exposed as REST endpoints so the operator can
//! control them remotely — if something goes wrong, just stop sending requests.
//!
//! Nothing in this module runs automatically. Every operation is triggered by
//! an explicit HTTP request and executed with a timeout.

pub mod gpio;
pub mod routes;
pub mod uart;

use crate::mcu::dsp_gpio::DspGpio;
use crate::mcu::uart::McuUart;
use std::sync::{Arc, Mutex};

/// Shared state for debug hardware tools.
///
/// Held behind `Arc<Mutex<_>>` so Axum handlers can access it.
/// GPIO and UART are created on-demand (None until the user inits them).
#[derive(Clone)]
pub struct DebugState {
    pub gpio: Arc<Mutex<Option<DspGpio>>>,
    pub uart: Arc<Mutex<Option<McuUart>>>,
}

impl DebugState {
    pub fn new() -> Self {
        Self {
            gpio: Arc::new(Mutex::new(None)),
            uart: Arc::new(Mutex::new(None)),
        }
    }
}
