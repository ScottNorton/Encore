//! SDP wire codec — record data + PDU response builders, no I/O.
//!
//! Split out of `encore-firmware::bluetooth::sdp` so it compiles and runs its
//! unit tests on any host (the firmware crate's `bluetooth` module is
//! `#[cfg(target_os = "linux")]`-gated end to end). The socket/session I/O
//! layer (`spawn_sdp_server`, `handle_sdp_client`) stays in `encore-firmware`,
//! which re-exports this module's items under the same `sdp::` path so no
//! call site needed to change.

// SDP PDU IDs
pub const SDP_SVC_SEARCH_REQ: u8 = 0x02;
const SDP_SVC_SEARCH_RSP: u8 = 0x03;
pub const SDP_SVC_ATTR_REQ: u8 = 0x04;
const SDP_SVC_ATTR_RSP: u8 = 0x05;
pub const SDP_SVC_SEARCH_ATTR_REQ: u8 = 0x06;
const SDP_SVC_SEARCH_ATTR_RSP: u8 = 0x07;
const SDP_ERROR_RSP: u8 = 0x01;

/// A2DP Sink service record handle.
const HANDLE_A2DP: u32 = 0x00010001;
/// AVRCP Target service record handle.
const HANDLE_AVRCP: u32 = 0x00010002;

/// Pre-built A2DP Sink service record (a Data Element Sequence of attr pairs):
/// ServiceRecordHandle, ServiceClassIDList(0x110B), ProtocolDescriptorList
/// (L2CAP PSM 25 + AVDTP 1.3), BrowseGroupList, ProfileDescriptorList(0x110D
/// 1.3), SupportedFeatures.
static A2DP_RECORD: &[u8] = &[
    0x35, 64, // Outer SEQ, 64 bytes
    // Attr 0x0000 ServiceRecordHandle = UINT32(0x00010001)
    0x09, 0x00, 0x00, 0x0A, 0x00, 0x01, 0x00, 0x01,
    // Attr 0x0001 ServiceClassIDList = SEQ { UUID16(0x110B) }
    0x09, 0x00, 0x01, 0x35, 0x03, 0x19, 0x11, 0x0B,
    // Attr 0x0004 ProtocolDescriptorList = SEQ { SEQ{L2CAP,PSM25}, SEQ{AVDTP,v1.3} }
    0x09, 0x00, 0x04, 0x35, 0x10, 0x35, 0x06, 0x19, 0x01, 0x00, 0x09, 0x00, 0x19, 0x35, 0x06, 0x19,
    0x00, 0x19, 0x09, 0x01, 0x03,
    // Attr 0x0005 BrowseGroupList = SEQ { UUID16(0x1002) }
    0x09, 0x00, 0x05, 0x35, 0x03, 0x19, 0x10, 0x02,
    // Attr 0x0009 ProfileDescriptorList = SEQ { SEQ { UUID16(0x110D), UINT16(0x0103) } }
    0x09, 0x00, 0x09, 0x35, 0x08, 0x35, 0x06, 0x19, 0x11, 0x0D, 0x09, 0x01, 0x03,
    // Attr 0x0311 SupportedFeatures = UINT16(0x0001)
    0x09, 0x03, 0x11, 0x09, 0x00, 0x01,
];

/// Pre-built AVRCP Target service record:
/// ServiceClassIDList(0x110C), ProtocolDescriptorList (L2CAP PSM 23 / AVCTP 1.4),
/// BrowseGroupList, ProfileDescriptorList(AV Remote Control 0x110E, 1.4),
/// SupportedFeatures(0x0002 = Category 2, Monitor/Amplifier — absolute volume).
static AVRCP_TG_RECORD: &[u8] = &[
    0x35, 64, // Outer SEQ, 64 bytes
    // Attr 0x0000 ServiceRecordHandle = UINT32(0x00010002)
    0x09, 0x00, 0x00, 0x0A, 0x00, 0x01, 0x00, 0x02,
    // Attr 0x0001 ServiceClassIDList = SEQ { UUID16(0x110C) }
    0x09, 0x00, 0x01, 0x35, 0x03, 0x19, 0x11, 0x0C,
    // Attr 0x0004 ProtocolDescriptorList = SEQ { SEQ{L2CAP,PSM23}, SEQ{AVCTP,v1.4} }
    0x09, 0x00, 0x04, 0x35, 0x10, 0x35, 0x06, 0x19, 0x01, 0x00, 0x09, 0x00, 0x17, 0x35, 0x06, 0x19,
    0x00, 0x17, 0x09, 0x01, 0x04,
    // Attr 0x0005 BrowseGroupList = SEQ { UUID16(0x1002) }
    0x09, 0x00, 0x05, 0x35, 0x03, 0x19, 0x10, 0x02,
    // Attr 0x0009 ProfileDescriptorList = SEQ { SEQ { UUID16(0x110E), UINT16(0x0104) } }
    0x09, 0x00, 0x09, 0x35, 0x08, 0x35, 0x06, 0x19, 0x11, 0x0E, 0x09, 0x01, 0x04,
    // Attr 0x0311 SupportedFeatures = UINT16(0x0002)
    0x09, 0x03, 0x11, 0x09, 0x00, 0x02,
];

/// A served SDP record: its handle, the UUID16s it contains (for ServiceSearch
/// matching), and its pre-built attribute list.
pub struct SdpRecord {
    pub handle: u32,
    uuids: &'static [u16],
    attrs: &'static [u8],
}

pub static RECORDS: &[SdpRecord] = &[
    SdpRecord {
        handle: HANDLE_A2DP,
        // AudioSink, L2CAP, AVDTP, AdvancedAudioDistribution, BrowseRoot
        uuids: &[0x110B, 0x0100, 0x0019, 0x110D, 0x1002],
        attrs: A2DP_RECORD,
    },
    SdpRecord {
        handle: HANDLE_AVRCP,
        // AVRemoteControlTarget, AVRemoteControl, L2CAP, AVCTP, BrowseRoot
        uuids: &[0x110C, 0x110E, 0x0100, 0x0017, 0x1002],
        attrs: AVRCP_TG_RECORD,
    },
];

/// Build a ServiceSearchResponse (PDU 0x03): the handles of all matching records.
pub fn build_search_response(txn_id: u16, params: &[u8]) -> Vec<u8> {
    let recs = matching_records(params);
    let count = recs.len() as u16;

    let mut resp = Vec::with_capacity(9 + recs.len() * 4);
    resp.push(SDP_SVC_SEARCH_RSP);
    resp.extend_from_slice(&txn_id.to_be_bytes());
    let param_len = 2 + 2 + count * 4 + 1;
    resp.extend_from_slice(&param_len.to_be_bytes());
    resp.extend_from_slice(&count.to_be_bytes()); // TotalServiceRecordCount
    resp.extend_from_slice(&count.to_be_bytes()); // CurrentServiceRecordCount
    for r in &recs {
        resp.extend_from_slice(&r.handle.to_be_bytes());
    }
    resp.push(0x00); // ContinuationState = none
    resp
}

/// Build a ServiceAttributeResponse (PDU 0x05) for a single record handle.
pub fn build_attr_response(txn_id: u16, params: &[u8]) -> Vec<u8> {
    let handle = if params.len() >= 4 {
        u32::from_be_bytes([params[0], params[1], params[2], params[3]])
    } else {
        0
    };

    let mut resp = Vec::with_capacity(80);
    resp.push(SDP_SVC_ATTR_RSP);
    resp.extend_from_slice(&txn_id.to_be_bytes());

    if let Some(r) = RECORDS.iter().find(|r| r.handle == handle) {
        let byte_count = r.attrs.len() as u16;
        let param_len = 2 + byte_count + 1;
        resp.extend_from_slice(&param_len.to_be_bytes());
        resp.extend_from_slice(&byte_count.to_be_bytes());
        resp.extend_from_slice(r.attrs);
        resp.push(0x00);
    } else {
        resp.extend_from_slice(&3u16.to_be_bytes());
        resp.extend_from_slice(&0u16.to_be_bytes());
        resp.push(0x00);
    }
    resp
}

/// Build a ServiceSearchAttributeResponse (PDU 0x07): the attribute lists of all
/// matching records wrapped in an outer AttributeLists sequence.
pub fn build_search_attr_response(txn_id: u16, params: &[u8]) -> Vec<u8> {
    let recs = matching_records(params);

    let mut resp = Vec::with_capacity(160);
    resp.push(SDP_SVC_SEARCH_ATTR_RSP);
    resp.extend_from_slice(&txn_id.to_be_bytes());

    if recs.is_empty() {
        let param_len: u16 = 2 + 2 + 1;
        resp.extend_from_slice(&param_len.to_be_bytes());
        resp.extend_from_slice(&2u16.to_be_bytes()); // byte_count = 2
        resp.push(0x35);
        resp.push(0x00); // empty SEQ
        resp.push(0x00); // ContinuationState
        return resp;
    }

    let inner_len: usize = recs.iter().map(|r| r.attrs.len()).sum();
    let outer_header: Vec<u8> = if inner_len < 256 {
        vec![0x35, inner_len as u8]
    } else {
        vec![0x36, (inner_len >> 8) as u8, (inner_len & 0xFF) as u8]
    };
    let total = outer_header.len() + inner_len;
    let param_len = 2 + total + 1;

    resp.extend_from_slice(&(param_len as u16).to_be_bytes());
    resp.extend_from_slice(&(total as u16).to_be_bytes()); // AttributeListsByteCount
    resp.extend_from_slice(&outer_header);
    for r in &recs {
        resp.extend_from_slice(r.attrs);
    }
    resp.push(0x00); // ContinuationState = none
    resp
}

/// Build an ErrorResponse (PDU 0x01).
pub fn build_error_response(txn_id: u16, error_code: u16) -> Vec<u8> {
    let mut resp = Vec::with_capacity(7);
    resp.push(SDP_ERROR_RSP);
    resp.extend_from_slice(&txn_id.to_be_bytes());
    resp.extend_from_slice(&2u16.to_be_bytes());
    resp.extend_from_slice(&error_code.to_be_bytes());
    resp
}

/// Extract the UUID16s from an SDP ServiceSearchPattern (a DES of UUIDs).
fn parse_pattern_uuids(data: &[u8]) -> Vec<u16> {
    let mut out = Vec::new();
    let mut pos = 0;
    // Skip the outer SEQ header if present.
    if data.len() > 1 && (data[0] == 0x35 || data[0] == 0x36) {
        pos = if data[0] == 0x35 { 2 } else { 3 };
    }
    while pos < data.len() {
        match data[pos] {
            0x19 if pos + 2 < data.len() => {
                out.push(u16::from_be_bytes([data[pos + 1], data[pos + 2]]));
                pos += 3;
            }
            0x1A if pos + 4 < data.len() => {
                // UUID32: a 16-bit alias has its high half zero.
                let hi = u16::from_be_bytes([data[pos + 1], data[pos + 2]]);
                let lo = u16::from_be_bytes([data[pos + 3], data[pos + 4]]);
                if hi == 0 {
                    out.push(lo);
                }
                pos += 5;
            }
            _ => pos += 1,
        }
    }
    out
}

/// Records whose UUID set contains every UUID in the search pattern.
pub fn matching_records(params: &[u8]) -> Vec<&'static SdpRecord> {
    let pattern = parse_pattern_uuids(params);
    if pattern.is_empty() {
        return Vec::new();
    }
    RECORDS
        .iter()
        .filter(|r| pattern.iter().all(|u| r.uuids.contains(u)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_are_valid_des() {
        for r in RECORDS {
            assert_eq!(r.attrs[0], 0x35, "record {:08x} must start with SEQ", r.handle);
            let declared = r.attrs[1] as usize;
            assert_eq!(declared + 2, r.attrs.len(), "record {:08x} length", r.handle);
        }
    }

    #[test]
    fn audio_sink_search_returns_only_a2dp() {
        // SEQ { UUID16(0x110B) }
        let recs = matching_records(&[0x35, 0x03, 0x19, 0x11, 0x0B]);
        assert_eq!(recs.len(), 1);
        assert_eq!(recs[0].handle, HANDLE_A2DP);
    }

    #[test]
    fn avrcp_search_returns_only_avrcp() {
        // Search for AV Remote Control (0x110E) — a phone's typical AVRCP query.
        let recs = matching_records(&[0x35, 0x03, 0x19, 0x11, 0x0E]);
        assert_eq!(recs.len(), 1);
        assert_eq!(recs[0].handle, HANDLE_AVRCP);
        // And the Target class id directly.
        let recs = matching_records(&[0x35, 0x03, 0x19, 0x11, 0x0C]);
        assert_eq!(recs.len(), 1);
        assert_eq!(recs[0].handle, HANDLE_AVRCP);
    }

    #[test]
    fn browse_root_returns_both() {
        let recs = matching_records(&[0x35, 0x03, 0x19, 0x10, 0x02]);
        assert_eq!(recs.len(), 2);
    }

    #[test]
    fn unknown_uuid_matches_nothing() {
        let recs = matching_records(&[0x35, 0x03, 0x19, 0x12, 0x34]);
        assert!(recs.is_empty());
    }

    #[test]
    fn search_response_lists_both_handles_for_browse() {
        let resp = build_search_response(0x0042, &[0x35, 0x03, 0x19, 0x10, 0x02]);
        assert_eq!(resp[0], SDP_SVC_SEARCH_RSP);
        assert_eq!(u16::from_be_bytes([resp[5], resp[6]]), 2); // total count
        assert_eq!(u16::from_be_bytes([resp[7], resp[8]]), 2); // current count
    }

    #[test]
    fn attr_response_returns_avrcp_record() {
        let mut params = HANDLE_AVRCP.to_be_bytes().to_vec();
        params.extend_from_slice(&[0xFF, 0xFF]); // max bytes
        let resp = build_attr_response(0x0001, &params);
        assert_eq!(resp[0], SDP_SVC_ATTR_RSP);
        let byte_count = u16::from_be_bytes([resp[5], resp[6]]);
        assert_eq!(byte_count as usize, AVRCP_TG_RECORD.len());
    }

    #[test]
    fn search_attr_response_includes_avrcp() {
        let resp = build_search_attr_response(
            0x0001,
            &[0x35, 0x03, 0x19, 0x11, 0x0E, 0x35, 0x05, 0x0A, 0x00, 0x00, 0xFF, 0xFF],
        );
        assert_eq!(resp[0], SDP_SVC_SEARCH_ATTR_RSP);
        let byte_count = u16::from_be_bytes([resp[5], resp[6]]) as usize;
        assert!(byte_count >= AVRCP_TG_RECORD.len());
        assert_eq!(*resp.last().unwrap(), 0x00); // continuation
    }
}
