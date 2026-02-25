//! TCP peer connection manager for the speaker group mesh.
//!
//! Each group member listens on port 48200 and connects to discovered peers.
//! To avoid duplicate connections, the speaker with the lexicographically
//! smaller peer_id initiates the connection.

use super::wire::{self, ChannelAssignment, GroupPacket, HEADER_SIZE};
use std::collections::HashMap;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;
use tracing::{info, warn};

/// TCP listen port for group mesh connections.
pub const GROUP_PORT: u16 = 48200;

/// Information about a connected peer.
#[derive(Debug, Clone)]
pub struct PeerState {
    pub peer_id: String,
    pub name: String,
    pub address: std::net::IpAddr,
    pub channel: ChannelAssignment,
    pub clock: Option<ClockSyncState>,
    pub connected: bool,
}

/// Serializable clock sync state for status reporting.
#[derive(Debug, Clone)]
pub struct ClockSyncState {
    pub offset_us: i64,
    pub rtt_us: u64,
    pub samples: u32,
}

/// Events from the peer layer to the group subsystem.
#[derive(Debug)]
pub enum PeerEvent {
    /// A new peer connected (either inbound or outbound).
    Connected {
        peer_id: String,
        name: String,
        address: std::net::IpAddr,
        channel: ChannelAssignment,
        writer: PeerWriter,
    },
    /// A peer disconnected.
    Disconnected { peer_id: String },
    /// Received a packet from a peer.
    Packet { peer_id: String, packet: GroupPacket },
}

/// Handle to a connected peer's write half.
pub struct PeerWriter {
    pub(super) tx: mpsc::Sender<Vec<u8>>,
}

impl std::fmt::Debug for PeerWriter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PeerWriter").finish()
    }
}

impl PeerWriter {
    /// Send a packet to this peer (non-blocking, drops on backpressure).
    pub fn send(&self, packet: &GroupPacket, seq: u32) -> bool {
        let data = wire::encode(packet, seq);
        self.tx.try_send(data).is_ok()
    }
}

/// Manages all TCP connections to group peers.
pub struct PeerManager {
    pub local_id: String,
    pub local_name: String,
    pub local_channel: ChannelAssignment,
    writers: HashMap<String, PeerWriter>,
    event_tx: mpsc::Sender<PeerEvent>,
    event_rx: Option<mpsc::Receiver<PeerEvent>>,
    seq: u32,
}

impl PeerManager {
    pub fn new(local_id: String, local_name: String, local_channel: ChannelAssignment) -> Self {
        let (event_tx, event_rx) = mpsc::channel(256);
        Self {
            local_id,
            local_name,
            local_channel,
            writers: HashMap::new(),
            event_tx,
            event_rx: Some(event_rx),
            seq: 0,
        }
    }

    /// Take the event receiver (can only be called once).
    pub fn take_event_rx(&mut self) -> mpsc::Receiver<PeerEvent> {
        self.event_rx.take().expect("event_rx already taken")
    }

    /// Start the TCP listener for inbound connections.
    pub fn start_listener(&self) -> tokio::task::JoinHandle<()> {
        let local_id = self.local_id.clone();
        let local_name = self.local_name.clone();
        let local_channel = self.local_channel;
        let event_tx = self.event_tx.clone();

        tokio::spawn(async move {
            let listener = match TcpListener::bind(("0.0.0.0", GROUP_PORT)).await {
                Ok(l) => l,
                Err(e) => {
                    warn!("Group: failed to bind TCP listener on port {}: {}", GROUP_PORT, e);
                    return;
                }
            };
            info!("Group: listening on port {}", GROUP_PORT);

            loop {
                match listener.accept().await {
                    Ok((stream, addr)) => {
                        info!("Group: inbound connection from {}", addr);
                        let local_id = local_id.clone();
                        let local_name = local_name.clone();
                        let event_tx = event_tx.clone();
                        tokio::spawn(async move {
                            handle_connection(
                                stream,
                                addr.ip(),
                                local_id,
                                local_name,
                                local_channel,
                                event_tx,
                                false,
                            )
                            .await;
                        });
                    }
                    Err(e) => {
                        warn!("Group: TCP accept error: {}", e);
                    }
                }
            }
        })
    }

    /// Initiate a connection to a discovered peer (if we should be the initiator).
    pub fn connect_to_peer(
        &self,
        peer_id: &str,
        address: std::net::IpAddr,
    ) -> Option<tokio::task::JoinHandle<()>> {
        // Only the speaker with the smaller peer_id initiates
        if self.local_id >= peer_id.to_string() {
            return None;
        }

        // Don't reconnect if already connected
        if self.writers.contains_key(peer_id) {
            return None;
        }

        let local_id = self.local_id.clone();
        let local_name = self.local_name.clone();
        let local_channel = self.local_channel;
        let event_tx = self.event_tx.clone();
        let peer_id_owned = peer_id.to_string();

        Some(tokio::spawn(async move {
            let addr = std::net::SocketAddr::new(address, GROUP_PORT);
            info!("Group: connecting to peer {} at {}", peer_id_owned, addr);

            match tokio::time::timeout(
                std::time::Duration::from_secs(5),
                TcpStream::connect(addr),
            )
            .await
            {
                Ok(Ok(stream)) => {
                    handle_connection(
                        stream,
                        address,
                        local_id,
                        local_name,
                        local_channel,
                        event_tx,
                        true,
                    )
                    .await;
                }
                Ok(Err(e)) => {
                    warn!("Group: connect to {} failed: {}", addr, e);
                }
                Err(_) => {
                    warn!("Group: connect to {} timed out", addr);
                }
            }
        }))
    }

    /// Register a writer for a connected peer.
    pub fn add_writer(&mut self, peer_id: String, writer: PeerWriter) {
        self.writers.insert(peer_id, writer);
    }

    /// Remove a disconnected peer's writer.
    pub fn remove_writer(&mut self, peer_id: &str) {
        self.writers.remove(peer_id);
    }

    /// Broadcast a packet to all connected peers.
    pub fn broadcast(&mut self, packet: &GroupPacket) {
        self.seq = self.seq.wrapping_add(1);
        let data = wire::encode(packet, self.seq);
        self.writers.retain(|id, writer| {
            if writer.tx.try_send(data.clone()).is_err() {
                warn!("Group: write backpressure to peer {}, dropping", id);
                false
            } else {
                true
            }
        });
    }

    /// Send a packet to a specific peer. Returns false if peer not found.
    pub fn send_to(&mut self, peer_id: &str, packet: &GroupPacket) -> bool {
        self.seq = self.seq.wrapping_add(1);
        if let Some(writer) = self.writers.get(peer_id) {
            writer.send(packet, self.seq)
        } else {
            false
        }
    }

    /// Number of connected peers.
    pub fn peer_count(&self) -> usize {
        self.writers.len()
    }
}

/// Handle a TCP connection (either inbound or outbound).
/// Performs PeerInfo handshake, then reads packets and forwards to event_tx.
async fn handle_connection(
    mut stream: TcpStream,
    address: std::net::IpAddr,
    local_id: String,
    local_name: String,
    local_channel: ChannelAssignment,
    event_tx: mpsc::Sender<PeerEvent>,
    is_initiator: bool,
) {
    // Disable Nagle for low-latency audio streaming
    let _ = stream.set_nodelay(true);

    // Handshake: send our PeerInfo with peer_id embedded in the name field
    // Format: "peer_id\0display_name"
    let our_info = wire::encode(
        &GroupPacket::PeerInfo {
            name: format!("{}\0{}", local_id, local_name),
            channel: local_channel,
        },
        0,
    );
    if stream.write_all(&our_info).await.is_err() {
        return;
    }

    // Read their PeerInfo
    let mut header_buf = [0u8; HEADER_SIZE];
    if stream.read_exact(&mut header_buf).await.is_err() {
        return;
    }
    let (ptype_opt, _, payload_len) = match wire::read_header(&header_buf) {
        Ok(h) => h,
        Err(_) => return,
    };
    let ptype = match ptype_opt {
        Some(p) => p,
        None => return,
    };
    if ptype != wire::PacketType::PeerInfo {
        warn!("Group: expected PeerInfo handshake, got {:?}", ptype);
        return;
    }
    let mut payload = vec![0u8; payload_len as usize];
    if stream.read_exact(&mut payload).await.is_err() {
        return;
    }
    let peer_info = match wire::decode_payload(ptype, &payload) {
        Ok(GroupPacket::PeerInfo { name, channel }) => (name, channel),
        _ => return,
    };

    // Parse peer_id and display name from "peer_id\0display_name"
    let (peer_id, peer_name) = if let Some(idx) = peer_info.0.find('\0') {
        (peer_info.0[..idx].to_string(), peer_info.0[idx + 1..].to_string())
    } else {
        (peer_info.0.clone(), peer_info.0)
    };
    let peer_channel = peer_info.1;

    info!(
        "Group: handshake complete with {} ({}) at {} [{}]",
        peer_name,
        peer_id,
        address,
        if is_initiator { "outbound" } else { "inbound" }
    );

    // Set up write channel
    let (write_tx, mut write_rx) = mpsc::channel::<Vec<u8>>(128);
    let peer_writer = PeerWriter { tx: write_tx };

    // Notify group subsystem of connection (includes writer for PeerManager)
    let _ = event_tx
        .send(PeerEvent::Connected {
            peer_id: peer_id.clone(),
            name: peer_name,
            address,
            channel: peer_channel,
            writer: peer_writer,
        })
        .await;

    // Split stream
    let (mut reader, mut writer) = stream.into_split();

    // Write task: drain the mpsc channel and write to TCP
    let write_handle = tokio::spawn(async move {
        while let Some(data) = write_rx.recv().await {
            if writer.write_all(&data).await.is_err() {
                break;
            }
        }
    });

    // Read loop: read header + payload, decode, forward
    loop {
        let mut hdr = [0u8; HEADER_SIZE];
        if reader.read_exact(&mut hdr).await.is_err() {
            break;
        }
        let (ptype_opt, _seq, payload_len) = match wire::read_header(&hdr) {
            Ok(h) => h,
            Err(e) => {
                warn!("Group: bad header from {}: {}", peer_id, e);
                break;
            }
        };
        let mut payload = vec![0u8; payload_len as usize];
        if payload_len > 0 && reader.read_exact(&mut payload).await.is_err() {
            break;
        }
        // Unknown packet types are silently skipped (forward compatibility)
        let Some(ptype) = ptype_opt else { continue };
        match wire::decode_payload(ptype, &payload) {
            Ok(packet) => {
                if event_tx
                    .send(PeerEvent::Packet {
                        peer_id: peer_id.clone(),
                        packet,
                    })
                    .await
                    .is_err()
                {
                    break;
                }
            }
            Err(e) => {
                warn!("Group: decode error from {}: {}", peer_id, e);
            }
        }
    }

    write_handle.abort();

    // Notify disconnection
    let _ = event_tx
        .send(PeerEvent::Disconnected {
            peer_id: peer_id.clone(),
        })
        .await;

    info!("Group: peer {} disconnected", peer_id);
}
