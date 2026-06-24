//! AVRCP Target over AVCTP (L2CAP PSM 23).
//!
//! Implements the sink-side AVRCP **Target**: the A2DP source (phone/PC) acts as
//! the AVRCP Controller and sends us absolute-volume commands, which we apply to
//! the master volume. We also answer the AV/C handshake (Unit/Subunit Info),
//! GetCapabilities and RegisterNotification so the controller's volume slider
//! works and stays live.
//!
//! Audio (A2DP/AVDTP) is independent — a malformed or unhandled AVRCP command
//! never affects playback. The wire codec ([`handle_frame`]) is pure and
//! host-tested; the session loop just does I/O and applies volume.

#[cfg(target_os = "linux")]
use super::l2cap;
#[cfg(target_os = "linux")]
use std::os::fd::{AsRawFd, OwnedFd, RawFd};
#[cfg(target_os = "linux")]
use tokio::sync::{broadcast, mpsc};
#[cfg(target_os = "linux")]
use tracing::{info, warn};

/// AVCTP control-channel PSM.
pub const PSM_AVCTP: u16 = 23;

/// AVRCP Bluetooth SIG profile id, carried in the AVCTP header.
const PID_AVRCP: u16 = 0x110E;

// ── AV/C framing ──
/// PANEL subunit (subunit_type 0x09 << 3).
const SUBUNIT_PANEL: u8 = 0x48;
/// UNIT addressing (used by Unit/Subunit Info).
const SUBUNIT_UNIT: u8 = 0xFF;
const OP_VENDOR: u8 = 0x00;
const OP_UNIT_INFO: u8 = 0x30;
const OP_SUBUNIT_INFO: u8 = 0x31;
const OP_PASSTHROUGH: u8 = 0x7C;

// AV/C response codes (low nibble of AV/C byte 0).
const RSP_NOT_IMPL: u8 = 0x08;
const RSP_ACCEPTED: u8 = 0x09;
const RSP_REJECTED: u8 = 0x0A;
const RSP_STABLE: u8 = 0x0C;
const RSP_INTERIM: u8 = 0x0F;

// ── AVRCP metadata (VENDOR DEPENDENT) ──
/// Bluetooth SIG company id, used by all AVRCP metadata PDUs.
const BT_SIG_COMPANY: [u8; 3] = [0x00, 0x19, 0x58];
const PDU_GET_CAPABILITIES: u8 = 0x10;
const PDU_REGISTER_NOTIFICATION: u8 = 0x31;
const PDU_SET_ABSOLUTE_VOLUME: u8 = 0x50;
const CAP_COMPANY_ID: u8 = 0x02;
const CAP_EVENTS_SUPPORTED: u8 = 0x03;
const EVENT_VOLUME_CHANGED: u8 = 0x0d;
/// AVRCP error: invalid parameter (used in a rejected metadata response).
const STATUS_INVALID_PARAM: u8 = 0x01;

/// AVRCP absolute volume is 7-bit (0..=0x7F).
const VOL_MAX: u8 = 0x7F;

/// AVRCP volume (0..=0x7F) -> master volume percent (0..=100), rounded.
fn avrcp_to_pct(v: u8) -> u8 {
    let v = v & VOL_MAX;
    ((v as u16 * 100 + VOL_MAX as u16 / 2) / VOL_MAX as u16) as u8
}

/// Master volume percent (0..=100) -> AVRCP volume (0..=0x7F), rounded.
fn pct_to_avrcp(p: u8) -> u8 {
    let p = p.min(100);
    ((p as u16 * VOL_MAX as u16 + 50) / 100) as u8
}

/// Result of handling one inbound AVCTP frame.
#[derive(Debug, Default, PartialEq)]
struct Action {
    /// Bytes to write back on the AVCTP socket (a full AVCTP+AV/C response).
    response: Option<Vec<u8>>,
    /// New master volume (0..=100) to apply, if the controller set it.
    set_volume_pct: Option<u8>,
    /// Transaction label the controller registered EVENT_VOLUME_CHANGED on, so
    /// we can send it a CHANGED when our local volume moves.
    register_vol_label: Option<u8>,
}

/// Build an EVENT_VOLUME_CHANGED notification (CHANGED) for `label`, reporting
/// `vol_pct` (0..=100) as an AVRCP absolute volume. Sent when our local volume
/// changes and the controller had registered for it.
pub fn build_volume_changed(label: u8, vol_pct: u8) -> Vec<u8> {
    vendor_response(
        label,
        RSP_CHANGED,
        PDU_REGISTER_NOTIFICATION,
        &[EVENT_VOLUME_CHANGED, pct_to_avrcp(vol_pct)],
    )
}

/// AVCTP single-packet response header for `label`: packet_type=0, cr=1 (response).
fn avctp_response_header(label: u8) -> [u8; 3] {
    [
        (label << 4) | 0x02,
        (PID_AVRCP >> 8) as u8,
        (PID_AVRCP & 0xFF) as u8,
    ]
}

/// Build a non-vendor AV/C response (Unit/Subunit Info, Passthrough).
fn avc_response(label: u8, rsp: u8, subunit: u8, opcode: u8, operands: &[u8]) -> Vec<u8> {
    let mut v = Vec::with_capacity(6 + operands.len());
    v.extend_from_slice(&avctp_response_header(label));
    v.push(rsp);
    v.push(subunit);
    v.push(opcode);
    v.extend_from_slice(operands);
    v
}

/// Build a VENDOR DEPENDENT (AVRCP metadata) response carrying one PDU.
fn vendor_response(label: u8, rsp: u8, pdu: u8, params: &[u8]) -> Vec<u8> {
    let mut v = Vec::with_capacity(11 + params.len());
    v.extend_from_slice(&avctp_response_header(label));
    v.push(rsp);
    v.push(SUBUNIT_PANEL);
    v.push(OP_VENDOR);
    v.extend_from_slice(&BT_SIG_COMPANY);
    v.push(pdu);
    v.push(0x00); // packet_type = single
    v.extend_from_slice(&(params.len() as u16).to_be_bytes());
    v.extend_from_slice(params);
    v
}

/// Decode one inbound AVCTP/AVRCP frame and produce the response (+ any volume
/// change). `cur_vol_pct` is our current master volume, reported in volume
/// notifications. Pure: no I/O, host-tested.
///
/// AVCTP single-packet header: byte0 = [txn:4][pkt_type:2][c/r:1][ipid:1], then
/// a 2-byte big-endian profile id; the remainder is the AV/C frame.
fn handle_frame(frame: &[u8], cur_vol_pct: u8) -> Action {
    if frame.len() < 4 {
        return Action::default();
    }
    let label = frame[0] >> 4;
    let is_response = (frame[0] >> 1) & 0x01 == 1;
    let pid = u16::from_be_bytes([frame[1], frame[2]]);
    // We are a Target: act only on commands addressed to AVRCP.
    if is_response || pid != PID_AVRCP {
        return Action::default();
    }
    let avc = &frame[3..];
    if avc.len() < 3 {
        return Action::default();
    }
    let subunit = avc[1];
    let opcode = avc[2];
    let operands = &avc[3..];

    match opcode {
        // AV/C Unit Info: identify ourselves as a PANEL unit.
        OP_UNIT_INFO => Action {
            response: Some(avc_response(
                label,
                RSP_STABLE,
                SUBUNIT_UNIT,
                OP_UNIT_INFO,
                &[0x07, SUBUNIT_PANEL, 0xFF, 0xFF, 0xFF],
            )),
            ..Default::default()
        },
        // AV/C Subunit Info: one PANEL subunit.
        OP_SUBUNIT_INFO => Action {
            response: Some(avc_response(
                label,
                RSP_STABLE,
                SUBUNIT_UNIT,
                OP_SUBUNIT_INFO,
                &[0x07, SUBUNIT_PANEL, 0xFF, 0xFF, 0xFF],
            )),
            ..Default::default()
        },
        // Passthrough (transport keys). We don't act on them as a Target, but
        // accept so the controller doesn't treat the key as failed.
        OP_PASSTHROUGH => Action {
            response: Some(avc_response(
                label,
                RSP_ACCEPTED,
                subunit,
                OP_PASSTHROUGH,
                operands,
            )),
            ..Default::default()
        },
        // AVRCP metadata.
        OP_VENDOR => handle_vendor(label, operands, cur_vol_pct),
        // Anything else: tell the controller we don't implement it.
        _ => Action {
            response: Some(avc_response(label, RSP_NOT_IMPL, subunit, opcode, operands)),
            ..Default::default()
        },
    }
}

/// Handle a VENDOR DEPENDENT operand block: company(3) pdu(1) pkt(1) len(2) params.
fn handle_vendor(label: u8, operands: &[u8], cur_vol_pct: u8) -> Action {
    if operands.len() < 7 || operands[..3] != BT_SIG_COMPANY {
        return Action {
            response: Some(vendor_response(label, RSP_REJECTED, 0x00, &[STATUS_INVALID_PARAM])),
            ..Default::default()
        };
    }
    let pdu = operands[3];
    // operands[4] = packet_type (single, ignored); operands[5..7] = param_len.
    let param_len = u16::from_be_bytes([operands[5], operands[6]]) as usize;
    let params = operands
        .get(7..7 + param_len)
        .unwrap_or(&operands[7.min(operands.len())..]);

    match pdu {
        PDU_GET_CAPABILITIES => {
            let cap = params.first().copied().unwrap_or(0);
            match cap {
                CAP_EVENTS_SUPPORTED => Action {
                    // We support exactly one event: volume changed.
                    response: Some(vendor_response(
                        label,
                        RSP_STABLE,
                        PDU_GET_CAPABILITIES,
                        &[CAP_EVENTS_SUPPORTED, 0x01, EVENT_VOLUME_CHANGED],
                    )),
                    ..Default::default()
                },
                CAP_COMPANY_ID => Action {
                    response: Some(vendor_response(
                        label,
                        RSP_STABLE,
                        PDU_GET_CAPABILITIES,
                        &[
                            CAP_COMPANY_ID,
                            0x01,
                            BT_SIG_COMPANY[0],
                            BT_SIG_COMPANY[1],
                            BT_SIG_COMPANY[2],
                        ],
                    )),
                    ..Default::default()
                },
                _ => Action {
                    response: Some(vendor_response(
                        label,
                        RSP_REJECTED,
                        PDU_GET_CAPABILITIES,
                        &[STATUS_INVALID_PARAM],
                    )),
                    ..Default::default()
                },
            }
        }
        PDU_REGISTER_NOTIFICATION => {
            let event = params.first().copied().unwrap_or(0);
            if event == EVENT_VOLUME_CHANGED {
                // Interim response carries the current volume; remember the label
                // so a later local volume change can send the CHANGED follow-up.
                Action {
                    response: Some(vendor_response(
                        label,
                        RSP_INTERIM,
                        PDU_REGISTER_NOTIFICATION,
                        &[EVENT_VOLUME_CHANGED, pct_to_avrcp(cur_vol_pct)],
                    )),
                    register_vol_label: Some(label),
                    ..Default::default()
                }
            } else {
                Action {
                    response: Some(vendor_response(
                        label,
                        RSP_NOT_IMPL,
                        PDU_REGISTER_NOTIFICATION,
                        &[event],
                    )),
                    ..Default::default()
                }
            }
        }
        PDU_SET_ABSOLUTE_VOLUME => {
            let vol = params.first().copied().unwrap_or(0) & VOL_MAX;
            Action {
                response: Some(vendor_response(
                    label,
                    RSP_ACCEPTED,
                    PDU_SET_ABSOLUTE_VOLUME,
                    &[vol],
                )),
                set_volume_pct: Some(avrcp_to_pct(vol)),
                ..Default::default()
            }
        }
        _ => Action {
            response: Some(vendor_response(label, RSP_REJECTED, pdu, &[STATUS_INVALID_PARAM])),
            ..Default::default()
        },
    }
}

// ── Controller (CT) side: query now-playing metadata from the source ──
const PDU_GET_ELEMENT_ATTRIBUTES: u8 = 0x20;
const PDU_GET_PLAY_STATUS: u8 = 0x30;
const CTYPE_STATUS: u8 = 0x01;
const CTYPE_NOTIFY: u8 = 0x03;
const RSP_CHANGED: u8 = 0x0D;
// AVRCP event ids (spec): 0x01 = playback-status, 0x02 = track, 0x05 = position.
const EVENT_TRACK_CHANGED: u8 = 0x02;
const EVENT_PLAYBACK_POS_CHANGED: u8 = 0x05;
const ATTR_TITLE: u32 = 0x01;
const ATTR_ARTIST: u32 = 0x02;
const ATTR_ALBUM: u32 = 0x03;

/// Now-playing metadata parsed from a GetElementAttributes response.
#[derive(Debug, Default, PartialEq, Clone)]
struct TrackMeta {
    title: String,
    artist: String,
    album: String,
}

/// What a controller-side (response) frame told us.
#[derive(Debug, PartialEq)]
enum CtEvent {
    /// Parsed track metadata.
    Track(TrackMeta),
    /// The source signalled a track change — re-query attributes + play status.
    TrackChanged,
    /// Play status: (song length ms, song position ms). 0xFFFFFFFF = unknown.
    PlayStatus(u32, u32),
    /// Live playback position (ms) from a position notification.
    Position(u32),
    /// Nothing actionable.
    None,
}

/// AVCTP single-packet command header for `label` (cr=0).
fn avctp_command_header(label: u8) -> [u8; 3] {
    [
        label << 4,
        (PID_AVRCP >> 8) as u8,
        (PID_AVRCP & 0xFF) as u8,
    ]
}

/// Build a VENDOR DEPENDENT command (controller -> target).
fn vendor_command(label: u8, ctype: u8, pdu: u8, params: &[u8]) -> Vec<u8> {
    let mut v = Vec::with_capacity(11 + params.len());
    v.extend_from_slice(&avctp_command_header(label));
    v.push(ctype);
    v.push(SUBUNIT_PANEL);
    v.push(OP_VENDOR);
    v.extend_from_slice(&BT_SIG_COMPANY);
    v.push(pdu);
    v.push(0x00); // packet type = single
    v.extend_from_slice(&(params.len() as u16).to_be_bytes());
    v.extend_from_slice(params);
    v
}

/// CT command: request title/artist/album for the currently playing element.
fn build_get_element_attributes(label: u8) -> Vec<u8> {
    let mut params = Vec::with_capacity(8 + 1 + 12);
    params.extend_from_slice(&[0u8; 8]); // element identifier 0 = PLAYING
    params.push(3); // attribute count
    for id in [ATTR_TITLE, ATTR_ARTIST, ATTR_ALBUM] {
        params.extend_from_slice(&id.to_be_bytes());
    }
    vendor_command(label, CTYPE_STATUS, PDU_GET_ELEMENT_ATTRIBUTES, &params)
}

/// CT command: register for an event notification with a playback interval
/// (seconds; only meaningful for the position event, 0 otherwise).
fn build_register(label: u8, event: u8, interval_secs: u32) -> Vec<u8> {
    let mut params = Vec::with_capacity(5);
    params.push(event);
    params.extend_from_slice(&interval_secs.to_be_bytes());
    vendor_command(label, CTYPE_NOTIFY, PDU_REGISTER_NOTIFICATION, &params)
}

/// CT command: get play status (song length, position, status).
fn build_get_play_status(label: u8) -> Vec<u8> {
    vendor_command(label, CTYPE_STATUS, PDU_GET_PLAY_STATUS, &[])
}

/// Parse a GetElementAttributes response payload: count(1) then per attribute
/// id(4) charset(2) len(2) value(len). Bounds-checked.
fn parse_element_attributes(params: &[u8]) -> Option<TrackMeta> {
    let num = *params.first()? as usize;
    let mut pos = 1;
    let mut track = TrackMeta::default();
    let mut found = false;
    for _ in 0..num {
        if pos + 8 > params.len() {
            break;
        }
        let id = u32::from_be_bytes([params[pos], params[pos + 1], params[pos + 2], params[pos + 3]]);
        let len = u16::from_be_bytes([params[pos + 6], params[pos + 7]]) as usize;
        pos += 8;
        if pos + len > params.len() {
            break;
        }
        let val = String::from_utf8_lossy(&params[pos..pos + len]).into_owned();
        pos += len;
        match id {
            ATTR_TITLE => {
                track.title = val;
                found = true;
            }
            ATTR_ARTIST => {
                track.artist = val;
                found = true;
            }
            ATTR_ALBUM => {
                track.album = val;
                found = true;
            }
            _ => {}
        }
    }
    found.then_some(track)
}

/// Interpret a response frame (cr=1) from the source's target.
fn parse_ct_response(frame: &[u8]) -> CtEvent {
    if frame.len() < 4 {
        return CtEvent::None;
    }
    // Same AVCTP profile-id guard handle_frame enforces, so a non-AVRCP frame
    // on this channel can't be misread as metadata.
    if u16::from_be_bytes([frame[1], frame[2]]) != PID_AVRCP {
        return CtEvent::None;
    }
    let avc = &frame[3..];
    if avc.len() < 3 || avc[2] != OP_VENDOR {
        return CtEvent::None;
    }
    let rsp = avc[0] & 0x0F;
    let operands = &avc[3..];
    if operands.len() < 7 || operands[..3] != BT_SIG_COMPANY {
        return CtEvent::None;
    }
    let pdu = operands[3];
    let param_len = u16::from_be_bytes([operands[5], operands[6]]) as usize;
    let params = operands
        .get(7..7 + param_len)
        .unwrap_or(&operands[7.min(operands.len())..]);
    let be32 = |s: &[u8]| u32::from_be_bytes([s[0], s[1], s[2], s[3]]);
    match pdu {
        PDU_GET_ELEMENT_ATTRIBUTES if rsp == RSP_STABLE => {
            parse_element_attributes(params).map_or(CtEvent::None, CtEvent::Track)
        }
        // GetPlayStatus response: length(4) position(4) status(1).
        PDU_GET_PLAY_STATUS if rsp == RSP_STABLE && params.len() >= 8 => {
            CtEvent::PlayStatus(be32(&params[0..4]), be32(&params[4..8]))
        }
        PDU_REGISTER_NOTIFICATION if rsp == RSP_CHANGED => match params.first() {
            // Track changed -> re-fetch metadata + play status.
            Some(&EVENT_TRACK_CHANGED) => CtEvent::TrackChanged,
            // Position changed -> params: event(1) position(4).
            Some(&EVENT_PLAYBACK_POS_CHANGED) if params.len() >= 5 => {
                CtEvent::Position(be32(&params[1..5]))
            }
            _ => CtEvent::None,
        },
        _ => CtEvent::None,
    }
}

// ── Transport (PASSTHROUGH) — controller sends play/pause/next/prev ──
pub const OP_PLAY: u8 = 0x44;
pub const OP_STOP: u8 = 0x45;
pub const OP_PAUSE: u8 = 0x46;
pub const OP_FORWARD: u8 = 0x4B;
pub const OP_BACKWARD: u8 = 0x4C;

/// Map a dashboard transport key to an AVRCP passthrough operation id.
pub fn passthrough_op(key: &str) -> Option<u8> {
    Some(match key {
        "play" => OP_PLAY,
        "pause" => OP_PAUSE,
        "stop" => OP_STOP,
        "next" => OP_FORWARD,
        "prev" => OP_BACKWARD,
        _ => return None,
    })
}

/// Build a PASSTHROUGH command: panel subunit, op id with the press/release bit
/// (bit 7 set = release).
fn build_passthrough(label: u8, op_id: u8, pressed: bool) -> Vec<u8> {
    let mut v = Vec::with_capacity(8);
    v.extend_from_slice(&avctp_command_header(label));
    v.push(0x00); // ctype = CONTROL
    v.push(SUBUNIT_PANEL);
    v.push(OP_PASSTHROUGH);
    v.push(if pressed { op_id } else { op_id | 0x80 });
    v.push(0x00); // operand data length
    v
}

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
                        info!("AVRCP: AVCTP connection from {}", l2cap::bdaddr_to_string(&addr));
                        // Publish the socket as a shared owning handle. The Arc
                        // keeps the fd alive while a subsystem writer holds a clone,
                        // so it can't be closed mid-write; clearing it to None ends
                        // the publication before our own clone drops (and closes it).
                        let fd = std::sync::Arc::new(fd);
                        *session_fd.lock().unwrap() = Some(fd.clone());
                        avctp_session(fd.as_raw_fd(), vol_tx.as_ref(), &cur_vol, ws_tx.as_ref(), &vol_label, &vol_registered);
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
    let msg = encore_common::protocol::ServerMsg::BluetoothTrack(encore_common::protocol::BtTrack {
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
        let is_response = frame.first().map_or(false, |b| (b >> 1) & 0x01 == 1);

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
                    let _ = l2cap::raw_write(raw, &build_register(4, EVENT_PLAYBACK_POS_CHANGED, 1));
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

#[cfg(test)]
mod tests {
    use super::*;

    fn label_of(resp: &[u8]) -> u8 {
        resp[0] >> 4
    }
    fn is_response(resp: &[u8]) -> bool {
        (resp[0] >> 1) & 0x01 == 1
    }

    #[test]
    fn volume_conversions_round_trip_endpoints() {
        assert_eq!(avrcp_to_pct(0), 0);
        assert_eq!(avrcp_to_pct(VOL_MAX), 100);
        assert_eq!(pct_to_avrcp(0), 0);
        assert_eq!(pct_to_avrcp(100), VOL_MAX);
        // Mid-scale stays mid-scale within rounding.
        assert!((avrcp_to_pct(0x40) as i16 - 50).abs() <= 1);
    }

    #[test]
    fn set_absolute_volume_applies_and_accepts() {
        // VENDOR DEPENDENT SetAbsoluteVolume(0x7F) from controller, label 3.
        let mut frame = vec![0x30, 0x11, 0x0E, 0x00, SUBUNIT_PANEL, OP_VENDOR];
        frame.extend_from_slice(&BT_SIG_COMPANY);
        frame.push(PDU_SET_ABSOLUTE_VOLUME);
        frame.push(0x00); // packet type
        frame.extend_from_slice(&1u16.to_be_bytes()); // param len
        frame.push(0x7F); // volume
        let action = handle_frame(&frame, 0);
        assert_eq!(action.set_volume_pct, Some(100));
        let resp = action.response.expect("response");
        assert!(is_response(&resp));
        assert_eq!(label_of(&resp), 3);
        // AV/C response code = ACCEPTED, pdu echoed, volume echoed.
        assert_eq!(resp[3], RSP_ACCEPTED);
        assert_eq!(resp[9], PDU_SET_ABSOLUTE_VOLUME); // pdu: after hdr(3)+rsp+subunit+opcode+company(3)
        assert_eq!(*resp.last().unwrap(), 0x7F);
    }

    #[test]
    fn get_capabilities_events_lists_volume_changed() {
        let mut frame = vec![0x00, 0x11, 0x0E, 0x01, SUBUNIT_PANEL, OP_VENDOR];
        frame.extend_from_slice(&BT_SIG_COMPANY);
        frame.push(PDU_GET_CAPABILITIES);
        frame.push(0x00);
        frame.extend_from_slice(&1u16.to_be_bytes());
        frame.push(CAP_EVENTS_SUPPORTED);
        let resp = handle_frame(&frame, 50).response.expect("response");
        assert_eq!(resp[3], RSP_STABLE);
        // params: [cap_id, count, event...] -> last byte is EVENT_VOLUME_CHANGED
        assert_eq!(*resp.last().unwrap(), EVENT_VOLUME_CHANGED);
    }

    #[test]
    fn register_volume_notification_is_interim_with_current() {
        let mut frame = vec![0x10, 0x11, 0x0E, 0x03, SUBUNIT_PANEL, OP_VENDOR];
        frame.extend_from_slice(&BT_SIG_COMPANY);
        frame.push(PDU_REGISTER_NOTIFICATION);
        frame.push(0x00);
        frame.extend_from_slice(&5u16.to_be_bytes());
        frame.push(EVENT_VOLUME_CHANGED);
        frame.extend_from_slice(&[0, 0, 0, 0]); // playback interval
        let resp = handle_frame(&frame, 100).response.expect("response");
        assert_eq!(resp[3], RSP_INTERIM);
        assert_eq!(*resp.last().unwrap(), VOL_MAX); // 100% -> 0x7F
    }

    #[test]
    fn unit_info_is_answered_stable() {
        let frame = [0x00, 0x11, 0x0E, 0x01, SUBUNIT_UNIT, OP_UNIT_INFO, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF];
        let resp = handle_frame(&frame, 0).response.expect("response");
        assert_eq!(resp[3], RSP_STABLE);
        assert_eq!(resp[5], OP_UNIT_INFO);
    }

    #[test]
    fn passthrough_is_accepted() {
        // label 1, ctype control, panel, passthrough, play (0x44) press
        let frame = [0x10, 0x11, 0x0E, 0x00, SUBUNIT_PANEL, OP_PASSTHROUGH, 0x44, 0x00];
        let resp = handle_frame(&frame, 0).response.expect("response");
        assert_eq!(resp[3], RSP_ACCEPTED);
        assert_eq!(resp[5], OP_PASSTHROUGH);
    }

    #[test]
    fn responses_are_ignored() {
        // A frame already marked as a response must not be acted on.
        let frame = [0x12, 0x11, 0x0E, 0x09, SUBUNIT_PANEL, OP_VENDOR];
        assert_eq!(handle_frame(&frame, 0), Action::default());
    }

    #[test]
    fn short_or_foreign_frames_are_ignored() {
        assert_eq!(handle_frame(&[0x10], 0), Action::default());
        assert_eq!(handle_frame(&[], 0), Action::default());
        // Wrong PID (not AVRCP).
        let frame = [0x00, 0x12, 0x34, 0x01, SUBUNIT_PANEL, OP_UNIT_INFO];
        assert_eq!(handle_frame(&frame, 0), Action::default());
    }

    // ── Controller (CT) side ──

    #[test]
    fn get_element_attributes_command_requests_three_attrs() {
        let cmd = build_get_element_attributes(1);
        assert_eq!((cmd[0] >> 1) & 0x01, 0); // command (cr=0)
        assert_eq!(cmd[3], CTYPE_STATUS);
        assert_eq!(cmd[5], OP_VENDOR);
        assert_eq!(cmd[9], PDU_GET_ELEMENT_ATTRIBUTES);
        // params: identifier(8) + count(1)=3 + 3*id(4)
        let params = &cmd[13..];
        assert_eq!(params[8], 3);
    }

    #[test]
    fn parse_element_attributes_extracts_title_artist_album() {
        // count=3, then (id, charset=UTF-8(106), len, value) for title/artist/album.
        let mut p = vec![3u8];
        for (id, val) in [(ATTR_TITLE, "Song"), (ATTR_ARTIST, "Band"), (ATTR_ALBUM, "Disc")] {
            p.extend_from_slice(&id.to_be_bytes());
            p.extend_from_slice(&106u16.to_be_bytes()); // charset UTF-8
            p.extend_from_slice(&(val.len() as u16).to_be_bytes());
            p.extend_from_slice(val.as_bytes());
        }
        let t = parse_element_attributes(&p).expect("track");
        assert_eq!(t.title, "Song");
        assert_eq!(t.artist, "Band");
        assert_eq!(t.album, "Disc");
    }

    #[test]
    fn parse_element_attributes_is_bounds_safe() {
        // Truncated value must not panic.
        let p = [1u8, 0, 0, 0, 1, 0, 106, 0, 99]; // claims len 99 but no bytes
        assert!(parse_element_attributes(&p).is_none());
        assert!(parse_element_attributes(&[]).is_none());
    }

    #[test]
    fn ct_response_parses_metadata_and_track_changed() {
        // GetElementAttributes STABLE response with one title attr.
        let mut params = vec![1u8];
        params.extend_from_slice(&ATTR_TITLE.to_be_bytes());
        params.extend_from_slice(&106u16.to_be_bytes());
        params.extend_from_slice(&2u16.to_be_bytes());
        params.extend_from_slice(b"Hi");
        let mut frame = vec![0x12, 0x11, 0x0E, RSP_STABLE, SUBUNIT_PANEL, OP_VENDOR];
        frame.extend_from_slice(&BT_SIG_COMPANY);
        frame.push(PDU_GET_ELEMENT_ATTRIBUTES);
        frame.push(0x00);
        frame.extend_from_slice(&(params.len() as u16).to_be_bytes());
        frame.extend_from_slice(&params);
        match parse_ct_response(&frame) {
            CtEvent::Track(t) => assert_eq!(t.title, "Hi"),
            other => panic!("expected Track, got {:?}", other),
        }

        // RegisterNotification CHANGED for track -> re-query.
        let mut frame = vec![0x02, 0x11, 0x0E, RSP_CHANGED, SUBUNIT_PANEL, OP_VENDOR];
        frame.extend_from_slice(&BT_SIG_COMPANY);
        frame.push(PDU_REGISTER_NOTIFICATION);
        frame.push(0x00);
        frame.extend_from_slice(&1u16.to_be_bytes());
        frame.push(EVENT_TRACK_CHANGED);
        assert_eq!(parse_ct_response(&frame), CtEvent::TrackChanged);
    }

    #[test]
    fn passthrough_op_maps_keys() {
        assert_eq!(passthrough_op("play"), Some(OP_PLAY));
        assert_eq!(passthrough_op("pause"), Some(OP_PAUSE));
        assert_eq!(passthrough_op("next"), Some(OP_FORWARD));
        assert_eq!(passthrough_op("prev"), Some(OP_BACKWARD));
        assert_eq!(passthrough_op("bogus"), None);
    }

    #[test]
    fn passthrough_press_then_release_bit() {
        let press = build_passthrough(2, OP_PLAY, true);
        let release = build_passthrough(2, OP_PLAY, false);
        assert_eq!((press[0] >> 1) & 1, 0); // command (cr=0)
        assert_eq!(press[3], 0x00); // ctype CONTROL
        assert_eq!(press[5], OP_PASSTHROUGH);
        assert_eq!(press[6], OP_PLAY); // press: bit7 clear
        assert_eq!(release[6], OP_PLAY | 0x80); // release: bit7 set
        assert_eq!(press[7], 0x00); // operand data length
    }

    #[test]
    fn get_play_status_command_shape() {
        let cmd = build_get_play_status(3);
        assert_eq!((cmd[0] >> 1) & 1, 0); // command (cr=0)
        assert_eq!(cmd[3], CTYPE_STATUS);
        assert_eq!(cmd[9], PDU_GET_PLAY_STATUS);
    }

    #[test]
    fn parse_play_status_and_position() {
        // GetPlayStatus STABLE: length=240000ms position=42000ms status=playing.
        let mut params = Vec::new();
        params.extend_from_slice(&240_000u32.to_be_bytes());
        params.extend_from_slice(&42_000u32.to_be_bytes());
        params.push(0x01);
        let mut frame = vec![0x02, 0x11, 0x0E, RSP_STABLE, SUBUNIT_PANEL, OP_VENDOR];
        frame.extend_from_slice(&BT_SIG_COMPANY);
        frame.push(PDU_GET_PLAY_STATUS);
        frame.push(0x00);
        frame.extend_from_slice(&(params.len() as u16).to_be_bytes());
        frame.extend_from_slice(&params);
        assert_eq!(parse_ct_response(&frame), CtEvent::PlayStatus(240_000, 42_000));

        // PLAYBACK_POS_CHANGED notification: event(1) + position(4).
        let mut frame = vec![0x02, 0x11, 0x0E, RSP_CHANGED, SUBUNIT_PANEL, OP_VENDOR];
        frame.extend_from_slice(&BT_SIG_COMPANY);
        frame.push(PDU_REGISTER_NOTIFICATION);
        frame.push(0x00);
        frame.extend_from_slice(&5u16.to_be_bytes());
        frame.push(EVENT_PLAYBACK_POS_CHANGED);
        frame.extend_from_slice(&50_000u32.to_be_bytes());
        assert_eq!(parse_ct_response(&frame), CtEvent::Position(50_000));
    }

    #[test]
    fn volume_changed_notification_shape() {
        let f = build_volume_changed(3, 100);
        assert_eq!((f[0] >> 1) & 1, 1); // response (cr=1)
        assert_eq!(f[0] >> 4, 3); // label echoed
        assert_eq!(f[3], RSP_CHANGED);
        assert_eq!(f[9], PDU_REGISTER_NOTIFICATION);
        // params: [event_id, volume]
        assert_eq!(f[f.len() - 2], EVENT_VOLUME_CHANGED);
        assert_eq!(*f.last().unwrap(), VOL_MAX); // 100% -> 0x7F
    }
}
