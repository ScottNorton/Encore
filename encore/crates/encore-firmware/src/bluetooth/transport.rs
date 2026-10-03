//! A2DP transport reader — RTP packet parsing and multi-codec decode pipeline.
//!
//! When a phone connects via A2DP, the AVDTP state machine accepts a media
//! transport L2CAP socket. This module reads RTP packets from that socket,
//! strips headers, decodes audio (SBC, aptX, or aptX HD), and pushes PCM
//! into a MixerSlot.

use crate::audio::mixer::MixerSlot;
use crate::audio::resample::{AdaptiveResampler, LatencyServo, ServoVerdict};
use crate::bluetooth::aptx::{AptxDecoder, APTX_MAX_SAMPLES};
use crate::bluetooth::sbc::{SbcDecoder, SBC_MAX_SAMPLES};
use crate::bluetooth::A2dpCodec;
use anyhow::{Context, Result};
use std::io::Read;
use std::os::fd::{FromRawFd, IntoRawFd, OwnedFd};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tracing::{debug, info, warn};

/// Sets the reader's `running` flag false on drop, so the AVDTP state machine
/// always learns when a reader thread exits — even on early return or panic.
///
/// The media read is a plain blocking read (full L2CAP SDUs, one RTP packet
/// per read — do NOT add SO_RCVTIMEO, it fragments the SDU on this kernel and
/// shreds the decode). A paused stream just parks the reader here harmlessly;
/// CLOSE/disconnect close the channel, so the read returns EOF/error and the
/// reader exits, clearing this flag.
struct RunningGuard(Arc<AtomicBool>);
impl Drop for RunningGuard {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

/// Bluetooth A2DP source sample rate (44100 Hz for virtually all phones/computers).
const BT_SOURCE_RATE: u32 = 44100;
/// Mixer/PCM output sample rate.
const MIXER_RATE: u32 = 48000;

/// Default latency-servo target when the config leaves it unset (ms).
const DEFAULT_LATENCY_TARGET_MS: u16 = 100;
/// Config clamp: below this the buffer can't ride out normal A2DP burst
/// pacing; above it lip sync is unwatchable anyway.
const LATENCY_TARGET_RANGE_MS: std::ops::RangeInclusive<u16> = 40..=400;

/// Resolve the configured target (0/absent = default) into a safe value.
pub fn resolve_latency_target_ms(configured: u16) -> u32 {
    let t = if configured == 0 {
        DEFAULT_LATENCY_TARGET_MS
    } else {
        configured
    };
    t.clamp(*LATENCY_TARGET_RANGE_MS.start(), *LATENCY_TARGET_RANGE_MS.end()) as u32
}

/// Resample a decoded chunk and push it into the mixer slot, steered by the
/// latency servo: the servo watches the slot fill (the device's own measure of
/// how far behind the live stream it is), trims the resample ratio to hold the
/// fill at the target — clock drift between the source and our DAC never
/// accumulates — and, when the stream has fallen catastrophically behind,
/// drops whole chunks until we've caught back up.
fn resample_servo_push(
    resampler: &mut AdaptiveResampler,
    servo: &mut LatencyServo,
    slot: &MixerSlot,
    pcm: &[i32],
    scratch: &mut Vec<i32>,
    codec_name: &str,
) {
    match servo.update(slot.available()) {
        ServoVerdict::Skip => {
            // Chunk deliberately dropped: catch-up at real-time rate.
            if servo.skipped_chunks == 1 || servo.skipped_chunks.is_multiple_of(500) {
                warn!(
                    "BT A2DP {}: behind live stream (buffer {} ms), skipping to catch up (total skipped {})",
                    codec_name,
                    servo.latency_ms(),
                    servo.skipped_chunks
                );
            }
        }
        ServoVerdict::Correct(ppm) => {
            resampler.set_correction_ppm(ppm);
            scratch.clear();
            resampler.process(pcm, scratch);
            let written = slot.push(scratch);
            if written < scratch.len() {
                debug!(
                    "BT A2DP {}: ring full, dropped {} samples",
                    codec_name,
                    scratch.len() - written
                );
            }
        }
    }
}

/// RTP header size (12 bytes).
const RTP_HEADER_SIZE: usize = 12;
/// A2DP media header (1 byte) — only present for SBC.
const A2DP_MEDIA_HEADER_SIZE: usize = 1;
/// Minimum SBC packet size.
const MIN_SBC_PACKET_SIZE: usize = RTP_HEADER_SIZE + A2DP_MEDIA_HEADER_SIZE;

/// Parse an RTP + A2DP media header from an SBC packet.
///
/// Returns `(sbc_payload_offset, frame_count)` or None if the packet is too small.
fn parse_rtp_sbc(buf: &[u8]) -> Option<(usize, u8)> {
    if buf.len() < MIN_SBC_PACKET_SIZE {
        return None;
    }

    let version = (buf[0] >> 6) & 0x03;
    if version != 2 {
        return None;
    }

    // A2DP media header: 1 byte after RTP header
    // Bits 7-4: frame count
    let frame_count = (buf[RTP_HEADER_SIZE] >> 4) & 0x0F;
    let payload_offset = RTP_HEADER_SIZE + A2DP_MEDIA_HEADER_SIZE;

    Some((payload_offset, frame_count))
}

/// Parse an RTP header for aptX/aptX HD packets.
///
/// Returns the payload offset. Some A2DP implementations (notably Windows)
/// send raw aptX codewords WITHOUT RTP headers — in that case, offset is 0.
fn parse_rtp_aptx(buf: &[u8], has_rtp: &mut Option<bool>) -> usize {
    // Once we've determined RTP presence, use cached result
    if let Some(rtp) = *has_rtp {
        return if rtp { RTP_HEADER_SIZE } else { 0 };
    }

    // Auto-detect: check if first byte looks like RTP v2
    if buf.len() >= RTP_HEADER_SIZE {
        let version = (buf[0] >> 6) & 0x03;
        if version == 2 {
            *has_rtp = Some(true);
            return RTP_HEADER_SIZE;
        }
    }

    // Not RTP — raw aptX codewords directly on L2CAP
    *has_rtp = Some(false);
    0
}

/// Handle to a running A2DP reader thread.
#[derive(Clone)]
pub struct ReaderHandle {
    /// Set to `true` to ask the reader to stop. A blocking read only observes
    /// this once it returns, so in practice the reader exits when the media
    /// channel closes (CLOSE/disconnect → EOF) rather than mid-read.
    pub stop: Arc<AtomicBool>,
    /// `true` while the reader thread is alive; cleared when it exits for any
    /// reason. Lets the AVDTP state machine tell "paused but alive" (resume in
    /// place) from "reader died" (must respawn).
    pub running: Arc<AtomicBool>,
}

impl ReaderHandle {
    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::Acquire)
    }
}

/// Spawn a dedicated reader thread for the A2DP transport FD.
///
/// Reads RTP packets, decodes audio frames, pushes PCM into the MixerSlot.
/// Returns a [`ReaderHandle`] — set `stop` to true to halt the thread; check
/// `running` to see whether it is still alive.
pub fn spawn_reader(
    fd: OwnedFd,
    read_mtu: u16,
    slot: Arc<MixerSlot>,
    codec: A2dpCodec,
    latency_target_ms: u32,
) -> Result<ReaderHandle> {
    let stop = Arc::new(AtomicBool::new(false));
    let running = Arc::new(AtomicBool::new(true));
    let stop_clone = stop.clone();
    let running_clone = running.clone();

    std::thread::Builder::new()
        .name("bt-a2dp-reader".into())
        .spawn(move || {
            let _guard = RunningGuard(running_clone);
            info!(
                "BT A2DP reader: started (codec={}, mtu={}, latency target {} ms)",
                codec, read_mtu, latency_target_ms
            );
            match codec {
                A2dpCodec::Sbc => {
                    reader_loop_sbc(fd, read_mtu, slot, stop_clone, latency_target_ms)
                }
                A2dpCodec::Aptx => {
                    reader_loop_aptx(fd, read_mtu, slot, stop_clone, false, latency_target_ms)
                }
                A2dpCodec::AptxHd => {
                    reader_loop_aptx(fd, read_mtu, slot, stop_clone, true, latency_target_ms)
                }
            }
        })
        .context("failed to spawn BT A2DP reader thread")?;

    Ok(ReaderHandle { stop, running })
}

/// SBC reader loop — original decode path.
fn reader_loop_sbc(
    fd: OwnedFd,
    read_mtu: u16,
    slot: Arc<MixerSlot>,
    stop: Arc<AtomicBool>,
    latency_target_ms: u32,
) {
    let mut file = unsafe { std::fs::File::from_raw_fd(fd.into_raw_fd()) };
    let mut buf = vec![0u8; read_mtu as usize];
    let mut decoder = SbcDecoder::new();
    let mut pcm_buf = vec![0i16; SBC_MAX_SAMPLES * 2]; // stereo interleaved
    let mut resampler = AdaptiveResampler::new(BT_SOURCE_RATE, MIXER_RATE);
    let mut servo = LatencyServo::new(latency_target_ms);
    let mut resampled = Vec::with_capacity(SBC_MAX_SAMPLES * 4);
    let mut total_packets: u64 = 0;

    slot.set_active(true);

    while !stop.load(Ordering::Relaxed) {
        let n = match file.read(&mut buf) {
            Ok(0) => {
                debug!("BT A2DP SBC reader: EOF");
                break;
            }
            Ok(n) => n,
            Err(e) => {
                if e.kind() == std::io::ErrorKind::Interrupted {
                    continue;
                }
                warn!("BT A2DP SBC reader: read error: {}", e);
                break;
            }
        };

        let packet = &buf[..n];

        let (payload_offset, frame_count) = match parse_rtp_sbc(packet) {
            Some(v) => v,
            None => continue,
        };

        total_packets += 1;
        let mut pos = payload_offset;

        for _ in 0..frame_count {
            if pos >= n {
                break;
            }

            match decoder.decode(&packet[pos..], &mut pcm_buf) {
                Ok((consumed, samples)) => {
                    pos += consumed;
                    // Upscale i16 → i32 (shift left 16 bits for 32-bit pipeline)
                    let s32: Vec<i32> = pcm_buf[..samples]
                        .iter()
                        .map(|&s| (s as i32) << 16)
                        .collect();
                    resample_servo_push(
                        &mut resampler,
                        &mut servo,
                        &slot,
                        &s32,
                        &mut resampled,
                        "SBC",
                    );
                }
                Err(e) => {
                    debug!("BT A2DP SBC: decode error: {}", e);
                    break;
                }
            }
        }

        // Log stream health periodically (every 5000 packets ≈ every ~100 seconds)
        if total_packets.is_multiple_of(5000) {
            info!(
                "BT A2DP SBC: packets={} latency={}ms (target {}) skipped={} slot_avail={}",
                total_packets,
                servo.latency_ms(),
                latency_target_ms,
                servo.skipped_chunks,
                slot.available()
            );
        }
    }

    slot.set_active(false);
    slot.clear();
    debug!("BT A2DP SBC reader: stopped");
}

/// aptX / aptX HD reader loop.
fn reader_loop_aptx(
    fd: OwnedFd,
    read_mtu: u16,
    slot: Arc<MixerSlot>,
    stop: Arc<AtomicBool>,
    hd: bool,
    latency_target_ms: u32,
) {
    let mut file = unsafe { std::fs::File::from_raw_fd(fd.into_raw_fd()) };
    let mut buf = vec![0u8; read_mtu as usize];

    let mut decoder = match AptxDecoder::new(hd) {
        Ok(d) => d,
        Err(e) => {
            warn!("BT A2DP aptX: decoder init failed: {}", e);
            return;
        }
    };

    let codec_name = if hd { "aptX HD" } else { "aptX" };
    let mut pcm_buf = vec![0i32; APTX_MAX_SAMPLES];
    let mut total_packets: u64 = 0;
    let mut total_samples: u64 = 0;
    let mut zero_sample_packets: u64 = 0;
    let mut has_rtp: Option<bool> = None;
    let mut resampler = AdaptiveResampler::new(BT_SOURCE_RATE, MIXER_RATE);
    let mut servo = LatencyServo::new(latency_target_ms);
    let mut resampled = Vec::with_capacity(APTX_MAX_SAMPLES * 2);

    slot.set_active(true);

    while !stop.load(Ordering::Relaxed) {
        let n = match file.read(&mut buf) {
            Ok(0) => {
                debug!("BT A2DP {} reader: EOF", codec_name);
                break;
            }
            Ok(n) => n,
            Err(e) => {
                if e.kind() == std::io::ErrorKind::Interrupted {
                    continue;
                }
                warn!("BT A2DP {} reader: read error: {}", codec_name, e);
                break;
            }
        };

        let packet = &buf[..n];

        // Auto-detect RTP presence on first packet
        if total_packets == 0 {
            debug!(
                "BT A2DP {}: first packet {} bytes, header: {:02x?}",
                codec_name,
                n,
                &packet[..n.min(16)]
            );
        }

        let payload_offset = parse_rtp_aptx(packet, &mut has_rtp);

        if total_packets == 0 {
            debug!(
                "BT A2DP {}: RTP {}detected, offset={}",
                codec_name,
                if has_rtp == Some(true) { "" } else { "NOT " },
                payload_offset
            );
        }

        let payload = &packet[payload_offset..];

        total_packets += 1;
        match decoder.decode(payload, &mut pcm_buf) {
            Ok((consumed, samples)) => {
                if samples > 0 {
                    if total_samples == 0 {
                        info!(
                            "BT A2DP {}: first decode output: {} samples from {} bytes consumed (pkt #{})",
                            codec_name, samples, consumed, total_packets
                        );
                    }
                    total_samples += samples as u64;
                    resample_servo_push(
                        &mut resampler,
                        &mut servo,
                        &slot,
                        &pcm_buf[..samples],
                        &mut resampled,
                        codec_name,
                    );
                } else {
                    zero_sample_packets += 1;
                    if zero_sample_packets <= 3 {
                        info!(
                            "BT A2DP {}: decode returned 0 samples (consumed={}, pkt #{})",
                            codec_name, consumed, total_packets
                        );
                    }
                }
                // Log decode stats periodically (every 5000 packets ≈ every ~100 seconds)
                if total_packets.is_multiple_of(5000) {
                    info!(
                        "BT A2DP {}: packets={} samples={} zero_decode={} latency={}ms (target {}) skipped={} slot_avail={}",
                        codec_name,
                        total_packets,
                        total_samples,
                        zero_sample_packets,
                        servo.latency_ms(),
                        latency_target_ms,
                        servo.skipped_chunks,
                        slot.available()
                    );
                }
            }
            Err(e) => {
                warn!("BT A2DP {}: decode error: {}", codec_name, e);
            }
        }
    }

    slot.set_active(false);
    slot.clear();
    debug!("BT A2DP {} reader: stopped", codec_name);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The config value 0 means "unset" and resolves to the default; explicit
    /// values are honored but clamped into the safe range.
    #[test]
    fn latency_target_resolution_and_clamping() {
        assert_eq!(
            resolve_latency_target_ms(0),
            DEFAULT_LATENCY_TARGET_MS as u32
        );
        assert_eq!(resolve_latency_target_ms(150), 150);
        assert_eq!(resolve_latency_target_ms(5), 40); // too low to ride A2DP bursts
        assert_eq!(resolve_latency_target_ms(2000), 400); // lip-sync ceiling
    }

    /// The running flag flips false on drop so the AVDTP loop learns a reader
    /// died (and must respawn) rather than silently resuming a dead stream.
    #[test]
    fn running_guard_clears_flag_on_drop() {
        let flag = Arc::new(AtomicBool::new(true));
        {
            let _g = RunningGuard(flag.clone());
            assert!(flag.load(Ordering::Acquire));
        }
        assert!(!flag.load(Ordering::Acquire));
    }

    /// RTP byte 0 with version field = 2 (top two bits), padding/extension/CSRC clear.
    const RTP_V2_BYTE0: u8 = 0x80; // 0b1000_0000 -> (>>6 & 0x03) == 2

    /// Build a minimal SBC packet: 12-byte RTP header + 1-byte A2DP media header.
    /// `media_header` carries the frame-count nibble in bits 7..4.
    fn sbc_packet(version_byte0: u8, media_header: u8) -> Vec<u8> {
        let mut p = vec![0u8; RTP_HEADER_SIZE + A2DP_MEDIA_HEADER_SIZE];
        p[0] = version_byte0;
        p[RTP_HEADER_SIZE] = media_header;
        p
    }

    #[test]
    fn parse_rtp_sbc_wellformed_returns_offset_and_frame_count() {
        // media header 0x50 -> frame count nibble = 5
        let pkt = sbc_packet(RTP_V2_BYTE0, 0x50);
        let parsed = parse_rtp_sbc(&pkt);
        assert_eq!(parsed, Some((RTP_HEADER_SIZE + A2DP_MEDIA_HEADER_SIZE, 5)));
        // Payload offset is exactly 13 (12 RTP + 1 media header).
        assert_eq!(parsed.unwrap().0, 13);
    }

    #[test]
    fn parse_rtp_sbc_frame_count_nibble_is_high_bits_only() {
        // 0xF3: high nibble = 0xF (15), low nibble must be ignored.
        let pkt = sbc_packet(RTP_V2_BYTE0, 0xF3);
        assert_eq!(parse_rtp_sbc(&pkt), Some((13, 15)));

        // 0x00 -> zero frames.
        let pkt0 = sbc_packet(RTP_V2_BYTE0, 0x00);
        assert_eq!(parse_rtp_sbc(&pkt0), Some((13, 0)));

        // 0x0A: high nibble = 0 (low nibble 0xA ignored).
        let pkt_low = sbc_packet(RTP_V2_BYTE0, 0x0A);
        assert_eq!(parse_rtp_sbc(&pkt_low), Some((13, 0)));
    }

    #[test]
    fn parse_rtp_sbc_too_short_returns_none() {
        // One byte short of the 13-byte minimum: must be None, never panic.
        let short = vec![RTP_V2_BYTE0; MIN_SBC_PACKET_SIZE - 1];
        assert_eq!(parse_rtp_sbc(&short), None);

        // Empty buffer also yields None (and no index panic).
        assert_eq!(parse_rtp_sbc(&[]), None);
    }

    #[test]
    fn parse_rtp_sbc_exact_minimum_length_is_accepted() {
        let pkt = sbc_packet(RTP_V2_BYTE0, 0x10);
        assert_eq!(pkt.len(), MIN_SBC_PACKET_SIZE);
        assert_eq!(parse_rtp_sbc(&pkt), Some((13, 1)));
    }

    #[test]
    fn parse_rtp_sbc_wrong_version_is_rejected() {
        // Version 0 (byte0 = 0x00): rejected even though length is sufficient.
        let v0 = sbc_packet(0x00, 0x20);
        assert_eq!(parse_rtp_sbc(&v0), None);

        // Version 1 (top two bits = 01 -> 0x40): rejected.
        let v1 = sbc_packet(0x40, 0x20);
        assert_eq!(parse_rtp_sbc(&v1), None);

        // Version 3 (top two bits = 11 -> 0xC0): rejected.
        let v3 = sbc_packet(0xC0, 0x20);
        assert_eq!(parse_rtp_sbc(&v3), None);
    }

    #[test]
    fn parse_rtp_aptx_detects_rtp_v2_and_caches() {
        let mut has_rtp: Option<bool> = None;
        let mut pkt = vec![0u8; RTP_HEADER_SIZE + 4];
        pkt[0] = RTP_V2_BYTE0;

        let offset = parse_rtp_aptx(&pkt, &mut has_rtp);
        assert_eq!(offset, RTP_HEADER_SIZE);
        assert_eq!(has_rtp, Some(true));

        // Cached path: even a buffer that doesn't look like RTP returns the cached offset.
        let raw = vec![0x00u8; 2];
        assert_eq!(parse_rtp_aptx(&raw, &mut has_rtp), RTP_HEADER_SIZE);
        assert_eq!(has_rtp, Some(true));
    }

    #[test]
    fn parse_rtp_aptx_no_rtp_when_not_v2() {
        let mut has_rtp: Option<bool> = None;
        // Long enough to inspect, but version bits are not 2 (byte0 = 0x00 -> v0).
        let pkt = vec![0x00u8; RTP_HEADER_SIZE + 4];

        let offset = parse_rtp_aptx(&pkt, &mut has_rtp);
        assert_eq!(offset, 0);
        assert_eq!(has_rtp, Some(false));

        // Cached as raw: a later RTP-looking packet still returns offset 0.
        let mut rtp_like = vec![0u8; RTP_HEADER_SIZE];
        rtp_like[0] = RTP_V2_BYTE0;
        assert_eq!(parse_rtp_aptx(&rtp_like, &mut has_rtp), 0);
        assert_eq!(has_rtp, Some(false));
    }

    #[test]
    fn parse_rtp_aptx_short_buffer_treated_as_raw() {
        let mut has_rtp: Option<bool> = None;
        // Shorter than the RTP header: can't be RTP, so treated as raw codewords.
        let short = vec![RTP_V2_BYTE0; RTP_HEADER_SIZE - 1];
        let offset = parse_rtp_aptx(&short, &mut has_rtp);
        assert_eq!(offset, 0);
        assert_eq!(has_rtp, Some(false));
    }

    #[test]
    fn parse_rtp_aptx_cached_true_returns_header_size() {
        // Pre-seeded cache short-circuits before any buffer inspection.
        let mut has_rtp: Option<bool> = Some(true);
        assert_eq!(parse_rtp_aptx(&[], &mut has_rtp), RTP_HEADER_SIZE);
    }

    #[test]
    fn parse_rtp_aptx_cached_false_returns_zero() {
        let mut has_rtp: Option<bool> = Some(false);
        let mut pkt = vec![0u8; RTP_HEADER_SIZE];
        pkt[0] = RTP_V2_BYTE0;
        assert_eq!(parse_rtp_aptx(&pkt, &mut has_rtp), 0);
    }
}
