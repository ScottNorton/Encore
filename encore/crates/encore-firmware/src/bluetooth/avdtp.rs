//! AVDTP signaling state machine and media transport.
//!
//! Listens on L2CAP PSM 25 for A2DP connections. The first connection on PSM 25
//! is the signaling channel; the second (in Open state) is the media transport.
//! The phone drives the negotiation: DISCOVER → GET_CAPS → SET_CONFIG → OPEN → START.

use super::l2cap;
use super::transport;
use super::transport::ReaderHandle;
use super::A2dpCodec;
use crate::audio::mixer::MixerSlot;
use std::os::fd::{AsRawFd, OwnedFd, RawFd};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

// ── AVDTP signal IDs ──
const AVDTP_DISCOVER: u8 = 0x01;
const AVDTP_GET_CAPABILITIES: u8 = 0x02;
const AVDTP_SET_CONFIGURATION: u8 = 0x03;
const AVDTP_GET_CONFIGURATION: u8 = 0x04;
const AVDTP_RECONFIGURE: u8 = 0x05;
const AVDTP_OPEN: u8 = 0x06;
const AVDTP_START: u8 = 0x07;
const AVDTP_CLOSE: u8 = 0x08;
const AVDTP_SUSPEND: u8 = 0x09;
const AVDTP_ABORT: u8 = 0x0A;
const AVDTP_GET_ALL_CAPABILITIES: u8 = 0x0C;

// Message type bits (in byte 0, bits 1-0)
const MSG_TYPE_COMMAND: u8 = 0x00;
const MSG_TYPE_ACCEPT: u8 = 0x02;
const MSG_TYPE_REJECT: u8 = 0x03;

// AVDTP capability categories
const CAP_MEDIA_TRANSPORT: u8 = 0x01;
const CAP_MEDIA_CODEC: u8 = 0x07;

// Codec types
const CODEC_SBC: u8 = 0x00;
const CODEC_VENDOR: u8 = 0xFF;

// ── SEP endpoint definitions ──

/// SBC capabilities: all freqs, all modes, bitpool 2-53.
const SBC_CAPS: [u8; 4] = [0xFF, 0xFF, 2, 53];
/// aptX HD: Qualcomm vendor 0x00D7, codec 0x0024, 44.1/48kHz stereo.
const APTX_HD_CAPS: [u8; 7] = [0xD7, 0x00, 0x00, 0x00, 0x24, 0x00, 0x32];
/// aptX: Qualcomm vendor 0x004F, codec 0x0001, 44.1/48kHz stereo.
const APTX_CAPS: [u8; 7] = [0x4F, 0x00, 0x00, 0x00, 0x01, 0x00, 0x32];

/// AVDTP state machine states.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AvdtpState {
    /// Listening for signaling connection.
    Idle,
    /// Signaling channel connected, awaiting DISCOVER.
    Connected,
    /// Codec configuration accepted.
    Configured,
    /// OPEN accepted, awaiting media transport + START.
    Open,
    /// Audio streaming (reader thread active).
    Streaming,
    /// Paused by phone (may resume with START).
    Suspended,
}

/// Events sent from the AVDTP task to the main BT event loop.
///
/// The AVDTP loop owns the reader thread's lifecycle; these events only ask the
/// main loop to update mixer-slot/group/UI state, never to stop the reader.
#[derive(Debug)]
pub enum AvdtpEvent {
    /// First START after OPEN — a reader thread is now producing audio.
    Streaming { codec: A2dpCodec },
    /// SUSPEND — source paused. The reader stays alive for an instant resume;
    /// the main loop just silences the slot.
    Paused,
    /// START after SUSPEND — audio resumes on the same media transport.
    Resumed { codec: A2dpCodec },
    /// CLOSE/ABORT — the stream tore down and the reader was stopped.
    Stopped,
    /// Signaling disconnected (phone went away).
    Disconnected,
}

/// Spawn the AVDTP accept loop as a background task.
///
/// The listener socket (PSM 25) must already be bound and listening.
/// Returns a channel receiver for AVDTP events.
pub fn spawn_avdtp(listener: OwnedFd, slot: Arc<MixerSlot>) -> mpsc::Receiver<AvdtpEvent> {
    let (tx, rx) = mpsc::channel(8);

    std::thread::Builder::new()
        .name("bt-avdtp".into())
        .spawn(move || {
            info!("AVDTP: listening on PSM 25");
            avdtp_loop(listener, slot, tx);
            info!("AVDTP: accept loop exited");
        })
        .expect("failed to spawn AVDTP thread");

    rx
}

/// Main AVDTP accept loop. Runs in a dedicated thread.
///
/// Accepts signaling connections, processes AVDTP commands, and manages
/// the media transport for audio streaming.
// Session-state locals are defensively initialized at the loop top and reassigned per
// session; configured_seid is negotiated but not yet applied to stream setup (PoC).
// The 'session loop drives a connection state machine, not a simple value stream,
// so the while_let_loop suggestion does not apply.
#[allow(unused_assignments, unused_variables, clippy::while_let_loop)]
fn avdtp_loop(listener: OwnedFd, slot: Arc<MixerSlot>, tx: mpsc::Sender<AvdtpEvent>) {
    let listener_fd = listener.as_raw_fd();

    loop {
        // Reset state for each new session
        let mut state = AvdtpState::Idle;
        let mut sig_fd: Option<OwnedFd> = None;
        let mut media_fd: Option<OwnedFd> = None;
        let mut codec: Option<A2dpCodec> = None;
        let mut configured_seid: u8 = 0;
        let mut reader: Option<ReaderHandle> = None;

        // Wait for a signaling connection
        let (accepted_fd, remote_addr) = match l2cap::l2cap_accept(listener_fd) {
            Ok(v) => v,
            Err(e) => {
                if e.kind() == std::io::ErrorKind::Interrupted {
                    continue;
                }
                warn!("AVDTP: accept error: {}", e);
                std::thread::sleep(std::time::Duration::from_millis(100));
                continue;
            }
        };

        info!(
            "AVDTP: signaling connection from {}",
            l2cap::bdaddr_to_string(&remote_addr)
        );
        sig_fd = Some(accepted_fd);
        state = AvdtpState::Connected;

        // Process signaling messages until disconnect
        let mut sig_buf = [0u8; 256];
        'session: loop {
            let sig = match sig_fd.as_ref() {
                Some(fd) => fd,
                None => break,
            };

            let n = match l2cap::raw_read(sig.as_raw_fd(), &mut sig_buf) {
                Ok(0) => {
                    info!("AVDTP: signaling EOF");
                    break 'session;
                }
                Ok(n) => n,
                Err(e) => {
                    if e.kind() == std::io::ErrorKind::Interrupted {
                        continue;
                    }
                    warn!("AVDTP: signaling read error: {} (kind={:?})", e, e.kind());
                    break 'session;
                }
            };

            if n < 2 {
                continue;
            }

            let header = sig_buf[0];
            let txn_label = (header >> 4) & 0x0F;
            let packet_type = (header >> 2) & 0x03;
            let msg_type = header & 0x03;

            debug!(
                "AVDTP: rx {} bytes: header=0x{:02x} txn={} ptype={} mtype={} raw={:02x?}",
                n,
                header,
                txn_label,
                packet_type,
                msg_type,
                &sig_buf[..n.min(16)]
            );

            if msg_type != MSG_TYPE_COMMAND {
                // We only process commands from the phone
                continue;
            }

            let signal_id = sig_buf[1] & 0x3F;
            let payload = &sig_buf[2..n];

            let response = match signal_id {
                AVDTP_DISCOVER => {
                    debug!("AVDTP: DISCOVER");
                    handle_discover(txn_label)
                }
                AVDTP_GET_CAPABILITIES | AVDTP_GET_ALL_CAPABILITIES | AVDTP_GET_CONFIGURATION => {
                    let seid = if !payload.is_empty() {
                        (payload[0] >> 2) & 0x3F
                    } else {
                        0
                    };
                    debug!("AVDTP: GET_CAPABILITIES seid={}", seid);
                    handle_get_capabilities(txn_label, signal_id, seid)
                }
                AVDTP_SET_CONFIGURATION => {
                    debug!("AVDTP: SET_CONFIGURATION");
                    let (resp, detected_codec, seid) = handle_set_configuration(txn_label, payload);
                    if detected_codec.is_some() {
                        codec = detected_codec;
                        configured_seid = seid;
                        state = AvdtpState::Configured;
                        info!("AVDTP: configured seid={} codec={}", seid, codec.unwrap());
                    }
                    resp
                }
                AVDTP_OPEN => {
                    debug!("AVDTP: OPEN");
                    state = AvdtpState::Open;
                    build_accept(txn_label, AVDTP_OPEN, &[])
                }
                AVDTP_START => {
                    debug!("AVDTP: START");
                    // START after SUSPEND resumes streaming on the SAME media
                    // transport — the source does not reopen it. If the reader
                    // is still alive, just resume in place; never accept a new
                    // transport (the old bug blocked here forever).
                    let resuming = reader.as_ref().map_or(false, |h| h.is_running());
                    if resuming {
                        if let Some(c) = codec {
                            state = AvdtpState::Streaming;
                            let _ = tx.blocking_send(AvdtpEvent::Resumed { codec: c });
                            info!("AVDTP: streaming resumed (codec={})", c);
                        }
                    } else {
                        // First START after OPEN, or the previous reader died:
                        // (re)acquire the media transport and spawn a reader.
                        if let Some(h) = reader.take() {
                            h.stop.store(true, Ordering::Relaxed);
                        }
                        if media_fd.is_none() {
                            media_fd = accept_media_bounded(listener_fd, 2000, &remote_addr);
                        }
                        if let (Some(mfd), Some(c)) = (media_fd.take(), codec) {
                            let read_mtu = l2cap::l2cap_get_options(mfd.as_raw_fd())
                                .map(|opts| opts.imtu)
                                .unwrap_or(672);
                            match transport::spawn_reader(mfd, read_mtu, slot.clone(), c) {
                                Ok(h) => {
                                    reader = Some(h);
                                    state = AvdtpState::Streaming;
                                    let _ = tx.blocking_send(AvdtpEvent::Streaming { codec: c });
                                    info!(
                                        "AVDTP: streaming started (codec={}, mtu={})",
                                        c, read_mtu
                                    );
                                }
                                Err(e) => {
                                    warn!("AVDTP: reader spawn failed: {}", e);
                                }
                            }
                        } else {
                            warn!("AVDTP: START but no media transport or codec");
                        }
                    }

                    build_accept(txn_label, AVDTP_START, &[])
                }
                AVDTP_SUSPEND => {
                    debug!("AVDTP: SUSPEND");
                    // Keep the reader and media transport alive — A2DP resumes
                    // on the same channel via START. Just mark paused (silent).
                    state = AvdtpState::Suspended;
                    let _ = tx.blocking_send(AvdtpEvent::Paused);
                    build_accept(txn_label, AVDTP_SUSPEND, &[])
                }
                AVDTP_CLOSE => {
                    debug!("AVDTP: CLOSE");
                    if let Some(h) = reader.take() {
                        h.stop.store(true, Ordering::Relaxed);
                    }
                    media_fd = None;
                    codec = None;
                    state = AvdtpState::Connected;
                    let _ = tx.blocking_send(AvdtpEvent::Stopped);
                    build_accept(txn_label, AVDTP_CLOSE, &[])
                }
                AVDTP_ABORT => {
                    debug!("AVDTP: ABORT");
                    if let Some(h) = reader.take() {
                        h.stop.store(true, Ordering::Relaxed);
                    }
                    media_fd = None;
                    codec = None;
                    state = AvdtpState::Connected;
                    let _ = tx.blocking_send(AvdtpEvent::Stopped);
                    build_accept(txn_label, AVDTP_ABORT, &[])
                }
                AVDTP_RECONFIGURE => {
                    debug!("AVDTP: RECONFIGURE");
                    if let Some(h) = reader.take() {
                        h.stop.store(true, Ordering::Relaxed);
                    }
                    let _ = tx.blocking_send(AvdtpEvent::Stopped);

                    let (resp, detected_codec, seid) = handle_set_configuration(txn_label, payload);
                    if detected_codec.is_some() {
                        codec = detected_codec;
                        configured_seid = seid;
                        state = AvdtpState::Open; // After reconfig, wait for START
                        info!("AVDTP: reconfigured seid={} codec={}", seid, codec.unwrap());
                    }
                    resp
                }
                _ => {
                    debug!("AVDTP: unknown signal_id=0x{:02x}", signal_id);
                    build_reject(txn_label, signal_id, 0x01) // BAD_HEADER_FORMAT
                }
            };

            // Send response
            if let Some(ref sig) = sig_fd {
                if l2cap::raw_write(sig.as_raw_fd(), &response).is_err() {
                    warn!("AVDTP: signaling write failed");
                    break 'session;
                }
            }

            // In Open state, try to accept media transport (non-blocking)
            // The phone opens a second L2CAP connection on PSM 25 after OPEN
            if state == AvdtpState::Open && media_fd.is_none() {
                // Set listener to non-blocking for a quick check
                l2cap::set_nonblocking(listener_fd).ok();
                if let Ok((fd, addr)) = l2cap::l2cap_accept(listener_fd) {
                    if addr == remote_addr {
                        media_fd = Some(fd);
                        debug!("AVDTP: accepted media transport (post-OPEN)");
                    } else {
                        // A different paired device opened PSM 25 mid-session — not
                        // our source's media channel. Drop it (fd closes on scope
                        // exit) rather than streaming the wrong device's audio.
                        warn!(
                            "AVDTP: rejected media transport from {} (peer is {})",
                            l2cap::bdaddr_to_string(&addr),
                            l2cap::bdaddr_to_string(&remote_addr)
                        );
                    }
                }
                // Restore blocking mode
                restore_blocking(listener_fd);
            }
        }

        // Session ended — clean up
        if let Some(h) = reader.take() {
            h.stop.store(true, Ordering::Relaxed);
        }
        let _ = tx.blocking_send(AvdtpEvent::Disconnected);

        // Drop the signaling and media fds
        drop(sig_fd);
        drop(media_fd);

        // Ensure listener is back to blocking mode
        restore_blocking(listener_fd);

        info!("AVDTP: session ended, waiting for new connection");
    }
}

/// Restore a fd to blocking mode.
fn restore_blocking(fd: std::os::fd::RawFd) {
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags >= 0 {
        unsafe { libc::fcntl(fd, libc::F_SETFL, flags & !libc::O_NONBLOCK) };
    }
}

/// Accept a media-transport connection without parking the signaling thread
/// forever. Polls the (non-blocking) listener for up to `max_ms`, then gives
/// up. START must never block here: on a SUSPEND→START resume the source
/// reuses the existing transport and opens nothing, so a blocking accept would
/// wedge the whole signaling channel (the original "no audio after pause" bug).
fn accept_media_bounded(listener_fd: RawFd, max_ms: u64, expected: &[u8; 6]) -> Option<OwnedFd> {
    l2cap::set_nonblocking(listener_fd).ok();
    let step = 20u64;
    let mut waited = 0u64;
    let result = loop {
        match l2cap::l2cap_accept(listener_fd) {
            Ok((fd, addr)) if addr == *expected => {
                debug!("AVDTP: accepted media transport");
                break Some(fd);
            }
            // A different paired device's connection — drop it (fd closes here)
            // and keep waiting for our session peer's media channel.
            Ok((_fd, addr)) => {
                warn!(
                    "AVDTP: ignored media transport from {} (waiting for {})",
                    l2cap::bdaddr_to_string(&addr),
                    l2cap::bdaddr_to_string(expected)
                );
                if waited >= max_ms {
                    break None;
                }
                std::thread::sleep(std::time::Duration::from_millis(step));
                waited += step;
            }
            Err(_) if waited < max_ms => {
                std::thread::sleep(std::time::Duration::from_millis(step));
                waited += step;
            }
            Err(_) => break None,
        }
    };
    restore_blocking(listener_fd);
    result
}

// ── AVDTP message builders ──

/// Build an accept response with the given signal_id and optional payload.
fn build_accept(txn_label: u8, signal_id: u8, payload: &[u8]) -> Vec<u8> {
    let mut msg = Vec::with_capacity(2 + payload.len());
    msg.push((txn_label << 4) | MSG_TYPE_ACCEPT); // single packet, accept
    msg.push(signal_id & 0x3F); // signal_id in bits 5:0, RFA=0 in bits 7:6
    msg.extend_from_slice(payload);
    msg
}

/// Build a reject response.
fn build_reject(txn_label: u8, signal_id: u8, error: u8) -> Vec<u8> {
    vec![
        (txn_label << 4) | MSG_TYPE_REJECT,
        signal_id & 0x3F, // signal_id in bits 5:0, RFA=0 in bits 7:6
        error,
    ]
}

/// Handle DISCOVER: return SEP table (3 endpoints).
fn handle_discover(txn_label: u8) -> Vec<u8> {
    // Each SEP is 2 bytes:
    // byte0: [seid:6][in_use:1][rfa:1]
    // byte1: [media_type:4][tsep:1][rfa:3]
    //   media_type=0x00 (audio), tsep=1 (sink)
    let sep_table: [u8; 6] = [
        (1 << 2),
        (1 << 3), // SEID 1: aptX HD
        (2 << 2),
        (1 << 3), // SEID 2: aptX
        (3 << 2),
        (1 << 3), // SEID 3: SBC
    ];
    build_accept(txn_label, AVDTP_DISCOVER, &sep_table)
}

/// Handle GET_CAPABILITIES: return capabilities for the requested SEID.
fn handle_get_capabilities(txn_label: u8, signal_id: u8, seid: u8) -> Vec<u8> {
    let caps = match seid {
        1 => build_codec_caps(CODEC_VENDOR, &APTX_HD_CAPS),
        2 => build_codec_caps(CODEC_VENDOR, &APTX_CAPS),
        3 => build_codec_caps(CODEC_SBC, &SBC_CAPS),
        _ => return build_reject(txn_label, signal_id, 0x12), // BAD_ACP_SEID
    };
    build_accept(txn_label, signal_id, &caps)
}

/// Build capability bytes: MEDIA_TRANSPORT + MEDIA_CODEC.
fn build_codec_caps(codec_type: u8, codec_caps: &[u8]) -> Vec<u8> {
    let mut caps = Vec::with_capacity(4 + 2 + codec_caps.len());
    // MEDIA_TRANSPORT (category 0x01, length 0)
    caps.push(CAP_MEDIA_TRANSPORT);
    caps.push(0x00);
    // MEDIA_CODEC (category 0x07)
    // LOSC = 2 (media_type + codec_type) + codec_caps.len()
    let losc = 2 + codec_caps.len();
    caps.push(CAP_MEDIA_CODEC);
    caps.push(losc as u8);
    caps.push(0x00); // media_type: audio (upper 4 bits = 0)
    caps.push(codec_type);
    caps.extend_from_slice(codec_caps);
    caps
}

/// Handle SET_CONFIGURATION: parse codec, return accept.
/// Returns (response_bytes, detected_codec, acp_seid).
fn handle_set_configuration(txn_label: u8, payload: &[u8]) -> (Vec<u8>, Option<A2dpCodec>, u8) {
    if payload.len() < 2 {
        return (
            build_reject(txn_label, AVDTP_SET_CONFIGURATION, 0x01),
            None,
            0,
        );
    }

    let acp_seid = (payload[0] >> 2) & 0x3F;
    // INT SEID at payload[1], service capabilities at payload[2..]

    // Determine codec from SEID (fast path) or by parsing capabilities
    let codec = match acp_seid {
        1 => Some(A2dpCodec::AptxHd),
        2 => Some(A2dpCodec::Aptx),
        3 => Some(A2dpCodec::Sbc),
        _ => parse_codec_from_caps(&payload[2..]),
    };

    if codec.is_some() {
        (
            build_accept(txn_label, AVDTP_SET_CONFIGURATION, &[]),
            codec,
            acp_seid,
        )
    } else {
        (
            build_reject(txn_label, AVDTP_SET_CONFIGURATION, 0x18), // BAD_MEDIA_TRANSPORT_FORMAT
            None,
            0,
        )
    }
}

/// Parse MEDIA_CODEC capability from SET_CONFIGURATION payload to identify codec.
fn parse_codec_from_caps(caps: &[u8]) -> Option<A2dpCodec> {
    let mut pos = 0;
    while pos + 1 < caps.len() {
        let category = caps[pos];
        let losc = caps[pos + 1] as usize;
        if category == CAP_MEDIA_CODEC && losc >= 2 {
            let codec_type = caps[pos + 3]; // skip media_type at pos+2
            if codec_type == CODEC_SBC {
                return Some(A2dpCodec::Sbc);
            } else if codec_type == CODEC_VENDOR && losc >= 8 {
                // Check vendor_id and codec_id
                let vendor = u32::from_le_bytes([
                    caps[pos + 4],
                    caps[pos + 5],
                    caps[pos + 6],
                    caps[pos + 7],
                ]);
                let cid = if losc >= 10 {
                    u16::from_le_bytes([caps[pos + 8], caps[pos + 9]])
                } else {
                    0
                };
                if vendor == 0x000000D7 && cid == 0x0024 {
                    return Some(A2dpCodec::AptxHd);
                } else if vendor == 0x0000004F && cid == 0x0001 {
                    return Some(A2dpCodec::Aptx);
                }
            }
        }
        pos += 2 + losc;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discover_response_has_3_seps() {
        let resp = handle_discover(0x05);
        assert_eq!(resp[0], (0x05 << 4) | MSG_TYPE_ACCEPT);
        assert_eq!(resp[1] & 0x3F, AVDTP_DISCOVER);
        // 2 bytes header + 6 bytes (3 SEPs × 2 bytes)
        assert_eq!(resp.len(), 8);
    }

    #[test]
    fn get_caps_sbc() {
        let resp = handle_get_capabilities(0x01, AVDTP_GET_CAPABILITIES, 3);
        // Accept response for SBC
        assert_eq!(resp[0] & 0x03, MSG_TYPE_ACCEPT);
        // Should contain MEDIA_TRANSPORT + MEDIA_CODEC
        let payload = &resp[2..];
        assert_eq!(payload[0], CAP_MEDIA_TRANSPORT);
        assert_eq!(payload[1], 0x00); // LOSC = 0
        assert_eq!(payload[2], CAP_MEDIA_CODEC);
        assert_eq!(payload[4], 0x00); // media_type = audio
        assert_eq!(payload[5], CODEC_SBC);
    }

    #[test]
    fn get_caps_aptx_hd() {
        let resp = handle_get_capabilities(0x01, AVDTP_GET_CAPABILITIES, 1);
        let payload = &resp[2..];
        assert_eq!(payload[2], CAP_MEDIA_CODEC);
        assert_eq!(payload[5], CODEC_VENDOR);
        // Vendor ID should be Qualcomm 0x00D7
        assert_eq!(payload[6], 0xD7);
    }

    #[test]
    fn get_caps_bad_seid() {
        let resp = handle_get_capabilities(0x01, AVDTP_GET_CAPABILITIES, 99);
        assert_eq!(resp[0] & 0x03, MSG_TYPE_REJECT);
    }

    #[test]
    fn set_configuration_sbc() {
        // payload: ACP_SEID=3, INT_SEID=1, then capabilities
        let mut payload = vec![(3 << 2), (1 << 2)];
        payload.extend_from_slice(&[CAP_MEDIA_TRANSPORT, 0x00]);
        payload.extend_from_slice(&[CAP_MEDIA_CODEC, 0x06, 0x00, CODEC_SBC]);
        payload.extend_from_slice(&SBC_CAPS);

        let (resp, codec, seid) = handle_set_configuration(0x01, &payload);
        assert_eq!(resp[0] & 0x03, MSG_TYPE_ACCEPT);
        assert_eq!(codec, Some(A2dpCodec::Sbc));
        assert_eq!(seid, 3);
    }

    #[test]
    fn set_configuration_aptx() {
        let payload = vec![(2 << 2), (1 << 2)];
        let (_, codec, seid) = handle_set_configuration(0x01, &payload);
        assert_eq!(codec, Some(A2dpCodec::Aptx));
        assert_eq!(seid, 2);
    }

    #[test]
    fn parse_codec_from_caps_vendor() {
        let mut caps = vec![CAP_MEDIA_TRANSPORT, 0x00, CAP_MEDIA_CODEC, 0x0B];
        caps.push(0x00); // media_type
        caps.push(CODEC_VENDOR);
        // Vendor ID: 0x000000D7 (LE)
        caps.extend_from_slice(&[0xD7, 0x00, 0x00, 0x00]);
        // Codec ID: 0x0024 (LE)
        caps.extend_from_slice(&[0x24, 0x00]);
        // Config byte
        caps.push(0x12);

        assert_eq!(parse_codec_from_caps(&caps), Some(A2dpCodec::AptxHd));
    }

    #[test]
    fn accept_message_format() {
        let msg = build_accept(0x0A, AVDTP_OPEN, &[]);
        assert_eq!(msg.len(), 2);
        assert_eq!(msg[0], (0x0A << 4) | MSG_TYPE_ACCEPT);
        assert_eq!(msg[1] & 0x3F, AVDTP_OPEN);
    }
}
