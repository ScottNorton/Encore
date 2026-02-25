//! TAS5756M DAC driver (PCM512x family, I2C address 0x4C).
//!
//! Handles DAC initialization, digital volume control, and the miniDSP
//! processing engine: 10-band parametric EQ via hardware biquad filters
//! (HybridFlow 6) with glitch-free CRAM double-buffer updates.

use super::i2c::I2CBus;
use anyhow::Result;
use encore_common::protocol::FilterType;
use tracing::{debug, info};

const I2C_BUS: &str = "/dev/i2c-0";
const DAC_ADDR: u16 = 0x4C;

/// DAC volume range (lower value = louder).
pub const DAC_VOL_LOUDEST: u8 = 0x00;
pub const DAC_VOL_DEFAULT: u8 = 0x60;
pub const DAC_VOL_QUIETEST: u8 = 0xA0;
pub const DAC_VOL_STEP: u8 = 0x04;

/// Safety cap: fixed DAC attenuation level that prevents the amp from
/// driving speakers beyond safe excursion at max DSP output.
/// 0x30 = -24dB from absolute max. This matches the stock init default.
pub const DAC_SAFETY_CAP: u8 = 0x30;

/// Map a 0-100 percentage to the DAC register value.
/// DAC register is inverted: 0x00 = loudest, 0xA0 = silent.
///   volume_to_reg(100) → 0x00 (loudest)
///   volume_to_reg(50)  → 0x50 (mid)
///   volume_to_reg(0)   → 0xA0 (silent)
pub fn volume_to_reg(percent: u8) -> u8 {
    let p = percent.min(100) as u16;
    (0xA0 - (p * 0xA0 / 100)) as u8
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

// TAS5756M page-selectable registers
const REG_PAGE_SEL: u8 = 0x00;
const REG_DSP_PROG_SEL: u8 = 0x2B;  // HybridFlow selection (page 0)
const HYBRIDFLOW_6: u8 = 6;          // 10 generic biquad filters

// CRAM (Coefficient RAM) pages
const CRAM_BUFFER_A_START: u8 = 44;
#[allow(dead_code)]
const CRAM_BUFFER_B_START: u8 = 62;
const CRAM_CTRL_REG: u8 = 0x01;      // Page 44, reg 0x01: buffer swap control
const CRAM_DATA_START: u8 = 0x08;     // First coefficient byte within a CRAM page

/// Number of EQ biquad filters available in HybridFlow 6
pub const EQ_BAND_COUNT: usize = 10;
/// Bytes per biquad: 5 coefficients x 4 bytes each
const BIQUAD_BYTE_SIZE: usize = 20;

/// 3.23 fixed-point representation of 1.0 (unity gain passthrough b0)
const FIXED_ONE: u32 = 0x0080_0000;

/// 5 biquad coefficients in TAS5756M 3.23 fixed-point format.
#[derive(Debug, Clone, Copy)]
struct BiquadCoeffs {
    b0: u32,
    b1: u32,
    b2: u32,
    a1: u32,
    a2: u32,
}

impl BiquadCoeffs {
    fn passthrough() -> Self {
        Self { b0: FIXED_ONE, b1: 0, b2: 0, a1: 0, a2: 0 }
    }

    fn to_fixed(val: f64) -> u32 {
        let clamped = val.clamp(-4.0, 3.999_999);
        let scaled = (clamped * (1 << 23) as f64).round() as i32;
        scaled as u32
    }

    /// Compute biquad coefficients using Robert Bristow-Johnson's Audio EQ Cookbook.
    /// TAS5756M expects a1/a2 with INVERTED signs vs. textbook.
    fn compute(freq_hz: f64, gain_db: f64, q: f64, filter_type: FilterType) -> Self {
        let sample_rate = 48000.0;
        let w0 = 2.0 * std::f64::consts::PI * freq_hz / sample_rate;
        let cos_w0 = w0.cos();
        let sin_w0 = w0.sin();
        let alpha = sin_w0 / (2.0 * q);
        let a_lin = 10.0_f64.powf(gain_db / 40.0);

        let (b0, b1, b2, a0, a1, a2) = match filter_type {
            FilterType::Peak => {
                (1.0 + alpha * a_lin, -2.0 * cos_w0, 1.0 - alpha * a_lin,
                 1.0 + alpha / a_lin, -2.0 * cos_w0, 1.0 - alpha / a_lin)
            }
            FilterType::LowShelf => {
                let tsa = 2.0 * a_lin.sqrt() * alpha;
                (a_lin * ((a_lin + 1.0) - (a_lin - 1.0) * cos_w0 + tsa),
                 2.0 * a_lin * ((a_lin - 1.0) - (a_lin + 1.0) * cos_w0),
                 a_lin * ((a_lin + 1.0) - (a_lin - 1.0) * cos_w0 - tsa),
                 (a_lin + 1.0) + (a_lin - 1.0) * cos_w0 + tsa,
                 -2.0 * ((a_lin - 1.0) + (a_lin + 1.0) * cos_w0),
                 (a_lin + 1.0) + (a_lin - 1.0) * cos_w0 - tsa)
            }
            FilterType::HighShelf => {
                let tsa = 2.0 * a_lin.sqrt() * alpha;
                (a_lin * ((a_lin + 1.0) + (a_lin - 1.0) * cos_w0 + tsa),
                 -2.0 * a_lin * ((a_lin - 1.0) + (a_lin + 1.0) * cos_w0),
                 a_lin * ((a_lin + 1.0) + (a_lin - 1.0) * cos_w0 - tsa),
                 (a_lin + 1.0) - (a_lin - 1.0) * cos_w0 + tsa,
                 2.0 * ((a_lin - 1.0) - (a_lin + 1.0) * cos_w0),
                 (a_lin + 1.0) - (a_lin - 1.0) * cos_w0 - tsa)
            }
            FilterType::Notch => {
                (1.0, -2.0 * cos_w0, 1.0,
                 1.0 + alpha, -2.0 * cos_w0, 1.0 - alpha)
            }
        };

        Self {
            b0: Self::to_fixed(b0 / a0),
            b1: Self::to_fixed(b1 / a0),
            b2: Self::to_fixed(b2 / a0),
            a1: Self::to_fixed(-(a1 / a0)),
            a2: Self::to_fixed(-(a2 / a0)),
        }
    }

    fn to_bytes(&self) -> [u8; BIQUAD_BYTE_SIZE] {
        let mut buf = [0u8; BIQUAD_BYTE_SIZE];
        buf[0..4].copy_from_slice(&self.b0.to_be_bytes());
        buf[4..8].copy_from_slice(&self.b1.to_be_bytes());
        buf[8..12].copy_from_slice(&self.b2.to_be_bytes());
        buf[12..16].copy_from_slice(&self.a1.to_be_bytes());
        buf[16..20].copy_from_slice(&self.a2.to_be_bytes());
        buf
    }
}

/// Driver for the DAC (TAS5756M (PCM512x family)) at 0x4C.
/// Controls audio DAC initialization and hardware volume.
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
        self.bus.write_reg(REG_VOL_LEFT, level)?;
        self.bus.write_reg(REG_VOL_RIGHT, level)?;
        debug!("DAC volume: 0x{:02X}", level);
        Ok(())
    }

    /// Hardware mute — set both channels to silent (0xFF).
    /// Used when user volume = 0 for true analog silence.
    pub fn mute(&mut self) -> Result<()> {
        self.bus.write_reg(REG_VOL_LEFT, 0xFF)?;
        self.bus.write_reg(REG_VOL_RIGHT, 0xFF)?;
        debug!("DAC: muted");
        Ok(())
    }

    /// Hardware unmute — restore both channels to the safety cap level.
    /// Called before DSP volume is applied (volume > 0).
    pub fn unmute(&mut self) -> Result<()> {
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

    /// Select a TAS5756M register page.
    fn select_page(&mut self, page: u8) -> Result<()> {
        self.bus.write_reg(REG_PAGE_SEL, page)
    }

    /// Read a register on a specific page.
    pub fn read_paged(&mut self, page: u8, reg: u8) -> Result<u8> {
        self.select_page(page)?;
        self.bus.read_reg(reg)
    }

    /// Write a register on a specific page.
    pub fn write_paged(&mut self, page: u8, reg: u8, value: u8) -> Result<()> {
        self.select_page(page)?;
        self.bus.write_reg(reg, value)
    }

    /// Select HybridFlow program. Call during init, after reset.
    pub fn select_hybridflow(&mut self, program: u8) -> Result<()> {
        self.write_paged(0, REG_DSP_PROG_SEL, program)?;
        info!("DAC: selected HybridFlow {}", program);
        Ok(())
    }

    /// Write biquad coefficients to CRAM for a single band.
    fn write_biquad_cram(&mut self, band: usize, coeffs: &BiquadCoeffs) -> Result<()> {
        let byte_offset = band * BIQUAD_BYTE_SIZE;
        let page = CRAM_BUFFER_A_START + (byte_offset / 120) as u8;
        let reg_start = CRAM_DATA_START + (byte_offset % 120) as u8;
        self.select_page(page)?;
        let bytes = coeffs.to_bytes();
        for (i, &byte) in bytes.iter().enumerate() {
            self.bus.write_reg(reg_start + i as u8, byte)?;
        }
        Ok(())
    }

    /// Swap CRAM buffers to apply new coefficients glitch-free.
    fn swap_cram_buffer(&mut self) -> Result<()> {
        self.select_page(CRAM_BUFFER_A_START)?;
        let ctrl = self.bus.read_reg(CRAM_CTRL_REG)?;
        self.bus.write_reg(CRAM_CTRL_REG, ctrl | 0x01)?;
        debug!("DAC: CRAM buffer swapped");
        Ok(())
    }

    /// Program all 10 EQ bands. Writes to CRAM + swaps buffer.
    pub fn program_eq(&mut self, bands: &[encore_common::protocol::EqBand; EQ_BAND_COUNT], enabled: bool) -> Result<()> {
        for (i, band) in bands.iter().enumerate() {
            let coeffs = if enabled && band.gain_cb != 0 {
                BiquadCoeffs::compute(
                    band.freq_hz as f64,
                    band.gain_cb as f64 / 10.0,
                    band.q_x10 as f64 / 10.0,
                    band.filter_type,
                )
            } else {
                BiquadCoeffs::passthrough()
            };
            self.write_biquad_cram(i, &coeffs)?;
        }
        self.swap_cram_buffer()?;
        info!("DAC: EQ programmed ({} bands, enabled={})", EQ_BAND_COUNT, enabled);
        Ok(())
    }

    /// Initialize EQ: select HybridFlow 6, load flat biquads.
    pub fn init_eq(&mut self) -> Result<()> {
        self.select_hybridflow(HYBRIDFLOW_6)?;
        let flat = [encore_common::protocol::EqBand::default(); EQ_BAND_COUNT];
        self.program_eq(&flat, true)?;
        info!("DAC: EQ initialized (HybridFlow 6, flat)");
        Ok(())
    }
}
