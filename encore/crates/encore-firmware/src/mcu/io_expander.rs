//! PCA9538 IO Expander driver (I2C address 0x20).
//!
//! Controls amp/DAC mute and DSP reset via GPIO bits. Mute sequencing
//! is critical: unmute DAC before AMP to avoid speaker pops.

use super::i2c::I2CBus;
use anyhow::Result;
use std::thread;
use std::time::Duration;
use tracing::{debug, info};

const I2C_BUS: &str = "/dev/i2c-0";
const IO_EXP_ADDR: u16 = 0x20;

// Registers
const REG_OUTPUT: u8 = 0x01;
const REG_POLARITY: u8 = 0x02;  // Stock uses this register for DSP power bit 3
const REG_DIRECTION: u8 = 0x03;

// Bit masks for REG_OUTPUT (0x01)
const BIT_DSP_RESET: u8 = 0x01;     // Bit 0: DSP reset (HIGH = released)
const BIT_AMP_MUTE: u8 = 0x02;      // Bit 1: AMP mute (HIGH = muted)
const BIT_DAC_MUTE: u8 = 0x04;      // Bit 2: DAC mute (HIGH = unmuted, active LOW)
const BIT_DSP_POWER1: u8 = 0x10;    // Bit 4, reg 0x01: DSP power 1 (active-low: set = off)

// Bit masks for REG_POLARITY (0x02)
const BIT_DSP_POWER2: u8 = 0x08;    // Bit 3, reg 0x02: DSP power 2 (active-low: set = off)

/// Driver for the IO Expander (PCA9538) at 0x20.
/// Controls amplifier mute, DAC mute, and DSP reset lines.
pub struct IoExpander {
    bus: I2CBus,
}

impl IoExpander {
    pub fn open() -> Result<Self> {
        let bus = I2CBus::open(I2C_BUS, IO_EXP_ADDR)?;
        Ok(Self { bus })
    }

    /// Initialize: set all pins as outputs, mute everything, ensure DSP power on.
    /// Init state: AMP muted, DAC muted, DSP reset released, DSP power on.
    pub fn init(&mut self) -> Result<()> {
        info!("IO Expander init");

        // All outputs
        self.bus.write_reg(REG_DIRECTION, 0x00)?;

        // Read current state
        let state = self.bus.read_reg(REG_OUTPUT)?;
        debug!("IO Expander current state: 0x{:02X}", state);

        // Mute AMP, ensure DSP power 1 on (clear bit 4 = active-low on)
        self.bus.write_reg(REG_OUTPUT, (state | BIT_AMP_MUTE) & !BIT_DSP_POWER1)?;
        thread::sleep(Duration::from_millis(100));

        // Mute DAC (clear bit 2), keep DSP reset released
        let state = self.bus.read_reg(REG_OUTPUT)?;
        self.bus
            .write_reg(REG_OUTPUT, (state & !BIT_DAC_MUTE) | BIT_DSP_RESET)?;

        // Ensure DSP power 2 on (clear bit 3 in reg 0x02)
        let polarity = self.bus.read_reg(REG_POLARITY)?;
        self.bus.write_reg(REG_POLARITY, polarity & !BIT_DSP_POWER2)?;

        info!("IO Expander init complete (all muted, DSP power on)");
        Ok(())
    }

    /// Unmute audio output. Order matters: DAC first, then AMP (avoids pop).
    pub fn unmute(&mut self) -> Result<()> {
        // Unmute DAC (set bit 2)
        let state = self.bus.read_reg(REG_OUTPUT)?;
        self.bus.write_reg(REG_OUTPUT, state | BIT_DAC_MUTE)?;

        // Unmute AMP (clear bit 1)
        let state = self.bus.read_reg(REG_OUTPUT)?;
        self.bus.write_reg(REG_OUTPUT, state & !BIT_AMP_MUTE)?;

        debug!("IO Expander: unmuted");
        Ok(())
    }

    /// Mute audio output. Order: AMP first, then DAC (reverse of unmute).
    pub fn mute(&mut self) -> Result<()> {
        // Mute AMP (set bit 1)
        let state = self.bus.read_reg(REG_OUTPUT)?;
        self.bus.write_reg(REG_OUTPUT, state | BIT_AMP_MUTE)?;

        // Mute DAC (clear bit 2)
        let state = self.bus.read_reg(REG_OUTPUT)?;
        self.bus.write_reg(REG_OUTPUT, state & !BIT_DAC_MUTE)?;

        debug!("IO Expander: muted");
        Ok(())
    }

    /// Mute AMP only (set bit 1). Fast — no stabilization needed.
    pub fn mute_amp(&mut self) -> Result<()> {
        let state = self.bus.read_reg(REG_OUTPUT)?;
        self.bus.write_reg(REG_OUTPUT, state | BIT_AMP_MUTE)?;
        debug!("IO Expander: AMP muted");
        Ok(())
    }

    /// Unmute AMP only (clear bit 1).
    pub fn unmute_amp(&mut self) -> Result<()> {
        let state = self.bus.read_reg(REG_OUTPUT)?;
        self.bus.write_reg(REG_OUTPUT, state & !BIT_AMP_MUTE)?;
        debug!("IO Expander: AMP unmuted");
        Ok(())
    }

    /// Mute DAC only (clear bit 2 — active LOW mute).
    pub fn mute_dac(&mut self) -> Result<()> {
        let state = self.bus.read_reg(REG_OUTPUT)?;
        self.bus.write_reg(REG_OUTPUT, state & !BIT_DAC_MUTE)?;
        debug!("IO Expander: DAC muted");
        Ok(())
    }

    /// Unmute DAC only (set bit 2 — HIGH = unmuted).
    pub fn unmute_dac(&mut self) -> Result<()> {
        let state = self.bus.read_reg(REG_OUTPUT)?;
        self.bus.write_reg(REG_OUTPUT, state | BIT_DAC_MUTE)?;
        debug!("IO Expander: DAC unmuted");
        Ok(())
    }

    /// Power off DSP via IO Expander bits 3-4 (active-low: set = power off).
    /// Sets bit 4 in reg 0x01 and bit 3 in reg 0x02.
    pub fn dsp_power_off(&mut self) -> Result<()> {
        let output = self.bus.read_reg(REG_OUTPUT)?;
        self.bus.write_reg(REG_OUTPUT, output | BIT_DSP_POWER1)?;
        let polarity = self.bus.read_reg(REG_POLARITY)?;
        self.bus.write_reg(REG_POLARITY, polarity | BIT_DSP_POWER2)?;
        info!("IO Expander: DSP powered off");
        Ok(())
    }

    /// Power on DSP via IO Expander bits 3-4 (active-low: clear = power on).
    /// Clears bit 4 in reg 0x01 and bit 3 in reg 0x02, then waits 50ms for stabilization.
    pub fn dsp_power_on(&mut self) -> Result<()> {
        let output = self.bus.read_reg(REG_OUTPUT)?;
        self.bus.write_reg(REG_OUTPUT, output & !BIT_DSP_POWER1)?;
        let polarity = self.bus.read_reg(REG_POLARITY)?;
        self.bus.write_reg(REG_POLARITY, polarity & !BIT_DSP_POWER2)?;
        thread::sleep(Duration::from_millis(50));
        info!("IO Expander: DSP powered on");
        Ok(())
    }

    /// Reset DSP by toggling bit 0 (LOW then HIGH).
    pub fn reset_dsp(&mut self) -> Result<()> {
        let state = self.bus.read_reg(REG_OUTPUT)?;

        // Set HIGH
        self.bus.write_reg(REG_OUTPUT, state | BIT_DSP_RESET)?;
        // Set LOW (hold reset)
        self.bus.write_reg(REG_OUTPUT, state & !BIT_DSP_RESET)?;
        thread::sleep(Duration::from_millis(20));
        // Release reset (HIGH)
        self.bus.write_reg(REG_OUTPUT, state | BIT_DSP_RESET)?;
        thread::sleep(Duration::from_millis(10));

        debug!("IO Expander: DSP reset");
        Ok(())
    }
}
