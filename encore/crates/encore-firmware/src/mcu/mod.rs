//! Hardware drivers for MCU, DAC, DSP, and IO expander on I2C bus 0.
//!
//! The TI MSP430 MCU (0x36) controls the LED ring, buttons, volume ring,
//! and proximity sensor. Communicates via a binary I2C protocol.

pub mod dac;
pub mod dsp;
pub mod dsp_gpio;
pub mod gpio;
pub mod i2c;
pub mod io_expander;
pub mod uart;

use anyhow::Result;
use i2c::I2CBus;
use std::thread;
use std::time::Duration;
use tracing::{debug, info};

const I2C_BUS: &str = "/dev/i2c-0";
const MCU_ADDR: u16 = 0x36;

/// Number of RGB LEDs on the ring (12 ring + 1 center)
pub const LED_COUNT: usize = 13;
/// Bytes per LED frame: 13 LEDs * 3 (R, G, B)
pub const LED_FRAME_SIZE: usize = LED_COUNT * 3;

// MCU command bytes
const CMD_VERSION_QUERY: [u8; 6] = [0x01, 0x00, 0x00, 0x00, 0x00, 0x00];
const CMD_LED_ACK: [u8; 6] = [0x23, 0x00, 0x00, 0x00, 0x6C, 0xBA];
const CMD_HEARTBEAT: [u8; 6] = [0x24, 0xF8, 0xE3, 0x01, 0x50, 0xF4];
const CMD_DEVICE_COLOR: [u8; 6] = [0x25, 0x00, 0x00, 0x00, 0x00, 0x00];
const CMD_STATUS_QUERY: [u8; 6] = [0x26, 0x00, 0x00, 0x00, 0x00, 0x00];

/// Volume sequence toggle values
const VOL_SEQ_A: [u8; 2] = [0xAB, 0xB6];
const VOL_SEQ_B: [u8; 2] = [0xAC, 0xBE];

/// MCU event types (byte 0 of 6-byte read)
#[derive(Debug, Clone)]
pub enum McuEvent {
    TouchShortPress,
    TouchLongPress,
    BluetoothShort,
    BluetoothLong,
    MicShort,
    MicLong,
    ResetShort,
    ResetLong,
    /// BT + MIC held together (long press). Stock used this combo to trigger a
    /// developer DSP memory-dump bugreport; it is the only multi-button gesture
    /// the MCU emits.
    BtMicCombo,
    VolumeUp(u8),
    VolumeDown(u8),
    VersionInfo([u8; 3]),
    Unknown([u8; 6]),
}

/// Driver for the MSP430FR5739 MCU on the Harman Kardon Invoke.
pub struct Mcu {
    bus: I2CBus,
    vol_seq_toggle: bool,
}

impl Mcu {
    pub fn open() -> Result<Self> {
        let bus = I2CBus::open(I2C_BUS, MCU_ADDR)?;
        Ok(Self {
            bus,
            vol_seq_toggle: false,
        })
    }

    /// Full MCU init sequence. Must be called before sending LED frames.
    pub fn init(&mut self) -> Result<()> {
        info!("MCU init: version query");
        self.bus.write(&CMD_VERSION_QUERY)?;
        thread::sleep(Duration::from_millis(100));

        // Read version response
        let mut buf = [0u8; 6];
        self.bus.read(&mut buf)?;
        info!("MCU version response: {:02X?}", buf);

        // LED acknowledge (exact bytes required)
        self.bus.write(&CMD_LED_ACK)?;
        thread::sleep(Duration::from_millis(50));

        // Drain pending events
        self.drain_events()?;

        // Set device color
        self.bus.write(&CMD_DEVICE_COLOR)?;
        thread::sleep(Duration::from_millis(50));

        // Query status
        self.bus.write(&CMD_STATUS_QUERY)?;
        thread::sleep(Duration::from_millis(50));
        self.bus.read(&mut buf)?;
        debug!("MCU status: {:02X?}", buf);

        info!("MCU init complete");
        Ok(())
    }

    /// Send heartbeat keepalive (call every ~30s).
    pub fn pet_watchdog(&mut self) -> Result<()> {
        self.bus.write(&CMD_HEARTBEAT)
    }

    /// Set volume level on the MCU (drives LED arc display).
    /// Level: 0-100.
    pub fn set_volume_level(&mut self, level: u8) -> Result<()> {
        let seq = if self.vol_seq_toggle {
            VOL_SEQ_B
        } else {
            VOL_SEQ_A
        };
        self.vol_seq_toggle = !self.vol_seq_toggle;

        let cmd = [0x03, level, seq[0], seq[1], 0x00, 0x00];
        self.bus.write(&cmd)
    }

    /// Write LED frames using stock protocol: [0x0E, flag, frame1..frameN].
    /// `first` = true sends flag=0x01 (enters external LED mode).
    /// `first` = false sends flag=0x00 (continue external mode).
    /// Up to 10 frames per call (392 bytes max I2C write).
    pub fn set_led_frames(&mut self, frames: &[[u8; LED_FRAME_SIZE]], first: bool) -> Result<()> {
        if frames.is_empty() {
            return Ok(());
        }
        let flag: u8 = if first { 0x01 } else { 0x00 };
        let mut buf = Vec::with_capacity(2 + frames.len() * LED_FRAME_SIZE);
        buf.push(0x0E);
        buf.push(flag);
        for frame in frames {
            buf.extend_from_slice(frame);
        }
        self.bus.write(&buf)
    }

    /// Read a 6-byte event from the MCU. Returns None if no event pending (all zeros).
    pub fn read_event(&mut self) -> Result<Option<McuEvent>> {
        let mut buf = [0u8; 6];
        self.bus.read(&mut buf)?;

        if buf == [0; 6] {
            return Ok(None);
        }

        let event = match (buf[0], buf[1]) {
            (0x04, 0x00) => McuEvent::TouchShortPress,
            (0x04, 0x01) => McuEvent::TouchLongPress,
            (0x04, 0x02) => McuEvent::BluetoothShort,
            (0x04, 0x03) => McuEvent::BluetoothLong,
            (0x04, 0x04) => McuEvent::MicShort,
            (0x04, 0x05) => McuEvent::MicLong,
            (0x04, 0x06) => McuEvent::ResetShort,
            (0x04, 0x07) => McuEvent::ResetLong,
            (0x04, 0x08) => McuEvent::VolumeUp(buf[2]),
            (0x04, 0x09) => McuEvent::VolumeDown(buf[2]),
            (0x04, 0x0A) => McuEvent::BtMicCombo,
            (0x01, 0x01) => McuEvent::VersionInfo([buf[3], buf[4], buf[5]]),
            _ => McuEvent::Unknown(buf),
        };

        Ok(Some(event))
    }

    /// Drain all pending events (max 10 reads).
    fn drain_events(&mut self) -> Result<()> {
        for _ in 0..10 {
            match self.read_event()? {
                Some(event) => debug!("MCU drain: {:?}", event),
                None => break,
            }
        }
        Ok(())
    }
}
