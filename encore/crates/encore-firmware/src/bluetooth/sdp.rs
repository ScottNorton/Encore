//! Minimal SDP server for A2DP Sink advertisement.
//!
//! Listens on L2CAP PSM 1 and responds to service search queries with a
//! pre-built A2DP Sink record. This replaces bluetoothd's built-in SDP server.

use super::l2cap;
use std::os::fd::{AsRawFd, OwnedFd};
use tracing::{debug, info, warn};

// SDP PDU IDs
const SDP_SVC_SEARCH_REQ: u8 = 0x02;
const SDP_SVC_SEARCH_RSP: u8 = 0x03;
const SDP_SVC_ATTR_REQ: u8 = 0x04;
const SDP_SVC_ATTR_RSP: u8 = 0x05;
const SDP_SVC_SEARCH_ATTR_REQ: u8 = 0x06;
const SDP_SVC_SEARCH_ATTR_RSP: u8 = 0x07;
const SDP_ERROR_RSP: u8 = 0x01;

/// Our service record handle.
const RECORD_HANDLE: u32 = 0x00010001;

/// Audio Sink UUID (0x110B).
const UUID_AUDIO_SINK: u16 = 0x110B;
/// Public Browse Root (0x1002) — used for SDP browsing.
const UUID_BROWSE_ROOT: u16 = 0x1002;
/// L2CAP UUID (0x0100).
const UUID_L2CAP: u16 = 0x0100;

/// Pre-built A2DP Sink service record as an SDP Attribute List.
///
/// Contains: ServiceRecordHandle, ServiceClassIDList, ProtocolDescriptorList,
/// BluetoothProfileDescriptorList, BrowseGroupList, SupportedFeatures.
///
/// Format: Data Element Sequence of (UINT16 attr_id, value) pairs.
static A2DP_RECORD: &[u8] = &[
    // Outer SEQ (length in next byte)
    0x35, 60,
    // Attr 0x0000 ServiceRecordHandle = UINT32(0x00010001)
    0x09, 0x00, 0x00, // UINT16 attr_id
    0x0A, 0x00, 0x01, 0x00, 0x01, // UINT32 value
    // Attr 0x0001 ServiceClassIDList = SEQ { UUID16(0x110B) }
    0x09, 0x00, 0x01,
    0x35, 0x03, 0x19, 0x11, 0x0B,
    // Attr 0x0004 ProtocolDescriptorList
    // = SEQ { SEQ { UUID16(L2CAP), UINT16(25) }, SEQ { UUID16(AVDTP), UINT16(0x0103) } }
    0x09, 0x00, 0x04,
    0x35, 0x10,
    0x35, 0x06, 0x19, 0x01, 0x00, 0x09, 0x00, 0x19, // L2CAP, PSM=25
    0x35, 0x06, 0x19, 0x00, 0x19, 0x09, 0x01, 0x03, // AVDTP, v1.3
    // Attr 0x0005 BrowseGroupList = SEQ { UUID16(0x1002) }
    0x09, 0x00, 0x05,
    0x35, 0x03, 0x19, 0x10, 0x02,
    // Attr 0x0009 BluetoothProfileDescriptorList
    // = SEQ { SEQ { UUID16(0x110D), UINT16(0x0103) } }
    0x09, 0x00, 0x09,
    0x35, 0x08, 0x35, 0x06, 0x19, 0x11, 0x0D, 0x09, 0x01, 0x03,
    // Attr 0x0311 SupportedFeatures = UINT16(0x0001)
    0x09, 0x03, 0x11,
    0x09, 0x00, 0x01,
];

/// Spawn the SDP server as a background task.
///
/// Accepts connections on the given L2CAP PSM 1 listener socket
/// and handles each client in a separate thread.
pub fn spawn_sdp_server(listener: OwnedFd) {
    std::thread::Builder::new()
        .name("bt-sdp-server".into())
        .spawn(move || {
            info!("SDP server: listening on PSM 1");
            loop {
                match l2cap::l2cap_accept(listener.as_raw_fd()) {
                    Ok((client_fd, addr)) => {
                        debug!(
                            "SDP: client from {}",
                            l2cap::bdaddr_to_string(&addr)
                        );
                        // Handle client synchronously (SDP is simple request-response)
                        handle_sdp_client(client_fd);
                    }
                    Err(e) => {
                        if e.kind() == std::io::ErrorKind::Interrupted {
                            continue;
                        }
                        warn!("SDP: accept error: {}", e);
                        // Brief backoff to avoid tight error loop
                        std::thread::sleep(std::time::Duration::from_millis(100));
                    }
                }
            }
        })
        .expect("failed to spawn SDP server thread");
}

/// Handle a single SDP client connection.
fn handle_sdp_client(fd: OwnedFd) {
    let raw = fd.as_raw_fd();
    let mut buf = [0u8; 512];

    // Read up to a few PDUs, then close
    for _ in 0..5 {
        let n = match l2cap::raw_read(raw, &mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) => {
                if e.kind() != std::io::ErrorKind::Interrupted {
                    debug!("SDP: read error: {}", e);
                }
                break;
            }
        };

        if n < 5 {
            break; // PDU header is at least 5 bytes
        }

        let pdu_id = buf[0];
        let txn_id = u16::from_be_bytes([buf[1], buf[2]]);
        // param_len at [3..5], then params

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

/// Build a ServiceSearchResponse (PDU 0x03).
///
/// Returns our record handle if the search pattern contains AudioSink, L2CAP,
/// or BrowseRoot UUID.
fn build_search_response(txn_id: u16, params: &[u8]) -> Vec<u8> {
    let matches = uuid_pattern_matches_a2dp(params);

    let mut resp = Vec::with_capacity(16);
    resp.push(SDP_SVC_SEARCH_RSP);
    resp.extend_from_slice(&txn_id.to_be_bytes());

    if matches {
        // param_len: 2 + 2 + 4 + 1 = 9
        resp.extend_from_slice(&9u16.to_be_bytes());
        resp.extend_from_slice(&1u16.to_be_bytes()); // TotalServiceRecordCount
        resp.extend_from_slice(&1u16.to_be_bytes()); // CurrentServiceRecordCount
        resp.extend_from_slice(&RECORD_HANDLE.to_be_bytes());
        resp.push(0x00); // ContinuationState = none
    } else {
        // param_len: 2 + 2 + 1 = 5
        resp.extend_from_slice(&5u16.to_be_bytes());
        resp.extend_from_slice(&0u16.to_be_bytes()); // Total = 0
        resp.extend_from_slice(&0u16.to_be_bytes()); // Current = 0
        resp.push(0x00);
    }
    resp
}

/// Build a ServiceAttributeResponse (PDU 0x05).
fn build_attr_response(txn_id: u16, params: &[u8]) -> Vec<u8> {
    // params: handle(4) + max_bytes(2) + attr_id_list + continuation
    let handle = if params.len() >= 4 {
        u32::from_be_bytes([params[0], params[1], params[2], params[3]])
    } else {
        0
    };

    let mut resp = Vec::with_capacity(A2DP_RECORD.len() + 10);
    resp.push(SDP_SVC_ATTR_RSP);
    resp.extend_from_slice(&txn_id.to_be_bytes());

    if handle == RECORD_HANDLE {
        let byte_count = A2DP_RECORD.len() as u16;
        let param_len = 2 + A2DP_RECORD.len() as u16 + 1; // byte_count + data + continuation
        resp.extend_from_slice(&param_len.to_be_bytes());
        resp.extend_from_slice(&byte_count.to_be_bytes());
        resp.extend_from_slice(A2DP_RECORD);
        resp.push(0x00); // ContinuationState = none
    } else {
        // Unknown handle — return empty
        resp.extend_from_slice(&3u16.to_be_bytes()); // param_len
        resp.extend_from_slice(&0u16.to_be_bytes()); // byte_count = 0
        resp.push(0x00);
    }
    resp
}

/// Build a ServiceSearchAttributeResponse (PDU 0x07).
fn build_search_attr_response(txn_id: u16, params: &[u8]) -> Vec<u8> {
    let matches = uuid_pattern_matches_a2dp(params);

    let mut resp = Vec::with_capacity(A2DP_RECORD.len() + 16);
    resp.push(SDP_SVC_SEARCH_ATTR_RSP);
    resp.extend_from_slice(&txn_id.to_be_bytes());

    if matches {
        // Wrap the record in an outer SEQ (AttributeLists is a SEQ of AttributeList)
        let inner_len = A2DP_RECORD.len();
        let outer_header = if inner_len < 256 {
            vec![0x35, inner_len as u8]
        } else {
            vec![0x36, (inner_len >> 8) as u8, (inner_len & 0xFF) as u8]
        };
        let total = outer_header.len() + inner_len;
        let param_len = 2 + total + 1; // byte_count + data + continuation

        resp.extend_from_slice(&(param_len as u16).to_be_bytes());
        resp.extend_from_slice(&(total as u16).to_be_bytes()); // AttributeListsByteCount
        resp.extend_from_slice(&outer_header);
        resp.extend_from_slice(A2DP_RECORD);
        resp.push(0x00); // ContinuationState = none
    } else {
        // No match — empty outer SEQ
        let param_len: u16 = 2 + 2 + 1; // byte_count(2) + empty_seq(2) + continuation(1)
        resp.extend_from_slice(&param_len.to_be_bytes());
        resp.extend_from_slice(&2u16.to_be_bytes()); // byte_count = 2
        resp.push(0x35);
        resp.push(0x00); // empty SEQ
        resp.push(0x00); // ContinuationState
    }
    resp
}

/// Build an ErrorResponse (PDU 0x01).
fn build_error_response(txn_id: u16, error_code: u16) -> Vec<u8> {
    let mut resp = Vec::with_capacity(7);
    resp.push(SDP_ERROR_RSP);
    resp.extend_from_slice(&txn_id.to_be_bytes());
    resp.extend_from_slice(&2u16.to_be_bytes()); // param_len
    resp.extend_from_slice(&error_code.to_be_bytes());
    resp
}

/// Check if an SDP ServiceSearchPattern contains UUIDs relevant to A2DP.
///
/// The pattern is a Data Element Sequence of UUIDs. We do a simple scan
/// for UUID16 values that match AudioSink, BrowseRoot, or L2CAP.
fn uuid_pattern_matches_a2dp(data: &[u8]) -> bool {
    // Quick scan: look for UUID16 data elements (type descriptor 0x19)
    // followed by a matching 2-byte UUID.
    let mut pos = 0;
    // Skip the outer SEQ header if present
    if data.len() > 2 && (data[0] == 0x35 || data[0] == 0x36) {
        pos = if data[0] == 0x35 { 2 } else { 3 };
    }

    while pos + 2 < data.len() {
        if data[pos] == 0x19 {
            // UUID16: next 2 bytes are the UUID
            let uuid = u16::from_be_bytes([data[pos + 1], data[pos + 2]]);
            if uuid == UUID_AUDIO_SINK || uuid == UUID_BROWSE_ROOT || uuid == UUID_L2CAP {
                return true;
            }
            pos += 3;
        } else {
            // Skip unknown data elements
            pos += 1;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a2dp_record_is_valid_des() {
        // First byte should be SEQ descriptor (0x35 for 1-byte length)
        assert_eq!(A2DP_RECORD[0], 0x35);
        let declared_len = A2DP_RECORD[1] as usize;
        assert_eq!(declared_len + 2, A2DP_RECORD.len());
    }

    #[test]
    fn uuid_pattern_matches_audio_sink() {
        // SEQ { UUID16(0x110B) }
        let pattern = [0x35, 0x03, 0x19, 0x11, 0x0B];
        assert!(uuid_pattern_matches_a2dp(&pattern));
    }

    #[test]
    fn uuid_pattern_matches_browse_root() {
        let pattern = [0x35, 0x03, 0x19, 0x10, 0x02];
        assert!(uuid_pattern_matches_a2dp(&pattern));
    }

    #[test]
    fn uuid_pattern_no_match() {
        // UUID16(0x110E) = AVRCP Target — we don't advertise this
        let pattern = [0x35, 0x03, 0x19, 0x11, 0x0E];
        assert!(!uuid_pattern_matches_a2dp(&pattern));
    }

    #[test]
    fn search_response_match() {
        let resp = build_search_response(0x0042, &[0x35, 0x03, 0x19, 0x11, 0x0B, 0x00, 0x01, 0x00]);
        assert_eq!(resp[0], SDP_SVC_SEARCH_RSP);
        assert_eq!(u16::from_be_bytes([resp[1], resp[2]]), 0x0042);
        // Should contain 1 record
        assert_eq!(u16::from_be_bytes([resp[5], resp[6]]), 1); // total
    }

    #[test]
    fn search_response_no_match() {
        let resp = build_search_response(0x0001, &[0x35, 0x03, 0x19, 0x11, 0x0E, 0x00, 0x01, 0x00]);
        assert_eq!(u16::from_be_bytes([resp[5], resp[6]]), 0); // total = 0
    }

    #[test]
    fn search_attr_response_structure() {
        let resp = build_search_attr_response(0x0001, &[0x35, 0x03, 0x19, 0x11, 0x0B, 0x00, 0xFF, 0x35, 0x05, 0x0A, 0x00, 0x00, 0xFF, 0xFF, 0x00]);
        assert_eq!(resp[0], SDP_SVC_SEARCH_ATTR_RSP);
        // Should have non-zero AttributeListsByteCount
        let byte_count = u16::from_be_bytes([resp[5], resp[6]]);
        assert!(byte_count > 0);
        // Last byte should be continuation state = 0
        assert_eq!(*resp.last().unwrap(), 0x00);
    }
}
