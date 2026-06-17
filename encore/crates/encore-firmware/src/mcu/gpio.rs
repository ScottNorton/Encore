//! GPIO sysfs interface for DSP SPI flow control.
//!
//! The ADSP-21489 DSP uses 4 GPIO pins for SPI message flow control:
//! - GPIO 4:  Output — CS/flow control toggle before reads
//! - GPIO 12: Input  — DSP has data to send
//! - GPIO 13: Output — ARM ready to communicate
//! - GPIO 15: Input  — DSP ready to receive

use anyhow::{Context, Result};
use std::fs;
use std::path::Path;

/// Export a GPIO pin via sysfs. No-op if already exported.
pub fn export(pin: u32) -> Result<()> {
    let gpio_path = format!("/sys/class/gpio/gpio{}", pin);
    if !Path::new(&gpio_path).exists() {
        fs::write("/sys/class/gpio/export", pin.to_string())
            .with_context(|| format!("GPIO: failed to export pin {}", pin))?;
    }
    Ok(())
}

/// Unexport a GPIO pin via sysfs. Ignores errors.
pub fn unexport(pin: u32) {
    let _ = fs::write("/sys/class/gpio/unexport", pin.to_string());
}

/// Set GPIO pin direction. `output=true` for output, `false` for input.
pub fn set_direction(pin: u32, output: bool) -> Result<()> {
    let dir = if output { "out" } else { "in" };
    fs::write(format!("/sys/class/gpio/gpio{}/direction", pin), dir)
        .with_context(|| format!("GPIO: failed to set pin {} direction to {}", pin, dir))
}

/// Read GPIO pin value. Returns `true` for HIGH, `false` for LOW.
pub fn read_value(pin: u32) -> Result<bool> {
    let val = fs::read_to_string(format!("/sys/class/gpio/gpio{}/value", pin))
        .with_context(|| format!("GPIO: failed to read pin {}", pin))?;
    Ok(val.trim() == "1")
}

/// Write GPIO pin value. `high=true` for HIGH, `false` for LOW.
pub fn write_value(pin: u32, high: bool) -> Result<()> {
    let val = if high { "1" } else { "0" };
    fs::write(format!("/sys/class/gpio/gpio{}/value", pin), val)
        .with_context(|| format!("GPIO: failed to write pin {}", pin))
}
