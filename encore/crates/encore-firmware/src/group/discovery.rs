//! mDNS-based discovery of group peers on the local network.
//!
//! Periodically browses for `_encore-group._tcp.local.` services using the
//! existing mDNS infrastructure. Maintains a peer table with TTL-based expiry.

use super::wire::ChannelAssignment;
use std::collections::HashMap;
use std::net::IpAddr;
use std::time::{Duration, Instant};
use tracing::info;

/// Discovered peer from mDNS.
#[derive(Debug, Clone)]
pub struct DiscoveredPeer {
    pub peer_id: String,
    pub name: String,
    pub address: IpAddr,
    pub port: u16,
    pub channel: ChannelAssignment,
    pub group_name: String,
    pub last_seen: Instant,
}

/// TTL for discovered peers (3 missed intervals = lost).
const PEER_TTL: Duration = Duration::from_secs(120);

/// mDNS service type for group discovery.
pub const GROUP_SERVICE_TYPE: &str = "_encore-group._tcp";

/// A discovery event from mDNS (cross-platform channel type).
#[derive(Debug, Clone)]
pub struct DiscoveryEvent {
    pub peer_id: String,
    pub name: String,
    pub address: IpAddr,
    pub port: u16,
    pub group_name: String,
    pub channel: String,
}

/// Manages discovered group peers.
pub struct PeerTable {
    peers: HashMap<String, DiscoveredPeer>,
}

impl PeerTable {
    pub fn new() -> Self {
        Self {
            peers: HashMap::new(),
        }
    }

    /// Update or insert a discovered peer.
    pub fn upsert(&mut self, peer: DiscoveredPeer) {
        let is_new = !self.peers.contains_key(&peer.peer_id);
        if is_new {
            info!(
                "Group discovery: new peer {} ({}) at {}",
                peer.name, peer.peer_id, peer.address
            );
        }
        self.peers.insert(peer.peer_id.clone(), peer);
    }

    /// Remove peers that haven't been seen within TTL.
    pub fn expire(&mut self) -> Vec<String> {
        let now = Instant::now();
        let expired: Vec<String> = self
            .peers
            .iter()
            .filter(|(_, p)| now.duration_since(p.last_seen) > PEER_TTL)
            .map(|(id, _)| id.clone())
            .collect();
        for id in &expired {
            info!("Group discovery: peer {} expired", id);
            self.peers.remove(id);
        }
        expired
    }

    /// Get all current peers.
    pub fn peers(&self) -> impl Iterator<Item = &DiscoveredPeer> {
        self.peers.values()
    }

    /// Get a specific peer by ID.
    pub fn get(&self, peer_id: &str) -> Option<&DiscoveredPeer> {
        self.peers.get(peer_id)
    }

    /// Number of known peers.
    pub fn len(&self) -> usize {
        self.peers.len()
    }

    /// Remove a peer by ID.
    pub fn remove(&mut self, peer_id: &str) {
        self.peers.remove(peer_id);
    }
}

/// Parse TXT record key-value pairs from mDNS service advertisement.
/// Expected keys: `id`, `group`, `channel`
pub fn parse_txt_records(txt: &[String]) -> (String, String, ChannelAssignment) {
    let mut peer_id = String::new();
    let mut group = String::new();
    let mut channel = ChannelAssignment::Stereo;

    for entry in txt {
        if let Some(val) = entry.strip_prefix("id=") {
            peer_id = val.to_string();
        } else if let Some(val) = entry.strip_prefix("group=") {
            group = val.to_string();
        } else if let Some(val) = entry.strip_prefix("channel=") {
            channel = match val {
                "left" | "L" => ChannelAssignment::Left,
                "right" | "R" => ChannelAssignment::Right,
                _ => ChannelAssignment::Stereo,
            };
        }
    }

    (peer_id, group, channel)
}

/// Format TXT records for our mDNS advertisement.
pub fn format_txt_records(
    peer_id: &str,
    group_name: &str,
    channel: ChannelAssignment,
) -> Vec<String> {
    let ch = match channel {
        ChannelAssignment::Left => "left",
        ChannelAssignment::Right => "right",
        ChannelAssignment::Stereo => "stereo",
    };
    vec![
        format!("id={}", peer_id),
        format!("group={}", group_name),
        format!("channel={}", ch),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn peer_table_upsert_and_get() {
        let mut table = PeerTable::new();
        table.upsert(DiscoveredPeer {
            peer_id: "abc-123".into(),
            name: "Living Room".into(),
            address: "192.168.1.100".parse().unwrap(),
            port: 48200,
            channel: ChannelAssignment::Left,
            group_name: "Home".into(),
            last_seen: Instant::now(),
        });
        assert_eq!(table.len(), 1);
        assert!(table.get("abc-123").is_some());
        assert_eq!(table.get("abc-123").unwrap().name, "Living Room");
    }

    #[test]
    fn peer_table_expire() {
        let mut table = PeerTable::new();
        table.upsert(DiscoveredPeer {
            peer_id: "old".into(),
            name: "Old".into(),
            address: "10.0.0.1".parse().unwrap(),
            port: 48200,
            channel: ChannelAssignment::Stereo,
            group_name: "Test".into(),
            last_seen: Instant::now() - Duration::from_secs(200),
        });
        table.upsert(DiscoveredPeer {
            peer_id: "new".into(),
            name: "New".into(),
            address: "10.0.0.2".parse().unwrap(),
            port: 48200,
            channel: ChannelAssignment::Stereo,
            group_name: "Test".into(),
            last_seen: Instant::now(),
        });
        let expired = table.expire();
        assert_eq!(expired, vec!["old"]);
        assert_eq!(table.len(), 1);
        assert!(table.get("new").is_some());
    }

    #[test]
    fn parse_txt_records_basic() {
        let txt = vec![
            "id=abc-123".into(),
            "group=Living Room".into(),
            "channel=left".into(),
        ];
        let (id, group, ch) = parse_txt_records(&txt);
        assert_eq!(id, "abc-123");
        assert_eq!(group, "Living Room");
        assert_eq!(ch, ChannelAssignment::Left);
    }

    #[test]
    fn format_and_parse_round_trip() {
        let txt = format_txt_records("my-id", "Kitchen", ChannelAssignment::Right);
        let (id, group, ch) = parse_txt_records(&txt);
        assert_eq!(id, "my-id");
        assert_eq!(group, "Kitchen");
        assert_eq!(ch, ChannelAssignment::Right);
    }
}
