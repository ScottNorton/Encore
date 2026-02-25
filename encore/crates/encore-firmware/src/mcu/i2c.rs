//! I2C bus wrapper with retry logic and post-write delay.
//!
//! Wraps Linux i2cdev with automatic retries (3 attempts, 20ms backoff)
//! and a 5ms post-write stabilization delay required by the MCU and DAC.

use anyhow::{Context, Result};
use i2cdev::core::I2CDevice;
use i2cdev::linux::LinuxI2CDevice;
use std::thread;
use std::time::Duration;
use tracing::{trace, warn};

const POST_WRITE_DELAY: Duration = Duration::from_millis(5);
const RETRY_DELAY: Duration = Duration::from_millis(20);
const MAX_RETRIES: u8 = 3;

/// Thin wrapper around LinuxI2CDevice with retry logic and post-write delay.
pub struct I2CBus {
    dev: LinuxI2CDevice,
    addr: u16,
}

impl I2CBus {
    pub fn open(bus: &str, addr: u16) -> Result<Self> {
        let dev = LinuxI2CDevice::new(bus, addr)
            .with_context(|| format!("failed to open I2C {bus} addr 0x{addr:02X}"))?;
        Ok(Self { dev, addr })
    }

    /// Write bytes with post-write delay and retry on I/O error.
    pub fn write(&mut self, data: &[u8]) -> Result<()> {
        for attempt in 0..MAX_RETRIES {
            match self.dev.write(data) {
                Ok(()) => {
                    trace!("I2C 0x{:02X} write {:02X?}", self.addr, data);
                    thread::sleep(POST_WRITE_DELAY);
                    return Ok(());
                }
                Err(e) if attempt < MAX_RETRIES - 1 => {
                    warn!(
                        "I2C 0x{:02X} write error (attempt {}): {}",
                        self.addr,
                        attempt + 1,
                        e
                    );
                    thread::sleep(RETRY_DELAY);
                }
                Err(e) => {
                    return Err(e).with_context(|| {
                        format!("I2C 0x{:02X} write failed after {MAX_RETRIES} attempts", self.addr)
                    });
                }
            }
        }
        unreachable!()
    }

    /// Write a single register value.
    pub fn write_reg(&mut self, reg: u8, val: u8) -> Result<()> {
        self.write(&[reg, val])
    }

    /// Read a single register value.
    pub fn read_reg(&mut self, reg: u8) -> Result<u8> {
        self.dev
            .smbus_read_byte_data(reg)
            .with_context(|| format!("I2C 0x{:02X} read reg 0x{reg:02X}", self.addr))
    }

    /// Read N bytes (raw read, no register prefix).
    pub fn read(&mut self, buf: &mut [u8]) -> Result<()> {
        self.dev
            .read(buf)
            .with_context(|| format!("I2C 0x{:02X} read {} bytes", self.addr, buf.len()))
    }
}
