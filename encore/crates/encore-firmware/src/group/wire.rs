//! Binary wire protocol for inter-speaker group communication.
//!
//! Little-endian format (matches ARM native byte order). Each message
//! starts with a 12-byte header, followed by a variable-length payload.
//!
//! Header layout:
//!   [0..2]   magic "HG"
//!   [2]      version (1)
//!   [3]      packet_type
//!   [4..8]   sequence (u32)
//!   [8..12]  payload_len (u32)

use std::io;

/// Wire protocol magic bytes.
const MAGIC: [u8; 2] = *b"HG";
/// Wire protocol version.
const VERSION: u8 = 1;
/// Maximum payload size (1 MB) — prevents OOM on malformed packets.
const MAX_PAYLOAD: u32 = 1_048_576;

/// Header size in bytes.
pub const HEADER_SIZE: usize = 12;

/// Frames of stereo PCM carried in one [`GroupPacket::AudioChunk`].
///
/// Sized so a whole encoded AudioChunk fits in a SINGLE network MTU. An
/// oversized UDP datagram is IP-fragmented, and on a lossy WiFi link losing any
/// one fragment drops the entire chunk (reassembly is all-or-nothing) — that is
/// what turned grouped playback into near-constant static (a 480-frame chunk
/// encoded to ~3871 bytes = 3 fragments, ~88% chunk loss measured on-device).
///
/// 144 frames -> 12 (header) + 19 (audio fields) + 144*2*4 = 1183-byte datagram,
/// comfortably under a 1500 MTU (and reduced-MTU VPN/PPPoE paths). 144 is a
/// multiple of 6, so the per-chunk duration is an exact integer microsecond
/// count (144 * 1_000_000 / 48_000 = 3000), avoiding playout-timeline drift.
/// The receiver (`follower.rs`) and sender (`leader.rs`) MUST agree on this.
pub const AUDIO_CHUNK_FRAMES: usize = 144;
/// Samples per AudioChunk (stereo interleaved L R L R ...).
pub const AUDIO_CHUNK_SAMPLES: usize = AUDIO_CHUNK_FRAMES * 2;
/// Duration of one AudioChunk in microseconds (exact for multiples of 6 frames).
pub const AUDIO_CHUNK_DUR_US: u64 = AUDIO_CHUNK_FRAMES as u64 * 1_000_000 / 48_000;

/// Packet types on the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum PacketType {
    ClockSyncReq = 1,
    ClockSyncResp = 2,
    LeaderClaim = 3,
    LeaderRelease = 4,
    AudioChunk = 5,
    GroupConfig = 6,
    Heartbeat = 7,
    PeerInfo = 8,
    PeerGossip = 9,
    ElectionStart = 10,
    ElectionVote = 11,
    ElectionResult = 12,
    HealthPing = 13,
    RelayAssignment = 14,
    VolumeSync = 15,
    PlayPause = 16,
}

impl PacketType {
    fn from_u8(v: u8) -> Option<Self> {
        match v {
            1 => Some(Self::ClockSyncReq),
            2 => Some(Self::ClockSyncResp),
            3 => Some(Self::LeaderClaim),
            4 => Some(Self::LeaderRelease),
            5 => Some(Self::AudioChunk),
            6 => Some(Self::GroupConfig),
            7 => Some(Self::Heartbeat),
            8 => Some(Self::PeerInfo),
            9 => Some(Self::PeerGossip),
            10 => Some(Self::ElectionStart),
            11 => Some(Self::ElectionVote),
            12 => Some(Self::ElectionResult),
            13 => Some(Self::HealthPing),
            14 => Some(Self::RelayAssignment),
            15 => Some(Self::VolumeSync),
            16 => Some(Self::PlayPause),
            _ => None,
        }
    }
}

/// Channel assignment for a speaker in the group.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize, Default)]
pub enum ChannelAssignment {
    #[default]
    Stereo = 0,
    Left = 1,
    Right = 2,
}

impl ChannelAssignment {
    pub fn from_u8(v: u8) -> Self {
        match v {
            1 => Self::Left,
            2 => Self::Right,
            _ => Self::Stereo,
        }
    }
}

/// Entry in a gossip peer exchange.
#[derive(Debug, Clone)]
pub struct GossipEntry {
    pub peer_id: String,
    pub address: std::net::IpAddr,
    pub port: u16,
    pub group_name: String,
    pub channel: ChannelAssignment,
}

/// A decoded wire protocol message.
#[derive(Debug, Clone)]
pub enum GroupPacket {
    ClockSyncReq {
        originate_us: u64,
    },
    ClockSyncResp {
        originate_us: u64,
        receive_us: u64,
        transmit_us: u64,
    },
    LeaderClaim {
        source: String,
        claim_time_us: u64,
    },
    LeaderRelease,
    AudioChunk {
        seq: u32,
        /// Stable small id of the leader that produced this chunk (FNV of its
        /// peer_id). Followers accept only chunks from their current leader, so
        /// a brief two-leaders window can never merge two streams.
        leader_id: u32,
        play_at_us: u64,
        frame_count: u16,
        hop_count: u8,
        pcm: Vec<i32>,
    },
    GroupConfig {
        channel: ChannelAssignment,
        buffer_ms: u16,
    },
    Heartbeat,
    PeerInfo {
        name: String,
        channel: ChannelAssignment,
    },
    PeerGossip {
        peers: Vec<GossipEntry>,
    },
    ElectionStart {
        election_id: u32,
        trigger: String,
    },
    ElectionVote {
        election_id: u32,
        score: u64,
        peer_id: String,
    },
    ElectionResult {
        election_id: u32,
        winner_id: String,
        source: String,
    },
    HealthPing {
        avg_rtt_us: u64,
        uptime_secs: u64,
        active_sources: u8,
        buffer_health: u8,
        packet_loss_pct: u8,
    },
    RelayAssignment {
        target_peer_id: String,
        relay_peer_id: String,
    },
    VolumeSync {
        master_volume: u8,
        originator: String,
    },
    PlayPause {
        source: String,
        action: String,
    },
}

/// Encode a GroupPacket into wire format (header + payload).
///
/// The header carries a `seq` for every packet type. For [`GroupPacket::AudioChunk`]
/// the payload also carries its own `seq`, and the UDP audio receiver decodes
/// timing solely from the payload (it ignores the header `seq`). Callers stream
/// audio with the same value in both fields; the header `seq` is informational
/// (e.g. capture/debug) for that type, not the field the jitter buffer keys on.
pub fn encode(packet: &GroupPacket, seq: u32) -> Vec<u8> {
    let (ptype, payload) = encode_typed(packet);

    let mut out = Vec::with_capacity(HEADER_SIZE + payload.len());
    out.extend_from_slice(&MAGIC);
    out.push(VERSION);
    out.push(ptype as u8);
    out.extend_from_slice(&seq.to_le_bytes());
    out.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    out.extend_from_slice(&payload);
    out
}

/// Encode just the payload bytes of a packet (no header), the inverse of
/// [`decode_payload`].
pub fn encode_payload(packet: &GroupPacket) -> Vec<u8> {
    encode_typed(packet).1
}

/// Build the `(PacketType, payload)` pair for a packet.
fn encode_typed(packet: &GroupPacket) -> (PacketType, Vec<u8>) {
    match packet {
        GroupPacket::ClockSyncReq { originate_us } => (
            PacketType::ClockSyncReq,
            originate_us.to_le_bytes().to_vec(),
        ),
        GroupPacket::ClockSyncResp {
            originate_us,
            receive_us,
            transmit_us,
        } => {
            let mut p = Vec::with_capacity(24);
            p.extend_from_slice(&originate_us.to_le_bytes());
            p.extend_from_slice(&receive_us.to_le_bytes());
            p.extend_from_slice(&transmit_us.to_le_bytes());
            (PacketType::ClockSyncResp, p)
        }
        GroupPacket::LeaderClaim {
            source,
            claim_time_us,
        } => {
            let src_bytes = source.as_bytes();
            let mut p = Vec::with_capacity(2 + src_bytes.len() + 8);
            p.extend_from_slice(&(src_bytes.len() as u16).to_le_bytes());
            p.extend_from_slice(src_bytes);
            p.extend_from_slice(&claim_time_us.to_le_bytes());
            (PacketType::LeaderClaim, p)
        }
        GroupPacket::LeaderRelease => (PacketType::LeaderRelease, Vec::new()),
        GroupPacket::AudioChunk {
            seq,
            leader_id,
            play_at_us,
            frame_count,
            hop_count,
            pcm,
        } => {
            let mut p = Vec::with_capacity(19 + pcm.len() * 4);
            p.extend_from_slice(&seq.to_le_bytes());
            p.extend_from_slice(&leader_id.to_le_bytes());
            p.extend_from_slice(&play_at_us.to_le_bytes());
            p.extend_from_slice(&frame_count.to_le_bytes());
            p.push(*hop_count);
            for &sample in pcm {
                p.extend_from_slice(&sample.to_le_bytes());
            }
            (PacketType::AudioChunk, p)
        }
        GroupPacket::GroupConfig { channel, buffer_ms } => {
            let mut p = Vec::with_capacity(3);
            p.push(*channel as u8);
            p.extend_from_slice(&buffer_ms.to_le_bytes());
            (PacketType::GroupConfig, p)
        }
        GroupPacket::Heartbeat => (PacketType::Heartbeat, Vec::new()),
        GroupPacket::PeerInfo { name, channel } => {
            let name_bytes = name.as_bytes();
            let mut p = Vec::with_capacity(3 + name_bytes.len());
            p.extend_from_slice(&(name_bytes.len() as u16).to_le_bytes());
            p.extend_from_slice(name_bytes);
            p.push(*channel as u8);
            (PacketType::PeerInfo, p)
        }
        GroupPacket::PeerGossip { peers } => {
            let mut p = Vec::new();
            p.extend_from_slice(&(peers.len() as u16).to_le_bytes());
            for entry in peers {
                let id_bytes = entry.peer_id.as_bytes();
                p.extend_from_slice(&(id_bytes.len() as u16).to_le_bytes());
                p.extend_from_slice(id_bytes);
                match entry.address {
                    std::net::IpAddr::V4(ip) => p.extend_from_slice(&ip.octets()),
                    std::net::IpAddr::V6(_) => p.extend_from_slice(&[0u8; 4]),
                }
                p.extend_from_slice(&entry.port.to_le_bytes());
                let group_bytes = entry.group_name.as_bytes();
                p.extend_from_slice(&(group_bytes.len() as u16).to_le_bytes());
                p.extend_from_slice(group_bytes);
                p.push(entry.channel as u8);
            }
            (PacketType::PeerGossip, p)
        }
        GroupPacket::ElectionStart {
            election_id,
            trigger,
        } => {
            let trig_bytes = trigger.as_bytes();
            let mut p = Vec::with_capacity(6 + trig_bytes.len());
            p.extend_from_slice(&election_id.to_le_bytes());
            p.extend_from_slice(&(trig_bytes.len() as u16).to_le_bytes());
            p.extend_from_slice(trig_bytes);
            (PacketType::ElectionStart, p)
        }
        GroupPacket::ElectionVote {
            election_id,
            score,
            peer_id,
        } => {
            let id_bytes = peer_id.as_bytes();
            let mut p = Vec::with_capacity(14 + id_bytes.len());
            p.extend_from_slice(&election_id.to_le_bytes());
            p.extend_from_slice(&score.to_le_bytes());
            p.extend_from_slice(&(id_bytes.len() as u16).to_le_bytes());
            p.extend_from_slice(id_bytes);
            (PacketType::ElectionVote, p)
        }
        GroupPacket::ElectionResult {
            election_id,
            winner_id,
            source,
        } => {
            let winner_bytes = winner_id.as_bytes();
            let source_bytes = source.as_bytes();
            let mut p = Vec::with_capacity(8 + winner_bytes.len() + source_bytes.len());
            p.extend_from_slice(&election_id.to_le_bytes());
            p.extend_from_slice(&(winner_bytes.len() as u16).to_le_bytes());
            p.extend_from_slice(winner_bytes);
            p.extend_from_slice(&(source_bytes.len() as u16).to_le_bytes());
            p.extend_from_slice(source_bytes);
            (PacketType::ElectionResult, p)
        }
        GroupPacket::HealthPing {
            avg_rtt_us,
            uptime_secs,
            active_sources,
            buffer_health,
            packet_loss_pct,
        } => {
            let mut p = Vec::with_capacity(19);
            p.extend_from_slice(&avg_rtt_us.to_le_bytes());
            p.extend_from_slice(&uptime_secs.to_le_bytes());
            p.push(*active_sources);
            p.push(*buffer_health);
            p.push(*packet_loss_pct);
            (PacketType::HealthPing, p)
        }
        GroupPacket::RelayAssignment {
            target_peer_id,
            relay_peer_id,
        } => {
            let target_bytes = target_peer_id.as_bytes();
            let relay_bytes = relay_peer_id.as_bytes();
            let mut p = Vec::with_capacity(4 + target_bytes.len() + relay_bytes.len());
            p.extend_from_slice(&(target_bytes.len() as u16).to_le_bytes());
            p.extend_from_slice(target_bytes);
            p.extend_from_slice(&(relay_bytes.len() as u16).to_le_bytes());
            p.extend_from_slice(relay_bytes);
            (PacketType::RelayAssignment, p)
        }
        GroupPacket::VolumeSync {
            master_volume,
            originator,
        } => {
            let orig_bytes = originator.as_bytes();
            let mut p = Vec::with_capacity(3 + orig_bytes.len());
            p.push(*master_volume);
            p.extend_from_slice(&(orig_bytes.len() as u16).to_le_bytes());
            p.extend_from_slice(orig_bytes);
            (PacketType::VolumeSync, p)
        }
        GroupPacket::PlayPause { source, action } => {
            let src_bytes = source.as_bytes();
            let act_bytes = action.as_bytes();
            let mut p = Vec::with_capacity(4 + src_bytes.len() + act_bytes.len());
            p.extend_from_slice(&(src_bytes.len() as u16).to_le_bytes());
            p.extend_from_slice(src_bytes);
            p.extend_from_slice(&(act_bytes.len() as u16).to_le_bytes());
            p.extend_from_slice(act_bytes);
            (PacketType::PlayPause, p)
        }
    }
}

/// Read exactly one header from a byte stream. Returns (packet_type, sequence, payload_len).
/// Returns None for unknown packet types (forward compatibility — reader should skip payload).
pub fn read_header(header: &[u8; HEADER_SIZE]) -> io::Result<(Option<PacketType>, u32, u32)> {
    if header[0..2] != MAGIC {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "bad magic"));
    }
    if header[2] != VERSION {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "unsupported version",
        ));
    }
    let ptype = PacketType::from_u8(header[3]);
    let seq = u32::from_le_bytes([header[4], header[5], header[6], header[7]]);
    let payload_len = u32::from_le_bytes([header[8], header[9], header[10], header[11]]);
    if payload_len > MAX_PAYLOAD {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "payload too large",
        ));
    }
    Ok((ptype, seq, payload_len))
}

/// Decode a payload buffer into a GroupPacket, given the packet type.
pub fn decode_payload(ptype: PacketType, payload: &[u8]) -> io::Result<GroupPacket> {
    match ptype {
        PacketType::ClockSyncReq => {
            if payload.len() < 8 {
                return Err(io::Error::new(io::ErrorKind::InvalidData, "short payload"));
            }
            let originate_us = u64::from_le_bytes(payload[0..8].try_into().unwrap());
            Ok(GroupPacket::ClockSyncReq { originate_us })
        }
        PacketType::ClockSyncResp => {
            if payload.len() < 24 {
                return Err(io::Error::new(io::ErrorKind::InvalidData, "short payload"));
            }
            Ok(GroupPacket::ClockSyncResp {
                originate_us: u64::from_le_bytes(payload[0..8].try_into().unwrap()),
                receive_us: u64::from_le_bytes(payload[8..16].try_into().unwrap()),
                transmit_us: u64::from_le_bytes(payload[16..24].try_into().unwrap()),
            })
        }
        PacketType::LeaderClaim => {
            if payload.len() < 10 {
                return Err(io::Error::new(io::ErrorKind::InvalidData, "short payload"));
            }
            let src_len = u16::from_le_bytes(payload[0..2].try_into().unwrap()) as usize;
            if payload.len() < 2 + src_len + 8 {
                return Err(io::Error::new(io::ErrorKind::InvalidData, "short payload"));
            }
            let source = String::from_utf8_lossy(&payload[2..2 + src_len]).into_owned();
            let claim_time_us =
                u64::from_le_bytes(payload[2 + src_len..2 + src_len + 8].try_into().unwrap());
            Ok(GroupPacket::LeaderClaim {
                source,
                claim_time_us,
            })
        }
        PacketType::LeaderRelease => Ok(GroupPacket::LeaderRelease),
        PacketType::AudioChunk => {
            if payload.len() < 19 {
                return Err(io::Error::new(io::ErrorKind::InvalidData, "short payload"));
            }
            let seq = u32::from_le_bytes(payload[0..4].try_into().unwrap());
            let leader_id = u32::from_le_bytes(payload[4..8].try_into().unwrap());
            let play_at_us = u64::from_le_bytes(payload[8..16].try_into().unwrap());
            let frame_count = u16::from_le_bytes(payload[16..18].try_into().unwrap());
            let hop_count = payload[18];
            let pcm_bytes = &payload[19..];
            if !pcm_bytes.len().is_multiple_of(4) {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "PCM not aligned",
                ));
            }
            let pcm: Vec<i32> = pcm_bytes
                .chunks_exact(4)
                .map(|c| i32::from_le_bytes(c.try_into().unwrap()))
                .collect();
            Ok(GroupPacket::AudioChunk {
                seq,
                leader_id,
                play_at_us,
                frame_count,
                hop_count,
                pcm,
            })
        }
        PacketType::GroupConfig => {
            if payload.len() < 3 {
                return Err(io::Error::new(io::ErrorKind::InvalidData, "short payload"));
            }
            Ok(GroupPacket::GroupConfig {
                channel: ChannelAssignment::from_u8(payload[0]),
                buffer_ms: u16::from_le_bytes(payload[1..3].try_into().unwrap()),
            })
        }
        PacketType::Heartbeat => Ok(GroupPacket::Heartbeat),
        PacketType::PeerInfo => {
            if payload.len() < 3 {
                return Err(io::Error::new(io::ErrorKind::InvalidData, "short payload"));
            }
            let name_len = u16::from_le_bytes(payload[0..2].try_into().unwrap()) as usize;
            if payload.len() < 2 + name_len + 1 {
                return Err(io::Error::new(io::ErrorKind::InvalidData, "short payload"));
            }
            let name = String::from_utf8_lossy(&payload[2..2 + name_len]).into_owned();
            let channel = ChannelAssignment::from_u8(payload[2 + name_len]);
            Ok(GroupPacket::PeerInfo { name, channel })
        }
        PacketType::PeerGossip => {
            if payload.len() < 2 {
                return Err(io::Error::new(io::ErrorKind::InvalidData, "short payload"));
            }
            let count = u16::from_le_bytes(payload[0..2].try_into().unwrap()) as usize;
            let mut pos = 2;
            let mut peers = Vec::with_capacity(count);
            for _ in 0..count {
                if pos + 2 > payload.len() {
                    break;
                }
                let id_len = u16::from_le_bytes(payload[pos..pos + 2].try_into().unwrap()) as usize;
                pos += 2;
                if pos + id_len + 4 + 2 + 2 > payload.len() {
                    break;
                }
                let peer_id = String::from_utf8_lossy(&payload[pos..pos + id_len]).into_owned();
                pos += id_len;
                let ip = std::net::IpAddr::V4(std::net::Ipv4Addr::new(
                    payload[pos],
                    payload[pos + 1],
                    payload[pos + 2],
                    payload[pos + 3],
                ));
                pos += 4;
                let port = u16::from_le_bytes(payload[pos..pos + 2].try_into().unwrap());
                pos += 2;
                let group_len =
                    u16::from_le_bytes(payload[pos..pos + 2].try_into().unwrap()) as usize;
                pos += 2;
                if pos + group_len + 1 > payload.len() {
                    break;
                }
                let group_name =
                    String::from_utf8_lossy(&payload[pos..pos + group_len]).into_owned();
                pos += group_len;
                let channel = ChannelAssignment::from_u8(payload[pos]);
                pos += 1;
                peers.push(GossipEntry {
                    peer_id,
                    address: ip,
                    port,
                    group_name,
                    channel,
                });
            }
            Ok(GroupPacket::PeerGossip { peers })
        }
        PacketType::ElectionStart => {
            if payload.len() < 6 {
                return Err(io::Error::new(io::ErrorKind::InvalidData, "short payload"));
            }
            let election_id = u32::from_le_bytes(payload[0..4].try_into().unwrap());
            let trig_len = u16::from_le_bytes(payload[4..6].try_into().unwrap()) as usize;
            if payload.len() < 6 + trig_len {
                return Err(io::Error::new(io::ErrorKind::InvalidData, "short payload"));
            }
            let trigger = String::from_utf8_lossy(&payload[6..6 + trig_len]).into_owned();
            Ok(GroupPacket::ElectionStart {
                election_id,
                trigger,
            })
        }
        PacketType::ElectionVote => {
            if payload.len() < 14 {
                return Err(io::Error::new(io::ErrorKind::InvalidData, "short payload"));
            }
            let election_id = u32::from_le_bytes(payload[0..4].try_into().unwrap());
            let score = u64::from_le_bytes(payload[4..12].try_into().unwrap());
            let id_len = u16::from_le_bytes(payload[12..14].try_into().unwrap()) as usize;
            if payload.len() < 14 + id_len {
                return Err(io::Error::new(io::ErrorKind::InvalidData, "short payload"));
            }
            let peer_id = String::from_utf8_lossy(&payload[14..14 + id_len]).into_owned();
            Ok(GroupPacket::ElectionVote {
                election_id,
                score,
                peer_id,
            })
        }
        PacketType::ElectionResult => {
            if payload.len() < 8 {
                return Err(io::Error::new(io::ErrorKind::InvalidData, "short payload"));
            }
            let election_id = u32::from_le_bytes(payload[0..4].try_into().unwrap());
            let winner_len = u16::from_le_bytes(payload[4..6].try_into().unwrap()) as usize;
            if payload.len() < 6 + winner_len + 2 {
                return Err(io::Error::new(io::ErrorKind::InvalidData, "short payload"));
            }
            let winner_id = String::from_utf8_lossy(&payload[6..6 + winner_len]).into_owned();
            let source_len =
                u16::from_le_bytes(payload[6 + winner_len..8 + winner_len].try_into().unwrap())
                    as usize;
            if payload.len() < 8 + winner_len + source_len {
                return Err(io::Error::new(io::ErrorKind::InvalidData, "short payload"));
            }
            let source =
                String::from_utf8_lossy(&payload[8 + winner_len..8 + winner_len + source_len])
                    .into_owned();
            Ok(GroupPacket::ElectionResult {
                election_id,
                winner_id,
                source,
            })
        }
        PacketType::HealthPing => {
            if payload.len() < 19 {
                return Err(io::Error::new(io::ErrorKind::InvalidData, "short payload"));
            }
            Ok(GroupPacket::HealthPing {
                avg_rtt_us: u64::from_le_bytes(payload[0..8].try_into().unwrap()),
                uptime_secs: u64::from_le_bytes(payload[8..16].try_into().unwrap()),
                active_sources: payload[16],
                buffer_health: payload[17],
                packet_loss_pct: payload[18],
            })
        }
        PacketType::RelayAssignment => {
            if payload.len() < 4 {
                return Err(io::Error::new(io::ErrorKind::InvalidData, "short payload"));
            }
            let target_len = u16::from_le_bytes(payload[0..2].try_into().unwrap()) as usize;
            if payload.len() < 2 + target_len + 2 {
                return Err(io::Error::new(io::ErrorKind::InvalidData, "short payload"));
            }
            let target_peer_id = String::from_utf8_lossy(&payload[2..2 + target_len]).into_owned();
            let relay_len =
                u16::from_le_bytes(payload[2 + target_len..4 + target_len].try_into().unwrap())
                    as usize;
            if payload.len() < 4 + target_len + relay_len {
                return Err(io::Error::new(io::ErrorKind::InvalidData, "short payload"));
            }
            let relay_peer_id =
                String::from_utf8_lossy(&payload[4 + target_len..4 + target_len + relay_len])
                    .into_owned();
            Ok(GroupPacket::RelayAssignment {
                target_peer_id,
                relay_peer_id,
            })
        }
        PacketType::VolumeSync => {
            if payload.len() < 3 {
                return Err(io::Error::new(io::ErrorKind::InvalidData, "short payload"));
            }
            let master_volume = payload[0];
            let orig_len = u16::from_le_bytes(payload[1..3].try_into().unwrap()) as usize;
            if payload.len() < 3 + orig_len {
                return Err(io::Error::new(io::ErrorKind::InvalidData, "short payload"));
            }
            let originator = String::from_utf8_lossy(&payload[3..3 + orig_len]).into_owned();
            Ok(GroupPacket::VolumeSync {
                master_volume,
                originator,
            })
        }
        PacketType::PlayPause => {
            if payload.len() < 4 {
                return Err(io::Error::new(io::ErrorKind::InvalidData, "short payload"));
            }
            let src_len = u16::from_le_bytes(payload[0..2].try_into().unwrap()) as usize;
            if payload.len() < 2 + src_len + 2 {
                return Err(io::Error::new(io::ErrorKind::InvalidData, "short payload"));
            }
            let source = String::from_utf8_lossy(&payload[2..2 + src_len]).into_owned();
            let act_len =
                u16::from_le_bytes(payload[2 + src_len..4 + src_len].try_into().unwrap()) as usize;
            if payload.len() < 4 + src_len + act_len {
                return Err(io::Error::new(io::ErrorKind::InvalidData, "short payload"));
            }
            let action =
                String::from_utf8_lossy(&payload[4 + src_len..4 + src_len + act_len]).into_owned();
            Ok(GroupPacket::PlayPause { source, action })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_clock_sync_req() {
        let pkt = GroupPacket::ClockSyncReq {
            originate_us: 123456789,
        };
        let encoded = encode(&pkt, 42);
        assert_eq!(&encoded[0..2], b"HG");
        assert_eq!(encoded[2], 1); // version
        assert_eq!(encoded[3], PacketType::ClockSyncReq as u8);

        let header: [u8; HEADER_SIZE] = encoded[..HEADER_SIZE].try_into().unwrap();
        let (pt, seq, plen) = {
            let (p, s, l) = read_header(&header).unwrap();
            (p.unwrap(), s, l)
        };
        assert_eq!(pt, PacketType::ClockSyncReq);
        assert_eq!(seq, 42);
        assert_eq!(plen, 8);

        let decoded = decode_payload(pt, &encoded[HEADER_SIZE..]).unwrap();
        match decoded {
            GroupPacket::ClockSyncReq { originate_us } => assert_eq!(originate_us, 123456789),
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn round_trip_clock_sync_resp() {
        let pkt = GroupPacket::ClockSyncResp {
            originate_us: 100,
            receive_us: 200,
            transmit_us: 300,
        };
        let encoded = encode(&pkt, 1);
        let header: [u8; HEADER_SIZE] = encoded[..HEADER_SIZE].try_into().unwrap();
        let (pt, _, _) = {
            let (p, s, l) = read_header(&header).unwrap();
            (p.unwrap(), s, l)
        };
        let decoded = decode_payload(pt, &encoded[HEADER_SIZE..]).unwrap();
        match decoded {
            GroupPacket::ClockSyncResp {
                originate_us,
                receive_us,
                transmit_us,
            } => {
                assert_eq!(originate_us, 100);
                assert_eq!(receive_us, 200);
                assert_eq!(transmit_us, 300);
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn round_trip_leader_claim() {
        let pkt = GroupPacket::LeaderClaim {
            source: "spotify".into(),
            claim_time_us: 999999,
        };
        let encoded = encode(&pkt, 0);
        let header: [u8; HEADER_SIZE] = encoded[..HEADER_SIZE].try_into().unwrap();
        let (pt, _, _) = {
            let (p, s, l) = read_header(&header).unwrap();
            (p.unwrap(), s, l)
        };
        let decoded = decode_payload(pt, &encoded[HEADER_SIZE..]).unwrap();
        match decoded {
            GroupPacket::LeaderClaim {
                source,
                claim_time_us,
            } => {
                assert_eq!(source, "spotify");
                assert_eq!(claim_time_us, 999999);
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn round_trip_leader_release() {
        let pkt = GroupPacket::LeaderRelease;
        let encoded = encode(&pkt, 5);
        let header: [u8; HEADER_SIZE] = encoded[..HEADER_SIZE].try_into().unwrap();
        let (pt, seq, plen) = {
            let (p, s, l) = read_header(&header).unwrap();
            (p.unwrap(), s, l)
        };
        assert_eq!(pt, PacketType::LeaderRelease);
        assert_eq!(seq, 5);
        assert_eq!(plen, 0);
        let decoded = decode_payload(pt, &[]).unwrap();
        assert!(matches!(decoded, GroupPacket::LeaderRelease));
    }

    #[test]
    fn round_trip_audio_chunk() {
        let pcm = vec![100i32, -200, 300, -400, 500, -600, 700, -800];
        let pkt = GroupPacket::AudioChunk {
            seq: 7,
            leader_id: 0,
            play_at_us: 5_000_000,
            frame_count: 4,
            hop_count: 0,
            pcm: pcm.clone(),
        };
        let encoded = encode(&pkt, 10);
        let header: [u8; HEADER_SIZE] = encoded[..HEADER_SIZE].try_into().unwrap();
        let (pt, _, _) = {
            let (p, s, l) = read_header(&header).unwrap();
            (p.unwrap(), s, l)
        };
        let decoded = decode_payload(pt, &encoded[HEADER_SIZE..]).unwrap();
        match decoded {
            GroupPacket::AudioChunk {
                seq,
                leader_id,
                play_at_us,
                frame_count,
                hop_count,
                pcm: decoded_pcm,
            } => {
                assert_eq!(seq, 7);
                assert_eq!(leader_id, 0);
                assert_eq!(play_at_us, 5_000_000);
                assert_eq!(frame_count, 4);
                assert_eq!(hop_count, 0);
                assert_eq!(decoded_pcm, pcm);
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn audio_chunk_datagram_fits_one_mtu() {
        // A full AudioChunk must encode to a single un-fragmented UDP datagram.
        // Above the MTU it fragments, and losing any one fragment drops the whole
        // chunk on a lossy link — the multi-room "static" bug. Regression guard.
        let pcm = vec![0i32; AUDIO_CHUNK_SAMPLES];
        let pkt = GroupPacket::AudioChunk {
            seq: 0,
            leader_id: 0,
            play_at_us: 0,
            frame_count: AUDIO_CHUNK_FRAMES as u16,
            hop_count: 0,
            pcm,
        };
        let datagram = encode(&pkt, 0).len();
        // 1500 MTU - 20 (IPv4) - 8 (UDP) = 1472; keep margin for reduced-MTU paths.
        assert!(
            datagram <= 1400,
            "AudioChunk datagram is {datagram}B; >1400 fragments on the wire"
        );
        // The per-chunk duration must land on an exact integer-microsecond grid.
        assert_eq!(
            AUDIO_CHUNK_DUR_US * 48_000,
            AUDIO_CHUNK_FRAMES as u64 * 1_000_000
        );
    }

    #[test]
    fn round_trip_group_config() {
        let pkt = GroupPacket::GroupConfig {
            channel: ChannelAssignment::Left,
            buffer_ms: 80,
        };
        let encoded = encode(&pkt, 0);
        let header: [u8; HEADER_SIZE] = encoded[..HEADER_SIZE].try_into().unwrap();
        let (pt, _, _) = {
            let (p, s, l) = read_header(&header).unwrap();
            (p.unwrap(), s, l)
        };
        let decoded = decode_payload(pt, &encoded[HEADER_SIZE..]).unwrap();
        match decoded {
            GroupPacket::GroupConfig { channel, buffer_ms } => {
                assert_eq!(channel, ChannelAssignment::Left);
                assert_eq!(buffer_ms, 80);
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn round_trip_heartbeat() {
        let pkt = GroupPacket::Heartbeat;
        let encoded = encode(&pkt, 99);
        let header: [u8; HEADER_SIZE] = encoded[..HEADER_SIZE].try_into().unwrap();
        let (pt, _, _) = {
            let (p, s, l) = read_header(&header).unwrap();
            (p.unwrap(), s, l)
        };
        let decoded = decode_payload(pt, &[]).unwrap();
        assert!(matches!(decoded, GroupPacket::Heartbeat));
    }

    #[test]
    fn round_trip_peer_info() {
        let pkt = GroupPacket::PeerInfo {
            name: "Living Room".into(),
            channel: ChannelAssignment::Right,
        };
        let encoded = encode(&pkt, 3);
        let header: [u8; HEADER_SIZE] = encoded[..HEADER_SIZE].try_into().unwrap();
        let (pt, _, _) = {
            let (p, s, l) = read_header(&header).unwrap();
            (p.unwrap(), s, l)
        };
        let decoded = decode_payload(pt, &encoded[HEADER_SIZE..]).unwrap();
        match decoded {
            GroupPacket::PeerInfo { name, channel } => {
                assert_eq!(name, "Living Room");
                assert_eq!(channel, ChannelAssignment::Right);
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn bad_magic_rejected() {
        let mut header = [0u8; HEADER_SIZE];
        header[0] = b'X';
        header[1] = b'X';
        assert!(read_header(&header).is_err());
    }

    #[test]
    fn bad_version_rejected() {
        let mut header = [0u8; HEADER_SIZE];
        header[0..2].copy_from_slice(&MAGIC);
        header[2] = 99;
        assert!(read_header(&header).is_err());
    }

    #[test]
    fn oversized_payload_rejected() {
        let mut header = [0u8; HEADER_SIZE];
        header[0..2].copy_from_slice(&MAGIC);
        header[2] = VERSION;
        header[3] = PacketType::Heartbeat as u8;
        // payload_len = 2MB (exceeds MAX_PAYLOAD)
        header[8..12].copy_from_slice(&2_000_000u32.to_le_bytes());
        assert!(read_header(&header).is_err());
    }

    #[test]
    fn channel_assignment_round_trip() {
        assert_eq!(ChannelAssignment::from_u8(0), ChannelAssignment::Stereo);
        assert_eq!(ChannelAssignment::from_u8(1), ChannelAssignment::Left);
        assert_eq!(ChannelAssignment::from_u8(2), ChannelAssignment::Right);
        assert_eq!(ChannelAssignment::from_u8(99), ChannelAssignment::Stereo);
    }

    #[test]
    fn unknown_packet_type_returns_none() {
        let mut header = [0u8; HEADER_SIZE];
        header[0..2].copy_from_slice(&MAGIC);
        header[2] = VERSION;
        header[3] = 200; // unknown type
        let (ptype, _, _) = read_header(&header).unwrap();
        assert!(ptype.is_none());
    }

    #[test]
    fn round_trip_gossip() {
        let pkt = GroupPacket::PeerGossip {
            peers: vec![
                GossipEntry {
                    peer_id: "abc-123".into(),
                    address: "192.168.1.100".parse().unwrap(),
                    port: 48200,
                    group_name: "Home".into(),
                    channel: ChannelAssignment::Stereo,
                },
                GossipEntry {
                    peer_id: "def-456".into(),
                    address: "10.0.0.5".parse().unwrap(),
                    port: 48200,
                    group_name: "Home".into(),
                    channel: ChannelAssignment::Left,
                },
            ],
        };
        let encoded = encode(&pkt, 1);
        let header: [u8; HEADER_SIZE] = encoded[..HEADER_SIZE].try_into().unwrap();
        let (pt, _, _) = {
            let (p, s, l) = read_header(&header).unwrap();
            (p.unwrap(), s, l)
        };
        assert_eq!(pt, PacketType::PeerGossip);
        let decoded = decode_payload(pt, &encoded[HEADER_SIZE..]).unwrap();
        match decoded {
            GroupPacket::PeerGossip { peers } => {
                assert_eq!(peers.len(), 2);
                assert_eq!(peers[0].peer_id, "abc-123");
                assert_eq!(
                    peers[0].address,
                    "192.168.1.100".parse::<std::net::IpAddr>().unwrap()
                );
                assert_eq!(peers[0].group_name, "Home");
                assert_eq!(peers[1].peer_id, "def-456");
                assert_eq!(peers[1].channel, ChannelAssignment::Left);
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn round_trip_election_start() {
        let pkt = GroupPacket::ElectionStart {
            election_id: 42,
            trigger: "audio_started".into(),
        };
        let encoded = encode(&pkt, 0);
        let header: [u8; HEADER_SIZE] = encoded[..HEADER_SIZE].try_into().unwrap();
        let (pt, _, _) = {
            let (p, s, l) = read_header(&header).unwrap();
            (p.unwrap(), s, l)
        };
        let decoded = decode_payload(pt, &encoded[HEADER_SIZE..]).unwrap();
        match decoded {
            GroupPacket::ElectionStart {
                election_id,
                trigger,
            } => {
                assert_eq!(election_id, 42);
                assert_eq!(trigger, "audio_started");
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn round_trip_election_vote() {
        let pkt = GroupPacket::ElectionVote {
            election_id: 42,
            score: 12345,
            peer_id: "voter-1".into(),
        };
        let encoded = encode(&pkt, 0);
        let header: [u8; HEADER_SIZE] = encoded[..HEADER_SIZE].try_into().unwrap();
        let (pt, _, _) = {
            let (p, s, l) = read_header(&header).unwrap();
            (p.unwrap(), s, l)
        };
        let decoded = decode_payload(pt, &encoded[HEADER_SIZE..]).unwrap();
        match decoded {
            GroupPacket::ElectionVote {
                election_id,
                score,
                peer_id,
            } => {
                assert_eq!(election_id, 42);
                assert_eq!(score, 12345);
                assert_eq!(peer_id, "voter-1");
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn round_trip_election_result() {
        let pkt = GroupPacket::ElectionResult {
            election_id: 42,
            winner_id: "best-speaker".into(),
            source: "audio_started".into(),
        };
        let encoded = encode(&pkt, 0);
        let header: [u8; HEADER_SIZE] = encoded[..HEADER_SIZE].try_into().unwrap();
        let (pt, _, _) = {
            let (p, s, l) = read_header(&header).unwrap();
            (p.unwrap(), s, l)
        };
        let decoded = decode_payload(pt, &encoded[HEADER_SIZE..]).unwrap();
        match decoded {
            GroupPacket::ElectionResult {
                election_id,
                winner_id,
                source,
            } => {
                assert_eq!(election_id, 42);
                assert_eq!(winner_id, "best-speaker");
                assert_eq!(source, "audio_started");
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn round_trip_health_ping() {
        let pkt = GroupPacket::HealthPing {
            avg_rtt_us: 5000,
            uptime_secs: 3600,
            active_sources: 2,
            buffer_health: 85,
            packet_loss_pct: 1,
        };
        let encoded = encode(&pkt, 0);
        let header: [u8; HEADER_SIZE] = encoded[..HEADER_SIZE].try_into().unwrap();
        let (pt, _, _) = {
            let (p, s, l) = read_header(&header).unwrap();
            (p.unwrap(), s, l)
        };
        let decoded = decode_payload(pt, &encoded[HEADER_SIZE..]).unwrap();
        match decoded {
            GroupPacket::HealthPing {
                avg_rtt_us,
                uptime_secs,
                active_sources,
                buffer_health,
                packet_loss_pct,
            } => {
                assert_eq!(avg_rtt_us, 5000);
                assert_eq!(uptime_secs, 3600);
                assert_eq!(active_sources, 2);
                assert_eq!(buffer_health, 85);
                assert_eq!(packet_loss_pct, 1);
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn round_trip_relay_assignment() {
        let pkt = GroupPacket::RelayAssignment {
            target_peer_id: "far-speaker".into(),
            relay_peer_id: "relay-speaker".into(),
        };
        let encoded = encode(&pkt, 0);
        let header: [u8; HEADER_SIZE] = encoded[..HEADER_SIZE].try_into().unwrap();
        let (pt, _, _) = {
            let (p, s, l) = read_header(&header).unwrap();
            (p.unwrap(), s, l)
        };
        let decoded = decode_payload(pt, &encoded[HEADER_SIZE..]).unwrap();
        match decoded {
            GroupPacket::RelayAssignment {
                target_peer_id,
                relay_peer_id,
            } => {
                assert_eq!(target_peer_id, "far-speaker");
                assert_eq!(relay_peer_id, "relay-speaker");
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn round_trip_volume_sync() {
        let pkt = GroupPacket::VolumeSync {
            master_volume: 75,
            originator: "speaker-1".into(),
        };
        let encoded = encode(&pkt, 0);
        let header: [u8; HEADER_SIZE] = encoded[..HEADER_SIZE].try_into().unwrap();
        let (pt, _, _) = {
            let (p, s, l) = read_header(&header).unwrap();
            (p.unwrap(), s, l)
        };
        let decoded = decode_payload(pt, &encoded[HEADER_SIZE..]).unwrap();
        match decoded {
            GroupPacket::VolumeSync {
                master_volume,
                originator,
            } => {
                assert_eq!(master_volume, 75);
                assert_eq!(originator, "speaker-1");
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn round_trip_audio_chunk_with_hop_count() {
        let pcm = vec![1000i32, -2000];
        let pkt = GroupPacket::AudioChunk {
            seq: 0,
            leader_id: 0,
            play_at_us: 1_000_000,
            frame_count: 1,
            hop_count: 2,
            pcm: pcm.clone(),
        };
        let encoded = encode(&pkt, 0);
        let header: [u8; HEADER_SIZE] = encoded[..HEADER_SIZE].try_into().unwrap();
        let (pt, _, _) = {
            let (p, s, l) = read_header(&header).unwrap();
            (p.unwrap(), s, l)
        };
        let decoded = decode_payload(pt, &encoded[HEADER_SIZE..]).unwrap();
        match decoded {
            GroupPacket::AudioChunk { hop_count, .. } => {
                assert_eq!(hop_count, 2);
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn audio_chunk_carries_seq() {
        let pkt = GroupPacket::AudioChunk {
            seq: 42,
            leader_id: 0,
            play_at_us: 123456,
            frame_count: 480,
            hop_count: 0,
            pcm: vec![1, 2, 3, 4],
        };
        let encoded = encode(&pkt, 0);
        let header: [u8; HEADER_SIZE] = encoded[..HEADER_SIZE].try_into().unwrap();
        let (pt, _, _) = {
            let (p, s, l) = read_header(&header).unwrap();
            (p.unwrap(), s, l)
        };
        let decoded = decode_payload(pt, &encoded[HEADER_SIZE..]).unwrap();
        match decoded {
            GroupPacket::AudioChunk {
                seq, play_at_us, ..
            } => {
                assert_eq!(seq, 42);
                assert_eq!(play_at_us, 123456);
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn audio_chunk_carries_leader_id() {
        let pkt = GroupPacket::AudioChunk {
            seq: 1,
            leader_id: 7,
            play_at_us: 0,
            frame_count: 480,
            hop_count: 0,
            pcm: vec![],
        };
        let back = decode_payload(PacketType::AudioChunk, &encode_payload(&pkt)).unwrap();
        match back {
            GroupPacket::AudioChunk { leader_id, .. } => assert_eq!(leader_id, 7),
            _ => panic!(),
        }
    }

    #[test]
    fn round_trip_play_pause() {
        let pkt = GroupPacket::PlayPause {
            source: "spotify".into(),
            action: "pause".into(),
        };
        let encoded = encode(&pkt, 7);
        let header: [u8; HEADER_SIZE] = encoded[..HEADER_SIZE].try_into().unwrap();
        let (pt, seq, _) = {
            let (p, s, l) = read_header(&header).unwrap();
            (p.unwrap(), s, l)
        };
        assert_eq!(pt, PacketType::PlayPause);
        assert_eq!(seq, 7);
        let decoded = decode_payload(pt, &encoded[HEADER_SIZE..]).unwrap();
        match decoded {
            GroupPacket::PlayPause { source, action } => {
                assert_eq!(source, "spotify");
                assert_eq!(action, "pause");
            }
            _ => panic!("wrong variant"),
        }
    }
}
