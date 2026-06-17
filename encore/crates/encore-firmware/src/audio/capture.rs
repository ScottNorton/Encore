//! Native ALSA capture pipeline.
//!
//! Opens `/dev/snd/pcmC1D0c` (DSP mic output) at 48kHz stereo S32_LE,
//! deinterleaves into left (recognition/beamformed) and right (call/near-field)
//! channels, downsamples 48→16kHz via 3:1 decimation, and distributes 16kHz
//! mono S16 chunks to registered consumers via mpsc channels.
//!
//! The capture thread is a dedicated OS thread (like the mixer thread) for
//! deterministic timing — it must not run on tokio's thread pool.

use super::pcm::{
    pcm_hw_params, pcm_prepare, pcm_sw_params, SndPcmHwParams, SndPcmSwParams,
    ACCESS_RW_INTERLEAVED, FORMAT_S32_LE, INTERVAL_BUFFER_SIZE, INTERVAL_CHANNELS,
    INTERVAL_PERIOD_SIZE, INTERVAL_RATE,
};
use anyhow::{bail, Context, Result};
use std::fs::OpenOptions;
use std::io::Read;
use std::os::unix::io::AsRawFd;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::{info, warn};

/// Which stereo channel to receive from the DSP capture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptureChannel {
    /// Left channel — far-field beamformed recognition stream.
    Left,
    /// Right channel — near-field call stream.
    Right,
}

/// Handle returned when registering a capture consumer.
pub struct CaptureConsumer {
    pub rx: mpsc::Receiver<Vec<i16>>,
    pub channel: CaptureChannel,
}

/// Manages capture consumers and the capture thread lifecycle.
pub struct CaptureManager {
    consumers: Vec<(CaptureChannel, mpsc::Sender<Vec<i16>>)>,
    running: Arc<AtomicBool>,
}

impl CaptureManager {
    pub fn new() -> Self {
        Self {
            consumers: Vec::new(),
            running: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Register a capture consumer for a specific channel.
    /// Returns a `CaptureConsumer` with an mpsc receiver for 16kHz mono S16 chunks.
    /// Must be called before `start()`.
    pub fn add_consumer(&mut self, channel: CaptureChannel) -> CaptureConsumer {
        // Buffer 8 chunks — at ~170 chunks/sec (256 frames @ 48kHz / 3 = ~170),
        // this is ~47ms of buffering before backpressure.
        let (tx, rx) = mpsc::channel(8);
        self.consumers.push((channel, tx));
        CaptureConsumer { rx, channel }
    }

    /// Are there any registered consumers?
    pub fn has_consumers(&self) -> bool {
        !self.consumers.is_empty()
    }

    /// Spawn the capture thread. Returns the thread join handle.
    /// The thread reads from ALSA, processes, and distributes to consumers.
    pub fn start(
        self,
        card: u32,
        device: u32,
    ) -> Result<(Arc<AtomicBool>, std::thread::JoinHandle<()>)> {
        let running = self.running.clone();
        running.store(true, Ordering::Release);

        let consumers = self.consumers;
        let running2 = running.clone();

        let handle = std::thread::Builder::new()
            .name("encore-capture".into())
            .spawn(move || {
                if let Err(e) = capture_thread(card, device, consumers, &running2) {
                    warn!("Capture thread exited with error: {}", e);
                }
                running2.store(false, Ordering::Release);
            })
            .context("failed to spawn capture thread")?;

        Ok((running, handle))
    }
}

/// Capture thread entry point. Opens ALSA capture device, reads periods,
/// processes and distributes to consumers.
fn capture_thread(
    card: u32,
    device: u32,
    consumers: Vec<(CaptureChannel, mpsc::Sender<Vec<i16>>)>,
    running: &AtomicBool,
) -> Result<()> {
    let path = format!("/dev/snd/pcmC{}D{}c", card, device);
    info!("Capture: opening {}", path);

    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(&path)
        .with_context(|| format!("failed to open {path}"))?;

    let fd = file.as_raw_fd();

    // Configure hardware params — stereo S32_LE at 48kHz
    let mut hw = SndPcmHwParams::new();
    hw.masks[0].set_single(ACCESS_RW_INTERLEAVED);
    hw.masks[1].set_single(FORMAT_S32_LE);
    hw.intervals[INTERVAL_CHANNELS].set_exact(2);
    hw.intervals[INTERVAL_RATE].set_exact(48000);
    hw.intervals[INTERVAL_PERIOD_SIZE].set_exact(256);
    let buffer_size = 256 * 16;
    hw.intervals[INTERVAL_BUFFER_SIZE].set_exact(buffer_size);

    unsafe {
        pcm_hw_params(fd, &mut hw).context("Capture: HW_PARAMS failed")?;
    }

    let actual_period = hw.intervals[INTERVAL_PERIOD_SIZE].min;
    let actual_buffer = hw.intervals[INTERVAL_BUFFER_SIZE].min;
    let actual_rate = hw.rate_num;
    info!(
        "Capture: hw_params: rate={}, period={}, buffer={}, channels=2",
        actual_rate, actual_period, actual_buffer
    );

    // Software params — start immediately on first read
    let mut boundary = actual_buffer;
    while boundary * 2 <= 0x7FFF_FFFF {
        boundary *= 2;
    }

    let mut sw = SndPcmSwParams {
        tstamp_mode: 0,
        period_step: 1,
        sleep_min: 0,
        avail_min: actual_period,
        xfer_align: 0,
        start_threshold: 1, // start capture immediately
        stop_threshold: actual_buffer,
        silence_threshold: 0,
        silence_size: 0,
        boundary,
        reserved: [0; 64],
    };

    unsafe {
        pcm_sw_params(fd, &mut sw).context("Capture: SW_PARAMS failed")?;
    }

    unsafe {
        pcm_prepare(fd).context("Capture: PREPARE failed")?;
    }

    info!("Capture: ready, starting read loop");

    // Allocate read buffer — one period of stereo S32 frames
    let frames_per_period = actual_period as usize;
    let samples_per_period = frames_per_period * 2; // stereo
    let mut read_buf: Vec<i32> = vec![0i32; samples_per_period];
    let period_bytes = samples_per_period * 4; // S32_LE = 4 bytes/sample

    // Downsampled output buffers (48kHz → 16kHz = 3:1)
    let out_frames = frames_per_period / 3;
    let mut left_16k: Vec<i16> = vec![0i16; out_frames];
    let mut right_16k: Vec<i16> = vec![0i16; out_frames];

    // Separate consumer lists by channel for fast dispatch
    let left_consumers: Vec<&mpsc::Sender<Vec<i16>>> = consumers
        .iter()
        .filter(|(ch, _)| *ch == CaptureChannel::Left)
        .map(|(_, tx)| tx)
        .collect();
    let right_consumers: Vec<&mpsc::Sender<Vec<i16>>> = consumers
        .iter()
        .filter(|(ch, _)| *ch == CaptureChannel::Right)
        .map(|(_, tx)| tx)
        .collect();

    while running.load(Ordering::Acquire) {
        // Read one full period of interleaved stereo S32 frames via the read()
        // syscall. The BG2CDP 3.8 kernel returns ENOTTY for the READI_FRAMES
        // ioctl, so capture uses read() the same way playback uses write()
        // (see AlsaPcm::write_frames). read() may return short, so accumulate
        // until a full period is buffered.
        let got_period = {
            let byte_buf = unsafe {
                std::slice::from_raw_parts_mut(read_buf.as_mut_ptr() as *mut u8, period_bytes)
            };
            let mut offset = 0usize;
            loop {
                if !running.load(Ordering::Acquire) {
                    break false;
                }
                match file.read(&mut byte_buf[offset..]) {
                    Ok(0) => std::thread::sleep(std::time::Duration::from_millis(1)),
                    Ok(n) => {
                        offset += n;
                        if offset >= period_bytes {
                            break true;
                        }
                    }
                    Err(e) if e.raw_os_error() == Some(32) => {
                        // EPIPE = overrun — re-prepare and refill from the next period
                        warn!("Capture: overrun, recovering");
                        unsafe {
                            pcm_prepare(fd).ok();
                        }
                        break false;
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(std::time::Duration::from_millis(1));
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                    Err(e) => bail!("Capture: read failed: {}", e),
                }
            }
        };
        if !got_period {
            continue;
        }
        let frames_read = frames_per_period;

        // Deinterleave stereo S32 → two mono channels + downsample 3:1 + scale S32→S16
        let out_count = frames_read / 3;
        for i in 0..out_count {
            let src = i * 3; // 3:1 decimation — average 3 input frames

            // Left channel: average 3 consecutive S32 samples, scale to S16
            let l0 = read_buf[src * 2] as i64;
            let l1 = read_buf[(src + 1) * 2] as i64;
            let l2 = read_buf[(src + 2) * 2] as i64;
            let l_avg = ((l0 + l1 + l2) / 3) >> 16; // S32 → S16
            left_16k[i] = l_avg.clamp(i16::MIN as i64, i16::MAX as i64) as i16;

            // Right channel: same process
            let r0 = read_buf[src * 2 + 1] as i64;
            let r1 = read_buf[(src + 1) * 2 + 1] as i64;
            let r2 = read_buf[(src + 2) * 2 + 1] as i64;
            let r_avg = ((r0 + r1 + r2) / 3) >> 16;
            right_16k[i] = r_avg.clamp(i16::MIN as i64, i16::MAX as i64) as i16;
        }

        // Distribute to consumers (try_send to avoid blocking capture thread)
        if !left_consumers.is_empty() {
            let chunk = left_16k[..out_count].to_vec();
            for tx in &left_consumers {
                let _ = tx.try_send(chunk.clone());
            }
        }
        if !right_consumers.is_empty() {
            let chunk = right_16k[..out_count].to_vec();
            for tx in &right_consumers {
                let _ = tx.try_send(chunk.clone());
            }
        }
    }

    info!("Capture thread stopped");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn downsample_3_to_1_averaging() {
        // Simulate 6 stereo S32 frames (12 samples), expect 2 output frames per channel
        let frames: Vec<i32> = vec![
            // Frame 0: L=0x10000, R=0x20000
            0x0001_0000,
            0x0002_0000,
            // Frame 1: L=0x20000, R=0x40000
            0x0002_0000,
            0x0004_0000,
            // Frame 2: L=0x30000, R=0x60000
            0x0003_0000,
            0x0006_0000,
            // Frame 3: L=0x40000, R=0x80000
            0x0004_0000,
            0x0008_0000,
            // Frame 4: L=0x50000, R=0xA0000
            0x0005_0000,
            0x000A_0000,
            // Frame 5: L=0x60000, R=0xC0000
            0x0006_0000,
            0x000C_0000,
        ];

        let out_count = 6 / 3; // 2 output frames
        let mut left = vec![0i16; out_count];
        let mut right = vec![0i16; out_count];

        for i in 0..out_count {
            let src = i * 3;
            let l0 = frames[src * 2] as i64;
            let l1 = frames[(src + 1) * 2] as i64;
            let l2 = frames[(src + 2) * 2] as i64;
            let l_avg = ((l0 + l1 + l2) / 3) >> 16;
            left[i] = l_avg.clamp(i16::MIN as i64, i16::MAX as i64) as i16;

            let r0 = frames[src * 2 + 1] as i64;
            let r1 = frames[(src + 1) * 2 + 1] as i64;
            let r2 = frames[(src + 2) * 2 + 1] as i64;
            let r_avg = ((r0 + r1 + r2) / 3) >> 16;
            right[i] = r_avg.clamp(i16::MIN as i64, i16::MAX as i64) as i16;
        }

        // Left: avg(1,2,3) = 2, avg(4,5,6) = 5
        assert_eq!(left[0], 2);
        assert_eq!(left[1], 5);
        // Right: avg(2,4,6) = 4, avg(8,10,12) = 10
        assert_eq!(right[0], 4);
        assert_eq!(right[1], 10);
    }

    #[test]
    fn capture_channel_equality() {
        assert_eq!(CaptureChannel::Left, CaptureChannel::Left);
        assert_ne!(CaptureChannel::Left, CaptureChannel::Right);
    }

    #[test]
    fn capture_manager_tracks_consumers() {
        let mut mgr = CaptureManager::new();
        assert!(!mgr.has_consumers());

        let _c1 = mgr.add_consumer(CaptureChannel::Left);
        assert!(mgr.has_consumers());

        let _c2 = mgr.add_consumer(CaptureChannel::Right);
        assert!(mgr.has_consumers());
    }
}
