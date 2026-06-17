//! Network manager subsystem.
//!
//! AP-first architecture: the Access Point is the #1 priority service.
//! It provides emergency access if WiFi fails. The subsystem runs a
//! state machine that always monitors AP health and WiFi connectivity.

pub mod ap;
pub mod firewall;
pub mod mdns;
pub mod ntp;
pub mod wpa;

use crate::subsystem::{Subsystem, SubsystemContext};
use anyhow::Result;
use encore_common::protocol::{NetworkState, ServerMsg, SubsystemState, WifiConnectResult};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::sync::{broadcast, mpsc};

/// Shared cache for the most recent WifiConnectResult, so reconnecting
/// WebSocket clients can retrieve the result they missed during WiFi transition.
pub type WifiResultCache = Arc<std::sync::Mutex<Option<WifiConnectResult>>>;

/// Shared cache for the most recent NetworkState, so reconnecting
/// WebSocket clients receive state immediately without a round-trip.
pub type NetworkStateCache = Arc<std::sync::Mutex<Option<NetworkState>>>;
use tokio::time::{interval, Duration};
use tracing::{debug, info, warn};
use wpa::WpaClient;

/// Commands sent to the network subsystem.
pub enum NetworkCmd {
    /// Enter AP-only mode (e.g. button press or setup mode).
    EnterApMode,
    /// Connect to a WiFi network (moved from main.rs SetWifi handler).
    ConnectWifi { ssid: String, password: String },
    /// Request current network state broadcast.
    RequestState,
    /// Set whether AP should stay alive after WiFi connects.
    SetApKeepAlive(bool),
    /// Device name changed — re-register mDNS + DHCP hostname.
    SetDeviceName(String),
}

/// Internal state machine states.
#[derive(Debug, Clone, PartialEq)]
enum NetState {
    /// AP up, no WiFi configured, monitoring AP health.
    ApOnly,
    /// AP up, WiFi connecting via wpa_supplicant.
    ApConnecting { ssid: String },
    /// AP up, WiFi connected, waiting for NTP or ap_keep_alive=true.
    ApConnected {
        ssid: String,
        ip: String,
        signal: i8,
        sta_freq: u32,
        ap_freq: u32,
    },
    /// AP intentionally stopped (WiFi + NTP synced, user opted out of AP).
    WifiOnly {
        ssid: String,
        ip: String,
        signal: i8,
        sta_freq: u32,
    },
    /// WiFi dropped, restarting AP + attempting WiFi reconnect.
    Recovering { ssid: String },
}

const MONITOR_INTERVAL: Duration = Duration::from_secs(5);

/// Cached AP frequency in MHz (set by start_ap_on_band, read by to_protocol_state).
static AP_FREQ_MHZ: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
const CONFIG_PATH: &str = "/lsync/encore/config.toml";
/// Lazily cached AP SSID (derived from MAC on first call).
fn ap_ssid() -> String {
    ap::default_ap_ssid()
}
/// How many monitor ticks to wait before retrying WiFi connection.
/// At 5s interval, 12 ticks = 60 seconds between retries.
const WIFI_RETRY_TICKS: u16 = 12;
/// Consecutive poor-signal readings (< -75 dBm) before logging a warning.
const POOR_SIGNAL_WARN_TICKS: u8 = 3;

pub struct NetworkSubsystem {
    ws_tx: Option<broadcast::Sender<String>>,
    cmd_rx: Option<mpsc::Receiver<NetworkCmd>>,
    /// Shared flag so the web server knows if AP is active (for captive portal).
    ap_active: Arc<AtomicBool>,
    hostname: String,
    /// DNS-SD services to advertise via mDNS.
    mdns_services: Vec<mdns::MdnsService>,
    /// mDNS responder handle — Some when registered, None otherwise.
    mdns_responder: Option<mdns::MdnsResponder>,
    /// Channel for discovered group peers from mDNS responses.
    discovery_tx: Option<mpsc::Sender<mdns::MdnsDiscovery>>,
    /// Our peer ID (for mDNS self-filtering).
    our_peer_id: String,
    /// Cached WiFi connect result for reconnecting WebSocket clients.
    wifi_result_cache: WifiResultCache,
    /// Cached network state for immediate delivery on WebSocket connect.
    network_state_cache: NetworkStateCache,
}

impl NetworkSubsystem {
    pub fn new(
        ws_tx: Option<broadcast::Sender<String>>,
        cmd_rx: Option<mpsc::Receiver<NetworkCmd>>,
        ap_active: Arc<AtomicBool>,
        mdns_services: Vec<mdns::MdnsService>,
    ) -> Self {
        Self {
            ws_tx,
            cmd_rx,
            ap_active,
            hostname: String::new(),
            mdns_services,
            mdns_responder: None,
            discovery_tx: None,
            our_peer_id: String::new(),
            wifi_result_cache: Arc::new(std::sync::Mutex::new(None)),
            network_state_cache: Arc::new(std::sync::Mutex::new(None)),
        }
    }

    /// Get a clone of the WiFi result cache Arc for sharing with the web module.
    pub fn wifi_result_cache(&self) -> WifiResultCache {
        self.wifi_result_cache.clone()
    }

    /// Get a clone of the network state cache Arc for sharing with the web module.
    pub fn network_state_cache(&self) -> NetworkStateCache {
        self.network_state_cache.clone()
    }

    /// Set the mDNS discovery channel and peer ID for group peer discovery.
    pub fn set_discovery(&mut self, tx: mpsc::Sender<mdns::MdnsDiscovery>, peer_id: String) {
        self.discovery_tx = Some(tx);
        self.our_peer_id = peer_id;
    }

    /// Broadcast network state change to dashboard via WebSocket.
    fn broadcast_state(&self, state: &NetworkState) {
        // Update cache for reconnecting WebSocket clients
        if let Ok(mut guard) = self.network_state_cache.lock() {
            *guard = Some(state.clone());
        }
        if let Some(ref tx) = self.ws_tx {
            let msg = ServerMsg::NetworkChanged(state.clone());
            if let Ok(json) = serde_json::to_string(&msg) {
                let _ = tx.send(json);
            }
        }
    }

    /// Broadcast a WifiConnectResult to the dashboard.
    fn broadcast_wifi_result(&self, result: &WifiConnectResult) {
        if let Some(ref tx) = self.ws_tx {
            let msg = ServerMsg::WifiConnectResult(result.clone());
            if let Ok(json) = serde_json::to_string(&msg) {
                let _ = tx.send(json);
            }
        }
    }

    /// Broadcast TimeSynced event.
    fn broadcast_time_synced(&self) {
        if let Some(ref tx) = self.ws_tx {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();
            let msg = ServerMsg::TimeSynced {
                timestamp_secs: now,
            };
            if let Ok(json) = serde_json::to_string(&msg) {
                let _ = tx.send(json);
            }
        }
    }

    /// Start mDNS responder if we have a wlan0 IP and haven't started yet.
    fn register_mdns(&mut self) {
        if self.mdns_responder.is_some() || self.hostname.is_empty() {
            return;
        }
        if let Some(ip_str) = read_interface_ip("wlan0") {
            if let Ok(ip) = ip_str.parse::<std::net::Ipv4Addr>() {
                match mdns::MdnsResponder::start(
                    &self.hostname,
                    ip,
                    self.mdns_services.clone(),
                    self.discovery_tx.clone(),
                    self.our_peer_id.clone(),
                ) {
                    Ok(responder) => {
                        self.mdns_responder = Some(responder);
                    }
                    Err(e) => {
                        warn!("mDNS: start failed: {}", e);
                    }
                }
            }
        }
    }

    /// Stop mDNS responder if currently running.
    fn unregister_mdns(&mut self) {
        if let Some(responder) = self.mdns_responder.take() {
            responder.shutdown();
        }
    }

    /// Convert internal state to protocol NetworkState.
    fn to_protocol_state(&self, state: &NetState) -> NetworkState {
        match state {
            NetState::ApOnly => NetworkState::ApMode {
                ssid: ap_ssid(),
                clients: ap::client_count(),
                ap_frequency_mhz: AP_FREQ_MHZ.load(Ordering::Relaxed),
            },
            NetState::ApConnecting { ssid } => NetworkState::Connecting { ssid: ssid.clone() },
            NetState::ApConnected {
                ssid,
                ip,
                signal,
                sta_freq,
                ap_freq,
            } => NetworkState::ConnectedWithAp {
                ssid: ssid.clone(),
                ip: ip.clone(),
                signal: *signal,
                hostname: self.hostname.clone(),
                frequency_mhz: *sta_freq,
                ap_ssid: ap_ssid(),
                ap_clients: ap::client_count(),
                ap_frequency_mhz: *ap_freq,
            },
            NetState::WifiOnly {
                ssid,
                ip,
                signal,
                sta_freq,
            } => NetworkState::Connected {
                ssid: ssid.clone(),
                ip: ip.clone(),
                signal: *signal,
                hostname: self.hostname.clone(),
                frequency_mhz: *sta_freq,
            },
            NetState::Recovering { ssid } => NetworkState::Connecting { ssid: ssid.clone() },
        }
    }
}

#[async_trait::async_trait]
impl Subsystem for NetworkSubsystem {
    fn name(&self) -> &'static str {
        "network"
    }

    async fn run(&mut self, mut ctx: SubsystemContext) -> Result<()> {
        ctx.health.set_state(SubsystemState::Running);

        // Apply firewall immediately
        firewall::apply().unwrap_or_else(|e| warn!("Firewall setup failed: {}", e));

        // Load config to determine initial state
        let cfg = encore_common::config::EncoreConfigFile::load(std::path::Path::new(CONFIG_PATH))
            .unwrap_or_default();
        let mut ap_keep_alive = cfg.network.ap_keep_alive;

        let has_wifi = cfg
            .network
            .wifi_ssid
            .as_ref()
            .map(|s| !s.is_empty())
            .unwrap_or(false);
        let mut wifi_password = cfg.network.wifi_password.clone().unwrap_or_default();
        let mut connecting_ticks: u16 = 0;
        // WiFi retry backoff: doubles after each failure, capped at 120 ticks (10 min).
        // Reset to WIFI_RETRY_TICKS when WiFi connects successfully.
        let mut wifi_retry_ticks: u16 = WIFI_RETRY_TICKS;
        let mut poor_signal_ticks: u8 = 0;
        // Guard flag: set during WiFi connect operations to prevent the
        // monitor loop from racing (restarting AP, false recovery transitions)
        let wifi_connecting = Arc::new(AtomicBool::new(false));

        // Compute hostname from config
        self.hostname = sanitize_hostname(&cfg.device.name);

        // Spawn AP init as background task — don't block the select loop.
        // AP will be started by start_ap.sh (p2p0 ready) or by us after 30s.
        let (ap_ready_tx, ap_ready_rx) = tokio::sync::oneshot::channel::<bool>();
        let ap_active_init = self.ap_active.clone();
        tokio::task::spawn_blocking(move || {
            // Check for p2p0_ready sentinel (from modified start_ap.sh) or running AP
            for i in 0..60 {
                if std::path::Path::new("/tmp/p2p0_ready").exists() || ap::is_ap_running() {
                    if i > 0 {
                        info!("Network: p2p0 ready after {}ms", i * 500);
                    }
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(500));
            }
            // Start AP ourselves (we are the sole AP manager when running)
            let ok = ap::ensure_ap().is_ok();
            ap_active_init.store(ok, Ordering::Relaxed);
            let _ = ap_ready_tx.send(ok);
        });

        // Fire-and-forget DNS + RPS init (idempotent, no synchronization needed)
        tokio::task::spawn_blocking(|| {
            refresh_rps();
            update_resolv_conf();
        });

        // Determine initial state
        let mut state = if has_wifi {
            let ssid = cfg.network.wifi_ssid.clone().unwrap_or_default();
            info!("Network: WiFi configured ({}), will connect", ssid);
            // Check if already connected (stock init may have done it)
            if let Some(ip) = read_interface_ip("wlan0") {
                info!("Network: wlan0 already has IP {}", ip);
                let sta_freq = wpa::WpaClient::connect()
                    .ok()
                    .and_then(|w| w.connected_frequency())
                    .unwrap_or(0);
                let ap_freq = AP_FREQ_MHZ.load(Ordering::Relaxed);
                NetState::ApConnected {
                    ssid,
                    ip,
                    signal: read_wifi_signal().unwrap_or(0),
                    sta_freq,
                    ap_freq,
                }
            } else {
                NetState::ApConnecting { ssid }
            }
        } else {
            info!("Network: no WiFi configured — AP only mode");
            NetState::ApOnly
        };

        // If already connected, register mDNS
        if matches!(state, NetState::ApConnected { .. }) {
            self.register_mdns();
        }

        // If we're in ApConnecting state, kick off the initial WiFi connection
        if let NetState::ApConnecting { ref ssid } = state {
            let ssid_clone = ssid.clone();
            let pw_clone = wifi_password.clone();
            let hn_clone = dhcp_name(&self.hostname);
            connecting_ticks = 0;
            info!(
                "Network: starting initial WiFi connection to {}",
                ssid_clone
            );
            let wc = wifi_connecting.clone();
            let ws = self.ws_tx.clone();
            let ap = self.ap_active.clone();
            let cache = self.wifi_result_cache.clone();
            tokio::task::spawn_blocking(move || {
                wc.store(true, Ordering::Relaxed);
                let result = connect_wifi_blocking(&ssid_clone, &pw_clone, &hn_clone);
                wc.store(false, Ordering::Relaxed);
                // Update AP flag and cache result (like user-initiated path)
                ap.store(ap::is_ap_running(), Ordering::Relaxed);
                if let Ok(mut guard) = cache.lock() {
                    *guard = Some(result.clone());
                }
                if let Some(tx) = ws {
                    let msg = ServerMsg::WifiConnectResult(result);
                    if let Ok(json) = serde_json::to_string(&msg) {
                        let _ = tx.send(json);
                    }
                }
            });
        }

        let proto_state = self.to_protocol_state(&state);
        self.broadcast_state(&proto_state);

        let mut ap_ready_rx = Some(ap_ready_rx);
        let mut time_synced = ntp::is_time_synced();
        let mut ticker = interval(MONITOR_INTERVAL);
        let mut cmd_rx = self.cmd_rx.take();
        // Cooldown ticks after AP TX-error reset — skip TX checks to let
        // the interface stabilize (counters reset, beacons start flowing).
        let mut tx_reset_cooldown: u8 = 6; // skip first 30s (boot settling)

        loop {
            tokio::select! {
                _ = ticker.tick() => {
                    // Heartbeat
                    let now = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs();
                    ctx.health.beat(now);

                    let mut state_changed = false;

                    // 1. AP health: if AP should be up and hostapd dead, restart
                    // Skip AP health checks while WiFi is connecting — AP is
                    // intentionally stopped during connect → DHCP → band restart
                    let skip_ap_check = wifi_connecting.load(Ordering::Relaxed);
                    let ap_should_be_up = !skip_ap_check && matches!(
                        state,
                        NetState::ApOnly | NetState::ApConnecting { .. } |
                        NetState::ApConnected { .. } | NetState::Recovering { .. }
                    );
                    if ap_should_be_up && !ap::is_restarting() {
                        let check_tx = tx_reset_cooldown == 0;
                        tx_reset_cooldown = tx_reset_cooldown.saturating_sub(1);

                        let (ap_running, tx_broken) = if check_tx {
                            tokio::task::spawn_blocking(|| {
                                (ap::is_ap_running(), ap::has_tx_errors())
                            }).await.unwrap_or((false, false))
                        } else {
                            let running = tokio::task::spawn_blocking(ap::is_ap_running)
                                .await.unwrap_or(false);
                            (running, false) // skip TX check during cooldown
                        };

                        if !ap_running || tx_broken {
                            if tx_broken && ap_running {
                                warn!("Network: AP has TX errors with no packets — resetting p2p0");
                                let task = tokio::task::spawn_blocking(|| {
                                    ap::set_restarting(true);
                                    ap::stop_ap().ok();
                                    ap::reset_interface();
                                    let r = ap::ensure_ap_unlocked();
                                    ap::set_restarting(false);
                                    r
                                });
                                match tokio::time::timeout(std::time::Duration::from_secs(20), task).await {
                                    Ok(_) => {}
                                    Err(_) => warn!("Network: AP reset timed out after 20s"),
                                }
                                tx_reset_cooldown = 6; // 30s cooldown after reset
                            } else {
                                warn!("Network: AP died, restarting");
                                let task = tokio::task::spawn_blocking(ap::ensure_ap);
                                match tokio::time::timeout(std::time::Duration::from_secs(15), task).await {
                                    Ok(_) => {}
                                    Err(_) => warn!("Network: AP restart timed out after 15s"),
                                }
                            }
                        }
                        // Re-check actual state rather than assuming success
                        let still_running = tokio::task::spawn_blocking(ap::is_ap_running)
                            .await
                            .unwrap_or(false);
                        self.ap_active.store(still_running, Ordering::Relaxed);
                    }

                    // 2. WiFi status poll
                    match &state {
                        NetState::ApConnecting { ssid } => {
                            // Check if WiFi connected
                            if let Some(ip) = read_interface_ip("wlan0") {
                                info!("Network: WiFi connected to {} ({})", ssid, ip);
                                connecting_ticks = 0;
                                wifi_retry_ticks = WIFI_RETRY_TICKS; // reset backoff on success
                                // Refresh RPS + DNS after initial connect
                                tokio::task::spawn_blocking(|| {
                                    refresh_rps();
                                    update_resolv_conf();
                                });
                                let sta_freq = wpa::WpaClient::connect()
                                    .ok()
                                    .and_then(|w| w.connected_frequency())
                                    .unwrap_or(0);
                                let ap_freq = AP_FREQ_MHZ.load(Ordering::Relaxed);
                                state = NetState::ApConnected {
                                    ssid: ssid.clone(),
                                    ip,
                                    signal: read_wifi_signal().unwrap_or(0),
                                    sta_freq,
                                    ap_freq,
                                };
                                self.register_mdns();
                                state_changed = true;
                            } else {
                                connecting_ticks += 1;
                                // ponytail: this retry (and the user-connect spawn) don't dedup
                                // against wifi_connecting, so two connects can briefly overlap and
                                // thrash the single AP. Self-heals via the monitor AP-health tick
                                // (<=5s). Upgrade = guard the spawn with wifi_connecting AND clear
                                // the flag via a Drop guard *together*; guarding alone strands the
                                // AP if the flag ever sticks (panic skipping wc.store(false)).
                                if connecting_ticks >= wifi_retry_ticks {
                                    connecting_ticks = 0;
                                    // Exponential backoff: 60s → 120s → 240s → … → 600s max
                                    wifi_retry_ticks = (wifi_retry_ticks * 2).min(120);
                                    let ssid_c = ssid.clone();
                                    let pw_c = wifi_password.clone();
                                    let hn_c = dhcp_name(&self.hostname);
                                    warn!("Network: WiFi not connected, retrying (next retry in {}s)", wifi_retry_ticks as u32 * 5);
                                    let wc = wifi_connecting.clone();
                                    tokio::task::spawn_blocking(move || {
                                        wc.store(true, Ordering::Relaxed);
                                        let r = connect_wifi_blocking(&ssid_c, &pw_c, &hn_c);
                                        wc.store(false, Ordering::Relaxed);
                                        r
                                    });
                                }
                            }
                        }
                        NetState::ApConnected { ssid, .. } => {
                            // Update signal, check WiFi still alive
                            if read_interface_ip("wlan0").is_some() {
                                let new_signal = read_wifi_signal().unwrap_or(0);
                                let new_ip = read_interface_ip("wlan0").unwrap_or_default();
                                // Track consecutive poor-signal readings
                                if new_signal < -75 && new_signal != 0 {
                                    poor_signal_ticks = poor_signal_ticks.saturating_add(1);
                                    if poor_signal_ticks == POOR_SIGNAL_WARN_TICKS {
                                        warn!("Network: sustained poor WiFi signal ({} dBm for {}s)", new_signal, POOR_SIGNAL_WARN_TICKS as u32 * 5);
                                    }
                                } else {
                                    poor_signal_ticks = 0;
                                }
                                if let NetState::ApConnected { ref mut signal, ref mut ip, .. } = state {
                                    if *signal != new_signal || *ip != new_ip {
                                        *signal = new_signal;
                                        *ip = new_ip;
                                        state_changed = true;
                                    }
                                }
                            } else if !wifi_connecting.load(Ordering::Relaxed) {
                                warn!("Network: WiFi dropped while AP+WiFi");
                                // Clear stale "success" result — WiFi is no longer connected
                                if let Ok(mut guard) = self.wifi_result_cache.lock() {
                                    *guard = None;
                                }
                                self.unregister_mdns();
                                connecting_ticks = 0;
                                poor_signal_ticks = 0;
                                state = NetState::Recovering { ssid: ssid.clone() };
                                state_changed = true;
                            }
                        }
                        NetState::WifiOnly { ssid, .. } => {
                            // WiFi-only: if WiFi drops, restart AP and recover
                            if read_interface_ip("wlan0").is_none() {
                                warn!("Network: WiFi dropped in WifiOnly mode, restarting AP");
                                if let Ok(mut guard) = self.wifi_result_cache.lock() {
                                    *guard = None;
                                }
                                self.unregister_mdns();
                                connecting_ticks = 0;
                                poor_signal_ticks = 0;
                                let _ = tokio::task::spawn_blocking(ap::ensure_ap).await;
                                self.ap_active.store(true, Ordering::Relaxed);
                                state = NetState::Recovering { ssid: ssid.clone() };
                                state_changed = true;
                            } else {
                                // Update signal
                                let new_signal = read_wifi_signal().unwrap_or(0);
                                // Track consecutive poor-signal readings
                                if new_signal < -75 && new_signal != 0 {
                                    poor_signal_ticks = poor_signal_ticks.saturating_add(1);
                                    if poor_signal_ticks == POOR_SIGNAL_WARN_TICKS {
                                        warn!("Network: sustained poor WiFi signal ({} dBm for {}s)", new_signal, POOR_SIGNAL_WARN_TICKS as u32 * 5);
                                    }
                                } else {
                                    poor_signal_ticks = 0;
                                }
                                if let NetState::WifiOnly { ref mut signal, ref mut ip, .. } = state {
                                    let new_ip = read_interface_ip("wlan0").unwrap_or_default();
                                    if *signal != new_signal || *ip != new_ip {
                                        *signal = new_signal;
                                        *ip = new_ip;
                                        state_changed = true;
                                    }
                                }
                            }
                        }
                        NetState::Recovering { ssid } => {
                            // Check if WiFi reconnected
                            if let Some(ip) = read_interface_ip("wlan0") {
                                info!("Network: WiFi recovered ({})", ip);
                                connecting_ticks = 0;
                                wifi_retry_ticks = WIFI_RETRY_TICKS; // reset backoff on success
                                // Refresh RPS + DNS after reconnect
                                tokio::task::spawn_blocking(|| {
                                    refresh_rps();
                                    update_resolv_conf();
                                });
                                let sta_freq = wpa::WpaClient::connect()
                                    .ok()
                                    .and_then(|w| w.connected_frequency())
                                    .unwrap_or(0);
                                let ap_freq = AP_FREQ_MHZ.load(Ordering::Relaxed);
                                state = NetState::ApConnected {
                                    ssid: ssid.clone(),
                                    ip,
                                    signal: read_wifi_signal().unwrap_or(0),
                                    sta_freq,
                                    ap_freq,
                                };
                                self.register_mdns();
                                state_changed = true;
                            } else {
                                connecting_ticks += 1;
                                // ponytail: this retry (and the user-connect spawn) don't dedup
                                // against wifi_connecting, so two connects can briefly overlap and
                                // thrash the single AP. Self-heals via the monitor AP-health tick
                                // (<=5s). Upgrade = guard the spawn with wifi_connecting AND clear
                                // the flag via a Drop guard *together*; guarding alone strands the
                                // AP if the flag ever sticks (panic skipping wc.store(false)).
                                if connecting_ticks >= wifi_retry_ticks {
                                    connecting_ticks = 0;
                                    // Exponential backoff: 60s → 120s → 240s → … → 600s max
                                    wifi_retry_ticks = (wifi_retry_ticks * 2).min(120);
                                    let ssid_c = ssid.clone();
                                    let pw_c = wifi_password.clone();
                                    let hn_c = dhcp_name(&self.hostname);
                                    warn!("Network: WiFi recovery retry (next retry in {}s)", wifi_retry_ticks as u32 * 5);
                                    let wc = wifi_connecting.clone();
                                    tokio::task::spawn_blocking(move || {
                                        wc.store(true, Ordering::Relaxed);
                                        let r = connect_wifi_blocking(&ssid_c, &pw_c, &hn_c);
                                        wc.store(false, Ordering::Relaxed);
                                        r
                                    });
                                }
                            }
                        }
                        NetState::ApOnly => {
                            // Update client count periodically
                            state_changed = true; // always refresh AP client count
                        }
                    }

                    // 3. NTP check
                    if !time_synced && matches!(state, NetState::ApConnected { .. } | NetState::WifiOnly { .. })
                        && ntp::is_time_synced() {
                            time_synced = true;
                            info!("Network: NTP synced");
                            self.broadcast_time_synced();
                        }

                    // 4. AP lifecycle: if Connected + NTP synced + !ap_keep_alive → stop AP
                    if let NetState::ApConnected { ref ssid, ref ip, signal, sta_freq, .. } = state {
                        if time_synced && !ap_keep_alive {
                            info!("Network: NTP synced and ap_keep_alive=false, stopping AP");
                            let _ = tokio::task::spawn_blocking(ap::stop_ap).await;
                            self.ap_active.store(false, Ordering::Relaxed);
                            state = NetState::WifiOnly {
                                ssid: ssid.clone(),
                                ip: ip.clone(),
                                signal,
                                sta_freq,
                            };
                            state_changed = true;
                        }
                    }

                    // Broadcast on any state change
                    if state_changed {
                        let proto = self.to_protocol_state(&state);
                        self.broadcast_state(&proto);
                    }
                }

                cmd = async {
                    if let Some(ref mut rx) = cmd_rx {
                        rx.recv().await
                    } else {
                        std::future::pending().await
                    }
                } => {
                    match cmd {
                        Some(NetworkCmd::EnterApMode) => {
                            info!("Network: entering AP mode (command)");
                            self.unregister_mdns();
                            let _ = tokio::task::spawn_blocking(ap::ensure_ap).await;
                            self.ap_active.store(true, Ordering::Relaxed);
                            state = NetState::ApOnly;
                            let proto = self.to_protocol_state(&state);
                            self.broadcast_state(&proto);
                        }
                        Some(NetworkCmd::ConnectWifi { ssid, password }) => {
                            info!("Network: ConnectWifi({})", ssid);

                            // Update stored password for retry logic
                            wifi_password = password.clone();

                            // Update hostname from current config
                            let file_cfg = encore_common::config::EncoreConfigFile::load(std::path::Path::new(CONFIG_PATH))
                                .unwrap_or_default();
                            self.hostname = sanitize_hostname(&file_cfg.device.name);

                            // Transition to connecting; reset backoff (explicit user request)
                            connecting_ticks = 0;
                            wifi_retry_ticks = WIFI_RETRY_TICKS;
                            state = NetState::ApConnecting { ssid: ssid.clone() };
                            let proto = self.to_protocol_state(&state);
                            self.broadcast_state(&proto);

                            // NOTE: Do NOT call ensure_ap() here — connect_wifi_blocking()
                            // owns the AP for the attempt: it stops the AP to free the radio,
                            // then on success restarts it on the STA band (if ap_keep_alive)
                            // or restores it on failure.

                            // Connect WiFi in a blocking task with result feedback
                            let ws_tx = self.ws_tx.clone();
                            let ap_active = self.ap_active.clone();
                            let hn = dhcp_name(&self.hostname);
                            let cache = self.wifi_result_cache.clone();
                            let wc = wifi_connecting.clone();
                            tokio::task::spawn_blocking(move || {
                                wc.store(true, Ordering::Relaxed);
                                let result = connect_wifi_user(&ssid, &password, &hn);
                                wc.store(false, Ordering::Relaxed);
                                // Update ap_active flag (AP may have been restarted on STA band, or left running on failure)
                                ap_active.store(ap::is_ap_running(), Ordering::Relaxed);
                                // Cache result for reconnecting clients
                                if let Ok(mut guard) = cache.lock() {
                                    *guard = Some(result.clone());
                                }
                                // Broadcast result
                                if let Some(tx) = ws_tx {
                                    let msg = ServerMsg::WifiConnectResult(result);
                                    if let Ok(json) = serde_json::to_string(&msg) {
                                        let _ = tx.send(json);
                                    }
                                }
                            });
                            // The monitor loop will detect the WiFi connected state
                        }
                        Some(NetworkCmd::RequestState) => {
                            let proto = self.to_protocol_state(&state);
                            self.broadcast_state(&proto);
                        }
                        Some(NetworkCmd::SetApKeepAlive(keep)) => {
                            info!("Network: SetApKeepAlive({})", keep);
                            ap_keep_alive = keep;
                            // If turning off and conditions met, stop AP now
                            if !keep && time_synced {
                                if let NetState::ApConnected { ref ssid, ref ip, signal, sta_freq, .. } = state {
                                    info!("Network: stopping AP per user request");
                                    let _ = tokio::task::spawn_blocking(ap::stop_ap).await;
                                    self.ap_active.store(false, Ordering::Relaxed);
                                    state = NetState::WifiOnly {
                                        ssid: ssid.clone(),
                                        ip: ip.clone(),
                                        signal,
                                        sta_freq,
                                    };
                                    let proto = self.to_protocol_state(&state);
                                    self.broadcast_state(&proto);
                                }
                            }
                            // If turning on and AP is down, start it
                            if keep && matches!(state, NetState::WifiOnly { .. }) {
                                let _ = tokio::task::spawn_blocking(ap::ensure_ap).await;
                                self.ap_active.store(true, Ordering::Relaxed);
                                if let NetState::WifiOnly { ssid, ip, signal, sta_freq } = state.clone() {
                                    let ap_freq = AP_FREQ_MHZ.load(Ordering::Relaxed);
                                    state = NetState::ApConnected { ssid, ip, signal, sta_freq, ap_freq };
                                    let proto = self.to_protocol_state(&state);
                                    self.broadcast_state(&proto);
                                }
                            }
                        }
                        Some(NetworkCmd::SetDeviceName(name)) => {
                            let new_hostname = sanitize_hostname(&name);
                            if new_hostname != self.hostname {
                                info!("Network: hostname changed {} -> {}", self.hostname, new_hostname);
                                self.hostname = new_hostname;
                                // Re-register mDNS with new name
                                self.unregister_mdns();
                                if matches!(state, NetState::ApConnected { .. } | NetState::WifiOnly { .. }) {
                                    self.register_mdns();
                                }
                                // Renew DHCP lease with new hostname
                                let hn = dhcp_name(&self.hostname);
                                tokio::task::spawn_blocking(move || {
                                    let _ = std::process::Command::new("dhcpcd")
                                        .args(["wlan0", "--noarp", "-h", &hn, "--timeout", "5"])
                                        .status();
                                });
                                // Broadcast updated state (hostname appears in NetworkState)
                                let proto = self.to_protocol_state(&state);
                                self.broadcast_state(&proto);
                            }
                        }
                        None => {
                            info!("Network: command channel closed");
                            break;
                        }
                    }
                }

                // AP init completion (oneshot, fires once)
                result = async {
                    if let Some(ref mut rx) = ap_ready_rx {
                        rx.await
                    } else {
                        std::future::pending().await
                    }
                } => {
                    ap_ready_rx = None; // consumed
                    if let Ok(ok) = result {
                        info!("Network: AP init complete (ok={})", ok);
                        self.ap_active.store(ok, Ordering::Relaxed);
                    }
                    // Broadcast initial state now that AP is confirmed
                    let proto = self.to_protocol_state(&state);
                    self.broadcast_state(&proto);
                }

                _ = ctx.shutdown.recv() => {
                    info!("Network: shutdown signal received");
                    break;
                }
            }
        }

        // Cleanup
        self.unregister_mdns();

        Ok(())
    }
}

/// Parse the default gateway IP from `/proc/net/route` content.
/// Returns None if no default route is found.
fn parse_gateway(route_content: &str) -> Option<String> {
    for line in route_content.lines().skip(1) {
        let fields: Vec<&str> = line.split('\t').collect();
        // Default route: Destination == 00000000, Flags & 0x2 (RTF_GATEWAY)
        if fields.len() >= 4 && fields[1] == "00000000" {
            let flags = u32::from_str_radix(fields[3], 16).unwrap_or(0);
            if flags & 0x2 == 0 {
                continue;
            }
            if let Ok(gw) = u32::from_str_radix(fields[2], 16) {
                let a = gw & 0xff;
                let b = (gw >> 8) & 0xff;
                let c = (gw >> 16) & 0xff;
                let d = (gw >> 24) & 0xff;
                return Some(format!("{}.{}.{}.{}", a, b, c, d));
            }
        }
    }
    None
}

/// Build resolv.conf content from a gateway IP (or None).
fn build_resolv_conf(gateway: Option<&str>) -> String {
    let mut nameservers = Vec::with_capacity(3);
    if let Some(gw) = gateway {
        nameservers.push(format!("nameserver {}", gw));
    }
    nameservers.push("nameserver 8.8.8.8".into());
    nameservers.push("nameserver 8.8.4.4".into());
    nameservers.join("\n") + "\n"
}

/// Update `/etc/resolv.conf` with the current default gateway as primary nameserver.
/// Reads the default route from `/proc/net/route` and writes the gateway IP plus
/// Google DNS fallbacks. Called after every DHCP lease and at startup.
fn update_resolv_conf() {
    let route = match std::fs::read_to_string("/proc/net/route") {
        Ok(r) => r,
        Err(e) => {
            debug!("Network: cannot read /proc/net/route: {}", e);
            return;
        }
    };

    let gateway_ip = parse_gateway(&route);
    let content = build_resolv_conf(gateway_ip.as_deref());
    match std::fs::write("/etc/resolv.conf", &content) {
        Ok(()) => {
            if gateway_ip.is_some() {
                info!(
                    "Network: resolv.conf updated (gateway={})",
                    gateway_ip.as_deref().unwrap_or("")
                );
            } else {
                debug!("Network: resolv.conf written with fallback DNS only (no gateway yet)");
            }
        }
        Err(e) => warn!("Network: failed to write /etc/resolv.conf: {}", e),
    }
}

/// Refresh Receive Packet Steering (RPS) on wlan0 queues.
/// Distributes WiFi RX interrupt processing across both Cortex-A7 cores.
/// Idempotent — safe to call repeatedly.
fn refresh_rps() {
    let mut ok = 0u8;
    for i in 0..4 {
        let path = format!("/sys/class/net/wlan0/queues/rx-{}/rps_cpus", i);
        if let Ok(()) = std::fs::write(&path, "3") {
            ok += 1;
        } // else: queue may not exist (driver exposes 1-4)
    }
    if ok > 0 {
        debug!("Network: RPS set on {} wlan0 RX queues", ok);
    }
}

/// Perform blocking WiFi connection (called from spawn_blocking).
/// `dhcp_hostname` is sent to the router via DHCP `-h` so it can register
/// the name in its local DNS (e.g. "encore" → router resolves "encore" or "encore.lan").
/// `scan_first`: if true, scan for the SSID before connecting (user-initiated).
///
/// Flow: stop the AP first (single radio — a running AP holds the channel and
/// starves the STA during scan/association) → STA connect → DHCP → on success
/// restart the AP on the STA's band if ap_keep_alive, otherwise leave it off.
/// On any failure the AP is restored so the dashboard stays reachable.
fn connect_wifi_blocking(ssid: &str, password: &str, dhcp_hostname: &str) -> WifiConnectResult {
    connect_wifi_inner(ssid, password, dhcp_hostname, false)
}

/// User-initiated WiFi connection with pre-connect scan gate.
fn connect_wifi_user(ssid: &str, password: &str, dhcp_hostname: &str) -> WifiConnectResult {
    connect_wifi_inner(ssid, password, dhcp_hostname, true)
}

fn connect_wifi_inner(
    ssid: &str,
    password: &str,
    dhcp_hostname: &str,
    scan_first: bool,
) -> WifiConnectResult {
    // Ensure wpa_supplicant is running
    if let Err(e) = wpa::ensure_running() {
        warn!("Network: ensure wpa_supplicant failed: {}", e);
        return WifiConnectResult {
            ssid: ssid.into(),
            success: false,
            error: Some(format!("cannot start wpa_supplicant: {}", e)),
        };
    }

    let wpa = match WpaClient::connect() {
        Ok(w) => w,
        Err(e) => {
            warn!("Network: WpaClient::connect failed: {}", e);
            return WifiConnectResult {
                ssid: ssid.into(),
                success: false,
                error: Some(format!("wpa_supplicant control failed: {}", e)),
            };
        }
    };

    // Single radio: the AP holds the channel and starves the STA during
    // scan/association — the STA reaches ASSOCIATED/4WAY_HANDSHAKE then loses the
    // radio and drops back to SCANNING. Free the radio UNCONDITIONALLY. We do NOT
    // gate on is_ap_running(): it only detects an AP this process started via
    // hostapd, and on a fresh device the AP is started by the boot script
    // (start_ap.sh) — so the gate was false and the radio was never freed.
    //
    // RadioGuard guarantees the AP comes back on ANY exit — early return,
    // association failure, or a panic unwinding through this function — so taking
    // the radio down can never strand the dashboard. This ADDS to (does not
    // replace) the supervisor's post-crash start_ap.sh and the monitor loop's 5s
    // AP-health net. It is disarmed only on the success paths that set AP state.
    struct RadioGuard {
        restore: bool,
    }
    impl Drop for RadioGuard {
        fn drop(&mut self) {
            if self.restore {
                info!("Network: restoring AP (connect exited without managing it)");
                let _ = ap::ensure_ap();
            }
        }
    }
    let mut radio_guard = RadioGuard { restore: true };

    // Hold the AP_RESTARTING lock across the radio-free + association window so
    // NO other actor can bring the AP back up and re-steal the single radio
    // mid-association — the device-log failure mode (boot AP-init finishing its
    // uaputl fallback, and the monitor's "AP died, restarting" tick, both re-upped
    // the AP ~3s into the window). acquire_restart_lock blocks (bounded) until any
    // in-flight ensure_ap finishes, so the AP comes fully up, we tear it down, and
    // it stays down — every ensure_ap/start_ap_on_band CAS-bails while we hold it.
    //
    // Drop order is load-bearing: radio_guard is declared FIRST and ap_lock SECOND,
    // so on any early return ap_lock drops first (frees the lock) and THEN
    // radio_guard's restore can re-acquire it. We also drop ap_lock explicitly the
    // instant association concludes, before the success-path restore. Holding it
    // across our own restore would self-bail and strand the AP.
    let mut ap_lock = Some(ap::acquire_restart_lock(std::time::Duration::from_secs(12)));

    info!("Network: freeing radio for association");
    ap::release_radio();
    // ponytail: 800ms is a tuning knob — the Marvell firmware's channel-release
    // time after the AP iface goes down. Bump if association still races a
    // not-yet-freed radio.
    std::thread::sleep(std::time::Duration::from_millis(800));

    // Scan gate: for user-initiated connections, verify the SSID is visible
    // before committing to a 15s association timeout.
    if scan_first {
        if let Err(e) = wpa.scan() {
            debug!(
                "Network: pre-connect scan failed: {} (proceeding anyway)",
                e
            );
        } else {
            // Wait for scan to complete (typical: 2-4s)
            std::thread::sleep(std::time::Duration::from_secs(3));
            match wpa.scan_results() {
                Ok(results) => {
                    let matches: Vec<&_> = results.iter().filter(|r| r.ssid == ssid).collect();
                    if matches.is_empty() {
                        warn!(
                            "Network: SSID '{}' not found in scan ({} networks visible)",
                            ssid,
                            results.len()
                        );
                        // radio_guard restores the AP on return.
                        return WifiConnectResult {
                            ssid: ssid.into(),
                            success: false,
                            error: Some(format!(
                                "Network '{}' not found (scanned {} networks)",
                                ssid,
                                results.len()
                            )),
                        };
                    }
                    // Log the band(s) the SSID is on. >5000 MHz = 5 GHz (ch149=5745,
                    // ch132=5660 is DFS); ~2412-2472 = 2.4 GHz. If association then
                    // fails with the AP confirmed down, a 5 GHz/DFS-only BSS is the
                    // suspect rather than radio contention.
                    let freqs: Vec<u32> = matches.iter().map(|r| r.frequency).collect();
                    info!(
                        "Network: SSID '{}' found in scan on {:?} MHz, proceeding",
                        ssid, freqs
                    );
                }
                Err(e) => {
                    debug!("Network: scan_results failed: {} (proceeding anyway)", e);
                }
            }
        }
    }

    if let Err(e) = wpa.connect_network(ssid, password) {
        warn!("Network: connect_network failed: {}", e);
        // radio_guard restores the AP on return.
        return WifiConnectResult {
            ssid: ssid.into(),
            success: false,
            error: Some(e.to_string()),
        };
    }

    info!("Network: wpa connecting to {}", ssid);

    // Poll wpa_state for up to 15 seconds
    let mut associated = false;
    for i in 0..30 {
        std::thread::sleep(std::time::Duration::from_millis(500));
        if let Ok(st) = wpa.status() {
            info!("Network: wpa_state={} ({}ms)", st.wpa_state, (i + 1) * 500);
            if st.wpa_state == "COMPLETED" {
                associated = true;
                break;
            }
        }
    }

    // Association concluded — release the AP lock so the restore paths (failure
    // RadioGuard, or the success-path start_ap_on_band/ensure_ap) can re-acquire
    // it. Must happen before either branch below.
    drop(ap_lock.take());

    if !associated {
        let detail = wpa
            .status()
            .ok()
            .map(|s| format!("wpa_state={}", s.wpa_state))
            .unwrap_or_else(|| "unknown state".into());
        warn!("Network: association failed for {}: {}", ssid, detail);
        // radio_guard restores the AP on return; the monitor loop retries on
        // its backoff schedule.
        return WifiConnectResult {
            ssid: ssid.into(),
            success: false,
            error: Some(format!("Association failed ({})", detail)),
        };
    }

    // SUCCESS: STA associated. The AP was stopped at the start of the attempt
    // to free the radio, so the radio is ours now — run DHCP directly.
    refresh_rps();

    info!("Network: STA associated, running DHCP");
    if !run_dhcp(dhcp_hostname) {
        warn!("Network: DHCP failed, retrying in 2s");
        std::thread::sleep(std::time::Duration::from_secs(2));
        if !run_dhcp(dhcp_hostname) {
            warn!("Network: DHCP retry also failed — connection may have no IP");
        }
    }

    update_resolv_conf();

    // Save credentials to config.toml BEFORE AP restart — the AP band switch
    // can fail or panic, and credential persistence is critical. This is the
    // primary persistence mechanism: wpa_supplicant's save_config is broken on
    // this platform (Android socket FD mechanism).
    let config_path = std::path::Path::new(CONFIG_PATH);
    let mut file_cfg =
        encore_common::config::EncoreConfigFile::load(config_path).unwrap_or_default();
    file_cfg.network.wifi_ssid = Some(ssid.to_string());
    file_cfg.network.wifi_password = Some(password.to_string());
    match file_cfg.save(config_path) {
        Ok(()) => {
            // Flush to NAND — yaffs2 may buffer writes
            let _ = std::process::Command::new("sync").status();
            info!(
                "Network: WiFi credentials saved to config.toml (ssid={})",
                ssid
            );
        }
        Err(e) => {
            warn!(
                "Network: CRITICAL — config save failed, WiFi won't persist across reboot: {}",
                e
            );
        }
    }

    // The radio was freed for association. If the user wants the AP kept alive,
    // bring it back on the STA's band, where AP+STA coexist cleanly. Otherwise
    // leave the radio to the STA — the device is reachable on the LAN now.
    if file_cfg.network.ap_keep_alive {
        let sta_freq = wpa.connected_frequency();
        if let Some(freq) = sta_freq {
            info!("Network: STA at {}MHz, restarting AP on same band", freq);
            if let Err(e) = ap::start_ap_on_band(freq) {
                warn!(
                    "Network: band-matched AP restart failed: {}, trying default",
                    e
                );
                let _ = ap::ensure_ap();
            }
        } else {
            info!("Network: unknown STA frequency, restarting AP on default band");
            let _ = ap::ensure_ap();
        }
    } else {
        info!("Network: leaving AP off after WiFi connect (ap_keep_alive=false), device is on the LAN");
    }
    // AP state is now set intentionally — don't let the guard override it on drop.
    radio_guard.restore = false;

    WifiConnectResult {
        ssid: ssid.into(),
        success: true,
        error: None,
    }
}

/// Run dhcpcd on wlan0 with a 15-second timeout. Returns true on success.
fn run_dhcp(hostname: &str) -> bool {
    std::process::Command::new("dhcpcd")
        .args(["wlan0", "--noarp", "-h", hostname, "--timeout", "15"])
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Read WiFi signal level from /proc/net/wireless (in dBm).
fn read_wifi_signal() -> Option<i8> {
    let content = std::fs::read_to_string("/proc/net/wireless").ok()?;
    for line in content.lines() {
        let line = line.trim();
        if line.starts_with("wlan0:") {
            let fields: Vec<&str> = line.split_whitespace().collect();
            if fields.len() >= 4 {
                let signal_str = fields[3].trim_end_matches('.');
                return signal_str.parse::<i8>().ok();
            }
        }
    }
    None
}

/// Get wlan0's IPv4 address as a `std::net::IpAddr` (for Spotify discovery binding).
pub fn get_wlan_ip() -> Option<std::net::IpAddr> {
    read_interface_ip("wlan0").and_then(|s| s.parse::<std::net::IpAddr>().ok())
}

fn read_interface_ip(iface: &str) -> Option<String> {
    let dev = std::fs::read_to_string("/proc/net/dev").ok()?;
    if !dev
        .lines()
        .any(|l| l.trim().starts_with(&format!("{}:", iface)))
    {
        return None;
    }
    let output = std::process::Command::new("ifconfig")
        .arg(iface)
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&output.stdout);
    for line in text.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("inet addr:") {
            return rest.split_whitespace().next().map(|s| s.to_string());
        }
        if let Some(rest) = line.strip_prefix("inet ") {
            return rest
                .split('/')
                .next()
                .and_then(|s| s.split_whitespace().next())
                .map(|s| s.to_string());
        }
    }
    None
}

/// Extract the short name from a `.local` hostname for DHCP registration.
/// `"encore.local"` → `"encore"`, `"living-room.local"` → `"living-room"`.
fn dhcp_name(hostname: &str) -> String {
    hostname
        .strip_suffix(".local")
        .unwrap_or(hostname)
        .to_string()
}

/// Sanitize a device name into a valid mDNS hostname.
/// Lowercase, replace spaces/underscores with hyphens, strip non-alphanumeric.
pub fn sanitize_hostname(name: &str) -> String {
    let sanitized: String = name
        .to_lowercase()
        .chars()
        .map(|c| if c == ' ' || c == '_' { '-' } else { c })
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
        .collect();
    let trimmed = sanitized.trim_matches('-').to_string();
    if trimmed.is_empty() {
        "encore.local".into()
    } else {
        format!("{}.local", trimmed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_hostname_basic() {
        assert_eq!(sanitize_hostname("Encore"), "encore.local");
        assert_eq!(sanitize_hostname("Living Room"), "living-room.local");
        assert_eq!(sanitize_hostname("My_Speaker"), "my-speaker.local");
        assert_eq!(sanitize_hostname("  "), "encore.local");
        assert_eq!(sanitize_hostname("---"), "encore.local");
        assert_eq!(sanitize_hostname("Invoke 2"), "invoke-2.local");
    }

    #[test]
    fn parse_gateway_from_proc_route() {
        let content =
            "Iface\tDestination\tGateway\tFlags\tRefCnt\tUse\tMetric\tMask\tMTU\tWindow\tIRTT\n\
                        wlan0\t00000000\t0102A8C0\t0003\t0\t0\t0\t00000000\t0\t0\t0\n\
                        wlan0\tFEA8C0\t00000000\t0001\t0\t0\t0\t00FFFFFF\t0\t0\t0";
        assert_eq!(parse_gateway(content), Some("192.168.2.1".into()));
    }

    #[test]
    fn parse_gateway_different_ip() {
        // 0x0101A8C0 little-endian = 192.168.1.1
        let content =
            "Iface\tDestination\tGateway\tFlags\tRefCnt\tUse\tMetric\tMask\tMTU\tWindow\tIRTT\n\
                        wlan0\t00000000\t0101A8C0\t0003\t0\t0\t0\t00000000\t0\t0\t0";
        assert_eq!(parse_gateway(content), Some("192.168.1.1".into()));
    }

    #[test]
    fn parse_gateway_no_default_route() {
        let content =
            "Iface\tDestination\tGateway\tFlags\tRefCnt\tUse\tMetric\tMask\tMTU\tWindow\tIRTT\n\
                        wlan0\t00A8C0\t00000000\t0001\t0\t0\t0\t00FFFFFF\t0\t0\t0";
        assert_eq!(parse_gateway(content), None);
    }

    #[test]
    fn parse_gateway_no_rtf_gateway_flag() {
        // Default destination but flags=0001 (RTF_UP only, no RTF_GATEWAY bit)
        let content =
            "Iface\tDestination\tGateway\tFlags\tRefCnt\tUse\tMetric\tMask\tMTU\tWindow\tIRTT\n\
                        wlan0\t00000000\t0102A8C0\t0001\t0\t0\t0\t00000000\t0\t0\t0";
        assert_eq!(parse_gateway(content), None);
    }

    #[test]
    fn parse_gateway_empty() {
        assert_eq!(parse_gateway(""), None);
        assert_eq!(parse_gateway("Iface\tDestination\tGateway\n"), None);
    }

    #[test]
    fn build_resolv_conf_with_gateway() {
        let content = build_resolv_conf(Some("192.168.1.1"));
        assert_eq!(
            content,
            "nameserver 192.168.1.1\nnameserver 8.8.8.8\nnameserver 8.8.4.4\n"
        );
    }

    #[test]
    fn build_resolv_conf_without_gateway() {
        let content = build_resolv_conf(None);
        assert_eq!(content, "nameserver 8.8.8.8\nnameserver 8.8.4.4\n");
    }
}
