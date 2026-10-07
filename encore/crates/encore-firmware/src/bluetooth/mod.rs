//! Bluetooth A2DP sink subsystem - pure interface
//!
//! Manages the entire Bluetooth lifecycle using three kernel sockets:
//! 1. Management API (HCI channel 3) — adapter config, pairing, link keys
//! 2. SDP server (L2CAP PSM 1) — advertises A2DP Sink service record
//! 3. AVDTP (L2CAP PSM 25) — codec negotiation and media transport
//!
//! Incoming A2DP audio is decoded (RTP → SBC/aptX/aptX HD → PCM) and pushed into a MixerSlot for playback.

#[cfg(target_os = "linux")]
mod aptx;
mod avdtp;
mod avrcp;
mod l2cap;
mod mgmt;
#[cfg(target_os = "linux")]
mod sbc;
mod sdp;
#[cfg(target_os = "linux")]
mod transport;

use crate::audio::mixer::MixerSlot;
use crate::subsystem::{Subsystem, SubsystemContext};
use anyhow::{Context, Result};
use encore_common::protocol::SubsystemState;
use std::os::fd::AsRawFd;
use std::sync::Arc;
use tokio::sync::{mpsc, watch};
use tokio::time::{interval, Duration};
use tracing::{debug, info, warn};

/// Which A2DP codec was negotiated. Lives in `encore_common::avdtp` (pure,
/// host-tested); re-exported here so existing `crate::bluetooth::A2dpCodec` /
/// `super::A2dpCodec` references need no change.
pub use encore_common::avdtp::A2dpCodec;

/// Path to the bt8xxx kernel module.
const BT_MODULE_PATH: &str =
    "/lib/modules/3.8.13-yocto-standard/kernel/arch/arm/mach-berlin/modules/bt_sd8887/bt8xxx.ko";
/// HCI sysfs path — appears when bt8xxx is loaded.
const HCI_SYSFS_PATH: &str = "/sys/class/bluetooth/hci0";
/// WiFi MAC sysfs path — used to derive BT MAC.
const WIFI_MAC_PATH: &str = "/sys/class/net/wlan0/address";
/// Remembers the last connected source so the speaker can re-page it on boot.
#[cfg(target_os = "linux")]
const BT_LAST_PATH: &str = "/lsync/encore/bt_last";

const POLL_INTERVAL: Duration = Duration::from_secs(5);

/// Persist the address of the source we just connected to (atomic write).
#[cfg(target_os = "linux")]
fn save_last_device(addr: &str) {
    let tmp = format!("{}.tmp", BT_LAST_PATH);
    if std::fs::write(&tmp, addr).is_ok() {
        let _ = std::fs::rename(&tmp, BT_LAST_PATH);
    }
}

/// Persist a new "visible as" Bluetooth name to the config so it survives reboot.
#[cfg(target_os = "linux")]
fn persist_device_name(name: &str) {
    let path = std::path::Path::new("/lsync/encore/config.toml");
    let mut cfg = encore_common::config::EncoreConfigFile::load(path).unwrap_or_default();
    cfg.device.name = name.to_string();
    if let Err(e) = cfg.save(path) {
        warn!("Bluetooth: failed to persist device name: {}", e);
    }
}

/// The address of the last source we connected to, if any.
#[cfg(target_os = "linux")]
fn load_last_device() -> Option<String> {
    std::fs::read_to_string(BT_LAST_PATH)
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

const BT_NAMES_PATH: &str = "/lsync/encore/bt_names";

/// Load the persisted MAC -> friendly-name map (tab-separated `MAC\tname` lines)
/// so the paired list can show readable names across reboots.
fn load_paired_names() -> std::collections::HashMap<String, String> {
    let mut m = std::collections::HashMap::new();
    if let Ok(s) = std::fs::read_to_string(BT_NAMES_PATH) {
        for line in s.lines() {
            if let Some((addr, name)) = line.split_once('\t') {
                if !addr.is_empty() && !name.is_empty() {
                    m.insert(addr.to_string(), name.to_string());
                }
            }
        }
    }
    m
}

/// Persist the MAC -> friendly-name map (atomic write so an interrupted save
/// can't truncate the file).
fn save_paired_names(map: &std::collections::HashMap<String, String>) {
    let mut out = String::new();
    for (addr, name) in map {
        out.push_str(addr);
        out.push('\t');
        out.push_str(name);
        out.push('\n');
    }
    let tmp = format!("{}.tmp", BT_NAMES_PATH);
    if std::fs::write(&tmp, out).is_ok() {
        let _ = std::fs::rename(&tmp, BT_NAMES_PATH);
    }
}

/// Bluetooth connection state.
#[derive(Debug, Clone, PartialEq)]
pub enum BtState {
    Off,
    Discoverable,
    Connected { name: String, address: String },
}

/// Bluetooth A2DP sink subsystem.
pub struct BluetoothSubsystem {
    slot: Arc<MixerSlot>,
    device_name: String,
    state: BtState,
    /// Codec negotiated for the active stream (for dashboard status).
    cur_codec: Option<A2dpCodec>,
    /// True while audio is actively streaming (false when paused).
    playing: bool,
    /// Bonded devices (address + persisted friendly name), mirrored from mgmt
    /// for dashboard status.
    paired: Vec<encore_common::protocol::BtPairedDevice>,
    /// Persisted MAC -> friendly-name map, so the paired list shows readable
    /// names across reboots (link keys store only the address).
    paired_names: std::collections::HashMap<String, String>,
    /// Remaining boot-time auto-reconnect attempts (the source may not be ready
    /// the instant we boot). Counts down on the poll ticker, zeroed on connect.
    auto_reconnect_retries: u8,
    /// Friendly name (or address) of the source being auto-reconnected, for the
    /// dashboard "Reconnecting to X…" hint. Only surfaced while
    /// `auto_reconnect_retries > 0` and nothing is connected.
    reconnect_name: Option<String>,
    /// Mesh name override: `Some(group_name)` when grouped and mesh is on, else
    /// `None` (advertise our own `device_name`).
    mesh_name: Option<String>,
    /// Whether we may be BT-discoverable. Set false only when meshed and this
    /// node is not the coordinator, so a phone sees the group as one device.
    /// Defaults true (a normal, un-meshed speaker is discoverable when idle).
    mesh_discoverable_allowed: bool,
    /// Last name applied via SET_LOCAL_NAME, to skip redundant updates.
    applied_name: String,
    /// Name sent to SET_LOCAL_NAME but not yet confirmed by CMD_COMPLETE;
    /// `applied_name` is only updated once the controller accepts it.
    pending_bt_name: Option<String>,
    /// Receives mesh-name updates from the group subsystem.
    bt_name_rx: Option<mpsc::Receiver<crate::group::BtMeshSignal>>,
    /// AVRCP absolute volume: the controller (source) sets our master volume
    /// (0..=100) through this; main.rs forwards it to the LED volume authority.
    avrcp_vol_tx: Option<mpsc::Sender<u8>>,
    cmd_rx: Option<mpsc::Receiver<encore_common::protocol::BtAction>>,
    ws_tx: Option<tokio::sync::broadcast::Sender<String>>,
    suspend_rx: Option<watch::Receiver<bool>>,
    group_cmd_tx: Option<mpsc::Sender<crate::group::GroupCmd>>,
    /// A2DP latency-servo target (ms), resolved from config at construction.
    latency_target_ms: u32,
}

impl BluetoothSubsystem {
    pub fn new(
        slot: Arc<MixerSlot>,
        device_name: Option<String>,
        cmd_rx: Option<mpsc::Receiver<encore_common::protocol::BtAction>>,
        ws_tx: Option<tokio::sync::broadcast::Sender<String>>,
    ) -> Self {
        let device_name = device_name.unwrap_or_else(|| "Encore".to_string());
        Self {
            slot,
            applied_name: device_name.clone(),
            pending_bt_name: None,
            device_name,
            state: BtState::Off,
            cur_codec: None,
            playing: false,
            paired: Vec::new(),
            paired_names: load_paired_names(),
            auto_reconnect_retries: 0,
            reconnect_name: None,
            mesh_name: None,
            mesh_discoverable_allowed: true,
            bt_name_rx: None,
            avrcp_vol_tx: None,
            cmd_rx,
            ws_tx,
            suspend_rx: None,
            group_cmd_tx: None,
            latency_target_ms: transport::resolve_latency_target_ms(0),
        }
    }

    /// Set the A2DP latency-servo target from config (0 = default).
    pub fn set_latency_target_ms(&mut self, configured: u16) {
        self.latency_target_ms = transport::resolve_latency_target_ms(configured);
    }

    /// Set the channel that receives mesh-name updates from the group subsystem.
    pub fn set_bt_name_rx(&mut self, rx: mpsc::Receiver<crate::group::BtMeshSignal>) {
        self.bt_name_rx = Some(rx);
    }

    /// Set the channel AVRCP uses to push absolute-volume changes (0..=100).
    pub fn set_avrcp_vol_tx(&mut self, tx: mpsc::Sender<u8>) {
        self.avrcp_vol_tx = Some(tx);
    }

    /// Set the suspend channel for group sync (follower mode pauses BT).
    pub fn set_suspend_rx(&mut self, rx: watch::Receiver<bool>) {
        self.suspend_rx = Some(rx);
    }

    /// Set the group command channel for notifying the group subsystem.
    pub fn set_group_tx(&mut self, tx: mpsc::Sender<crate::group::GroupCmd>) {
        self.group_cmd_tx = Some(tx);
    }

    fn broadcast_bt_event(&self, event: encore_common::protocol::BtEvent) {
        if let Some(ref tx) = self.ws_tx {
            let msg = encore_common::protocol::ServerMsg::BluetoothEvent(event);
            if let Ok(json) = serde_json::to_string(&msg) {
                let _ = tx.send(json);
            }
        }
    }

    /// Broadcast the current connection + streaming status to dashboards.
    /// Sent on every state change and rebroadcast periodically so a
    /// late-joining client sees the live codec/playing state, not just events.
    fn broadcast_bt_status(&self) {
        let connected = if let BtState::Connected { name, address } = &self.state {
            Some(encore_common::protocol::BtConnectedDevice {
                name: name.clone(),
                addr: address.clone(),
                codec: self.cur_codec.map(|c| c.to_string()).unwrap_or_default(),
            })
        } else {
            None
        };
        // Reconnecting = the boot-time paging window is still open and nothing
        // has connected. Derived from the retry budget so the four sites that
        // zero it (connect, cancel paths, natural exhaustion) all end the hint
        // without needing to clear reconnect_name.
        let reconnecting = if self.auto_reconnect_retries > 0 && connected.is_none() {
            self.reconnect_name.clone()
        } else {
            None
        };
        // Buffered audio depth = how far this speaker is behind the live
        // stream; the latency servo holds it at its target while streaming.
        let latency_ms = if self.playing {
            (self.slot.available() / 96).min(u16::MAX as usize) as u16
        } else {
            0
        };
        let status = encore_common::protocol::BtStatus {
            connected,
            playing: self.playing,
            paired: self.paired.clone(),
            name: self.applied_name.clone(),
            reconnecting,
            // The advertised name is the group name whenever a mesh name is set.
            meshed: self.mesh_name.is_some(),
            latency_ms,
        };
        if let Some(ref tx) = self.ws_tx {
            let msg = encore_common::protocol::ServerMsg::BluetoothStatus(status);
            if let Ok(json) = serde_json::to_string(&msg) {
                let _ = tx.send(json);
            }
        }
    }

    /// Mirror the bonded-device list out of the mgmt socket so dashboard status
    /// reflects pairings (call after setup and whenever link keys change).
    #[cfg(target_os = "linux")]
    fn refresh_paired(&mut self, mgmt: &mgmt::MgmtSocket) {
        self.paired = mgmt
            .paired_addrs()
            .iter()
            .map(|a| {
                let addr = l2cap::bdaddr_to_string(a);
                let name = self.paired_names.get(&addr).cloned().unwrap_or_default();
                encore_common::protocol::BtPairedDevice { addr, name }
            })
            .collect();
        // Keep the name cache aligned to bonded devices (plus the live
        // connection, which may not be bonded yet) so names a scan once cached
        // for transient/foreign devices don't linger — and existing cruft from
        // before the DeviceFound fix is dropped on the next refresh.
        let keep: std::collections::HashSet<String> =
            self.paired.iter().map(|p| p.addr.clone()).collect();
        let connected = match &self.state {
            BtState::Connected { address, .. } => Some(address.clone()),
            _ => None,
        };
        let before = self.paired_names.len();
        self.paired_names
            .retain(|addr, _| keep.contains(addr) || connected.as_deref() == Some(addr));
        if self.paired_names.len() != before {
            save_paired_names(&self.paired_names);
        }
    }

    /// Record a device's friendly name (learned on connect) so the paired list
    /// shows it across reboots. Returns true if the map changed.
    #[cfg(target_os = "linux")]
    fn remember_name(&mut self, addr: &str, name: &str) -> bool {
        if name.is_empty() || name == "Unknown" || name == addr {
            return false;
        }
        if self.paired_names.get(addr).map(String::as_str) == Some(name) {
            return false;
        }
        self.paired_names.insert(addr.to_string(), name.to_string());
        save_paired_names(&self.paired_names);
        true
    }

    /// Apply the effective BT name — the mesh group name when set, else our own
    /// device name — via SET_LOCAL_NAME, skipping redundant updates.
    #[cfg(target_os = "linux")]
    async fn apply_bt_name(&mut self, mgmt: &mgmt::MgmtSocket) {
        let desired = self
            .mesh_name
            .clone()
            .unwrap_or_else(|| self.device_name.clone());
        // Send only if it's not already on air and not already in flight; commit
        // `applied_name` (and report it) from the SET_LOCAL_NAME CMD_COMPLETE, so a
        // controller-rejected name isn't reported as the live one.
        if desired != self.applied_name && self.pending_bt_name.as_deref() != Some(&desired) {
            info!("Bluetooth: advertised name -> {} (pending)", desired);
            mgmt.set_local_name(&desired).await;
            self.pending_bt_name = Some(desired);
        }
    }

    /// Set discoverability from the current idle state and mesh permission.
    /// Discoverable only when idle (state Discoverable) AND allowed — when meshed
    /// only the coordinator is allowed, so the group shows as one BT device.
    async fn apply_discoverable(&self, mgmt: &mgmt::MgmtSocket) {
        let want = self.mesh_discoverable_allowed && matches!(self.state, BtState::Discoverable);
        mgmt.set_discoverable(want).await;
    }
}

#[async_trait::async_trait]
impl Subsystem for BluetoothSubsystem {
    fn name(&self) -> &'static str {
        "bluetooth"
    }

    async fn run(&mut self, mut ctx: SubsystemContext) -> Result<()> {
        ctx.health.set_state(SubsystemState::Running);

        // ── Phase 1: Hardware setup ──
        // Load kernel module and wait for HCI adapter.
        #[cfg(target_os = "linux")]
        {
            if let Err(e) = ensure_bt_module_loaded().await {
                warn!(
                    "Bluetooth: module load failed (may already be loaded): {}",
                    e
                );
            }
            if let Err(e) = wait_for_hci().await {
                warn!("Bluetooth: hci0 not found ({}), continuing anyway", e);
            }
        }

        // ── Phase 1.5: Unblock rfkill, bring adapter UP, set device class ──
        #[cfg(target_os = "linux")]
        {
            unblock_bt_rfkill().await; // This may not affect states anymore
            hci_dev_up().await;
            set_device_class_hci();
        }

        // ── Phase 2: Management socket — configure adapter ──
        #[cfg(target_os = "linux")]
        let mut mgmt = {
            let m = mgmt::MgmtSocket::open().context("failed to open management socket")?;
            info!("Bluetooth: management socket opened");
            m
        };

        #[cfg(target_os = "linux")]
        {
            match mgmt.setup_adapter(&self.device_name).await {
                Ok(bdaddr) => {
                    info!(
                        "Bluetooth: adapter ready ({})",
                        l2cap::bdaddr_to_string(&bdaddr)
                    );
                    self.state = BtState::Discoverable;
                    self.refresh_paired(&mgmt);
                }
                Err(e) => {
                    return Err(e).context("adapter setup failed");
                }
            }
        }

        // Shared owning handle to the active AVCTP socket, for subsystem-driven
        // AVRCP transport keys / volume notifications. An Arc<OwnedFd> (not a raw
        // int) so a writer holding a clone keeps the fd alive across its write —
        // the session can't close it mid-write (SEQPACKET keeps writes atomic).
        #[cfg(target_os = "linux")]
        let avrcp_fd = std::sync::Arc::new(std::sync::Mutex::new(
            None::<std::sync::Arc<std::os::fd::OwnedFd>>,
        ));
        // Volume-changed notification registration (controller -> us): the label
        // it registered on, and whether a notification is currently armed.
        #[cfg(target_os = "linux")]
        let avrcp_vol_label = std::sync::Arc::new(std::sync::atomic::AtomicU8::new(0));
        #[cfg(target_os = "linux")]
        let avrcp_vol_registered = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));

        // ── Phase 3: Open SDP and AVDTP sockets ──
        #[cfg(target_os = "linux")]
        let (avdtp_tx, mut avdtp_rx) = {
            // SDP server on L2CAP PSM 1
            let sdp_fd = l2cap::l2cap_socket().context("SDP: socket")?;
            l2cap::l2cap_bind(sdp_fd.as_raw_fd(), 1).context("SDP: bind PSM 1")?;
            l2cap::l2cap_listen(sdp_fd.as_raw_fd(), 1).context("SDP: listen")?;
            sdp::spawn_sdp_server(sdp_fd);
            info!("Bluetooth: SDP server started on PSM 1");

            // AVDTP on L2CAP PSM 25
            let avdtp_fd = l2cap::l2cap_socket().context("AVDTP: socket")?;
            // Require auth + encryption for A2DP
            l2cap::l2cap_set_link_mode(
                avdtp_fd.as_raw_fd(),
                l2cap::L2CAP_LM_AUTH | l2cap::L2CAP_LM_ENCRYPT,
            )
            .context("AVDTP: set link mode")?;
            l2cap::l2cap_bind(avdtp_fd.as_raw_fd(), 25).context("AVDTP: bind PSM 25")?;
            l2cap::l2cap_listen(avdtp_fd.as_raw_fd(), 2).context("AVDTP: listen")?;

            let (avdtp_tx, rx) =
                avdtp::spawn_avdtp(avdtp_fd, self.slot.clone(), self.latency_target_ms);
            info!("Bluetooth: AVDTP listening on PSM 25");

            // AVCTP (AVRCP control) on L2CAP PSM 23. Permissive link mode for
            // now — some sources connect AVRCP without encryption, and the
            // audio path is independent, so accept whatever connects and log.
            let avctp_fd = l2cap::l2cap_socket().context("AVCTP: socket")?;
            l2cap::l2cap_bind(avctp_fd.as_raw_fd(), avrcp::PSM_AVCTP).context("AVCTP: bind")?;
            l2cap::l2cap_listen(avctp_fd.as_raw_fd(), 2).context("AVCTP: listen")?;
            // AVRCP target: absolute volume -> master volume. Seed current volume
            // at mid-scale; the controller overwrites it on connect.
            let avrcp_cur_vol = std::sync::Arc::new(std::sync::atomic::AtomicU8::new(50));
            avrcp::spawn_avctp(
                avctp_fd,
                self.avrcp_vol_tx.clone(),
                avrcp_cur_vol,
                self.ws_tx.clone(),
                avrcp_fd.clone(),
                avrcp_vol_label.clone(),
                avrcp_vol_registered.clone(),
            );
            info!(
                "Bluetooth: AVCTP target listening on PSM {}",
                avrcp::PSM_AVCTP
            );

            (avdtp_tx, rx)
        };

        // ── Phase 4: Event loop ──
        #[cfg(target_os = "linux")]
        {
            let mut ticker = interval(POLL_INTERVAL);

            // Auto-reconnect: A2DP sinks wait for the source, so a reboot left
            // the speaker silent until a manual reconnect. Page the last-used
            // source (if still bonded) on the poll ticker for a short window —
            // retries cover the case where the source isn't ready the instant
            // we boot. Zeroed once any device connects.
            let auto_reconnect_addr =
                load_last_device().filter(|a| self.paired.iter().any(|p| p.addr == *a));
            if let Some(ref addr) = auto_reconnect_addr {
                self.auto_reconnect_retries = 3;
                // Resolve a readable label now (paired is already populated —
                // the addr was just filtered against it); fall back to the MAC.
                self.reconnect_name = Some(
                    self.paired
                        .iter()
                        .find(|p| &p.addr == addr)
                        .map(|p| p.name.clone())
                        .filter(|n| !n.is_empty())
                        .unwrap_or_else(|| addr.clone()),
                );
            }

            let has_cmds = self.cmd_rx.is_some();
            let (_dummy_tx, dummy_rx) = mpsc::channel::<encore_common::protocol::BtAction>(1);
            let mut cmd_rx = self.cmd_rx.take().unwrap_or(dummy_rx);
            let has_suspend = self.suspend_rx.is_some();
            let (_suspend_dummy_tx, suspend_dummy_rx) = watch::channel(false);
            let mut suspend_rx = self.suspend_rx.take().unwrap_or(suspend_dummy_rx);
            let mut suspended = false;
            let has_bt_name = self.bt_name_rx.is_some();
            let (_btname_dummy_tx, btname_dummy_rx) =
                mpsc::channel::<crate::group::BtMeshSignal>(1);
            let mut bt_name_rx = self.bt_name_rx.take().unwrap_or(btname_dummy_rx);

            // Watch the dashboard broadcast for VolumeChanged so we can tell a
            // registered AVRCP controller (the source) when our volume moves.
            let has_ws = self.ws_tx.is_some();
            let (_ws_dummy_tx, ws_dummy_rx) = tokio::sync::broadcast::channel::<String>(1);
            let mut ws_rx = self
                .ws_tx
                .as_ref()
                .map(|t| t.subscribe())
                .unwrap_or(ws_dummy_rx);

            loop {
                tokio::select! {
                    // Management events (pairing, connect/disconnect, discovery)
                    Ok(event) = mgmt.read_event() => {
                        self.handle_mgmt_event(&mut mgmt, event).await;
                    }

                    // AVDTP events (start/pause/resume/stop/disconnect).
                    // The AVDTP loop owns the reader thread; these events only
                    // drive mixer-slot, group and UI state.
                    Some(event) = avdtp_rx.recv() => {
                        match event {
                            avdtp::AvdtpEvent::Streaming { codec } => {
                                self.slot.set_active(true);
                                self.cur_codec = Some(codec);
                                self.playing = true;
                                // A live stream proves the source is connected even
                                // if mgmt's DeviceConnected was missed (e.g. an Encore
                                // restart over a surviving ACL); cancel auto-reconnect.
                                self.auto_reconnect_retries = 0;
                                info!("Bluetooth: A2DP {} streaming", codec);
                                self.broadcast_bt_status();
                                if let Some(ref tx) = self.group_cmd_tx {
                                    let _ = tx.try_send(crate::group::GroupCmd::LocalAudioStarted { source: "bluetooth".into() });
                                }
                            }
                            avdtp::AvdtpEvent::Resumed { codec } => {
                                self.slot.set_active(true);
                                self.cur_codec = Some(codec);
                                self.playing = true;
                                self.auto_reconnect_retries = 0;
                                info!("Bluetooth: A2DP {} resumed", codec);
                                self.broadcast_bt_status();
                                if let Some(ref tx) = self.group_cmd_tx {
                                    let _ = tx.try_send(crate::group::GroupCmd::LocalAudioStarted { source: "bluetooth".into() });
                                }
                            }
                            avdtp::AvdtpEvent::Paused => {
                                // Reader stays alive for a fast resume; just mute.
                                self.slot.set_active(false);
                                self.playing = false;
                                self.broadcast_bt_status();
                                if let Some(ref tx) = self.group_cmd_tx {
                                    let _ = tx.try_send(crate::group::GroupCmd::LocalAudioStopped { source: "bluetooth".into() });
                                }
                            }
                            avdtp::AvdtpEvent::Stopped => {
                                self.slot.set_active(false);
                                self.playing = false;
                                self.broadcast_bt_status();
                                if let Some(ref tx) = self.group_cmd_tx {
                                    let _ = tx.try_send(crate::group::GroupCmd::LocalAudioStopped { source: "bluetooth".into() });
                                }
                            }
                            avdtp::AvdtpEvent::Disconnected => {
                                self.slot.set_active(false);
                                self.slot.clear();
                                self.cur_codec = None;
                                self.playing = false;
                                if let Some(ref tx) = self.group_cmd_tx {
                                    let _ = tx.try_send(crate::group::GroupCmd::LocalAudioStopped { source: "bluetooth".into() });
                                }
                                if let BtState::Connected { address, .. } = &self.state {
                                    let addr = address.clone();
                                    info!("Bluetooth: DISCONNECTED (AVDTP)");
                                    self.broadcast_bt_event(
                                        encore_common::protocol::BtEvent::DeviceDisconnected { addr },
                                    );
                                    self.state = BtState::Discoverable;
                                    // Re-advertise here too: an AVDTP teardown can be
                                    // the only disconnect we see, and once state has
                                    // left Connected the mgmt DeviceDisconnected arm
                                    // skips its own set_discoverable(true).
                                    self.apply_discoverable(&mgmt).await;
                                }
                                self.broadcast_bt_status();
                            }
                        }
                    }

                    // Dashboard commands
                    Some(action) = cmd_rx.recv(), if has_cmds => {
                        match action {
                            encore_common::protocol::BtAction::SetName { name } => {
                                info!("Bluetooth: visible-as name -> {}", name);
                                self.device_name = name.clone();
                                persist_device_name(&name);
                                // Applies now if not meshed; if meshed, the mesh
                                // name stays on air and this takes effect later.
                                self.apply_bt_name(&mgmt).await;
                            }
                            encore_common::protocol::BtAction::Forget { addr } => {
                                if let Some(bdaddr) = l2cap::string_to_bdaddr(&addr) {
                                    mgmt.remove_link_key(&bdaddr);
                                }
                                // Drop the persisted friendly name too, so a
                                // forgotten device doesn't leave a stale entry.
                                if self.paired_names.remove(&addr).is_some() {
                                    save_paired_names(&self.paired_names);
                                }
                                mgmt.handle_action(
                                    encore_common::protocol::BtAction::Forget { addr },
                                )
                                .await;
                                self.refresh_paired(&mgmt);
                                self.broadcast_bt_status();
                            }
                            encore_common::protocol::BtAction::RequestStatus => {
                                self.broadcast_bt_status();
                            }
                            encore_common::protocol::BtAction::Connect { addr } => {
                                // Sink-initiated A2DP: connect PSM 25 ourselves
                                // and drive the stream up. A bare paged ACL is
                                // not enough — sources (Windows included) only
                                // stream over sessions somebody actually opens,
                                // which is why headphones do exactly this.
                                if self.playing || self.cur_codec.is_some() {
                                    info!("Bluetooth: Connect ignored (already streaming)");
                                } else if let Some(bdaddr) = l2cap::string_to_bdaddr(&addr) {
                                    info!("Bluetooth: sink-initiated connect -> {}", addr);
                                    avdtp::spawn_initiator(
                                        bdaddr,
                                        self.slot.clone(),
                                        avdtp_tx.clone(),
                                        self.latency_target_ms,
                                    );
                                } else {
                                    warn!("Bluetooth: Connect with bad addr '{}'", addr);
                                }
                            }
                            encore_common::protocol::BtAction::Transport { key } => {
                                // Clone the owning fd handle; holding the Arc keeps
                                // the socket alive for the duration of the write.
                                let session = avrcp_fd.lock().unwrap().clone();
                                match (avrcp::passthrough_op(&key), session) {
                                    (Some(op), Some(fd)) => {
                                        info!("Bluetooth: AVRCP transport '{}'", key);
                                        avrcp::send_passthrough(fd.as_raw_fd(), op);
                                    }
                                    (Some(_), None) => info!(
                                        "Bluetooth: transport '{}' ignored (no AVRCP session)",
                                        key
                                    ),
                                    (None, _) => {
                                        info!("Bluetooth: unknown transport key '{}'", key)
                                    }
                                }
                            }
                            other => {
                                mgmt.handle_action(other).await;
                            }
                        }
                    }

                    // Group suspend/resume
                    Ok(()) = suspend_rx.changed(), if has_suspend => {
                        let suspend = *suspend_rx.borrow_and_update();
                        // Follower mode mutes local BT output but keeps the
                        // reader running, so unfollowing resumes instantly
                        // without re-handshaking the source.
                        if suspend && !suspended {
                            info!("Bluetooth: suspended by group (follower mode)");
                            suspended = true;
                            self.slot.set_active(false);
                        } else if !suspend && suspended {
                            info!("Bluetooth: resumed by group");
                            suspended = false;
                            self.slot.set_active(true);
                        }
                    }

                    // Mesh name + discoverability updates from the group subsystem.
                    Some(signal) = bt_name_rx.recv(), if has_bt_name => {
                        self.mesh_name = signal.name;
                        let discoverable_changed =
                            self.mesh_discoverable_allowed != signal.discoverable;
                        self.mesh_discoverable_allowed = signal.discoverable;
                        self.apply_bt_name(&mgmt).await;
                        // Reflect a coordinator change immediately (a node that
                        // just became a meshed non-coordinator hides now), but only
                        // on an actual flip — this signal arrives on every status
                        // broadcast, so don't re-issue set_discoverable each time.
                        if discoverable_changed {
                            self.apply_discoverable(&mgmt).await;
                        }
                    }

                    // Local volume changed → tell a registered AVRCP controller.
                    res = ws_rx.recv(), if has_ws => {
                        if let Ok(json) = res {
                            if json.contains("VolumeChanged") {
                                if let Ok(encore_common::protocol::ServerMsg::VolumeChanged { level, .. }) =
                                    serde_json::from_str::<encore_common::protocol::ServerMsg>(&json)
                                {
                                    use std::sync::atomic::Ordering;
                                    if avrcp_vol_registered.swap(false, Ordering::Relaxed) {
                                        // Clone the owning fd handle so it stays
                                        // alive across the write (no use-after-close).
                                        if let Some(fd) = avrcp_fd.lock().unwrap().clone() {
                                            let label = avrcp_vol_label.load(Ordering::Relaxed);
                                            let _ = l2cap::raw_write(
                                                fd.as_raw_fd(),
                                                &avrcp::build_volume_changed(label, level),
                                            );
                                        }
                                    }
                                }
                            }
                        }
                    }

                    // Health heartbeat
                    _ = ticker.tick() => {
                        let now = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_secs();
                        ctx.health.beat(now);
                        // Rebroadcast BT status so dashboards that connected
                        // mid-stream pick up the live codec/playing state.
                        self.broadcast_bt_status();
                        // Boot-time auto-reconnect: page the last source until
                        // it connects or the retry budget runs out.
                        if self.auto_reconnect_retries > 0
                            && matches!(self.state, BtState::Discoverable)
                            && self.cur_codec.is_none()
                            && !self.playing
                        {
                            if let Some(ref addr) = auto_reconnect_addr {
                                info!(
                                    "Bluetooth: auto-reconnect attempt ({} left) -> {}",
                                    self.auto_reconnect_retries, addr
                                );
                                mgmt.handle_action(
                                    encore_common::protocol::BtAction::Connect {
                                        addr: addr.clone(),
                                    },
                                )
                                .await;
                            }
                            self.auto_reconnect_retries -= 1;
                        }
                    }

                    // Shutdown
                    _ = ctx.shutdown.recv() => {
                        info!("Bluetooth: shutdown");
                        break;
                    }
                }
            }

            // Clean shutdown — the detached AVDTP thread and its reader exit
            // with the process; just silence the slot here.
            self.slot.set_active(false);
        }

        // Non-Linux: nothing to do (module is cfg-gated but this compiles)
        #[cfg(not(target_os = "linux"))]
        {
            let _ = ctx.shutdown.recv().await;
        }

        Ok(())
    }
}

#[cfg(target_os = "linux")]
impl BluetoothSubsystem {
    /// Process a management event.
    async fn handle_mgmt_event(&mut self, mgmt: &mut mgmt::MgmtSocket, event: mgmt::MgmtEvent) {
        match event {
            mgmt::MgmtEvent::DeviceConnected { addr, name, .. } => {
                let addr_str = l2cap::bdaddr_to_string(&addr);
                // Prefer the EIR name; if the source didn't send one, fall back to
                // a name we learned on a past connection, then to "Unknown".
                let name_str = name
                    .filter(|n| !n.is_empty())
                    .or_else(|| self.paired_names.get(&addr_str).cloned())
                    .unwrap_or_else(|| "Unknown".to_string());
                info!("Bluetooth: CONNECTED: {} ({})", name_str, addr_str);

                self.state = BtState::Connected {
                    name: name_str.clone(),
                    address: addr_str.clone(),
                };
                // Remember the friendly name so the paired list can show it
                // later (link keys store only the address).
                if self.remember_name(&addr_str, &name_str) {
                    self.refresh_paired(mgmt);
                }
                save_last_device(&addr_str);
                // Connected — stop any pending boot auto-reconnect retries.
                self.auto_reconnect_retries = 0;
                // Hide from new-device scans while in use (paired devices can
                // still reconnect — connectable stays on).
                mgmt.set_discoverable(false).await;
                self.broadcast_bt_event(encore_common::protocol::BtEvent::DeviceConnected {
                    name: name_str,
                    addr: addr_str,
                });
                self.broadcast_bt_status();
            }
            mgmt::MgmtEvent::DeviceDisconnected { addr, .. } => {
                let addr_str = l2cap::bdaddr_to_string(&addr);
                if let BtState::Connected { .. } = &self.state {
                    info!("Bluetooth: DISCONNECTED: {}", addr_str);
                    self.broadcast_bt_event(encore_common::protocol::BtEvent::DeviceDisconnected {
                        addr: addr_str,
                    });
                    self.state = BtState::Discoverable;
                    self.cur_codec = None;
                    self.playing = false;
                    // Advertise again now that we're free (unless a meshed
                    // non-coordinator, which stays hidden).
                    self.apply_discoverable(mgmt).await;
                    self.broadcast_bt_status();
                }
            }
            mgmt::MgmtEvent::NewLinkKey { .. } => {
                mgmt.handle_new_link_key(&event);
                self.refresh_paired(mgmt);
                self.broadcast_bt_status();
            }
            mgmt::MgmtEvent::UserConfirmRequest {
                addr,
                addr_type,
                value,
            } => {
                info!(
                    "Bluetooth: auto-confirm pairing {} (value={})",
                    l2cap::bdaddr_to_string(&addr),
                    value
                );
                if let Err(e) = mgmt.auto_confirm(&addr, addr_type).await {
                    warn!("Bluetooth: confirm reply failed: {}", e);
                }
            }
            mgmt::MgmtEvent::DeviceFound {
                addr, rssi, name, ..
            } => {
                let addr_str = l2cap::bdaddr_to_string(&addr);
                let name_str = name.unwrap_or_else(|| "Unknown".to_string());
                debug!(
                    "Bluetooth: discovered {} ({}) rssi={}",
                    name_str, addr_str, rssi
                );
                // A scan doubles as a name probe for our *bonded* devices: only
                // cache the name (and refresh the list) when this is a device we're
                // paired with. The bonded check must short-circuit first — otherwise
                // remember_name persists a name for every passing phone, growing
                // /lsync/encore/bt_names without bound.
                if self.paired.iter().any(|p| p.addr == addr_str)
                    && self.remember_name(&addr_str, &name_str)
                {
                    self.refresh_paired(mgmt);
                    self.broadcast_bt_status();
                }
                self.broadcast_bt_event(encore_common::protocol::BtEvent::DiscoveryResult {
                    name: name_str,
                    addr: addr_str,
                    rssi: rssi as i16,
                });
            }
            mgmt::MgmtEvent::CmdComplete { opcode, status, .. } => {
                // Commit a pending advertised-name change only once the controller
                // confirms it; a rejected name never became the live name.
                if opcode == mgmt::MGMT_OP_SET_LOCAL_NAME {
                    if let Some(name) = self.pending_bt_name.take() {
                        if status == 0 {
                            self.applied_name = name;
                            self.broadcast_bt_status();
                        } else {
                            warn!(
                                "Bluetooth: SET_LOCAL_NAME rejected (status=0x{:02x})",
                                status
                            );
                        }
                    }
                } else if status != 0 {
                    debug!(
                        "Bluetooth: cmd 0x{:04x} completed with status=0x{:02x}",
                        opcode, status
                    );
                }
            }

            mgmt::MgmtEvent::Other { opcode } => {
                debug!("Bluetooth: unhandled mgmt event 0x{:04x}", opcode);
            }
        }
    }
}

// ── Phase 1 helpers: hardware infrastructure setup ──

/// Unblock Bluetooth rfkill if it's soft-blocked.
/// The Marvell bt8xxx driver registers an rfkill device that starts blocked.
/// rfkill sysfs: state=0 means blocked, state=1 means unblocked.
/// soft=0 means not soft-blocked, soft=1 means soft-blocked.
#[cfg(target_os = "linux")]
async fn unblock_bt_rfkill() {
    // still unsure if this is affective anymore
    let Ok(entries) = std::fs::read_dir("/sys/class/rfkill") else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let type_path = path.join("type");
        if let Ok(t) = std::fs::read_to_string(&type_path) {
            if t.trim() == "bluetooth" {
                // Read current state for logging
                let state_before = std::fs::read_to_string(path.join("state"))
                    .unwrap_or_default()
                    .trim()
                    .to_string();
                let soft_before = std::fs::read_to_string(path.join("soft"))
                    .unwrap_or_default()
                    .trim()
                    .to_string();
                info!(
                    "Bluetooth: rfkill {} before: state={} soft={}",
                    path.display(),
                    state_before,
                    soft_before
                );

                // Clear soft block by writing "0" to the soft file
                let soft_path = path.join("soft");
                if std::fs::write(&soft_path, "0").is_err() {
                    // Fallback: write "1" to state (1 = unblock)
                    let _ = std::fs::write(path.join("state"), "1");
                }

                // Brief delay for rfkill state to settle
                tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

                // Verify
                let state_after = std::fs::read_to_string(path.join("state"))
                    .unwrap_or_default()
                    .trim()
                    .to_string();
                let soft_after = std::fs::read_to_string(path.join("soft"))
                    .unwrap_or_default()
                    .trim()
                    .to_string();
                info!(
                    "Bluetooth: rfkill {} after: state={} soft={}",
                    path.display(),
                    state_after,
                    soft_after
                );

                if state_after == "1" {
                    info!("Bluetooth: rfkill unblocked successfully");
                } else {
                    warn!("Bluetooth: rfkill still blocked after unblock attempt");
                }
            }
        }
    }
}

/// Set the Bluetooth Class of Device via raw HCI command.
/// The management API SET_DEV_CLASS doesn't work on kernel 3.8.13,
/// so we send HCI Write_Class_of_Device (opcode 0x0C24) directly.
/// CoD 0x240428 = Audio/Video major class, Loudspeaker minor, Rendering+Audio service.
#[cfg(target_os = "linux")]
fn set_device_class_hci() {
    // Open raw HCI socket bound to hci0 (channel 0 = HCI_CHANNEL_RAW)
    let fd = unsafe { libc::socket(l2cap::AF_BLUETOOTH, libc::SOCK_RAW, l2cap::BTPROTO_HCI) };
    if fd < 0 {
        warn!("Bluetooth: failed to create HCI raw socket for CoD");
        return;
    }

    let addr = l2cap::SockaddrHci {
        hci_family: l2cap::AF_BLUETOOTH as u16,
        hci_dev: 0,     // hci0
        hci_channel: 0, // HCI_CHANNEL_RAW
    };

    let ret = unsafe {
        libc::bind(
            fd,
            &addr as *const l2cap::SockaddrHci as *const libc::sockaddr,
            std::mem::size_of::<l2cap::SockaddrHci>() as libc::socklen_t,
        )
    };
    if ret < 0 {
        let e = std::io::Error::last_os_error();
        warn!("Bluetooth: failed to bind HCI raw socket: {}", e);
        unsafe { libc::close(fd) };
        return;
    }

    // HCI command: Write_Class_of_Device (OGF=0x03, OCF=0x0024, opcode=0x0C24)
    // CoD: 0x240428 → bytes [0x28, 0x04, 0x24] (little-endian)
    let cmd: [u8; 7] = [
        0x01, // HCI command packet type
        0x24, 0x0C, // Opcode 0x0C24 (little-endian)
        0x03, // Parameter length
        0x28, // CoD byte 0: minor=Loudspeaker(0x0A<<2=0x28)
        0x04, // CoD byte 1: major=Audio/Video(0x04)
        0x24, // CoD byte 2: service=Rendering(0x04)|Audio(0x20)=0x24
    ];

    let n = unsafe { libc::write(fd, cmd.as_ptr() as *const libc::c_void, cmd.len()) };

    if n == cmd.len() as isize {
        info!("Bluetooth: Class of Device set to 0x240428 (Audio/Loudspeaker)");
    } else if n < 0 {
        let e = std::io::Error::last_os_error();
        warn!("Bluetooth: Write_Class_of_Device failed: {}", e);
    } else {
        warn!(
            "Bluetooth: Write_Class_of_Device short write: {}/{}",
            n,
            cmd.len()
        );
    }

    unsafe { libc::close(fd) };
}

/// Bring hci0 UP via HCIDEVUP ioctl.
/// The bt8xxx vendor driver creates the HCI adapter but doesn't bring it up.
/// On this old kernel (3.8.13), the management API SET_POWERED doesn't
/// reliably call hci_dev_open(), so we do it explicitly.
#[cfg(target_os = "linux")]
async fn hci_dev_up() {
    use std::os::fd::FromRawFd;

    // HCIDEVUP ioctl number on ARM: _IOW('H', 201, int) = 0x400448C9
    const HCIDEVUP: libc::c_int = 0x400448C9u32 as libc::c_int;

    let fd = unsafe { libc::socket(l2cap::AF_BLUETOOTH, libc::SOCK_RAW, l2cap::BTPROTO_HCI) };
    if fd < 0 {
        warn!("Bluetooth: failed to create HCI control socket");
        return;
    }
    let _owned = unsafe { std::os::fd::OwnedFd::from_raw_fd(fd) };

    // `ioctl`'s request arg is c_ulong on glibc (host/CI) and c_int on musl (device);
    // `as _` lets it infer per target, matching the pattern in vpn/mod.rs.
    let ret = unsafe { libc::ioctl(fd, HCIDEVUP as _, 0 as libc::c_int) };
    if ret < 0 {
        let err = std::io::Error::last_os_error();
        if err.raw_os_error() == Some(libc::EALREADY) {
            info!("Bluetooth: hci0 already UP");
        } else {
            warn!(
                "Bluetooth: HCIDEVUP failed: {} — adapter may need management API power-on",
                err
            );
        }
    } else {
        info!("Bluetooth: hci0 brought UP via ioctl");
    }

    // Give the adapter time to initialize
    tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
}

/// Ensure bt8xxx.ko is loaded. If not, derive BT MAC from WiFi MAC and insmod.
#[cfg(target_os = "linux")]
async fn ensure_bt_module_loaded() -> Result<()> {
    let modules = tokio::fs::read_to_string("/proc/modules")
        .await
        .unwrap_or_default();
    if modules.contains("bt8xxx") {
        info!("Bluetooth: bt8xxx already loaded");
        return Ok(());
    }

    let wifi_mac = tokio::fs::read_to_string(WIFI_MAC_PATH)
        .await
        .context("failed to read WiFi MAC")?;
    let wifi_mac = wifi_mac.trim();
    let bt_mac = derive_bt_mac(wifi_mac)?;

    info!("Bluetooth: loading bt8xxx.ko (bt_mac={})", bt_mac);

    let output = tokio::process::Command::new("insmod")
        .arg(BT_MODULE_PATH)
        .arg(format!("bt_mac={}", bt_mac))
        .arg("fw_name=mrvl/sd8887_bt_a2_new.bin")
        .arg("bt_fw_serial=0")
        .output()
        .await
        .context("failed to run insmod")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("insmod bt8xxx.ko failed: {}", stderr.trim());
    }

    info!("Bluetooth: bt8xxx loaded");
    Ok(())
}

/// Derive BT MAC by incrementing the last byte of the WiFi MAC.
#[cfg(target_os = "linux")]
fn derive_bt_mac(wifi_mac: &str) -> Result<String> {
    let parts: Vec<&str> = wifi_mac.split(':').collect();
    if parts.len() != 6 {
        anyhow::bail!("invalid WiFi MAC format: {}", wifi_mac);
    }

    let mut bytes = [0u8; 6];
    for (i, part) in parts.iter().enumerate() {
        bytes[i] = u8::from_str_radix(part, 16).context("invalid MAC byte")?;
    }

    bytes[5] = bytes[5].wrapping_add(1);

    Ok(format!(
        "{:02X}:{:02X}:{:02X}:{:02X}:{:02X}:{:02X}",
        bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5]
    ))
}

/// Wait for hci0 to appear in sysfs.
#[cfg(target_os = "linux")]
async fn wait_for_hci() -> Result<()> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);

    loop {
        if std::path::Path::new(HCI_SYSFS_PATH).exists() {
            info!("Bluetooth: hci0 found");
            return Ok(());
        }

        if tokio::time::Instant::now() >= deadline {
            anyhow::bail!("timeout waiting for hci0 at {}", HCI_SYSFS_PATH);
        }

        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}
