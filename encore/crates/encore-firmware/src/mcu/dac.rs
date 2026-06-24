//! PCM5121 DAC driver (PCM512x family, I2C address 0x4C). Silkscreen-confirmed
//! on the amp board (40-HKTANA-MAE4G).
//!
//! Scope: initialization, digital volume (with the safe loudness cap), mute, and
//! standby. The DAC runs on its default Program 1 (reconstruction filter only).
//! Its on-chip Program-5 EQ engine is deliberately NOT used: it requires the
//! complete PurePath flow coefficient image (which we do not have), and selecting
//! Program 5 without it zeroes the downstream mixer/volume coefficients and mutes
//! the output. All EQ/tone shaping is done in software in the mixer thread (see
//! `encore_common::dsp`). `read_paged`/`write_paged` remain for the dashboard
//! register explorer.

use super::i2c::I2CBus;
use anyhow::Result;
use tracing::{debug, info};

const I2C_BUS: &str = "/dev/i2c-0";
const DAC_ADDR: u16 = 0x4C;

/// Silent DAC register value (lower = louder). Used by `volume_to_reg`.
pub const DAC_VOL_QUIETEST: u8 = 0xA0;

/// Safety cap: fixed DAC attenuation level that prevents the amp from
/// driving speakers beyond safe excursion at max DSP output.
/// 0x30 = -24dB from absolute max. This matches the stock init default.
pub const DAC_SAFETY_CAP: u8 = 0x30;

/// Absolute hard floor on the loudest DAC register the firmware will ever drive,
/// regardless of config. 0x18 = -12 dB from full scale. `volume_to_reg` clamps
/// any configured cap to this, so a mis-set config can only ever make the device
/// quieter than this floor, never louder — protecting the woofer and hearing.
pub const MIN_SAFE_VOLUME_REG: u8 = 0x18;

/// Map a 0-100 percentage into the safe DAC register range `[max_reg ..= 0xA0]`.
/// The DAC register is inverted: lower = louder. `max_reg` is the loudest
/// (smallest) value the loudness cap permits, clamped to [`MIN_SAFE_VOLUME_REG`].
///   volume_to_reg(0, _)      → 0xA0 (silent)
///   volume_to_reg(100, 0x30) → 0x30 (-24 dB, the stock loudest)
pub fn volume_to_reg(percent: u8, max_reg: u8) -> u8 {
    let max_reg = max_reg.max(MIN_SAFE_VOLUME_REG);
    let span = (DAC_VOL_QUIETEST - max_reg) as u16;
    let p = percent.min(100) as u16;
    DAC_VOL_QUIETEST - (p * span / 100) as u8
}

// Power control (page 0)
const REG_POWER: u8 = 0x02;
const POWER_ACTIVE: u8 = 0x00;
const POWER_STANDBY: u8 = 0x10;

// Volume registers
const REG_VOL_LEFT: u8 = 0x3D;
const REG_VOL_RIGHT: u8 = 0x3E;

/// Init sequence: (register, value) pairs with 5ms between each (handled by I2CBus).
const INIT_SEQUENCE: [(u8, u8); 10] = [
    (0x00, 0x00), // Page select
    (0x01, 0x11), // Reset / standby control
    (0x0D, 0x10), // I2S data path config
    (0x25, 0x08), // Clock config
    (0x41, 0x04), // Volume ramp start
    (0x41, 0x07), // Volume ramp config
    (0x08, 0x3F), // Clock source / analog gain
    (0x28, 0x00), // I2S config -- CRITICAL: never change this register
    (0x3D, 0x30), // Left channel volume (default)
    (0x3E, 0x30), // Right channel volume (default)
];

/// Page-select register. The dashboard register explorer pages the chip via
/// `read_paged`/`write_paged`.
const REG_PAGE_SEL: u8 = 0x00;

/// Driver for the PCM5121 DAC at I2C 0x4C: initialization, digital volume, mute,
/// and standby. Tone/EQ is done in software in the mixer, not on this chip.
pub struct Dac {
    bus: I2CBus,
}

impl Dac {
    pub fn open() -> Result<Self> {
        let bus = I2CBus::open(I2C_BUS, DAC_ADDR)?;
        Ok(Self { bus })
    }

    /// Run the full 10-register initialization sequence.
    pub fn init(&mut self) -> Result<()> {
        info!("DAC init: writing {} registers", INIT_SEQUENCE.len());

        // Ensure DAC is not in standby (reg 0x02). Stock firmware never writes
        // this register, but a previous firmware version may have left it in
        // standby mode. The PCM5121 retains register state across soft resets.
        self.write_paged(0, REG_POWER, POWER_ACTIVE)?;

        for (reg, val) in &INIT_SEQUENCE {
            self.bus.write_reg(*reg, *val)?;
        }

        info!("DAC init complete");
        Ok(())
    }

    /// Set hardware volume (both channels).
    /// Lower values = louder. Range: 0x00 (max) to 0xA0+ (silent).
    /// Use DAC_VOL_* constants for safe values.
    pub fn set_volume(&mut self, level: u8) -> Result<()> {
        // Volume regs (0x3D/0x3E) live on page 0. The register explorer can leave
        // the page pointer elsewhere, so always select page 0 before writing them.
        self.select_page(0)?;
        self.bus.write_reg(REG_VOL_LEFT, level)?;
        self.bus.write_reg(REG_VOL_RIGHT, level)?;
        debug!("DAC volume: 0x{:02X}", level);
        Ok(())
    }

    /// Hardware mute — set both channels to silent (0xFF).
    /// Used when user volume = 0 for true analog silence.
    pub fn mute(&mut self) -> Result<()> {
        self.select_page(0)?;
        self.bus.write_reg(REG_VOL_LEFT, 0xFF)?;
        self.bus.write_reg(REG_VOL_RIGHT, 0xFF)?;
        debug!("DAC: muted");
        Ok(())
    }

    /// Hardware unmute — restore both channels to the safety cap level.
    /// Called before DSP volume is applied (volume > 0).
    pub fn unmute(&mut self) -> Result<()> {
        self.select_page(0)?;
        self.bus.write_reg(REG_VOL_LEFT, DAC_SAFETY_CAP)?;
        self.bus.write_reg(REG_VOL_RIGHT, DAC_SAFETY_CAP)?;
        debug!("DAC: unmuted (safety cap 0x{:02X})", DAC_SAFETY_CAP);
        Ok(())
    }

    /// Enter standby mode — reduces DAC power from ~372 mW to ~81 mW.
    /// Writes page 0, reg 0x02 = 0x10.
    pub fn enter_standby(&mut self) -> Result<()> {
        self.write_paged(0, REG_POWER, POWER_STANDBY)?;
        info!("DAC: entered standby");
        Ok(())
    }

    /// Exit standby mode — returns to active operation.
    /// Writes page 0, reg 0x02 = 0x00.
    pub fn exit_standby(&mut self) -> Result<()> {
        self.write_paged(0, REG_POWER, POWER_ACTIVE)?;
        info!("DAC: exited standby");
        Ok(())
    }

    /// Select a register page.
    fn select_page(&mut self, page: u8) -> Result<()> {
        self.bus.write_reg(REG_PAGE_SEL, page)
    }

    /// Read a register on a specific page (used by the dashboard register explorer).
    pub fn read_paged(&mut self, page: u8, reg: u8) -> Result<u8> {
        self.select_page(page)?;
        self.bus.read_reg(reg)
    }

    /// Write a register on a specific page (used by the dashboard register explorer).
    pub fn write_paged(&mut self, page: u8, reg: u8, value: u8) -> Result<()> {
        self.select_page(page)?;
        self.bus.write_reg(reg, value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn volume_maps_endpoints_and_is_monotonic() {
        // 0% is always silent; 100% lands exactly on the cap.
        assert_eq!(volume_to_reg(0, 0x30), DAC_VOL_QUIETEST);
        assert_eq!(volume_to_reg(100, 0x30), 0x30);
        assert_eq!(volume_to_reg(100, 0x37), 0x37);
        // Louder = smaller register; volume rises monotonically with percent.
        assert!(volume_to_reg(80, 0x37) < volume_to_reg(40, 0x37));
    }

    #[test]
    fn cap_cannot_exceed_hard_floor() {
        // A reckless config (0x00 = full scale) is clamped so 100% never drives
        // louder than the safety floor.
        assert_eq!(volume_to_reg(100, 0x00), MIN_SAFE_VOLUME_REG);
        assert!(volume_to_reg(100, 0x05) >= MIN_SAFE_VOLUME_REG);
    }
}
