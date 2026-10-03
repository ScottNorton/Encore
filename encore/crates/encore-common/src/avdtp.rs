//! AVDTP wire codec — pure PDU encode/decode + `A2dpCodec`, no I/O.
//!
//! Split out of `encore-firmware::bluetooth::avdtp` so it compiles and runs
//! its unit tests on any host (the firmware crate's `bluetooth` module is
//! `#[cfg(target_os = "linux")]`-gated end to end). The signaling state
//! machine and media-transport accept loop (`spawn_avdtp`, `avdtp_loop`,
//! `AvdtpState`, `AvdtpEvent`) stay in `encore-firmware`, which re-exports
//! this module's items under the same `avdtp::` path so no call site needed
//! to change. `A2dpCodec` itself moved here too and is re-exported from
//! `bluetooth::mod` (where callers reference it as `crate::bluetooth::A2dpCodec`
//! / `super::A2dpCodec`).

/// Which A2DP codec was negotiated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum A2dpCodec {
    Sbc,
    Aptx,
    AptxHd,
}

impl std::fmt::Display for A2dpCodec {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            A2dpCodec::Sbc => write!(f, "SBC"),
            A2dpCodec::Aptx => write!(f, "aptX"),
            A2dpCodec::AptxHd => write!(f, "aptX HD"),
        }
    }
}

// ── AVDTP signal IDs ──
pub const AVDTP_DISCOVER: u8 = 0x01;
pub const AVDTP_GET_CAPABILITIES: u8 = 0x02;
pub const AVDTP_SET_CONFIGURATION: u8 = 0x03;
pub const AVDTP_GET_CONFIGURATION: u8 = 0x04;
pub const AVDTP_RECONFIGURE: u8 = 0x05;
pub const AVDTP_OPEN: u8 = 0x06;
pub const AVDTP_START: u8 = 0x07;
pub const AVDTP_CLOSE: u8 = 0x08;
pub const AVDTP_SUSPEND: u8 = 0x09;
pub const AVDTP_ABORT: u8 = 0x0A;
pub const AVDTP_GET_ALL_CAPABILITIES: u8 = 0x0C;

// Message type bits (in byte 0, bits 1-0)
pub const MSG_TYPE_COMMAND: u8 = 0x00;
pub const MSG_TYPE_ACCEPT: u8 = 0x02;
pub const MSG_TYPE_REJECT: u8 = 0x03;

// AVDTP capability categories
pub const CAP_MEDIA_TRANSPORT: u8 = 0x01;
pub const CAP_MEDIA_CODEC: u8 = 0x07;

// Codec types
pub const CODEC_SBC: u8 = 0x00;
pub const CODEC_VENDOR: u8 = 0xFF;

// ── SEP endpoint definitions ──

/// SBC capabilities: all freqs, all modes, bitpool 2-53.
pub const SBC_CAPS: [u8; 4] = [0xFF, 0xFF, 2, 53];
/// aptX HD: Qualcomm vendor 0x00D7, codec 0x0024, 44.1/48kHz stereo.
pub const APTX_HD_CAPS: [u8; 7] = [0xD7, 0x00, 0x00, 0x00, 0x24, 0x00, 0x32];
/// aptX: Qualcomm vendor 0x004F, codec 0x0001, 44.1/48kHz stereo.
pub const APTX_CAPS: [u8; 7] = [0x4F, 0x00, 0x00, 0x00, 0x01, 0x00, 0x32];

/// Build an accept response.
pub fn build_accept(txn_label: u8, signal_id: u8, payload: &[u8]) -> Vec<u8> {
    let mut msg = Vec::with_capacity(2 + payload.len());
    msg.push((txn_label << 4) | MSG_TYPE_ACCEPT); // single packet, accept
    msg.push(signal_id & 0x3F); // signal_id in bits 5:0, RFA=0 in bits 7:6
    msg.extend_from_slice(payload);
    msg
}

/// Build a reject response.
pub fn build_reject(txn_label: u8, signal_id: u8, error: u8) -> Vec<u8> {
    vec![
        (txn_label << 4) | MSG_TYPE_REJECT,
        signal_id & 0x3F, // signal_id in bits 5:0, RFA=0 in bits 7:6
        error,
    ]
}

/// Handle DISCOVER: return SEP table (3 endpoints).
pub fn handle_discover(txn_label: u8) -> Vec<u8> {
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
pub fn handle_get_capabilities(txn_label: u8, signal_id: u8, seid: u8) -> Vec<u8> {
    let caps = match seid {
        1 => build_codec_caps(CODEC_VENDOR, &APTX_HD_CAPS),
        2 => build_codec_caps(CODEC_VENDOR, &APTX_CAPS),
        3 => build_codec_caps(CODEC_SBC, &SBC_CAPS),
        _ => return build_reject(txn_label, signal_id, 0x12), // BAD_ACP_SEID
    };
    build_accept(txn_label, signal_id, &caps)
}

/// Build capability bytes: MEDIA_TRANSPORT + MEDIA_CODEC.
pub fn build_codec_caps(codec_type: u8, codec_caps: &[u8]) -> Vec<u8> {
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
pub fn handle_set_configuration(txn_label: u8, payload: &[u8]) -> (Vec<u8>, Option<A2dpCodec>, u8) {
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
pub fn parse_codec_from_caps(caps: &[u8]) -> Option<A2dpCodec> {
    let mut pos = 0;
    while pos + 1 < caps.len() {
        let category = caps[pos];
        let losc = caps[pos + 1] as usize;
        // losc is attacker-declared; a truncated PDU can claim more bytes than are
        // present. Bound every field read against the real buffer length (never
        // against losc alone) or a malformed MEDIA_CODEC capability panics the
        // AVDTP thread and kills A2DP until reboot.
        if category == CAP_MEDIA_CODEC && losc >= 2 && pos + 3 < caps.len() {
            let codec_type = caps[pos + 3]; // skip media_type at pos+2
            if codec_type == CODEC_SBC {
                return Some(A2dpCodec::Sbc);
            } else if codec_type == CODEC_VENDOR && losc >= 8 && pos + 8 <= caps.len() {
                // Check vendor_id and codec_id
                let vendor = u32::from_le_bytes([
                    caps[pos + 4],
                    caps[pos + 5],
                    caps[pos + 6],
                    caps[pos + 7],
                ]);
                let cid = if losc >= 10 && pos + 10 <= caps.len() {
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
    fn parse_codec_from_caps_malformed_does_not_panic() {
        // A remote source can send a MEDIA_CODEC capability whose declared losc
        // overstates the bytes actually present. These previously indexed out of
        // bounds and panicked the AVDTP thread (remote A2DP denial of service).
        // losc=2 but no codec_type byte follows (would read caps[3]):
        assert_eq!(parse_codec_from_caps(&[CAP_MEDIA_CODEC, 0x02]), None);
        // vendor codec claims losc=8 but only media_type present (would read caps[4..8]):
        assert_eq!(
            parse_codec_from_caps(&[CAP_MEDIA_CODEC, 0x08, 0x00, CODEC_VENDOR]),
            None
        );
        // vendor with full vendor_id but losc=10 truncated before codec_id (caps[8..10]):
        assert_eq!(
            parse_codec_from_caps(&[
                CAP_MEDIA_CODEC,
                0x0A,
                0x00,
                CODEC_VENDOR,
                0xD7,
                0x00,
                0x00,
                0x00
            ]),
            None
        );
        // benign leading element with an overstated length must not panic either:
        assert_eq!(parse_codec_from_caps(&[CAP_MEDIA_TRANSPORT, 0xFF, 0x00]), None);
    }

    #[test]
    fn accept_message_format() {
        let msg = build_accept(0x0A, AVDTP_OPEN, &[]);
        assert_eq!(msg.len(), 2);
        assert_eq!(msg[0], (0x0A << 4) | MSG_TYPE_ACCEPT);
        assert_eq!(msg[1] & 0x3F, AVDTP_OPEN);
    }
}
