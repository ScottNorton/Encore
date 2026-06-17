//! GPIO flow control for DSP SPI communication.
//!
//! Implements the vendor GPIO handshake protocol for reliable SPI messaging
//! with the ADSP-21489 DSP. Without GPIO, SPI commands use blind sleep-based
//! timing. With GPIO, we get proper flow control and can receive unsolicited
//! DSP events (wake word trigger, boot events).
//!
//! SAFETY: GPIO pins are only exported after DSP firmware upload succeeds.
//! If GPIO export causes problems (CPU peg), the watchdog reboots within 30s
//! and after 3 crashes, the supervisor disables Encore entirely.

use super::gpio;
use anyhow::{bail, Context, Result};
use std::time::{Duration, Instant};
use tracing::info;

// DSP flow control GPIO pins (from vendor dsp-client.c)
const PIN_CS: u32 = 4; // Output: CS toggle before reading DSP data
const PIN_DATA_READY: u32 = 12; // Input: DSP has data to send (HIGH = ready)
const PIN_ARM_READY: u32 = 13; // Output: ARM ready to communicate
const PIN_DSP_READY: u32 = 15; // Input: DSP ready to receive (active LOW!)

const ALL_PINS: [u32; 4] = [PIN_CS, PIN_DATA_READY, PIN_ARM_READY, PIN_DSP_READY];

const DSP_READY_TIMEOUT: Duration = Duration::from_millis(500);
const DSP_READY_POLL_INTERVAL: Duration = Duration::from_millis(1);

/// Diagnostic snapshot of GPIO pin states.
pub struct GpioPinState {
    pub cs: bool,
    pub data_ready: bool,
    pub arm_ready: bool,
    pub dsp_ready: bool,
}

/// GPIO flow control wrapper for DSP SPI communication.
pub struct DspGpio {
    initialized: bool,
}

impl DspGpio {
    /// Export and configure all DSP flow control GPIO pins.
    pub fn init() -> Result<Self> {
        info!("DSP GPIO: exporting pins 4, 12, 13, 15");

        // Export all pins
        gpio::export(PIN_CS).context("GPIO 4 export")?;
        gpio::export(PIN_DATA_READY).context("GPIO 12 export")?;
        gpio::export(PIN_ARM_READY).context("GPIO 13 export")?;
        gpio::export(PIN_DSP_READY).context("GPIO 15 export")?;

        // Small delay for sysfs to create attribute files
        std::thread::sleep(Duration::from_millis(50));

        // Set directions
        gpio::set_direction(PIN_CS, true).context("GPIO 4 direction")?; // output
        gpio::set_direction(PIN_DATA_READY, false).context("GPIO 12 direction")?; // input
        gpio::set_direction(PIN_ARM_READY, true).context("GPIO 13 direction")?; // output
        gpio::set_direction(PIN_DSP_READY, false).context("GPIO 15 direction")?; // input

        // Set idle states
        gpio::write_value(PIN_CS, true)?; // CS idle HIGH
        gpio::write_value(PIN_ARM_READY, false)?; // ARM not ready

        info!("DSP GPIO: initialized (pins 4,12,13,15 exported)");
        Ok(Self { initialized: true })
    }

    /// Check if DSP has data to send (GPIO 12 HIGH = data ready).
    pub fn has_data(&self) -> Result<bool> {
        if !self.initialized {
            return Ok(false);
        }
        gpio::read_value(PIN_DATA_READY)
    }

    /// Wait for DSP to be ready to receive (GPIO 15 LOW = ready, active-LOW).
    pub fn wait_dsp_ready(&self) -> Result<()> {
        let start = Instant::now();
        loop {
            let val = gpio::read_value(PIN_DSP_READY)?;
            if !val {
                // LOW = ready (active-LOW)
                return Ok(());
            }
            if start.elapsed() > DSP_READY_TIMEOUT {
                bail!("DSP GPIO: timeout waiting for DSP ready (GPIO 15 stuck HIGH)");
            }
            std::thread::sleep(DSP_READY_POLL_INTERVAL);
        }
    }

    /// Pre-send handshake: prepare GPIO state before SPI write.
    /// Caller must do the SPI transfer, then call `post_send()`.
    pub fn pre_send(&self) -> Result<()> {
        // 1. ARM not ready
        gpio::write_value(PIN_ARM_READY, false)?;
        // 2. Wait for DSP ready
        self.wait_dsp_ready()?;
        // 3. Signal ARM ready
        std::thread::sleep(Duration::from_micros(1));
        gpio::write_value(PIN_ARM_READY, true)?;
        std::thread::sleep(Duration::from_micros(1));
        Ok(())
    }

    /// Post-send handshake: signal completion after SPI write.
    pub fn post_send(&self) -> Result<()> {
        std::thread::sleep(Duration::from_micros(1));
        gpio::write_value(PIN_ARM_READY, false)?;
        Ok(())
    }

    /// Pre-receive handshake: CS pulse + ARM ready before SPI read.
    /// Caller must do the SPI transfer, then call `post_receive()`.
    pub fn pre_receive(&self) -> Result<()> {
        // CS pulse (GPIO 4: LOW then HIGH)
        gpio::write_value(PIN_CS, false)?;
        std::thread::sleep(Duration::from_micros(1));
        gpio::write_value(PIN_CS, true)?;
        // Signal ARM ready
        gpio::write_value(PIN_ARM_READY, true)?;
        std::thread::sleep(Duration::from_micros(1));
        Ok(())
    }

    /// Post-receive handshake: signal completion after SPI read.
    pub fn post_receive(&self) -> Result<()> {
        std::thread::sleep(Duration::from_micros(1));
        gpio::write_value(PIN_ARM_READY, false)?;
        Ok(())
    }

    /// Read current pin states for diagnostics.
    pub fn read_state(&self) -> GpioPinState {
        GpioPinState {
            cs: gpio::read_value(PIN_CS).unwrap_or(false),
            data_ready: gpio::read_value(PIN_DATA_READY).unwrap_or(false),
            arm_ready: gpio::read_value(PIN_ARM_READY).unwrap_or(false),
            dsp_ready: gpio::read_value(PIN_DSP_READY).unwrap_or(false),
        }
    }

    /// Unexport all GPIO pins. Called on clean shutdown.
    pub fn deinit(&mut self) {
        if self.initialized {
            info!("DSP GPIO: unexporting pins");
            gpio::write_value(PIN_ARM_READY, false).ok();
            for &pin in &ALL_PINS {
                gpio::unexport(pin);
            }
            self.initialized = false;
        }
    }
}

impl Drop for DspGpio {
    fn drop(&mut self) {
        self.deinit();
    }
}
