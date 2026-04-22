//! Multi-speaker synchronized audio playback subsystem.
//!
//! Handles discovery, clock sync, leader election, audio streaming,
//! and follower playback for groups of Harman Kardon Invoke speakers on the same LAN.

pub mod clock;
pub mod discovery;
pub mod election;
pub mod follower;
pub mod leader;
pub mod peer;
pub mod relay;
pub mod wire;

use crate::audio::mixer::MixerSlot;
use crate::subsystem::{Subsystem, SubsystemContext};
use anyhow::Result;
use clock::ClockSync;
use discovery::PeerTable;
use election::{ElectionAction, ElectionState};
use follower::JitterBuffer;
use encore_common::protocol::SubsystemState;
use peer::{PeerEvent, PeerManager};
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Instant;
use tokio::net::UdpSocket;
use tokio::sync::mpsc;
use tracing::{info, warn};
use wire::{ChannelAssignment, GossipEntry, GroupPacket};

/// Commands sent to the GroupSubsystem from other subsystems.
#[derive(Debug)]
pub enum GroupCmd {
    /// A local audio source started producing audio.
    LocalAudioStarted { source: String },
    /// A local audio source stopped producing audio.
    LocalAudioStopped { source: String },
    /// Update channel assignment from web UI.
    SetChannel(ChannelAssignment),
    /// Update buffer depth from web UI.
    SetBufferMs(u16),
    /// Update group name from web UI.
    SetGroupName(String),
    /// Enable/disable group mode.
    SetEnabled(bool),
    /// Request immediate status broadcast.
    RequestStatus,
    /// Set volume for the whole group (from touch ring or web UI).
    SetVolume(u8),
    /// Play/pause control (propagated from follower → leader).
    PlayPause { action: String },
    /// Toggle party mode (accept streams from any group).
    SetPartyMode(bool),
}

/// State machine for this speaker's role in the group.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GroupRole {
    /// Normal operation: all sources active, no streaming.
    Standalone,
    /// This speaker is the audio source: streaming to followers.
    Leader,
    /// This speaker is receiving audio from the leader.
    Follower,
}

/// State of a connected peer.
struct ConnectedPeer {
    name: String,
    address: std::net::IpAddr,
    channel: ChannelAssignment,
    clock: ClockSync,
    role: GroupRole,
    /// Last time we received any packet from this peer.
    last_seen: Instant,
    /// Missed heartbeat count.
    missed_heartbeats: u32,
    /// Packets received from this peer.
    packets_received: u64,
    /// Estimated packet loss percentage.
    packet_loss_pct: u8,
    /// RTT variance for instability detection.
    rtt_variance_us: u64,
    /// Who this peer receives audio from (relay topology).
    upstream_id: Option<String>,
    /// Peers this speaker relays audio to.
    downstream_ids: Vec<String>,
    /// Hop count from leader.
    hop_count: u8,
    /// Buffer health percentage (follower only, from HealthPing).
    buffer_health: u8,
}

/// Multi-speaker group subsystem.
pub struct GroupSubsystem {
    /// Peer ID (UUID v4, persisted in config).
    peer_id: String,
    /// Display name for this speaker.
    device_name: String,
    /// Group name (all speakers in the same group sync together).
    group_name: String,
    /// Channel assignment for this speaker.
    channel: ChannelAssignment,
    /// Jitter buffer depth in ms.
    buffer_ms: u16,
    /// Whether group mode is enabled.
    enabled: bool,
    /// Bootstrap peers for cross-subnet discovery.
    bootstrap_peers: Vec<String>,
    /// Party mode: accept streams from any group.
    party_mode: bool,
    /// Master volume level (for group status reporting).
    volume: u8,

    /// Mixer slot for network audio (follower writes, mixer reads).
    network_slot: Arc<MixerSlot>,
    /// Mixer tap for network audio (mixer writes, leader reads).
    network_tap: Arc<MixerSlot>,
    /// Controls whether the mixer tap is active.
    tap_active: Arc<AtomicBool>,

    /// Command channel from other subsystems.
    cmd_rx: mpsc::Receiver<GroupCmd>,

    /// Suspend channels for audio sources (sent true to suspend, false to resume).
    suspend_txs: Vec<mpsc::Sender<bool>>,

    /// Volume set channel: sends u8 volume level for group volume sync.
    volume_set_tx: Option<mpsc::Sender<u8>>,

    /// WebSocket broadcast for status updates.
    ws_tx: Option<tokio::sync::broadcast::Sender<String>>,

    /// Spotify command channel for play/pause routing from followers.
    spotify_cmd_tx: Option<mpsc::Sender<encore_common::protocol::SpotifyAction>>,

    /// mDNS discovery channel: receives discovered group peers.
    discovery_rx: Option<mpsc::Receiver<discovery::DiscoveryEvent>>,
}

impl GroupSubsystem {
    pub fn new(
        peer_id: String,
        device_name: String,
        group_name: String,
        channel: ChannelAssignment,
        buffer_ms: u16,
        enabled: bool,
        network_slot: Arc<MixerSlot>,
        network_tap: Arc<MixerSlot>,
        tap_active: Arc<AtomicBool>,
        cmd_rx: mpsc::Receiver<GroupCmd>,
        bootstrap_peers: Vec<String>,
        party_mode: bool,
    ) -> Self {
        Self {
            peer_id,
            device_name,
            group_name,
            channel,
            buffer_ms,
            enabled,
            bootstrap_peers,
            party_mode,
            volume: 0,
            network_slot,
            network_tap,
            tap_active,
            cmd_rx,
            suspend_txs: Vec::new(),
            volume_set_tx: None,
            ws_tx: None,
            spotify_cmd_tx: None,
            discovery_rx: None,
        }
    }

    /// Add a suspend channel for an audio source subsystem.
    pub fn add_suspend_tx(&mut self, tx: mpsc::Sender<bool>) {
        self.suspend_txs.push(tx);
    }

    /// Set WebSocket broadcast channel.
    pub fn set_ws_tx(&mut self, tx: tokio::sync::broadcast::Sender<String>) {
        self.ws_tx = Some(tx);
    }

    /// Set volume sender for group volume sync.
    pub fn set_volume_tx(&mut self, tx: mpsc::Sender<u8>) {
        self.volume_set_tx = Some(tx);
    }

    /// Set Spotify command sender for play/pause routing from followers.
    pub fn set_spotify_tx(&mut self, tx: mpsc::Sender<encore_common::protocol::SpotifyAction>) {
        self.spotify_cmd_tx = Some(tx);
    }

    /// Set mDNS discovery receiver for auto-connecting discovered peers.
    pub fn set_discovery_rx(&mut self, rx: mpsc::Receiver<discovery::DiscoveryEvent>) {
        self.discovery_rx = Some(rx);
    }

    /// Suspend all local audio sources (entering follower mode).
    #[allow(dead_code)]
    async fn suspend_sources(&self) {
        for tx in &self.suspend_txs {
            let _ = tx.send(true).await;
        }
    }

    /// Resume all local audio sources (leaving follower mode).
    #[allow(dead_code)]
    async fn resume_sources(&self) {
        for tx in &self.suspend_txs {
            let _ = tx.send(false).await;
        }
    }

    /// Broadcast group status to WebSocket clients.
    fn broadcast_status(
        &self,
        role: GroupRole,
        peers: &HashMap<String, ConnectedPeer>,
    ) {
        let Some(ref ws) = self.ws_tx else { return };

        let peer_infos: Vec<encore_common::protocol::PeerInfo> = peers
            .iter()
            .map(|(id, p)| encore_common::protocol::PeerInfo {
                peer_id: id.clone(),
                name: p.name.clone(),
                address: p.address.to_string(),
                channel: match p.channel {
                    ChannelAssignment::Stereo => "stereo".into(),
                    ChannelAssignment::Left => "left".into(),
                    ChannelAssignment::Right => "right".into(),
                },
                role: match p.role {
                    GroupRole::Standalone => "standalone".into(),
                    GroupRole::Leader => "leader".into(),
                    GroupRole::Follower => "follower".into(),
                },
                latency_us: p.clock.rtt_us() as i64,
                connected: true,
                packet_loss_pct: p.packet_loss_pct as f32,
                clock_offset_us: p.clock.offset_us(),
                buffer_health: p.buffer_health,
                hop_count: p.hop_count,
                is_relay: !p.downstream_ids.is_empty(),
                instability_score: (p.rtt_variance_us + p.packet_loss_pct as u64 * 1000) as u32,
            })
            .collect();

        let status = encore_common::protocol::GroupStatus {
            enabled: self.enabled,
            group_name: self.group_name.clone(),
            role: match role {
                GroupRole::Standalone => "standalone".into(),
                GroupRole::Leader => "leader".into(),
                GroupRole::Follower => "follower".into(),
            },
            peers: peer_infos,
            buffer_ms: self.buffer_ms,
            channel: match self.channel {
                ChannelAssignment::Stereo => "stereo".into(),
                ChannelAssignment::Left => "left".into(),
                ChannelAssignment::Right => "right".into(),
            },
            party_mode: self.party_mode,
            volume: self.volume,
        };

        let msg = encore_common::protocol::ServerMsg::GroupStatus(status);
        if let Ok(json) = serde_json::to_string(&msg) {
            let _ = ws.send(json);
        }
    }

    /// Compute the average RTT to all connected peers.
    fn compute_avg_rtt(peers: &HashMap<String, ConnectedPeer>) -> u64 {
        if peers.is_empty() {
            return 0;
        }
        let total: u64 = peers.values().map(|p| p.clock.rtt_us()).sum();
        total / peers.len() as u64
    }

    /// Process an election action: broadcast the appropriate packets.
    fn handle_election_action(
        action: &ElectionAction,
        peer_mgr: &mut PeerManager,
        our_id: &str,
        our_score: u64,
    ) {
        match action {
            ElectionAction::StartElection { election_id, trigger } => {
                info!("Group: starting election {} (trigger: {})", election_id, trigger);
                peer_mgr.broadcast(&GroupPacket::ElectionStart {
                    election_id: *election_id,
                    trigger: trigger.clone(),
                });
                // Also cast our own vote
                peer_mgr.broadcast(&GroupPacket::ElectionVote {
                    election_id: *election_id,
                    score: our_score,
                    peer_id: our_id.to_string(),
                });
            }
            ElectionAction::CastVote { election_id, score } => {
                peer_mgr.broadcast(&GroupPacket::ElectionVote {
                    election_id: *election_id,
                    score: *score,
                    peer_id: our_id.to_string(),
                });
            }
            _ => {}
        }
    }

    /// Apply an election result: winner becomes leader, losers become followers.
    #[allow(clippy::too_many_arguments)]
    fn apply_election_result(
        our_id: &str,
        winner_id: &str,
        election_id: u32,
        source: &str,
        role: &mut GroupRole,
        leader_id: &mut Option<String>,
        peer_mgr: &mut PeerManager,
        peers: &HashMap<String, ConnectedPeer>,
        leader_task_handle: &mut Option<tokio::task::JoinHandle<()>>,
        network_tap: &Arc<MixerSlot>,
        tap_active: &Arc<AtomicBool>,
        udp_socket: &Arc<UdpSocket>,
        buffer_ms: u64,
        network_slot: &Arc<MixerSlot>,
        jitter_buffer: &mut JitterBuffer,
    ) {
        info!("Group: election {} result: winner={} source={}", election_id, winner_id, source);

        if winner_id == our_id {
            // We won — become leader (election scoring already ensures audio source wins)
            if *role != GroupRole::Leader {
                info!("Group: we won election, claiming leadership");
                *role = GroupRole::Leader;
                *leader_id = None;

                tap_active.store(true, Ordering::Relaxed);
                let tap = network_tap.clone();
                let active = tap_active.clone();
                let sock = udp_socket.clone();
                let addrs: Vec<std::net::SocketAddr> = peers.values()
                    .map(|p| std::net::SocketAddr::new(p.address, peer::GROUP_AUDIO_PORT))
                    .collect();
                *leader_task_handle = Some(tokio::spawn(async move {
                    leader::leader_stream_task(tap, active, sock, addrs, buffer_ms).await;
                }));

                let claim = GroupPacket::LeaderClaim {
                    source: source.to_string(),
                    claim_time_us: clock::now_us(),
                };
                peer_mgr.broadcast(&claim);

                // Broadcast election result so all peers agree
                peer_mgr.broadcast(&GroupPacket::ElectionResult {
                    election_id,
                    winner_id: winner_id.to_string(),
                    source: source.to_string(),
                });
            }
        } else {
            // We lost — become follower
            if *role == GroupRole::Leader {
                tap_active.store(false, Ordering::Relaxed);
                if let Some(h) = leader_task_handle.take() {
                    h.abort();
                }
            }

            *role = GroupRole::Follower;
            *leader_id = Some(winner_id.to_string());
            jitter_buffer.clear();
            network_slot.set_active(true);
            info!("Group: election lost, entering follower mode (leader={})", winner_id);
        }
    }
}

#[async_trait::async_trait]
impl Subsystem for GroupSubsystem {
    fn name(&self) -> &'static str {
        "group"
    }

    fn is_vital(&self) -> bool {
        false
    }

    async fn run(&mut self, mut ctx: SubsystemContext) -> Result<()> {
        ctx.health.set_state(SubsystemState::Running);

        if !self.enabled {
            info!("Group: disabled, waiting for enable");
            loop {
                tokio::select! {
                    cmd = self.cmd_rx.recv() => {
                        match cmd {
                            Some(GroupCmd::SetEnabled(true)) => {
                                self.enabled = true;
                                break;
                            }
                            Some(GroupCmd::RequestStatus) => {
                                let empty = HashMap::new();
                                self.broadcast_status(GroupRole::Standalone, &empty);
                            }
                            None => return Ok(()),
                            _ => {}
                        }
                    }
                    _ = ctx.shutdown.recv() => return Ok(()),
                }
            }
        }

        info!(
            "Group: starting (peer_id={}, group={}, channel={:?}, buffer={}ms)",
            self.peer_id, self.group_name, self.channel, self.buffer_ms
        );

        // Peer manager handles TCP connections
        let mut peer_mgr = PeerManager::new(
            self.peer_id.clone(),
            self.device_name.clone(),
            self.channel,
        );
        let mut peer_events = peer_mgr.take_event_rx();

        // Start TCP listener
        let _listener_handle = peer_mgr.start_listener();

        // UDP socket for audio streaming (fire-and-forget, no backpressure)
        let udp_socket = match UdpSocket::bind(("0.0.0.0", peer::GROUP_AUDIO_PORT)).await {
            Ok(s) => Arc::new(s),
            Err(e) => {
                warn!("Group: failed to bind UDP audio port {}: {}, using random port", peer::GROUP_AUDIO_PORT, e);
                Arc::new(UdpSocket::bind("0.0.0.0:0").await?)
            }
        };
        info!("Group: UDP audio socket bound to {:?}", udp_socket.local_addr());

        // Spawn UDP audio receiver task
        let (udp_audio_tx, mut udp_audio_rx) = mpsc::channel::<(u64, Vec<i32>)>(256);
        {
            let sock = udp_socket.clone();
            tokio::spawn(async move {
                let mut buf = vec![0u8; 8192]; // max AudioChunk ~3.9KB
                loop {
                    let n = match sock.recv(&mut buf).await {
                        Ok(n) => n,
                        Err(_) => continue,
                    };
                    if n < wire::HEADER_SIZE {
                        continue;
                    }
                    let hdr: &[u8; wire::HEADER_SIZE] = match buf[..wire::HEADER_SIZE].try_into() {
                        Ok(h) => h,
                        Err(_) => continue,
                    };
                    let (ptype_opt, _seq, payload_len) = match wire::read_header(hdr) {
                        Ok(h) => h,
                        Err(_) => continue,
                    };
                    let Some(ptype) = ptype_opt else { continue };
                    if ptype != wire::PacketType::AudioChunk {
                        continue;
                    }
                    let payload_end = wire::HEADER_SIZE + payload_len as usize;
                    if payload_end > n {
                        continue;
                    }
                    if let Ok(GroupPacket::AudioChunk { play_at_us, pcm, .. }) =
                        wire::decode_payload(ptype, &buf[wire::HEADER_SIZE..payload_end])
                    {
                        let _ = udp_audio_tx.try_send((play_at_us, pcm));
                    }
                }
            });
        }

        // State
        let mut role = GroupRole::Standalone;
        let mut peers: HashMap<String, ConnectedPeer> = HashMap::new();
        let mut leader_id: Option<String> = None;
        let mut jitter_buffer = JitterBuffer::new(self.buffer_ms, self.channel);
        let mut discovery_table = PeerTable::new();
        let mut active_sources: HashSet<String> = HashSet::new();
        let mut election_state = ElectionState::new();
        let uptime_start = Instant::now();
        // Track peers with pending duplicate disconnect events to ignore
        let mut pending_dup_disconnects: HashMap<String, u32> = HashMap::new();
        // Reconnect queue: (peer_id, address, next_attempt_time, attempt_count)
        let mut reconnect_queue: VecDeque<(String, std::net::IpAddr, Instant, u32)> = VecDeque::new();

        let mut leader_task_handle: Option<tokio::task::JoinHandle<()>> = None;

        // Timers
        let mut heartbeat_interval = tokio::time::interval(std::time::Duration::from_secs(5));
        let mut clock_sync_interval =
            tokio::time::interval(std::time::Duration::from_millis(500));
        let mut discovery_interval =
            tokio::time::interval(std::time::Duration::from_secs(30));
        let mut follower_drain_interval =
            tokio::time::interval(std::time::Duration::from_millis(2));
        let mut status_interval =
            tokio::time::interval(std::time::Duration::from_secs(2));
        let mut health_check_interval =
            tokio::time::interval(std::time::Duration::from_secs(5));
        let mut election_check_interval =
            tokio::time::interval(std::time::Duration::from_millis(500));
        let mut bootstrap_interval =
            tokio::time::interval(std::time::Duration::from_secs(60));
        let mut reconnect_interval =
            tokio::time::interval(std::time::Duration::from_secs(5));

        // mDNS discovery receiver
        let mut discovery_rx = self.discovery_rx.take();

        // Connect to bootstrap peers at startup
        for addr_str in &self.bootstrap_peers {
            if let Ok(addr) = addr_str.parse::<std::net::IpAddr>() {
                info!("Group: connecting to bootstrap peer {}", addr);
                // Use a dummy peer_id that sorts before ours to force connection
                peer_mgr.connect_to_peer(&format!("bootstrap-{}", addr), addr);
            }
        }

        self.broadcast_status(role, &peers);

        loop {
            tokio::select! {
                // ── Peer events from TCP connections ──
                event = peer_events.recv() => {
                    let Some(event) = event else { break };
                    match event {
                        PeerEvent::Connected { peer_id, name, address, channel, writer } => {
                            // Skip self-connections (shouldn't reach here after peer.rs fix, but guard)
                            if peer_id == self.peer_id {
                                info!("Group: ignoring self-connection event");
                                continue;
                            }

                            // If already connected to this peer, drop the duplicate
                            if peers.contains_key(&peer_id) {
                                info!("Group: dropping duplicate connection from {} ({})", name, peer_id);
                                // Track that we expect a spurious Disconnected event
                                *pending_dup_disconnects.entry(peer_id.clone()).or_insert(0) += 1;
                                // Don't register writer — let the channel drop,
                                // which closes the TCP write task and read loop
                                continue;
                            }

                            info!("Group: peer connected: {} ({}) at {}", name, peer_id, address);

                            // Register writer so PeerManager can send to this peer
                            peer_mgr.add_writer(peer_id.clone(), writer);

                            peers.insert(peer_id.clone(), ConnectedPeer {
                                name,
                                address,
                                channel,
                                clock: ClockSync::new(),
                                role: GroupRole::Standalone,
                                last_seen: Instant::now(),
                                missed_heartbeats: 0,
                                packets_received: 0,
                                packet_loss_pct: 0,
                                rtt_variance_us: 0,
                                upstream_id: None,
                                downstream_ids: Vec::new(),
                                hop_count: 0,
                                buffer_health: 100,
                            });

                            // Send gossip: exchange known peers on connect
                            let gossip_entries: Vec<GossipEntry> = peers.iter()
                                .filter(|(id, _)| id.as_str() != peer_id)
                                .map(|(id, p)| GossipEntry {
                                    peer_id: id.clone(),
                                    address: p.address,
                                    port: peer::GROUP_PORT,
                                    group_name: self.group_name.clone(),
                                    channel: p.channel,
                                })
                                .collect();
                            if !gossip_entries.is_empty() {
                                peer_mgr.send_to(&peer_id, &GroupPacket::PeerGossip { peers: gossip_entries });
                            }

                            // If we have audio and just gained a peer while Standalone, trigger election
                            if !active_sources.is_empty() && role == GroupRole::Standalone {
                                let avg_rtt = Self::compute_avg_rtt(&peers);
                                let our_score = election::election_score(avg_rtt, uptime_start.elapsed().as_secs(), &self.peer_id, !active_sources.is_empty());
                                let action = election_state.trigger("peer_connected", &self.peer_id, our_score, peers.len());
                                Self::handle_election_action(&action, &mut peer_mgr, &self.peer_id, our_score);
                            }

                            self.broadcast_status(role, &peers);
                            ctx.health.inc_msg();
                        }
                        PeerEvent::Disconnected { peer_id } => {
                            // Ignore disconnect events from dropped duplicate connections
                            if let Some(count) = pending_dup_disconnects.get_mut(&peer_id) {
                                *count -= 1;
                                if *count == 0 {
                                    pending_dup_disconnects.remove(&peer_id);
                                }
                                info!("Group: ignoring duplicate disconnect for {}", peer_id);
                                continue;
                            }

                            info!("Group: peer disconnected: {}", peer_id);
                            // Save address for reconnect before removing
                            let peer_addr = peers.get(&peer_id).map(|p| p.address);
                            peers.remove(&peer_id);
                            peer_mgr.remove_writer(&peer_id);

                            // Schedule reconnect with exponential backoff
                            if let Some(addr) = peer_addr {
                                reconnect_queue.push_back((
                                    peer_id.clone(),
                                    addr,
                                    Instant::now() + std::time::Duration::from_secs(3),
                                    0,
                                ));
                            }

                            // If the leader disconnected, trigger re-election
                            if leader_id.as_deref() == Some(&peer_id) {
                                info!("Group: leader disconnected, triggering re-election");
                                leader_id = None;
                                if role == GroupRole::Follower {
                                    role = GroupRole::Standalone;
                                    self.network_slot.set_active(false);
                                    jitter_buffer.clear();
                                }

                                // Trigger election if we have active sources and peers
                                if !active_sources.is_empty() && !peers.is_empty() {
                                    let avg_rtt = Self::compute_avg_rtt(&peers);
                                    let our_score = election::election_score(avg_rtt, uptime_start.elapsed().as_secs(), &self.peer_id, !active_sources.is_empty());
                                    let action = election_state.trigger("leader_disconnected", &self.peer_id, our_score, peers.len());
                                    Self::handle_election_action(&action, &mut peer_mgr, &self.peer_id, our_score);
                                }
                            }

                            // If we're leader and all followers disconnected, stop streaming
                            if role == GroupRole::Leader && peers.is_empty() {
                                info!("Group: no more followers, returning to standalone");
                                role = GroupRole::Standalone;
                                self.tap_active.store(false, Ordering::Relaxed);
                                if let Some(h) = leader_task_handle.take() {
                                    h.abort();
                                }
                            }

                            self.broadcast_status(role, &peers);
                            ctx.health.inc_msg();
                        }
                        PeerEvent::Packet { peer_id, packet } => {
                            // Every packet proves liveness — update last_seen
                            if let Some(peer) = peers.get_mut(&peer_id) {
                                peer.last_seen = Instant::now();
                                peer.missed_heartbeats = 0;
                            }
                            match packet {
                                GroupPacket::ClockSyncReq { originate_us } => {
                                    // We're the leader: respond with timestamps
                                    let now = clock::now_us();
                                    let resp = GroupPacket::ClockSyncResp {
                                        originate_us,
                                        receive_us: now,
                                        transmit_us: clock::now_us(),
                                    };
                                    peer_mgr.send_to(&peer_id, &resp);
                                }
                                GroupPacket::ClockSyncResp { originate_us, receive_us, transmit_us } => {
                                    let t4 = clock::now_us();
                                    if let Some(peer) = peers.get_mut(&peer_id) {
                                        peer.clock.process_response(originate_us, receive_us, transmit_us, t4);
                                    }
                                }
                                GroupPacket::LeaderClaim { source, claim_time_us: _ } => {
                                    info!("Group: leader claim from {} (source: {})", peer_id, source);

                                    if role == GroupRole::Leader {
                                        // Conflict: two leaders — lower peer_id wins
                                        if self.peer_id < peer_id {
                                            // We win, reject their claim
                                            info!("Group: rejecting leader claim from {} (our ID is lower)", peer_id);
                                        } else {
                                            // They win, we yield
                                            info!("Group: yielding leadership to {} (their ID is lower)", peer_id);
                                            self.tap_active.store(false, Ordering::Relaxed);
                                            if let Some(h) = leader_task_handle.take() {
                                                h.abort();
                                            }
                                            role = GroupRole::Follower;
                                            leader_id = Some(peer_id.clone());
                                            jitter_buffer.clear();
                                            self.network_slot.set_active(true);
                                        }
                                    } else if role != GroupRole::Follower {
                                        // Accept: enter follower mode (mixed audio — local sources keep playing)
                                        role = GroupRole::Follower;
                                        leader_id = Some(peer_id.clone());
                                        jitter_buffer.clear();
                                        self.network_slot.set_active(true);
                                        info!("Group: entered follower mode (mixed audio)");
                                    }

                                    if let Some(peer) = peers.get_mut(&peer_id) {
                                        peer.role = GroupRole::Leader;
                                    }
                                    self.broadcast_status(role, &peers);
                                }
                                GroupPacket::LeaderRelease => {
                                    info!("Group: leader released by {}", peer_id);
                                    if leader_id.as_deref() == Some(&peer_id) {
                                        leader_id = None;
                                        if role == GroupRole::Follower {
                                            role = GroupRole::Standalone;
                                            self.network_slot.set_active(false);
                                            jitter_buffer.clear();
                                            // Phase 2: No resume needed — local sources were never suspended
                                            info!("Group: returned to standalone");
                                        }
                                    }
                                    if let Some(peer) = peers.get_mut(&peer_id) {
                                        peer.role = GroupRole::Standalone;
                                    }
                                    self.broadcast_status(role, &peers);
                                }
                                GroupPacket::AudioChunk { play_at_us, frame_count: _, hop_count: _, pcm } => {
                                    // Audio normally arrives via UDP, but accept via TCP as fallback
                                    if role == GroupRole::Follower {
                                        jitter_buffer.insert(play_at_us, pcm);
                                    }
                                }
                                GroupPacket::Heartbeat => {
                                    // last_seen already updated at top of Packet handler
                                }
                                GroupPacket::PeerInfo { name, channel } => {
                                    if let Some(peer) = peers.get_mut(&peer_id) {
                                        peer.name = name;
                                        peer.channel = channel;
                                    }
                                    self.broadcast_status(role, &peers);
                                }
                                GroupPacket::GroupConfig { .. } => {
                                    // Remote config update (informational)
                                }
                                GroupPacket::PeerGossip { peers: gossip_peers } => {
                                    for entry in gossip_peers {
                                        // Skip ourselves
                                        if entry.peer_id == self.peer_id { continue; }
                                        // Filter by group name (unless party mode)
                                        if !self.party_mode && entry.group_name != self.group_name { continue; }
                                        // Skip already-connected peers
                                        if peers.contains_key(&entry.peer_id) { continue; }

                                        info!("Group: gossip discovered peer {} at {}", entry.peer_id, entry.address);
                                        discovery_table.upsert(discovery::DiscoveredPeer {
                                            peer_id: entry.peer_id.clone(),
                                            name: String::new(),
                                            address: entry.address,
                                            port: peer::GROUP_PORT,
                                            channel: entry.channel,
                                            group_name: entry.group_name.clone(),
                                            last_seen: Instant::now(),
                                        });
                                        peer_mgr.connect_to_peer(&entry.peer_id, entry.address);
                                    }
                                }
                                GroupPacket::ElectionStart { election_id, trigger: _ } => {
                                    let avg_rtt = Self::compute_avg_rtt(&peers);
                                    let our_score = election::election_score(avg_rtt, uptime_start.elapsed().as_secs(), &self.peer_id, !active_sources.is_empty());
                                    let action = election_state.handle_start(election_id, &self.peer_id, our_score, peers.len());
                                    Self::handle_election_action(&action, &mut peer_mgr, &self.peer_id, our_score);
                                }
                                GroupPacket::ElectionVote { election_id, score, peer_id: voter_id } => {
                                    let action = election_state.handle_vote(election_id, &voter_id, score, &self.peer_id);
                                    match action {
                                        ElectionAction::Winner { election_id: eid, ref winner_id, ref source } => {
                                            Self::apply_election_result(
                                                &self.peer_id, winner_id, eid, source,
                                                &mut role, &mut leader_id, &mut peer_mgr, &peers,
                                                &mut leader_task_handle, &self.network_tap, &self.tap_active,
                                                &udp_socket, self.buffer_ms as u64,
                                                &self.network_slot, &mut jitter_buffer,
                                            );
                                            self.broadcast_status(role, &peers);
                                        }
                                        _ => {}
                                    }
                                }
                                GroupPacket::ElectionResult { election_id, winner_id, source } => {
                                    let action = election_state.handle_result(election_id, &winner_id, &source);
                                    match action {
                                        ElectionAction::Winner { election_id: eid, ref winner_id, ref source } => {
                                            Self::apply_election_result(
                                                &self.peer_id, winner_id, eid, source,
                                                &mut role, &mut leader_id, &mut peer_mgr, &peers,
                                                &mut leader_task_handle, &self.network_tap, &self.tap_active,
                                                &udp_socket, self.buffer_ms as u64,
                                                &self.network_slot, &mut jitter_buffer,
                                            );
                                            self.broadcast_status(role, &peers);
                                        }
                                        _ => {}
                                    }
                                }
                                GroupPacket::HealthPing { avg_rtt_us: _, uptime_secs: _, active_sources: _, buffer_health, packet_loss_pct } => {
                                    // last_seen already updated at top of Packet handler
                                    if let Some(peer) = peers.get_mut(&peer_id) {
                                        peer.buffer_health = buffer_health;
                                        peer.packet_loss_pct = packet_loss_pct;
                                    }
                                }
                                GroupPacket::RelayAssignment { target_peer_id, relay_peer_id } => {
                                    info!("Group: relay assignment: {} via {}", target_peer_id, relay_peer_id);
                                    if let Some(peer) = peers.get_mut(&target_peer_id) {
                                        peer.upstream_id = Some(relay_peer_id.clone());
                                    }
                                    if let Some(relay_peer) = peers.get_mut(&relay_peer_id) {
                                        if !relay_peer.downstream_ids.contains(&target_peer_id) {
                                            relay_peer.downstream_ids.push(target_peer_id);
                                        }
                                    }
                                }
                                GroupPacket::VolumeSync { master_volume, originator } => {
                                    // Anti-loop: don't apply if we originated this
                                    if originator != self.peer_id {
                                        info!("Group: volume sync from {}: {}", originator, master_volume);
                                        if let Some(ref tx) = self.volume_set_tx {
                                            let _ = tx.try_send(master_volume);
                                        }
                                    }
                                }
                                GroupPacket::PlayPause { source: _, action } => {
                                    // Only the leader handles play/pause from followers
                                    if role == GroupRole::Leader {
                                        info!("Group: play/pause from follower {}: {}", peer_id, action);
                                        if let Some(ref tx) = self.spotify_cmd_tx {
                                            use encore_common::protocol::SpotifyAction;
                                            match action.as_str() {
                                                "play" => { let _ = tx.try_send(SpotifyAction::Play); }
                                                "pause" => { let _ = tx.try_send(SpotifyAction::Pause); }
                                                "next" => { let _ = tx.try_send(SpotifyAction::Next); }
                                                "prev" => { let _ = tx.try_send(SpotifyAction::Previous); }
                                                _ => {}
                                            }
                                        }
                                    }
                                }
                            }
                            ctx.health.inc_msg();
                        }
                    }
                }

                // ── UDP audio from leader (follower receives) ──
                audio = udp_audio_rx.recv() => {
                    if let Some((play_at_us, pcm)) = audio {
                        if role == GroupRole::Follower {
                            jitter_buffer.insert(play_at_us, pcm);
                            // UDP audio proves leader is alive
                            if let Some(ref lid) = leader_id {
                                if let Some(peer) = peers.get_mut(lid.as_str()) {
                                    peer.last_seen = Instant::now();
                                    peer.missed_heartbeats = 0;
                                }
                            }
                        }
                    }
                }

                // ── Commands from other subsystems ──
                cmd = self.cmd_rx.recv() => {
                    let Some(cmd) = cmd else { break };
                    match cmd {
                        GroupCmd::LocalAudioStarted { source } => {
                            active_sources.insert(source.clone());
                            info!("Group: local audio started ({}), active sources: {:?}", source, active_sources);

                            // Audio source should always be the leader
                            if !peers.is_empty() && role != GroupRole::Leader {
                                let avg_rtt = Self::compute_avg_rtt(&peers);
                                let our_score = election::election_score(avg_rtt, uptime_start.elapsed().as_secs(), &self.peer_id, true);
                                let action = election_state.trigger(&source, &self.peer_id, our_score, peers.len());
                                Self::handle_election_action(&action, &mut peer_mgr, &self.peer_id, our_score);
                            }
                        }
                        GroupCmd::LocalAudioStopped { source } => {
                            active_sources.remove(&source);
                            info!("Group: local audio stopped ({}), active sources: {:?}", source, active_sources);

                            // Only release leadership when ALL local sources stop
                            if role == GroupRole::Leader && active_sources.is_empty() {
                                info!("Group: all sources stopped, releasing leadership");
                                role = GroupRole::Standalone;
                                self.tap_active.store(false, Ordering::Relaxed);
                                if let Some(h) = leader_task_handle.take() {
                                    h.abort();
                                }

                                let release = GroupPacket::LeaderRelease;
                                peer_mgr.broadcast(&release);

                                self.broadcast_status(role, &peers);
                            }
                        }
                        GroupCmd::SetVolume(vol) => {
                            self.volume = vol;
                            // Apply locally
                            if let Some(ref tx) = self.volume_set_tx {
                                let _ = tx.try_send(vol);
                            }
                            // Broadcast to all peers (with originator for anti-loop)
                            let sync_pkt = GroupPacket::VolumeSync {
                                master_volume: vol,
                                originator: self.peer_id.clone(),
                            };
                            peer_mgr.broadcast(&sync_pkt);
                        }
                        GroupCmd::SetChannel(ch) => {
                            self.channel = ch;
                            jitter_buffer.set_channel(ch);
                            // Inform peers
                            let info_pkt = GroupPacket::PeerInfo {
                                name: self.device_name.clone(),
                                channel: ch,
                            };
                            peer_mgr.broadcast(&info_pkt);
                            self.broadcast_status(role, &peers);
                        }
                        GroupCmd::SetBufferMs(ms) => {
                            self.buffer_ms = ms;
                            jitter_buffer.set_buffer_ms(ms);
                        }
                        GroupCmd::SetGroupName(name) => {
                            self.group_name = name;
                            self.broadcast_status(role, &peers);
                        }
                        GroupCmd::RequestStatus => {
                            self.broadcast_status(role, &peers);
                        }
                        GroupCmd::PlayPause { action } => {
                            if role == GroupRole::Leader {
                                // We're the leader — route to local source
                                if let Some(ref tx) = self.spotify_cmd_tx {
                                    use encore_common::protocol::SpotifyAction;
                                    match action.as_str() {
                                        "play" => { let _ = tx.try_send(SpotifyAction::Play); }
                                        "pause" => { let _ = tx.try_send(SpotifyAction::Pause); }
                                        "next" => { let _ = tx.try_send(SpotifyAction::Next); }
                                        "prev" => { let _ = tx.try_send(SpotifyAction::Previous); }
                                        _ => {}
                                    }
                                }
                            } else if role == GroupRole::Follower {
                                // We're a follower — forward to leader
                                if let Some(ref lid) = leader_id {
                                    peer_mgr.send_to(lid, &GroupPacket::PlayPause {
                                        source: "group".into(),
                                        action,
                                    });
                                }
                            }
                        }
                        GroupCmd::SetPartyMode(enabled) => {
                            info!("Group: SetPartyMode({})", enabled);
                            self.party_mode = enabled;
                            // Save to config
                            let config_path = std::path::Path::new("/lsync/encore/config.toml");
                            if let Ok(mut cfg) = encore_common::config::EncoreConfigFile::load(config_path) {
                                cfg.group.party_mode = enabled;
                                let _ = cfg.save(config_path);
                            }
                            self.broadcast_status(role, &peers);
                        }
                        GroupCmd::SetEnabled(enabled) => {
                            self.enabled = enabled;
                            if !enabled {
                                // Return to standalone, disconnect peers
                                if role == GroupRole::Leader {
                                    self.tap_active.store(false, Ordering::Relaxed);
                                    if let Some(h) = leader_task_handle.take() {
                                        h.abort();
                                    }
                                    peer_mgr.broadcast(&GroupPacket::LeaderRelease);
                                } else if role == GroupRole::Follower {
                                    self.network_slot.set_active(false);
                                    jitter_buffer.clear();
                                }
                                role = GroupRole::Standalone;
                                self.broadcast_status(role, &peers);
                            } else {
                                // Re-enabled: trigger election if we have audio and peers
                                if !active_sources.is_empty() && !peers.is_empty() && role == GroupRole::Standalone {
                                    let avg_rtt = Self::compute_avg_rtt(&peers);
                                    let our_score = election::election_score(avg_rtt, uptime_start.elapsed().as_secs(), &self.peer_id, !active_sources.is_empty());
                                    let action = election_state.trigger("re_enabled", &self.peer_id, our_score, peers.len());
                                    Self::handle_election_action(&action, &mut peer_mgr, &self.peer_id, our_score);
                                }
                                self.broadcast_status(role, &peers);
                            }
                        }
                    }
                    ctx.health.inc_msg();
                }

                // ── Heartbeat / health ping to all peers ──
                _ = heartbeat_interval.tick() => {
                    if !peers.is_empty() {
                        let avg_rtt = Self::compute_avg_rtt(&peers);
                        let health_pkt = GroupPacket::HealthPing {
                            avg_rtt_us: avg_rtt,
                            uptime_secs: uptime_start.elapsed().as_secs(),
                            active_sources: active_sources.len() as u8,
                            buffer_health: if role == GroupRole::Follower {
                                jitter_buffer.health_percent()
                            } else {
                                100
                            },
                            packet_loss_pct: 0,
                        };
                        peer_mgr.broadcast(&health_pkt);
                    }
                }

                // ── Clock sync (bidirectional — all roles measure RTT to peers) ──
                _ = clock_sync_interval.tick() => {
                    if role == GroupRole::Follower {
                        // Follower syncs to leader for audio timing
                        if let Some(ref lid) = leader_id {
                            let req = GroupPacket::ClockSyncReq {
                                originate_us: clock::now_us(),
                            };
                            peer_mgr.send_to(lid, &req);

                            // Adapt sync rate based on convergence
                            if let Some(peer) = peers.get(lid) {
                                let desired_us = peer.clock.sync_interval_us();
                                let desired = std::time::Duration::from_micros(desired_us);
                                clock_sync_interval = tokio::time::interval(desired);
                            }
                        }
                    } else if !peers.is_empty() {
                        // Leader/Standalone syncs to all peers for RTT measurement
                        let req = GroupPacket::ClockSyncReq {
                            originate_us: clock::now_us(),
                        };
                        peer_mgr.broadcast(&req);
                    }
                }

                // ── Discovery: expire old peers ──
                _ = discovery_interval.tick() => {
                    discovery_table.expire();
                }

                // ── Follower: drain jitter buffer into network_slot ──
                _ = follower_drain_interval.tick() => {
                    if role == GroupRole::Follower {
                        if let Some(ref lid) = leader_id {
                            if let Some(peer) = peers.get(lid) {
                                let now = clock::now_us();
                                let samples = jitter_buffer.drain_ready(now, &peer.clock);
                                if !samples.is_empty() {
                                    self.network_slot.push(&samples);
                                }
                            }
                        }
                    }
                }

                // ── Health check: detect dead peers ──
                _ = health_check_interval.tick() => {
                    let mut dead_peers = Vec::new();
                    for (id, peer) in peers.iter_mut() {
                        if peer.last_seen.elapsed().as_secs() > 15 {
                            peer.missed_heartbeats += 1;
                            if peer.missed_heartbeats >= 2 {
                                warn!("Group: peer {} timed out ({}s)", id, peer.last_seen.elapsed().as_secs());
                                dead_peers.push(id.clone());
                            }
                        }
                    }
                    for dead_id in dead_peers {
                        peers.remove(&dead_id);
                        peer_mgr.remove_writer(&dead_id);

                        if leader_id.as_deref() == Some(&dead_id) {
                            info!("Group: leader timed out, triggering re-election");
                            leader_id = None;
                            if role == GroupRole::Follower {
                                role = GroupRole::Standalone;
                                self.network_slot.set_active(false);
                                jitter_buffer.clear();
                            }
                            if !active_sources.is_empty() && !peers.is_empty() {
                                let avg_rtt = Self::compute_avg_rtt(&peers);
                                let our_score = election::election_score(avg_rtt, uptime_start.elapsed().as_secs(), &self.peer_id, !active_sources.is_empty());
                                let action = election_state.trigger("leader_timeout", &self.peer_id, our_score, peers.len());
                                Self::handle_election_action(&action, &mut peer_mgr, &self.peer_id, our_score);
                            }
                        }

                        if role == GroupRole::Leader && peers.is_empty() {
                            role = GroupRole::Standalone;
                            self.tap_active.store(false, Ordering::Relaxed);
                            if let Some(h) = leader_task_handle.take() {
                                h.abort();
                            }
                        }
                        self.broadcast_status(role, &peers);
                    }
                }

                // ── Election timeout check ──
                _ = election_check_interval.tick() => {
                    let action = election_state.check_timeout(&self.peer_id);
                    match action {
                        ElectionAction::Winner { election_id, ref winner_id, ref source } => {
                            Self::apply_election_result(
                                &self.peer_id, winner_id, election_id, source,
                                &mut role, &mut leader_id, &mut peer_mgr, &peers,
                                &mut leader_task_handle, &self.network_tap, &self.tap_active,
                                &udp_socket, self.buffer_ms as u64,
                                &self.network_slot, &mut jitter_buffer,
                            );
                            self.broadcast_status(role, &peers);
                        }
                        _ => {}
                    }
                }

                // ── mDNS discovery: auto-connect discovered peers ──
                disc_event = async {
                    if let Some(ref mut rx) = discovery_rx {
                        rx.recv().await
                    } else {
                        std::future::pending::<Option<discovery::DiscoveryEvent>>().await
                    }
                } => {
                    if let Some(disc) = disc_event {
                        // Skip self-discovery
                        if disc.peer_id == self.peer_id {
                            continue;
                        }
                        // Skip unknown/empty peer IDs (mDNS without TXT records)
                        if disc.peer_id.is_empty() || disc.peer_id == "unknown" {
                            continue;
                        }
                        // Skip already-connected peers
                        if peers.contains_key(&disc.peer_id) {
                            continue;
                        }
                        // Filter by group name (unless party mode)
                        if self.party_mode || disc.group_name == self.group_name || disc.group_name.is_empty() {
                            info!("Group: mDNS discovered peer {} ({}) at {}", disc.name, disc.peer_id, disc.address);
                            let channel = match disc.channel.as_str() {
                                "left" => wire::ChannelAssignment::Left,
                                "right" => wire::ChannelAssignment::Right,
                                _ => wire::ChannelAssignment::Stereo,
                            };
                            discovery_table.upsert(discovery::DiscoveredPeer {
                                peer_id: disc.peer_id.clone(),
                                name: disc.name,
                                address: disc.address,
                                port: disc.port,
                                channel,
                                group_name: disc.group_name,
                                last_seen: Instant::now(),
                            });
                            peer_mgr.connect_to_peer(&disc.peer_id, disc.address);
                        }
                    }
                }

                // ── Reconnect queue: retry disconnected peers ──
                _ = reconnect_interval.tick() => {
                    let now = Instant::now();
                    let mut retry_later = VecDeque::new();
                    while let Some((pid, addr, next_at, attempts)) = reconnect_queue.pop_front() {
                        if now < next_at {
                            retry_later.push_back((pid, addr, next_at, attempts));
                            continue;
                        }
                        // Already reconnected?
                        if peers.contains_key(&pid) {
                            continue;
                        }
                        // Max 5 attempts
                        if attempts >= 5 {
                            info!("Group: giving up reconnect to {} after {} attempts", pid, attempts);
                            continue;
                        }
                        info!("Group: reconnecting to {} (attempt {})", pid, attempts + 1);
                        peer_mgr.connect_to_peer(&pid, addr);
                        // Exponential backoff: 3s, 6s, 12s, 24s, 48s
                        let backoff = std::time::Duration::from_secs(3 * (1 << attempts));
                        retry_later.push_back((pid, addr, now + backoff, attempts + 1));
                    }
                    reconnect_queue = retry_later;
                }

                // ── Bootstrap peer reconnection ──
                _ = bootstrap_interval.tick() => {
                    for addr_str in &self.bootstrap_peers {
                        if let Ok(addr) = addr_str.parse::<std::net::IpAddr>() {
                            // Only reconnect if not already connected to anyone at this address
                            let already_connected = peers.values().any(|p| p.address == addr);
                            if !already_connected {
                                peer_mgr.connect_to_peer(&format!("bootstrap-{}", addr), addr);
                            }
                        }
                    }
                }

                // ── Periodic status broadcast ──
                _ = status_interval.tick() => {
                    self.broadcast_status(role, &peers);
                    ctx.health.beat(
                        std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_secs(),
                    );
                }

                // ── Shutdown ──
                _ = ctx.shutdown.recv() => {
                    info!("Group: shutdown");
                    break;
                }
            }
        }

        // Cleanup
        if role == GroupRole::Leader {
            self.tap_active.store(false, Ordering::Relaxed);
            if let Some(h) = leader_task_handle.take() {
                h.abort();
            }
            peer_mgr.broadcast(&GroupPacket::LeaderRelease);
        } else if role == GroupRole::Follower {
            self.network_slot.set_active(false);
        }

        Ok(())
    }
}
