//! AVRCP Target over AVCTP (L2CAP PSM 23) — socket/session I/O layer.
//!
//! Implements the sink-side AVRCP **Target**: the A2DP source (phone/PC) acts as
//! the AVRCP Controller and sends us absolute-volume commands, which we apply to
//! the master volume. We also answer the AV/C handshake (Unit/Subunit Info),
//! GetCapabilities and RegisterNotification so the controller's volume slider
//! works and stays live.
//!
//! Audio (A2DP/AVDTP) is independent — a malformed or unhandled AVRCP command
//! never affects playback.
//!
//! The wire codec (frame encode/decode) is pure and lives in
//! `encore_common::avrcp`, re-exported here under the same `avrcp::` path so
//! callers in this crate don't need to change. It was split out so those unit
//! tests run on any host — this module is `#[cfg(target_os = "linux")]`-gated
//! end to end (raw sockets), which meant they never ran on a non-Linux dev
//! machine.

pub use encore_common::avrcp::*;

#[cfg(target_os = "linux")]
use super::l2cap;
#[cfg(target_os = "linux")]
use std::os::fd::{AsRawFd, OwnedFd, RawFd};
#[cfg(target_os = "linux")]
use tokio::sync::{broadcast, mpsc};
#[cfg(target_os = "linux")]
use tracing::{info, warn};

/// Send a passthrough key (press then release) to the source on `raw_fd`.
#[cfg(target_os = "linux")]
pub fn send_passthrough(raw_fd: std::os::fd::RawFd, op_id: u8) {
    let _ = l2cap::raw_write(raw_fd, &build_passthrough(2, op_id, true));
    let _ = l2cap::raw_write(raw_fd, &build_passthrough(2, op_id, false));
}

/// Spawn the AVCTP accept loop. `vol_tx` carries master-volume percents the
/// controller sets via SetAbsoluteVolume; `cur_vol` is read to answer volume
/// notifications; `ws_tx` broadcasts now-playing metadata to the dashboard;
/// `session_fd` exposes the active socket as a shared owning handle so the
/// subsystem can send transport keys / volume notifications. It holds an
/// `Arc<OwnedFd>` (not a raw int) so a writer that has cloned it keeps the
/// descriptor alive — the session can't close it out from under an in-flight
/// write, and the SEQPACKET socket keeps concurrent writes atomic.
#[cfg(target_os = "linux")]
#[allow(clippy::too_many_arguments)]
pub fn spawn_avctp(
    listener: OwnedFd,
    vol_tx: Option<mpsc::Sender<u8>>,
    cur_vol: std::sync::Arc<std::sync::atomic::AtomicU8>,
    ws_tx: Option<broadcast::Sender<String>>,
    session_fd: std::sync::Arc<std::sync::Mutex<Option<std::sync::Arc<OwnedFd>>>>,
    vol_label: std::sync::Arc<std::sync::atomic::AtomicU8>,
    vol_registered: std::sync::Arc<std::sync::atomic::AtomicBool>,
) {
    use std::sync::atomic::Ordering;
    std::thread::Builder::new()
        .name("bt-avctp".into())
        .spawn(move || {
            info!("AVRCP: AVCTP listening on PSM {}", PSM_AVCTP);
            let lfd = listener.as_raw_fd();
            loop {
                match l2cap::l2cap_accept(lfd) {
                    Ok((fd, addr)) => {
                        info!(
                            "AVRCP: AVCTP connection from {}",
                            l2cap::bdaddr_to_string(&addr)
                        );
                        // Publish the socket as a shared owning handle. The Arc
                        // keeps the fd alive while a subsystem writer holds a clone,
                        // so it can't be closed mid-write; clearing it to None ends
                        // the publication before our own clone drops (and closes it).
                        let fd = std::sync::Arc::new(fd);
                        *session_fd.lock().unwrap() = Some(fd.clone());
                        avctp_session(
                            fd.as_raw_fd(),
                            vol_tx.as_ref(),
                            &cur_vol,
                            ws_tx.as_ref(),
                            &vol_label,
                            &vol_registered,
                        );
                        *session_fd.lock().unwrap() = None;
                        vol_registered.store(false, Ordering::Relaxed);
                        info!("AVRCP: AVCTP session ended");
                        // Clear now-playing + progress on disconnect.
                        broadcast_track(ws_tx.as_ref(), &TrackMeta::default());
                        broadcast_play_status(ws_tx.as_ref(), 0, 0);
                    }
                    Err(e) => {
                        if e.kind() == std::io::ErrorKind::Interrupted {
                            continue;
                        }
                        warn!("AVRCP: accept error: {}", e);
                        std::thread::sleep(std::time::Duration::from_millis(100));
                    }
                }
            }
        })
        .expect("failed to spawn AVCTP thread");
}

/// Broadcast now-playing metadata to the dashboard as a `BluetoothTrack`.
#[cfg(target_os = "linux")]
fn broadcast_track(ws_tx: Option<&broadcast::Sender<String>>, t: &TrackMeta) {
    let Some(ws) = ws_tx else { return };
    let msg =
        encore_common::protocol::ServerMsg::BluetoothTrack(encore_common::protocol::BtTrack {
            title: t.title.clone(),
            artist: t.artist.clone(),
            album: t.album.clone(),
        });
    if let Ok(json) = serde_json::to_string(&msg) {
        let _ = ws.send(json);
    }
}

/// Broadcast playback position/duration to the dashboard.
#[cfg(target_os = "linux")]
fn broadcast_play_status(
    ws_tx: Option<&broadcast::Sender<String>>,
    position_ms: u32,
    duration_ms: u32,
) {
    let Some(ws) = ws_tx else { return };
    let msg = encore_common::protocol::ServerMsg::BluetoothPlayStatus(
        encore_common::protocol::BtPlayStatus {
            position_ms,
            duration_ms,
        },
    );
    if let Ok(json) = serde_json::to_string(&msg) {
        let _ = ws.send(json);
    }
}

/// Service one AVCTP session: as target, answer volume/handshake; as controller,
/// pull now-playing metadata and follow track changes.
#[cfg(target_os = "linux")]
fn avctp_session(
    raw: RawFd,
    vol_tx: Option<&mpsc::Sender<u8>>,
    cur_vol: &std::sync::atomic::AtomicU8,
    ws_tx: Option<&broadcast::Sender<String>>,
    vol_label: &std::sync::atomic::AtomicU8,
    vol_registered: &std::sync::atomic::AtomicBool,
) {
    use std::sync::atomic::Ordering;

    // Kick off the controller side: register for track changes and fetch the
    // current track. We use our own transaction labels (0, 1); responses are
    // matched by PDU, not label, so collisions with the source are harmless.
    let _ = l2cap::raw_write(raw, &build_register(0, EVENT_TRACK_CHANGED, 0));
    let _ = l2cap::raw_write(raw, &build_get_element_attributes(1));
    let _ = l2cap::raw_write(raw, &build_get_play_status(3));
    let _ = l2cap::raw_write(raw, &build_register(4, EVENT_PLAYBACK_POS_CHANGED, 1));
    let mut last_track = TrackMeta::default();
    let mut cur_duration: u32 = 0;

    let mut buf = [0u8; 512];
    loop {
        let n = match l2cap::raw_read(raw, &mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) => {
                if e.kind() == std::io::ErrorKind::Interrupted {
                    continue;
                }
                break;
            }
        };
        let frame = &buf[..n];
        let is_response = frame.first().is_some_and(|b| (b >> 1) & 0x01 == 1);

        if is_response {
            // Controller side: a response to one of our queries.
            match parse_ct_response(frame) {
                CtEvent::Track(t) => {
                    if t != last_track {
                        info!("AVRCP: now playing: {} — {}", t.artist, t.title);
                        broadcast_track(ws_tx, &t);
                        last_track = t;
                    }
                }
                CtEvent::TrackChanged => {
                    // Re-register (notifications are one-shot) and re-query the
                    // new track's metadata + play status (duration).
                    let _ = l2cap::raw_write(raw, &build_register(0, EVENT_TRACK_CHANGED, 0));
                    let _ = l2cap::raw_write(raw, &build_get_element_attributes(1));
                    let _ = l2cap::raw_write(raw, &build_get_play_status(3));
                }
                CtEvent::PlayStatus(length_ms, position_ms) => {
                    cur_duration = length_ms;
                    broadcast_play_status(ws_tx, position_ms, length_ms);
                }
                CtEvent::Position(position_ms) => {
                    broadcast_play_status(ws_tx, position_ms, cur_duration);
                    // Position notifications are one-shot; re-arm for the next tick.
                    let _ =
                        l2cap::raw_write(raw, &build_register(4, EVENT_PLAYBACK_POS_CHANGED, 1));
                }
                CtEvent::None => {}
            }
            continue;
        }

        // Target side: a command addressed to us.
        let action = handle_frame(frame, cur_vol.load(Ordering::Relaxed));
        if let Some(pct) = action.set_volume_pct {
            cur_vol.store(pct, Ordering::Relaxed);
            if let Some(tx) = vol_tx {
                let _ = tx.try_send(pct);
            }
            info!("AVRCP: absolute volume -> {}%", pct);
        }
        if let Some(label) = action.register_vol_label {
            // Controller wants volume-change notifications: remember the label so
            // the subsystem can send a CHANGED when our local volume moves.
            vol_label.store(label, Ordering::Relaxed);
            vol_registered.store(true, Ordering::Relaxed);
        }
        if let Some(resp) = action.response {
            if l2cap::raw_write(raw, &resp).is_err() {
                break;
            }
        }
    }
}
