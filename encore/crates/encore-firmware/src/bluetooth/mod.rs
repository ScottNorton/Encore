//! Bluetooth A2DP sink subsystem — pure kernel interface, no BlueZ/D-Bus.
//!
//! Manages the entire Bluetooth lifecycle using three kernel sockets:
//! 1. Management API (HCI channel 3) — adapter config, pairing, link keys
//! 2. SDP server (L2CAP PSM 1) — advertises A2DP Sink service record
//! 3. AVDTP (L2CAP PSM 25) — codec negotiation and media transport
//!
//! Incoming A2DP audio is decoded (RTP → SBC/aptX/aptX HD → PCM) and pushed
//! into a MixerSlot for playback.

#[cfg(target_os = "linux")]
mod aptx;
mod avdtp;
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
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio::time::{interval, Duration};
use tracing::{debug, info, warn};

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

/// Path to the bt8xxx kernel module.
const BT_MODULE_PATH: &str =
    "/lib/modules/3.8.13-yocto-standard/kernel/arch/arm/mach-berlin/modules/bt_sd8887/bt8xxx.ko";
/// HCI sysfs path — appears when bt8xxx is loaded.
const HCI_SYSFS_PATH: &str = "/sys/class/bluetooth/hci0";
/// WiFi MAC sysfs path — used to derive BT MAC.
const WIFI_MAC_PATH: &str = "/sys/class/net/wlan0/address";

const POLL_INTERVAL: Duration = Duration::from_secs(5);

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
    cmd_rx: Option<mpsc::Receiver<encore_common::protocol::BtAction>>,
    ws_tx: Option<tokio::sync::broadcast::Sender<String>>,
    suspend_rx: Option<mpsc::Receiver<bool>>,
    group_cmd_tx: Option<mpsc::Sender<crate::group::GroupCmd>>,
}

impl BluetoothSubsystem {
    pub fn new(
        slot: Arc<MixerSlot>,
        device_name: Option<String>,
        cmd_rx: Option<mpsc::Receiver<encore_common::protocol::BtAction>>,
        ws_tx: Option<tokio::sync::broadcast::Sender<String>>,
    ) -> Self {
        Self {
            slot,
            device_name: device_name.unwrap_or_else(|| "Encore".to_string()),
            state: BtState::Off,
            cmd_rx,
            ws_tx,
            suspend_rx: None,
            group_cmd_tx: None,
        }
    }

    /// Set the suspend channel for group sync (follower mode pauses BT).
    pub fn set_suspend_rx(&mut self, rx: mpsc::Receiver<bool>) {
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
                warn!("Bluetooth: module load failed (may already be loaded): {}", e);
            }
            if let Err(e) = wait_for_hci().await {
                warn!("Bluetooth: hci0 not found ({}), continuing anyway", e);
            }
        }

        // ── Phase 2: Management socket — configure adapter ──
        #[cfg(target_os = "linux")]
        let mut mgmt = {
            let m = mgmt::MgmtSocket::open()
                .context("failed to open management socket")?;
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
                }
                Err(e) => {
                    return Err(e).context("adapter setup failed");
                }
            }
        }

        // ── Phase 3: Open SDP and AVDTP sockets ──
        #[cfg(target_os = "linux")]
        let mut avdtp_rx = {
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

            let rx = avdtp::spawn_avdtp(avdtp_fd, self.slot.clone());
            info!("Bluetooth: AVDTP listening on PSM 25");
            rx
        };

        // ── Phase 4: Event loop ──
        #[cfg(target_os = "linux")]
        {
            let mut ticker = interval(POLL_INTERVAL);
            let mut reader_stop: Option<Arc<AtomicBool>> = None;
            let has_cmds = self.cmd_rx.is_some();
            let (_dummy_tx, dummy_rx) = mpsc::channel::<encore_common::protocol::BtAction>(1);
            let mut cmd_rx = self.cmd_rx.take().unwrap_or(dummy_rx);
            let has_suspend = self.suspend_rx.is_some();
            let (_suspend_dummy_tx, suspend_dummy_rx) = mpsc::channel::<bool>(1);
            let mut suspend_rx = self.suspend_rx.take().unwrap_or(suspend_dummy_rx);
            let mut suspended = false;

            loop {
                tokio::select! {
                    // Management events (pairing, connect/disconnect, discovery)
                    Ok(event) = mgmt.read_event() => {
                        self.handle_mgmt_event(&mut mgmt, event).await;
                    }

                    // AVDTP events (streaming start/stop/disconnect)
                    Some(event) = avdtp_rx.recv() => {
                        match event {
                            avdtp::AvdtpEvent::Streaming { codec, stop } => {
                                if let Some(old) = reader_stop.take() {
                                    old.store(true, Ordering::Relaxed);
                                }
                                reader_stop = Some(stop);
                                info!("Bluetooth: A2DP {} streaming", codec);
                                if let Some(ref tx) = self.group_cmd_tx {
                                    let _ = tx.try_send(crate::group::GroupCmd::LocalAudioStarted { source: "bluetooth".into() });
                                }
                            }
                            avdtp::AvdtpEvent::Stopped => {
                                if let Some(stop) = reader_stop.take() {
                                    stop.store(true, Ordering::Relaxed);
                                }
                                self.slot.set_active(false);
                                if let Some(ref tx) = self.group_cmd_tx {
                                    let _ = tx.try_send(crate::group::GroupCmd::LocalAudioStopped { source: "bluetooth".into() });
                                }
                            }
                            avdtp::AvdtpEvent::Disconnected => {
                                if let Some(stop) = reader_stop.take() {
                                    stop.store(true, Ordering::Relaxed);
                                }
                                self.slot.set_active(false);
                                self.slot.clear();
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
                                }
                            }
                        }
                    }

                    // Dashboard commands
                    Some(action) = cmd_rx.recv(), if has_cmds => {
                        if let encore_common::protocol::BtAction::Forget { ref addr } = action {
                            if let Some(bdaddr) = l2cap::string_to_bdaddr(addr) {
                                mgmt.remove_link_key(&bdaddr);
                            }
                        }
                        mgmt.handle_action(action).await;
                    }

                    // Group suspend/resume
                    Some(suspend) = suspend_rx.recv(), if has_suspend => {
                        if suspend && !suspended {
                            info!("Bluetooth: suspended by group (follower mode)");
                            suspended = true;
                            if let Some(stop) = reader_stop.take() {
                                stop.store(true, Ordering::Relaxed);
                            }
                            self.slot.set_active(false);
                        } else if !suspend && suspended {
                            info!("Bluetooth: resumed by group");
                            suspended = false;
                        }
                    }

                    // Health heartbeat
                    _ = ticker.tick() => {
                        let now = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_secs();
                        ctx.health.beat(now);
                    }

                    // Shutdown
                    _ = ctx.shutdown.recv() => {
                        info!("Bluetooth: shutdown");
                        break;
                    }
                }
            }

            // Clean shutdown
            if let Some(stop) = reader_stop.take() {
                stop.store(true, Ordering::Relaxed);
            }
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
            mgmt::MgmtEvent::DeviceConnected {
                addr,
                name,
                ..
            } => {
                let addr_str = l2cap::bdaddr_to_string(&addr);
                let name_str = name.unwrap_or_else(|| "Unknown".to_string());
                info!("Bluetooth: CONNECTED: {} ({})", name_str, addr_str);

                self.state = BtState::Connected {
                    name: name_str.clone(),
                    address: addr_str.clone(),
                };
                self.broadcast_bt_event(encore_common::protocol::BtEvent::DeviceConnected {
                    name: name_str,
                    addr: addr_str,
                });
            }
            mgmt::MgmtEvent::DeviceDisconnected { addr, .. } => {
                let addr_str = l2cap::bdaddr_to_string(&addr);
                if let BtState::Connected { .. } = &self.state {
                    info!("Bluetooth: DISCONNECTED: {}", addr_str);
                    self.broadcast_bt_event(
                        encore_common::protocol::BtEvent::DeviceDisconnected {
                            addr: addr_str,
                        },
                    );
                    self.state = BtState::Discoverable;
                }
            }
            mgmt::MgmtEvent::NewLinkKey { .. } => {
                mgmt.handle_new_link_key(&event);
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
                addr,
                rssi,
                name,
                ..
            } => {
                let addr_str = l2cap::bdaddr_to_string(&addr);
                let name_str = name.unwrap_or_else(|| "Unknown".to_string());
                debug!(
                    "Bluetooth: discovered {} ({}) rssi={}",
                    name_str, addr_str, rssi
                );
                self.broadcast_bt_event(encore_common::protocol::BtEvent::DiscoveryResult {
                    name: name_str,
                    addr: addr_str,
                    rssi: rssi as i16,
                });
            }
            mgmt::MgmtEvent::CmdComplete { opcode, status, .. } => {
                if status != 0 {
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

