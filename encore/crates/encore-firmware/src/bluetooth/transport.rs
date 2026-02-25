//! A2DP transport reader — RTP packet parsing and multi-codec decode pipeline.
//!
//! When a phone connects via A2DP, the AVDTP state machine accepts a media
//! transport L2CAP socket. This module reads RTP packets from that socket,
//! strips headers, decodes audio (SBC, aptX, or aptX HD), and pushes PCM
//! into a MixerSlot.

use crate::audio::mixer::MixerSlot;
use crate::bluetooth::aptx::{AptxDecoder, APTX_MAX_SAMPLES};
use crate::bluetooth::sbc::{SbcDecoder, SBC_MAX_SAMPLES};
use crate::bluetooth::A2dpCodec;
use anyhow::{Context, Result};
use std::io::Read;
use std::os::fd::{FromRawFd, IntoRawFd, OwnedFd};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tracing::{debug, info, warn};

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
/// aptX packets have RTP header (12 bytes) but NO A2DP media header —
/// payload immediately follows the RTP header.
///
/// Returns payload offset or None if invalid.
fn parse_rtp_aptx(buf: &[u8]) -> Option<usize> {
    if buf.len() < RTP_HEADER_SIZE {
        return None;
    }

    let version = (buf[0] >> 6) & 0x03;
    if version != 2 {
        return None;
    }

    Some(RTP_HEADER_SIZE)
}

/// Spawn a dedicated reader thread for the A2DP transport FD.
///
/// Reads RTP packets, decodes audio frames, pushes PCM into the MixerSlot.
/// Returns an `Arc<AtomicBool>` stop handle — set to `true` to stop the thread.
pub fn spawn_reader(
    fd: OwnedFd,
    read_mtu: u16,
    slot: Arc<MixerSlot>,
    codec: A2dpCodec,
) -> Result<Arc<AtomicBool>> {
    let stop = Arc::new(AtomicBool::new(false));
    let stop_clone = stop.clone();

    std::thread::Builder::new()
        .name("bt-a2dp-reader".into())
        .spawn(move || {
            info!("BT A2DP reader: started (codec={}, mtu={})", codec, read_mtu);
            match codec {
                A2dpCodec::Sbc => reader_loop_sbc(fd, read_mtu, slot, stop_clone),
                A2dpCodec::Aptx => reader_loop_aptx(fd, read_mtu, slot, stop_clone, false),
                A2dpCodec::AptxHd => reader_loop_aptx(fd, read_mtu, slot, stop_clone, true),
            }
        })
        .context("failed to spawn BT A2DP reader thread")?;

    Ok(stop)
}

/// SBC reader loop — original decode path.
fn reader_loop_sbc(
    fd: OwnedFd,
    read_mtu: u16,
    slot: Arc<MixerSlot>,
    stop: Arc<AtomicBool>,
) {
    let mut file = unsafe { std::fs::File::from_raw_fd(fd.into_raw_fd()) };
    let mut buf = vec![0u8; read_mtu as usize];
    let mut decoder = SbcDecoder::new();
    let mut pcm_buf = vec![0i16; SBC_MAX_SAMPLES * 2]; // stereo interleaved

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

        let mut pos = payload_offset;

        for _ in 0..frame_count {
            if pos >= n {
                break;
            }

            match decoder.decode(&packet[pos..], &mut pcm_buf) {
                Ok((consumed, samples)) => {
                    pos += consumed;
                    // Upscale i16 → i32 (shift left 16 bits for 32-bit pipeline)
                    let s32: Vec<i32> = pcm_buf[..samples].iter().map(|&s| (s as i32) << 16).collect();
                    let written = slot.push(&s32);
                    if written < samples {
                        debug!("BT A2DP SBC: ring full, dropped {} samples", samples - written);
                    }
                }
                Err(e) => {
                    debug!("BT A2DP SBC: decode error: {}", e);
                    break;
                }
            }
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

        let payload_offset = match parse_rtp_aptx(packet) {
            Some(v) => v,
            None => continue,
        };

        let payload = &packet[payload_offset..];

        match decoder.decode(payload, &mut pcm_buf) {
            Ok((_consumed, samples)) => {
                if samples > 0 {
                    // aptx.rs already outputs i32 in full 32-bit range
                    let written = slot.push(&pcm_buf[..samples]);
                    if written < samples {
                        debug!("BT A2DP {}: ring full, dropped {} samples", codec_name, samples - written);
                    }
                }
            }
            Err(e) => {
                debug!("BT A2DP {}: decode error: {}", codec_name, e);
            }
        }
    }

    slot.set_active(false);
    slot.clear();
    debug!("BT A2DP {} reader: stopped", codec_name);
}
