//! Raw ALSA PCM interface via kernel i/o controls — RW (WRITEI) mode.
//!
//! Talks directly to /dev/snd/pcmC*D*p.
//! Uses RW_INTERLEAVED access with WRITEI_FRAMES ioctl
//!
//! Format: S32_LE stereo 48kHz, period=256, buffer=4096.
//! Struct layouts are for 32-bit ARM (kernel 3.8 ABI-compatible).

use anyhow::{bail, Context, Result};
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::os::unix::io::AsRawFd;
use tracing::{info, warn};

// ── Compile-time struct size assertions (32-bit ARM) ────────────────
#[cfg(target_pointer_width = "32")]
const _: () = {
    assert!(std::mem::size_of::<SndPcmHwParams>() == 604);
    assert!(std::mem::size_of::<SndPcmSwParams>() == 104);
};

// ── Kernel ALSA constants ────────────────────────────────────────────

pub(crate) const ALSA_MAGIC: u8 = b'A';

// Access types (mask bit positions)
#[allow(dead_code)]
pub(crate) const ACCESS_MMAP_INTERLEAVED: u32 = 0;
pub(crate) const ACCESS_RW_INTERLEAVED: u32 = 3;

// Format types (mask bit positions)
#[allow(dead_code)]
pub(crate) const FORMAT_S16_LE: u32 = 2;
pub(crate) const FORMAT_S32_LE: u32 = 10;

// HW param indices (for intervals array, offset by FIRST_INTERVAL=8)
pub(crate) const INTERVAL_CHANNELS: usize = 2;    // param 10 - 8
pub(crate) const INTERVAL_RATE: usize = 3;        // param 11 - 8
pub(crate) const INTERVAL_PERIOD_SIZE: usize = 5; // param 13 - 8
pub(crate) const INTERVAL_BUFFER_SIZE: usize = 9; // param 17 - 8

// ── Kernel structs (repr(C), 32-bit ARM layout) ─────────────────────

#[repr(C)]
#[derive(Clone)]
pub(crate) struct SndMask {
    pub(crate) bits: [u32; 8],
}

impl SndMask {
    pub(crate) fn new_all() -> Self {
        Self { bits: [!0u32; 8] }
    }

    pub(crate) fn set_single(&mut self, bit: u32) {
        for b in self.bits.iter_mut() {
            *b = 0;
        }
        self.bits[(bit / 32) as usize] |= 1 << (bit % 32);
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
pub(crate) struct SndInterval {
    pub(crate) min: u32,
    pub(crate) max: u32,
    pub(crate) flags: u32,
}

impl SndInterval {
    pub(crate) fn new_any() -> Self {
        Self {
            min: 0,
            max: !0u32,
            flags: 0,
        }
    }

    pub(crate) fn set_exact(&mut self, val: u32) {
        self.min = val;
        self.max = val;
        self.flags = 0x4; // integer bit
    }
}

#[repr(C)]
pub(crate) struct SndPcmHwParams {
    pub(crate) flags: u32,
    pub(crate) masks: [SndMask; 3],        // ACCESS, FORMAT, SUBFORMAT
    pub(crate) mres: [SndMask; 5],         // reserved masks
    pub(crate) intervals: [SndInterval; 12], // SAMPLE_BITS..TICK_TIME
    pub(crate) ires: [SndInterval; 9],     // reserved intervals
    pub(crate) rmask: u32,
    pub(crate) cmask: u32,
    pub(crate) info: u32,
    pub(crate) msbits: u32,
    pub(crate) rate_num: u32,
    pub(crate) rate_den: u32,
    pub(crate) fifo_size: u32,
    pub(crate) reserved: [u8; 64],
}

impl SndPcmHwParams {
    pub(crate) fn new() -> Self {
        let mut p: Self = unsafe { std::mem::zeroed() };
        for mask in p.masks.iter_mut() {
            *mask = SndMask::new_all();
        }
        for mask in p.mres.iter_mut() {
            *mask = SndMask::new_all();
        }
        for interval in p.intervals.iter_mut() {
            *interval = SndInterval::new_any();
        }
        for interval in p.ires.iter_mut() {
            *interval = SndInterval::new_any();
        }
        p.rmask = !0u32;
        p.info = !0u32;
        p
    }
}

#[repr(C)]
pub(crate) struct SndPcmSwParams {
    pub(crate) tstamp_mode: i32,
    pub(crate) period_step: u32,
    pub(crate) sleep_min: u32,
    pub(crate) avail_min: u32,
    pub(crate) xfer_align: u32,
    pub(crate) start_threshold: u32,
    pub(crate) stop_threshold: u32,
    pub(crate) silence_threshold: u32,
    pub(crate) silence_size: u32,
    pub(crate) boundary: u32,
    pub(crate) reserved: [u8; 64],
}

/// Transfer struct for READI/WRITEI ioctls.
#[repr(C)]
pub(crate) struct SndXferi {
    pub(crate) result: i32,
    pub(crate) buf: *mut u8,
    pub(crate) frames: u32,
}

// ── nix ioctl declarations ──────────────────────────────────────────

nix::ioctl_none!(pcm_prepare, ALSA_MAGIC, 0x40);
nix::ioctl_none!(pcm_start, ALSA_MAGIC, 0x42);
nix::ioctl_none!(pcm_drop, ALSA_MAGIC, 0x43);
nix::ioctl_none!(pcm_drain, ALSA_MAGIC, 0x44);

nix::ioctl_readwrite!(pcm_hw_params, ALSA_MAGIC, 0x11, SndPcmHwParams);
nix::ioctl_readwrite!(pcm_sw_params, ALSA_MAGIC, 0x13, SndPcmSwParams);
// Note: WRITEI_FRAMES ioctl (0x50) returns ENOTTY on BG2CDP kernel 3.8.
// Use write() syscall instead. READI_FRAMES (0x51) also returns ENOTTY —
// capture will need read() syscall too.
nix::ioctl_readwrite!(pcm_readi_frames, ALSA_MAGIC, 0x51, SndXferi);

// ── Public API ──────────────────────────────────────────────────────

/// PCM configuration
pub struct PcmConfig {
    pub card: u32,
    pub device: u32,
    pub rate: u32,
    pub channels: u32,
    pub period_size: u32,
    pub periods: u32,
}

impl Default for PcmConfig {
    fn default() -> Self {
        Self {
            card: 1,
            device: 0,
            rate: 48000,
            channels: 2,
            // S32_LE = 8 bytes/frame; DMA max 2048 bytes/period → 256 frames
            period_size: 256,
            periods: 16,
        }
    }
}

/// Raw ALSA PCM playback device using RW_INTERLEAVED (WRITEI) access.
pub struct AlsaPcm {
    file: File,
    buffer_size: u32,
    period_size: u32,
    channels: u32,
    frame_bytes: u32,
}

// SAFETY: AlsaPcm is moved to the mixer thread and never shared.
unsafe impl Send for AlsaPcm {}

impl AlsaPcm {
    /// Open and configure a PCM playback device with RW_INTERLEAVED access.
    pub fn open(config: &PcmConfig) -> Result<Self> {
        let path = format!("/dev/snd/pcmC{}D{}p", config.card, config.device);
        info!("Opening PCM: {} (RW mode)", path);

        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&path)
            .with_context(|| format!("failed to open {path}"))?;

        let fd = file.as_raw_fd();

        // Configure hardware params — match speaker-test's proven config
        let mut hw = SndPcmHwParams::new();
        hw.masks[0].set_single(ACCESS_RW_INTERLEAVED);
        hw.masks[1].set_single(FORMAT_S32_LE);
        hw.intervals[INTERVAL_CHANNELS].set_exact(config.channels);
        hw.intervals[INTERVAL_RATE].set_exact(config.rate);
        hw.intervals[INTERVAL_PERIOD_SIZE].set_exact(config.period_size);
        let buffer_size = config.period_size * config.periods;
        hw.intervals[INTERVAL_BUFFER_SIZE].set_exact(buffer_size);

        unsafe {
            pcm_hw_params(fd, &mut hw)
                .context("SNDRV_PCM_IOCTL_HW_PARAMS failed")?;
        }

        let actual_period = hw.intervals[INTERVAL_PERIOD_SIZE].min;
        let actual_buffer = hw.intervals[INTERVAL_BUFFER_SIZE].min;
        let actual_rate = hw.rate_num;
        info!(
            "PCM hw_params: rate={}, period={}, buffer={}, channels={} (RW)",
            actual_rate, actual_period, actual_buffer, config.channels
        );
        if actual_rate != config.rate {
            warn!(
                "PCM: requested {}Hz but hardware set {}Hz",
                config.rate, actual_rate
            );
        }

        let frame_bytes = config.channels * 4; // S32_LE

        // Configure software params — match speaker-test exactly
        let mut boundary = actual_buffer;
        while boundary * 2 <= 0x7FFF_FFFF {
            boundary *= 2;
        }

        let mut sw = SndPcmSwParams {
            tstamp_mode: 0, // NONE (speaker-test default)
            period_step: 1,
            sleep_min: 0,
            avail_min: actual_period,
            xfer_align: 0,
            start_threshold: actual_buffer, // match speaker-test: start after buffer full
            stop_threshold: actual_buffer,  // match speaker-test
            silence_threshold: 0,
            silence_size: 0,
            boundary,
            reserved: [0; 64],
        };

        unsafe {
            pcm_sw_params(fd, &mut sw).context("SNDRV_PCM_IOCTL_SW_PARAMS failed")?;
        }

        // Prepare the device
        unsafe {
            pcm_prepare(fd).context("SNDRV_PCM_IOCTL_PREPARE failed")?;
        }

        info!(
            "PCM ready: {}Hz {}ch S32_LE, period={} buffer={} frames (RW, start_threshold={})",
            actual_rate, config.channels, actual_period, actual_buffer, actual_buffer
        );

        Ok(Self {
            file,
            buffer_size: actual_buffer,
            period_size: actual_period,
            channels: config.channels,
            frame_bytes,
        })
    }

    /// Write interleaved S32_LE frames via write() syscall. Returns number of frames written.
    /// The kernel PCM device supports write() for data transfer (not WRITEI ioctl).
    /// Blocks until all frames are written or an unrecoverable error occurs.
    pub fn write_frames(&mut self, data: &[i32]) -> Result<u32> {
        let total_bytes = data.len() * 4; // i32 = 4 bytes
        let buf = unsafe {
            std::slice::from_raw_parts(data.as_ptr() as *const u8, total_bytes)
        };
        let mut offset = 0usize;

        while offset < total_bytes {
            match self.file.write(&buf[offset..]) {
                Ok(0) => {
                    std::thread::sleep(std::time::Duration::from_millis(1));
                }
                Ok(n) => {
                    offset += n;
                }
                Err(e) if e.raw_os_error() == Some(32) => {
                    // EPIPE = XRUN
                    warn!("PCM XRUN, recovering");
                    let fd = self.file.as_raw_fd();
                    unsafe {
                        pcm_prepare(fd).context("XRUN recovery failed")?;
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(std::time::Duration::from_millis(1));
                }
                Err(e) => {
                    bail!("PCM write failed: {}", e);
                }
            }
        }

        Ok((total_bytes / self.frame_bytes as usize) as u32)
    }

    /// Start playback explicitly (if start_threshold wasn't hit).
    pub fn start(&mut self) -> Result<()> {
        let fd = self.file.as_raw_fd();
        unsafe {
            pcm_start(fd).context("PCM start failed")?;
        }
        Ok(())
    }

    /// Stop playback immediately (drop remaining frames).
    pub fn drop_pcm(&self) -> Result<()> {
        let fd = self.file.as_raw_fd();
        unsafe {
            pcm_drop(fd).ok();
        }
        Ok(())
    }

    /// Drain playback (wait for all frames to play, then stop).
    pub fn drain(&self) -> Result<()> {
        let fd = self.file.as_raw_fd();
        unsafe {
            pcm_drain(fd).context("PCM drain failed")?;
        }
        Ok(())
    }

    pub fn period_size(&self) -> u32 {
        self.period_size
    }

    pub fn buffer_size(&self) -> u32 {
        self.buffer_size
    }

    pub fn channels(&self) -> u32 {
        self.channels
    }

    pub fn frame_bytes(&self) -> u32 {
        self.frame_bytes
    }
}

impl Drop for AlsaPcm {
    fn drop(&mut self) {
        let fd = self.file.as_raw_fd();
        unsafe {
            let _ = pcm_drop(fd);
        }
    }
}
