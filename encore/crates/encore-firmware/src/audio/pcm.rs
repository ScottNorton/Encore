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
    assert!(std::mem::size_of::<SndXferi>() == 12);
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
pub(crate) const INTERVAL_CHANNELS: usize = 2; // param 10 - 8
pub(crate) const INTERVAL_RATE: usize = 3; // param 11 - 8
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
    pub(crate) masks: [SndMask; 3], // ACCESS, FORMAT, SUBFORMAT
    pub(crate) mres: [SndMask; 5],  // reserved masks
    pub(crate) intervals: [SndInterval; 12], // SAMPLE_BITS..TICK_TIME
    pub(crate) ires: [SndInterval; 9], // reserved intervals
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

/// Argument for the interleaved transfer ioctls (`struct snd_xferi`).
/// 32-bit ARM ABI: `result` (long), `buf` (pointer), `frames` (unsigned long) = 12 bytes.
#[repr(C)]
pub(crate) struct SndXferi {
    pub(crate) result: isize,
    pub(crate) buf: *mut core::ffi::c_void,
    pub(crate) frames: usize,
}

// ── nix ioctl declarations ──────────────────────────────────────────

nix::ioctl_none!(pcm_prepare, ALSA_MAGIC, 0x40);
nix::ioctl_none!(pcm_start, ALSA_MAGIC, 0x42);
nix::ioctl_none!(pcm_drop, ALSA_MAGIC, 0x43);
nix::ioctl_none!(pcm_drain, ALSA_MAGIC, 0x44);

nix::ioctl_readwrite!(pcm_hw_params, ALSA_MAGIC, 0x11, SndPcmHwParams);
nix::ioctl_readwrite!(pcm_sw_params, ALSA_MAGIC, 0x13, SndPcmSwParams);
// SNDRV_PCM_IOCTL_READI_FRAMES is `_IOR` (not `_IOWR`). The kernel encodes it as
// _IOR('A', 0x51, struct snd_xferi) = 0x800c4151 on this BG2CDP 3.8 / arm32 build
// (snd_xferi = 12 bytes). Verified against the stock arecord via raw strace: every
// capture READI is ioctl(fd, 0x800c4151, ...). Using `_IOWR` (0xc00c4151) flips the
// direction bits, so the kernel's capture-ioctl switch misses the case and falls
// through to the common handler's -ENOTTY — which is why the plain read() syscall
// (also ENOTTY) was wrongly blamed before. Direction matters here, not just nr/size.
nix::ioctl_read!(pcm_readi, ALSA_MAGIC, 0x51, SndXferi);
// Playback uses the write() syscall (works on this kernel); see AlsaPcm::write_frames.

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
            pcm_hw_params(fd, &mut hw).context("SNDRV_PCM_IOCTL_HW_PARAMS failed")?;
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
        let buf = unsafe { std::slice::from_raw_parts(data.as_ptr() as *const u8, total_bytes) };
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

#[cfg(test)]
mod tests {
    use super::*;

    // ── SndMask ─────────────────────────────────────────────────────

    #[test]
    fn mask_new_all_sets_every_bit() {
        let m = SndMask::new_all();
        for word in m.bits.iter() {
            assert_eq!(*word, !0u32);
        }
    }

    #[test]
    fn set_single_clears_all_other_bits() {
        // Start from an all-ones mask to prove set_single zeroes everything first.
        let mut m = SndMask::new_all();
        m.set_single(0);
        assert_eq!(m.bits[0], 1);
        for word in m.bits.iter().skip(1) {
            assert_eq!(*word, 0);
        }
    }

    #[test]
    fn set_single_bit_zero_in_word_zero() {
        let mut m = SndMask::new_all();
        m.set_single(0);
        assert_eq!(m.bits[0], 1 << 0);
    }

    #[test]
    fn set_single_format_s32_le_position() {
        // FORMAT_S32_LE = 10 lands in word 0, bit 10.
        let mut m = SndMask::new_all();
        m.set_single(FORMAT_S32_LE);
        assert_eq!(m.bits[0], 1 << 10);
        for word in m.bits.iter().skip(1) {
            assert_eq!(*word, 0);
        }
    }

    #[test]
    fn set_single_access_rw_interleaved_position() {
        // ACCESS_RW_INTERLEAVED = 3 lands in word 0, bit 3.
        let mut m = SndMask::new_all();
        m.set_single(ACCESS_RW_INTERLEAVED);
        assert_eq!(m.bits[0], 1 << 3);
    }

    #[test]
    fn set_single_word_boundary_bit_31_and_32() {
        // bit 31 is the high bit of word 0.
        let mut m = SndMask::new_all();
        m.set_single(31);
        assert_eq!(m.bits[0], 0x8000_0000);
        assert_eq!(m.bits[1], 0);

        // bit 32 is the low bit of word 1.
        let mut m2 = SndMask::new_all();
        m2.set_single(32);
        assert_eq!(m2.bits[0], 0);
        assert_eq!(m2.bits[1], 1);
    }

    #[test]
    fn set_single_last_word_high_bit() {
        // bit 255 is the high bit of the final (8th) word, index 7.
        let mut m = SndMask::new_all();
        m.set_single(255);
        assert_eq!(m.bits[7], 0x8000_0000);
        for word in m.bits.iter().take(7) {
            assert_eq!(*word, 0);
        }
    }

    #[test]
    fn set_single_is_idempotent() {
        // Calling twice with the same bit leaves exactly one bit set.
        let mut m = SndMask::new_all();
        m.set_single(45);
        m.set_single(45);
        assert_eq!(m.bits[1], 1 << 13); // 45 = word 1, bit 13
        assert_eq!(m.bits[0], 0);
    }

    // ── SndInterval ─────────────────────────────────────────────────

    #[test]
    fn interval_new_any_is_full_range() {
        let iv = SndInterval::new_any();
        assert_eq!(iv.min, 0);
        assert_eq!(iv.max, !0u32);
        assert_eq!(iv.flags, 0);
    }

    #[test]
    fn set_exact_pins_min_and_max() {
        let mut iv = SndInterval::new_any();
        iv.set_exact(48000);
        assert_eq!(iv.min, 48000);
        assert_eq!(iv.max, 48000);
        assert_eq!(iv.min, iv.max);
    }

    #[test]
    fn set_exact_sets_integer_flag() {
        let mut iv = SndInterval::new_any();
        iv.set_exact(256);
        assert_eq!(iv.flags, 0x4);
    }

    #[test]
    fn set_exact_zero_boundary() {
        let mut iv = SndInterval::new_any();
        iv.set_exact(0);
        assert_eq!(iv.min, 0);
        assert_eq!(iv.max, 0);
        assert_eq!(iv.flags, 0x4);
    }

    #[test]
    fn set_exact_max_value_boundary() {
        let mut iv = SndInterval::new_any();
        iv.set_exact(u32::MAX);
        assert_eq!(iv.min, u32::MAX);
        assert_eq!(iv.max, u32::MAX);
        assert_eq!(iv.flags, 0x4);
    }

    // ── SndPcmHwParams::new ─────────────────────────────────────────

    #[test]
    fn hw_params_new_initializes_masks_and_intervals() {
        let p = SndPcmHwParams::new();

        // All three real masks start all-ones.
        for mask in p.masks.iter() {
            assert_eq!(mask.bits[0], !0u32);
            assert_eq!(mask.bits[7], !0u32);
        }
        // Reserved masks also all-ones.
        for mask in p.mres.iter() {
            assert_eq!(mask.bits[0], !0u32);
        }
        // Intervals start as full "any" ranges.
        for iv in p.intervals.iter() {
            assert_eq!(iv.min, 0);
            assert_eq!(iv.max, !0u32);
            assert_eq!(iv.flags, 0);
        }
        for iv in p.ires.iter() {
            assert_eq!(iv.min, 0);
            assert_eq!(iv.max, !0u32);
        }

        // Scalar fields set by new().
        assert_eq!(p.rmask, !0u32);
        assert_eq!(p.info, !0u32);
        // cmask was left zeroed by mem::zeroed and not touched by new().
        assert_eq!(p.cmask, 0);
    }
}
