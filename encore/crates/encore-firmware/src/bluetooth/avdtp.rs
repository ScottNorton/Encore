//! AVDTP signaling state machine and media transport — socket I/O layer.
//!
//! Listens on L2CAP PSM 25 for A2DP connections. The first connection on PSM 25
//! is the signaling channel; the second (in Open state) is the media transport.
//! The phone drives the negotiation: DISCOVER → GET_CAPS → SET_CONFIG → OPEN → START.
//!
//! The PDU encode/decode (`build_accept`, `handle_discover`,
//! `handle_set_configuration`, `parse_codec_from_caps`, ...) and `A2dpCodec`
//! are pure and live in `encore_common::avdtp`, re-exported here under the
//! same `avdtp::` path so callers in this crate don't need to change. They
//! were split out so those unit tests run on any host — this module is
//! `#[cfg(target_os = "linux")]`-gated end to end (raw sockets), which meant
//! they never ran on a non-Linux dev machine.

pub use encore_common::avdtp::*;

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
/// Returns the event sender (for [`spawn_initiator`] sessions, which emit the
/// same events) and the receiver for the main BT event loop.
pub fn spawn_avdtp(
    listener: OwnedFd,
    slot: Arc<MixerSlot>,
    latency_target_ms: u32,
) -> (mpsc::Sender<AvdtpEvent>, mpsc::Receiver<AvdtpEvent>) {
    let (tx, rx) = mpsc::channel(8);
    let loop_tx = tx.clone();

    std::thread::Builder::new()
        .name("bt-avdtp".into())
        .spawn(move || {
            info!("AVDTP: listening on PSM 25");
            avdtp_loop(listener, slot, loop_tx, latency_target_ms);
            info!("AVDTP: accept loop exited");
        })
        .expect("failed to spawn AVDTP thread");

    (tx, rx)
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
fn avdtp_loop(
    listener: OwnedFd,
    slot: Arc<MixerSlot>,
    tx: mpsc::Sender<AvdtpEvent>,
    latency_target_ms: u32,
) {
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
                    let resuming = reader.as_ref().is_some_and(|h| h.is_running());
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
                            match transport::spawn_reader(
                                mfd,
                                read_mtu,
                                slot.clone(),
                                c,
                                latency_target_ms,
                            ) {
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
// ── Sink-initiated sessions (INT role) ──
//
// A2DP sources with a UI (phones, PCs) normally connect to us. But a bare
// paged ACL is not enough to make Windows stream — the profile must be
// brought up by whoever initiated, which is why real headphones open AVDTP
// themselves after paging. This is that: the speaker connects PSM 25, drives
// DISCOVER → GET_CAPABILITIES → SET_CONFIGURATION → OPEN → (media connect) →
// START, then reuses the same reader/transport as accepted sessions.
//
// Kept deliberately separate from the acceptor loop: if the source later
// tears the stream down (CLOSE/ABORT), the initiated session ends entirely
// and the source reconnects through the normal acceptor path — no shared
// listener juggling between two state machines.

/// AVDTP error code used to refuse commands an initiated session can't serve.
const AVDTP_ERR_BAD_STATE: u8 = 0x31;

/// Wait up to `timeout_ms` for readability, then read one signaling PDU.
/// poll() before a blocking read — never SO_RCVTIMEO on these sockets (it
/// fragments L2CAP SDUs on this kernel; see the transport regression note).
fn poll_read(fd: RawFd, buf: &mut [u8], timeout_ms: i32) -> std::io::Result<usize> {
    let mut pfd = libc::pollfd {
        fd,
        events: libc::POLLIN,
        revents: 0,
    };
    let n = unsafe { libc::poll(&mut pfd, 1, timeout_ms) };
    if n < 0 {
        return Err(std::io::Error::last_os_error());
    }
    if n == 0 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::TimedOut,
            "poll timeout",
        ));
    }
    l2cap::raw_read(fd, buf)
}

/// Send one AVDTP command PDU (single packet).
fn send_command(fd: RawFd, txn: u8, signal: u8, payload: &[u8]) -> std::io::Result<()> {
    let mut msg = Vec::with_capacity(2 + payload.len());
    msg.push((txn << 4) | MSG_TYPE_COMMAND);
    msg.push(signal & 0x3F);
    msg.extend_from_slice(payload);
    l2cap::raw_write(fd, &msg).map(|_| ())
}

/// Send `signal` and wait for its ACCEPT, answering benign incoming commands
/// (a racing source may probe us mid-handshake) and rejecting the rest.
/// Returns the accept payload, or Err on reject/timeout/io.
fn request(fd: RawFd, txn: u8, signal: u8, payload: &[u8]) -> Result<Vec<u8>, String> {
    send_command(fd, txn, signal, payload).map_err(|e| format!("send: {e}"))?;
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(4000);
    let mut buf = [0u8; 256];
    loop {
        let remain = deadline
            .saturating_duration_since(std::time::Instant::now())
            .as_millis() as i32;
        if remain <= 0 {
            return Err("response timeout".into());
        }
        let n = match poll_read(fd, &mut buf, remain) {
            Ok(0) => return Err("EOF".into()),
            Ok(n) => n,
            Err(e) if e.kind() == std::io::ErrorKind::TimedOut => {
                return Err("response timeout".into())
            }
            Err(e) => return Err(format!("read: {e}")),
        };
        if n < 2 {
            continue;
        }
        let (hdr, sig_id) = (buf[0], buf[1] & 0x3F);
        match hdr & 0x03 {
            MSG_TYPE_ACCEPT if (hdr >> 4) == txn && sig_id == signal => {
                return Ok(buf[2..n].to_vec());
            }
            MSG_TYPE_REJECT if (hdr >> 4) == txn && sig_id == signal => {
                let err = if n > 2 { buf[2] } else { 0 };
                return Err(format!("rejected (0x{err:02x})"));
            }
            MSG_TYPE_COMMAND => {
                // Answer probes so the peer's own state machine doesn't stall.
                let their_txn = hdr >> 4;
                let resp = match sig_id {
                    AVDTP_DISCOVER => handle_discover(their_txn),
                    AVDTP_GET_CAPABILITIES | AVDTP_GET_ALL_CAPABILITIES => {
                        let seid = if n > 2 { (buf[2] >> 2) & 0x3F } else { 0 };
                        handle_get_capabilities(their_txn, sig_id, seid)
                    }
                    other => build_reject(their_txn, other, AVDTP_ERR_BAD_STATE),
                };
                let _ = l2cap::raw_write(fd, &resp);
            }
            _ => {} // stale/unmatched response — keep waiting
        }
    }
}

/// Parse a DISCOVER accept payload into (seid, in_use, is_sink, media_type).
fn parse_discover_seps(payload: &[u8]) -> Vec<(u8, bool, bool, u8)> {
    payload
        .chunks_exact(2)
        .map(|p| {
            (
                (p[0] >> 2) & 0x3F,
                p[0] & 0x02 != 0,
                p[1] & 0x08 != 0,
                p[1] >> 4,
            )
        })
        .collect()
}

/// Bounded TLV walk over a capability list: return the MEDIA_CODEC element's
/// (codec_type, codec-specific bytes) if present and intact.
fn find_media_codec(caps: &[u8]) -> Option<(u8, Vec<u8>)> {
    let mut pos = 0;
    while pos + 1 < caps.len() {
        let category = caps[pos];
        let losc = caps[pos + 1] as usize;
        let end = pos + 2 + losc;
        if category == CAP_MEDIA_CODEC && losc >= 2 && end <= caps.len() {
            // element: media_type, codec_type, specific...
            return Some((caps[pos + 3], caps[pos + 4..end].to_vec()));
        }
        pos = end;
    }
    None
}

/// Build the SET_CONFIGURATION service capabilities for a remote SEP whose
/// MEDIA_CODEC element was `(codec_type, specific)`. Chooses 44.1 kHz stereo —
/// the decode path resamples from a fixed 44.1 k — and returns
/// `(our_int_seid, codec, caps)` or None if the SEP can't serve that.
fn build_set_config(codec_type: u8, specific: &[u8]) -> Option<(u8, A2dpCodec, Vec<u8>)> {
    let (int_seid, codec, cfg): (u8, A2dpCodec, Vec<u8>) = match codec_type {
        CODEC_VENDOR if specific.len() >= 7 => {
            let vendor = u32::from_le_bytes([specific[0], specific[1], specific[2], specific[3]]);
            let cid = u16::from_le_bytes([specific[4], specific[5]]);
            let caps_byte = specific[6];
            // Require 44.1 kHz (0x20) + stereo (0x02).
            if caps_byte & 0x20 == 0 || caps_byte & 0x02 == 0 {
                return None;
            }
            let mut cfg = specific[..6].to_vec();
            cfg.push(0x22); // 44.1 kHz stereo
            match (vendor, cid) {
                (0x0000_00D7, 0x0024) => (1, A2dpCodec::AptxHd, cfg),
                (0x0000_004F, 0x0001) => (2, A2dpCodec::Aptx, cfg),
                _ => return None,
            }
        }
        CODEC_SBC if specific.len() >= 4 => {
            let (freq_ch, blocks, min_bp, max_bp) =
                (specific[0], specific[1], specific[2], specific[3]);
            // 44.1 kHz + a 2-channel mode; 16 blocks / 8 subbands / loudness.
            if freq_ch & 0x20 == 0 {
                return None;
            }
            let ch = if freq_ch & 0x01 != 0 {
                0x01 // joint stereo
            } else if freq_ch & 0x02 != 0 {
                0x02 // stereo
            } else {
                return None;
            };
            if blocks & 0x10 == 0 || blocks & 0x04 == 0 || blocks & 0x01 == 0 {
                return None;
            }
            let lo = min_bp.max(2);
            let hi = max_bp.min(53);
            if lo > hi {
                return None;
            }
            (3, A2dpCodec::Sbc, vec![0x20 | ch, 0x15, lo, hi])
        }
        _ => return None,
    };

    let mut caps = Vec::with_capacity(6 + 2 + cfg.len());
    caps.push(CAP_MEDIA_TRANSPORT);
    caps.push(0x00);
    caps.push(CAP_MEDIA_CODEC);
    caps.push((2 + cfg.len()) as u8);
    caps.push(0x00); // media_type: audio
    caps.push(match codec {
        A2dpCodec::Sbc => CODEC_SBC,
        _ => CODEC_VENDOR,
    });
    caps.extend_from_slice(&cfg);
    Some((int_seid, codec, caps))
}

/// Vendor-codec preference rank for remote SEP selection (higher = better).
fn codec_rank(c: A2dpCodec) -> u8 {
    match c {
        A2dpCodec::AptxHd => 3,
        A2dpCodec::Aptx => 2,
        A2dpCodec::Sbc => 1,
    }
}

/// Connect to a bonded source and drive the full INT handshake through START.
/// Emits the same [`AvdtpEvent`]s as accepted sessions; on source-side CLOSE
/// or disconnect the session ends and the source reconnects via the acceptor.
pub fn spawn_initiator(
    remote: [u8; 6],
    slot: Arc<MixerSlot>,
    tx: mpsc::Sender<AvdtpEvent>,
    latency_target_ms: u32,
) {
    std::thread::Builder::new()
        .name("bt-avdtp-int".into())
        .spawn(move || {
            let addr_str = l2cap::bdaddr_to_string(&remote);
            info!("AVDTP INT: connecting to {}", addr_str);
            match initiator_session(remote, &slot, &tx, latency_target_ms) {
                Ok(()) => info!("AVDTP INT: session with {} ended", addr_str),
                Err(e) => warn!("AVDTP INT: {}: {}", addr_str, e),
            }
        })
        .expect("failed to spawn AVDTP initiator thread");
}

fn initiator_session(
    remote: [u8; 6],
    slot: &Arc<MixerSlot>,
    tx: &mpsc::Sender<AvdtpEvent>,
    latency_target_ms: u32,
) -> Result<(), String> {
    // Signaling channel. l2cap_connect pages the bonded link itself.
    let sig = l2cap::l2cap_connect(&remote, 25).map_err(|e| format!("signaling connect: {e}"))?;
    let sig_fd = sig.as_raw_fd();
    let mut txn: u8 = 0;
    let mut next_txn = || {
        txn = (txn + 1) & 0x0F;
        txn
    };

    // DISCOVER → audio source SEPs, not in use.
    let seps = request(sig_fd, next_txn(), AVDTP_DISCOVER, &[])?;
    let candidates: Vec<u8> = parse_discover_seps(&seps)
        .into_iter()
        .filter(|&(_, in_use, is_sink, media)| !in_use && !is_sink && media == 0)
        .map(|(seid, _, _, _)| seid)
        .collect();
    if candidates.is_empty() {
        return Err("no free audio-source SEP".into());
    }

    // GET_CAPABILITIES each; pick the best codec we can decode at 44.1 kHz.
    let mut best: Option<(u8, u8, A2dpCodec, Vec<u8>)> = None; // (acp, int, codec, caps)
    for acp in candidates {
        let caps = match request(sig_fd, next_txn(), AVDTP_GET_CAPABILITIES, &[acp << 2]) {
            Ok(c) => c,
            Err(_) => continue,
        };
        if let Some((ctype, specific)) = find_media_codec(&caps) {
            if let Some((int_seid, codec, cfg)) = build_set_config(ctype, &specific) {
                if best
                    .as_ref()
                    .is_none_or(|b| codec_rank(codec) > codec_rank(b.2))
                {
                    best = Some((acp, int_seid, codec, cfg));
                }
            }
        }
    }
    let (acp, int_seid, codec, cfg) = best.ok_or("no usable codec on source")?;
    info!("AVDTP INT: selected remote SEP {} codec={}", acp, codec);

    // SET_CONFIGURATION → OPEN → media transport (we initiate it too) → START.
    let mut sc = vec![acp << 2, int_seid << 2];
    sc.extend_from_slice(&cfg);
    request(sig_fd, next_txn(), AVDTP_SET_CONFIGURATION, &sc)?;
    request(sig_fd, next_txn(), AVDTP_OPEN, &[acp << 2])?;
    let media = l2cap::l2cap_connect(&remote, 25).map_err(|e| format!("media connect: {e}"))?;
    request(sig_fd, next_txn(), AVDTP_START, &[acp << 2])?;

    let read_mtu = l2cap::l2cap_get_options(media.as_raw_fd())
        .map(|o| o.imtu)
        .unwrap_or(672);
    let mut reader = Some(
        transport::spawn_reader(media, read_mtu, slot.clone(), codec, latency_target_ms)
            .map_err(|e| format!("reader spawn: {e}"))?,
    );
    let _ = tx.blocking_send(AvdtpEvent::Streaming { codec });
    info!("AVDTP INT: streaming (codec={}, mtu={})", codec, read_mtu);

    // Post-handshake: serve the source's commands until it walks away.
    let mut buf = [0u8; 256];
    loop {
        let n = match l2cap::raw_read(sig_fd, &mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => break,
        };
        if n < 2 || buf[0] & 0x03 != MSG_TYPE_COMMAND {
            continue;
        }
        let (their_txn, sig_id) = (buf[0] >> 4, buf[1] & 0x3F);
        let resp = match sig_id {
            AVDTP_SUSPEND => {
                let _ = tx.blocking_send(AvdtpEvent::Paused);
                build_accept(their_txn, AVDTP_SUSPEND, &[])
            }
            AVDTP_START => {
                if reader.as_ref().is_some_and(|h| h.is_running()) {
                    let _ = tx.blocking_send(AvdtpEvent::Resumed { codec });
                    build_accept(their_txn, AVDTP_START, &[])
                } else {
                    build_reject(their_txn, AVDTP_START, AVDTP_ERR_BAD_STATE)
                }
            }
            AVDTP_CLOSE | AVDTP_ABORT => {
                let r = build_accept(their_txn, sig_id, &[]);
                let _ = l2cap::raw_write(sig_fd, &r);
                break;
            }
            AVDTP_DISCOVER => handle_discover(their_txn),
            AVDTP_GET_CAPABILITIES | AVDTP_GET_ALL_CAPABILITIES => {
                let seid = if n > 2 { (buf[2] >> 2) & 0x3F } else { 0 };
                handle_get_capabilities(their_txn, sig_id, seid)
            }
            other => build_reject(their_txn, other, AVDTP_ERR_BAD_STATE),
        };
        if l2cap::raw_write(sig_fd, &resp).is_err() {
            break;
        }
    }

    if let Some(h) = reader.take() {
        h.stop.store(true, Ordering::Relaxed);
    }
    let _ = tx.blocking_send(AvdtpEvent::Stopped);
    let _ = tx.blocking_send(AvdtpEvent::Disconnected);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── initiator (INT role) codecs ──

    #[test]
    fn discover_parse_filters_source_seps() {
        // Two SEPs: seid 1 = audio source (tsep bit clear), seid 2 = audio sink.
        let payload = [(1u8 << 2), 0x00, (2u8 << 2), 0x08];
        let seps = parse_discover_seps(&payload);
        assert_eq!(seps.len(), 2);
        assert_eq!(seps[0], (1, false, false, 0)); // source
        assert_eq!(seps[1], (2, false, true, 0)); // sink
                                                  // in_use flag
        let busy = [(3u8 << 2) | 0x02, 0x00];
        assert_eq!(parse_discover_seps(&busy)[0], (3, true, false, 0));
    }

    #[test]
    fn find_media_codec_walks_tlv_and_bounds() {
        // MEDIA_TRANSPORT then MEDIA_CODEC(SBC, 4 specific bytes).
        let caps = [
            CAP_MEDIA_TRANSPORT,
            0x00,
            CAP_MEDIA_CODEC,
            0x06,
            0x00,
            CODEC_SBC,
            0xFF,
            0xFF,
            2,
            53,
        ];
        let (ctype, specific) = find_media_codec(&caps).unwrap();
        assert_eq!(ctype, CODEC_SBC);
        assert_eq!(specific, vec![0xFF, 0xFF, 2, 53]);
        // Truncated losc must not panic or return garbage.
        assert!(find_media_codec(&[CAP_MEDIA_CODEC, 0x0A, 0x00, CODEC_SBC]).is_none());
    }

    #[test]
    fn set_config_prefers_44100_stereo_sbc() {
        // Full SBC caps: everything supported, bitpool 2..53.
        let (int_seid, codec, caps) = build_set_config(CODEC_SBC, &[0xFF, 0xFF, 2, 53]).unwrap();
        assert_eq!(int_seid, 3);
        assert_eq!(codec, A2dpCodec::Sbc);
        // caps = MT + MC(losc=6, audio, SBC, 44.1|joint, 16blk/8sub/loudness, 2, 53)
        assert_eq!(
            caps,
            vec![
                CAP_MEDIA_TRANSPORT,
                0x00,
                CAP_MEDIA_CODEC,
                0x06,
                0x00,
                CODEC_SBC,
                0x21,
                0x15,
                2,
                53
            ]
        );
        // A 48k-only source can't serve our fixed 44.1 decode path.
        assert!(build_set_config(CODEC_SBC, &[0x10 | 0x01, 0xFF, 2, 53]).is_none());
    }

    #[test]
    fn set_config_vendor_aptx_requires_44100() {
        // aptX caps: vendor 0x4F, codec 0x0001, 44.1+48 stereo (0x32).
        let spec = [0x4F, 0x00, 0x00, 0x00, 0x01, 0x00, 0x32];
        let (int_seid, codec, caps) = build_set_config(CODEC_VENDOR, &spec).unwrap();
        assert_eq!(int_seid, 2);
        assert_eq!(codec, A2dpCodec::Aptx);
        // Chosen config byte = 0x22 (44.1 stereo), vendor/codec ids preserved.
        assert_eq!(&caps[6..12], &[0x4F, 0x00, 0x00, 0x00, 0x01, 0x00]);
        assert_eq!(caps[12], 0x22);
        // 48k-only aptX (0x12) is refused.
        let spec48 = [0x4F, 0x00, 0x00, 0x00, 0x01, 0x00, 0x12];
        assert!(build_set_config(CODEC_VENDOR, &spec48).is_none());
        // Unknown vendor refused.
        let unk = [0x99, 0x00, 0x00, 0x00, 0x01, 0x00, 0x32];
        assert!(build_set_config(CODEC_VENDOR, &unk).is_none());
    }

    #[test]
    fn set_config_ranks_aptx_hd_above_aptx_above_sbc() {
        assert!(codec_rank(A2dpCodec::AptxHd) > codec_rank(A2dpCodec::Aptx));
        assert!(codec_rank(A2dpCodec::Aptx) > codec_rank(A2dpCodec::Sbc));
    }
}
