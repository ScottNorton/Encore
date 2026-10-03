//! Minimal SDP server for A2DP Sink + AVRCP Target advertisement — socket I/O layer.
//!
//! Listens on L2CAP PSM 1 and answers service search / attribute queries from a
//! small static table of records. This replaces bluetoothd's built-in SDP
//! server. Two records: A2DP Sink (so sources stream audio to us) and AVRCP
//! Target (so a source's volume slider can drive our master volume).
//!
//! The record data and PDU response builders are pure and live in
//! `encore_common::sdp`, re-exported here under the same `sdp::` path so
//! callers in this crate don't need to change. It was split out so those unit
//! tests run on any host — this module is `#[cfg(target_os = "linux")]`-gated
//! end to end (raw sockets), which meant they never ran on a non-Linux dev
//! machine.

pub use encore_common::sdp::*;

use super::l2cap;
use std::os::fd::{AsRawFd, OwnedFd};
use tracing::{debug, info, warn};

/// Spawn the SDP server as a background thread.
pub fn spawn_sdp_server(listener: OwnedFd) {
    std::thread::Builder::new()
        .name("bt-sdp-server".into())
        .spawn(move || {
            info!("SDP server: listening on PSM 1 ({} records)", RECORDS.len());
            loop {
                match l2cap::l2cap_accept(listener.as_raw_fd()) {
                    Ok((client_fd, addr)) => {
                        debug!("SDP: client from {}", l2cap::bdaddr_to_string(&addr));
                        handle_sdp_client(client_fd);
                    }
                    Err(e) => {
                        if e.kind() == std::io::ErrorKind::Interrupted {
                            continue;
                        }
                        warn!("SDP: accept error: {}", e);
                        std::thread::sleep(std::time::Duration::from_millis(100));
                    }
                }
            }
        })
        .expect("failed to spawn SDP server thread");
}

/// Handle a single SDP client connection (simple request-response).
///
/// Reads are non-blocking with an idle timeout: a peer that connects but stops
/// sending (without closing) is abandoned rather than parking the single SDP
/// accept thread forever — which would block service discovery for everyone
/// else, gating A2DP.
fn handle_sdp_client(fd: OwnedFd) {
    let raw = fd.as_raw_fd();
    l2cap::set_nonblocking(raw).ok();
    let mut buf = [0u8; 512];
    const IDLE_LIMIT_MS: u64 = 500;

    for _ in 0..5 {
        let mut idle = 0u64;
        let n = loop {
            match l2cap::raw_read(raw, &mut buf) {
                Ok(0) => return, // EOF: client closed
                Ok(n) => break n,
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    if idle >= IDLE_LIMIT_MS {
                        return; // idle too long — give the thread back
                    }
                    std::thread::sleep(std::time::Duration::from_millis(20));
                    idle += 20;
                }
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                Err(e) => {
                    debug!("SDP: read error: {}", e);
                    return;
                }
            }
        };

        if n < 5 {
            break; // PDU header is at least 5 bytes
        }

        let pdu_id = buf[0];
        let txn_id = u16::from_be_bytes([buf[1], buf[2]]);

        let response = match pdu_id {
            SDP_SVC_SEARCH_REQ => build_search_response(txn_id, &buf[5..n]),
            SDP_SVC_ATTR_REQ => build_attr_response(txn_id, &buf[5..n]),
            SDP_SVC_SEARCH_ATTR_REQ => build_search_attr_response(txn_id, &buf[5..n]),
            _ => build_error_response(txn_id, 0x0003), // Invalid Request Syntax
        };

        if l2cap::raw_write(raw, &response).is_err() {
            break;
        }
    }
}
