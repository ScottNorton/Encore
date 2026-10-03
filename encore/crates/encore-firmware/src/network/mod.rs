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
    /// Raise/lower explicit AP demand (group host, manual). The single radio is
    /// reconfigured as needed so AP+STA coexist co-channel on a non-DFS channel.
    SetApRequested(bool),
    /// Device name changed — re-register mDNS + DHCP hostname.
    SetDeviceName(String),
    /// Group coordinator changed the Spotify Connect zone. `advertise=false`
    /// removes the `_spotify-connect._tcp` advert entirely (follower disappears
    /// from the picker); `advertise=true` (re)advertises it under `name`.
    SetSpotifyZone { advertise: bool, name: String },
}

/// Internal state machine states.
#[derive(Debug, Clone, PartialEq)]
enum NetState {
    /// AP up for setup/fallback, no usable STA, monitoring AP health.
    ApOnly,
    /// WiFi connecting via wpa_supplicant (AP up for setup access).
    ApConnecting { ssid: String },
    /// STA connected AND the AP is up co-channel (explicit AP demand is raised).
    ApConnected {
        ssid: String,
        ip: String,
        signal: i8,
        sta_freq: u32,
        ap_freq: u32,
    },
    /// STA connected, AP down — the STA owns the radio (no AP demand). This is
    /// the default once connected; the AP returns on demand (SetApRequested).
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
/// Ticks between AP bring-up attempts after a failed reconcile (6 × 5s = 30s).
const AP_RECONCILE_TICKS: u8 = 6;

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
    /// LED command channel — lights the wifi-setup animation in AP mode.
    led_tx: Option<mpsc::Sender<crate::led::LedCmd>>,
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
            led_tx: None,
        }
    }

    /// Set the LED command channel (for the wifi-setup / AP-mode ring animation).
    pub fn set_led_tx(&mut self, tx: mpsc::Sender<crate::led::LedCmd>) {
        self.led_tx = Some(tx);
    }

    /// Send an LED command if the channel is wired (no-op without an MCU).
    fn send_led(&self, cmd: crate::led::LedCmd) {
        if let Some(ref tx) = self.led_tx {
            let _ = tx.try_send(cmd);
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
        // Runtime AP demand. The AP is a first-class on-demand service: when no
        // STA is usable it is up for setup/fallback; once the STA is connected it
        // is up only when something explicitly needs it (group host, manual).
        // Raised/lowered via NetworkCmd::SetApRequested. Replaces the old static
        // `ap_keep_alive` policy (the single radio decides at runtime).
        let explicit_ap_demand = Arc::new(AtomicBool::new(false));

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
        // Set while the wifi-setup ring animation is being held (EnterApMode),
        // so we clear it exactly once when provisioning connects — and never
        // stomp the ring on a plain boot-with-creds connect.
        let mut wifi_setup_led = false;
        // Guard flag: set during WiFi connect operations to prevent the
        // monitor loop from racing (restarting AP, false recovery transitions)
        let wifi_connecting = Arc::new(AtomicBool::new(false));
        // Guard flag: set while reconciling the AP to demand (bring-up + STA
        // re-home). Suppresses the monitor's AP-health/TX-reset so the re-home's
        // brief STA churn can't cascade into a reset that wedges AP_RESTARTING.
        let ap_reconciling = Arc::new(AtomicBool::new(false));
        // Ticks to wait before re-attempting an AP bring-up after a failure, so a
        // failing reconcile doesn't hammer the radio every 5s. Reset to 0 on a
        // demand change so SetApRequested acts on the next tick.
        let mut ap_reconcile_cooldown: u8 = 0;

        // Compute hostname from config
        self.hostname = sanitize_hostname(&cfg.device.name);

        // Take exclusive ownership of wpa_supplicant BEFORE any connect. The stock
        // init.rc runs its own wpa_supplicant on wlan0 that auto-connects from the
        // saved config and fights us for the single radio (the STA connects then
        // drops as the two instances stomp each other). Stop it and own one clean
        // instance. Blocking (~1.5s) — fine at startup.
        let _ = tokio::task::spawn_blocking(wpa::take_ownership).await;

        // Spawn AP init as background task — don't block the select loop.
        // Encore is the sole AP owner while running (boot no longer launches
        // start_ap.sh). With NO credentials, bring the setup AP up as soon as the
        // driver creates p2p0. With credentials, do NOT bring the AP up here: the
        // STA connect below owns the single radio, and the AP returns only if the
        // connect fails (RadioGuard) or on explicit demand. Running the AP-init
        // concurrently with the connect raced for the radio and knocked the STA
        // out of its 4-way handshake at boot (the restart lock timed out instead
        // of being acquired, and the AP-init's uaputl fallback stole the channel).
        let (ap_ready_tx, ap_ready_rx) = tokio::sync::oneshot::channel::<bool>();
        if has_wifi {
            info!("Network: WiFi configured — STA connect owns the radio, AP on demand");
            drop(ap_ready_tx);
        } else {
            let ap_active_init = self.ap_active.clone();
            tokio::task::spawn_blocking(move || {
                // Wait for the mlan/sd8xxx driver to create the uAP interface.
                for i in 0..60 {
                    if std::path::Path::new("/sys/class/net/p2p0").exists() || ap::is_ap_running() {
                        if i > 0 {
                            info!("Network: p2p0 present after {}ms", i * 500);
                        }
                        break;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(500));
                }
                let ok = ap::ensure_ap().is_ok();
                ap_active_init.store(ok, Ordering::Relaxed);
                let _ = ap_ready_tx.send(ok);
            });
        }

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
            let ap_wanted = explicit_ap_demand.load(Ordering::Relaxed);
            tokio::task::spawn_blocking(move || {
                wc.store(true, Ordering::Relaxed);
                let result = connect_wifi_blocking(&ssid_clone, &pw_clone, &hn_clone, ap_wanted);
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
                    ap_reconcile_cooldown = ap_reconcile_cooldown.saturating_sub(1);

                    // 1. AP health: if AP should be up and hostapd dead, restart.
                    // Skip while WiFi is connecting (AP intentionally down for the
                    // connect) or while reconciling the AP to demand (the re-home's
                    // brief STA churn must not trigger a TX-reset that wedges the
                    // restart lock).
                    let skip_ap_check = wifi_connecting.load(Ordering::Relaxed)
                        || ap_reconciling.load(Ordering::Relaxed);
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
                                    // RAII lock: clears AP_RESTARTING when this closure
                                    // returns even if the outer timeout abandoned the
                                    // await — a raw set_restarting(false) at the end is
                                    // skipped on abandon and wedges every future bring-up.
                                    let _lock = ap::acquire_restart_lock(
                                        std::time::Duration::from_secs(12),
                                    );
                                    ap::stop_ap().ok();
                                    ap::reset_interface();
                                    ap::ensure_ap_unlocked()
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
                                // Provisioning is done — stop the looping wifi-setup
                                // animation held on the ring since EnterApMode. Gated
                                // on wifi_setup_led so a plain boot-with-creds connect
                                // (also ApConnecting) doesn't blank the ring.
                                if wifi_setup_led {
                                    wifi_setup_led = false;
                                    self.send_led(crate::led::LedCmd::Animate(
                                        encore_common::protocol::LedAnimation::Off,
                                    ));
                                }
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
                                // The connect path already set the AP to match
                                // demand; land in ApConnected only if it is really
                                // beaconing, otherwise STA-only (WifiOnly).
                                let ap_up = tokio::task::spawn_blocking(ap::is_ap_running)
                                    .await
                                    .unwrap_or(false);
                                let signal = read_wifi_signal().unwrap_or(0);
                                state = if ap_up {
                                    NetState::ApConnected {
                                        ssid: ssid.clone(),
                                        ip,
                                        signal,
                                        sta_freq,
                                        ap_freq,
                                    }
                                } else {
                                    NetState::WifiOnly {
                                        ssid: ssid.clone(),
                                        ip,
                                        signal,
                                        sta_freq,
                                    }
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
                                    let ap_wanted = explicit_ap_demand.load(Ordering::Relaxed);
                                    tokio::task::spawn_blocking(move || {
                                        wc.store(true, Ordering::Relaxed);
                                        let r = connect_wifi_blocking(&ssid_c, &pw_c, &hn_c, ap_wanted);
                                        wc.store(false, Ordering::Relaxed);
                                        r
                                    });
                                }
                            }
                        }
                        NetState::ApConnected { ssid, .. } => {
                            if read_interface_ip("wlan0").is_none() {
                                if !wifi_connecting.load(Ordering::Relaxed) {
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
                            } else if !explicit_ap_demand.load(Ordering::Relaxed) {
                                // AP no longer needed — drop it; the STA keeps the
                                // radio at full capability.
                                info!("Network: AP demand cleared, stopping AP (STA stays connected)");
                                let _ = tokio::task::spawn_blocking(ap::stop_ap).await;
                                self.ap_active.store(false, Ordering::Relaxed);
                                if let NetState::ApConnected { ssid, ip, signal, sta_freq, .. } =
                                    state.clone()
                                {
                                    state = NetState::WifiOnly { ssid, ip, signal, sta_freq };
                                }
                                state_changed = true;
                            } else {
                                // AP still wanted and STA up — update signal/ip.
                                let new_signal = read_wifi_signal().unwrap_or(0);
                                let new_ip = read_interface_ip("wlan0").unwrap_or_default();
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
                                // STA up. Reconcile state to the ACTUAL AP status, so a
                                // slow concurrent bring-up (start_ap_on_band's carrier
                                // poll can return before the BSS beacons) doesn't leave
                                // state desynced from reality.
                                let demand = explicit_ap_demand.load(Ordering::Relaxed);
                                let ap_up = tokio::task::spawn_blocking(ap::is_ap_running)
                                    .await
                                    .unwrap_or(false);
                                if demand && ap_up {
                                    // AP is up as wanted (may have beaconed after the
                                    // bring-up poll returned) — record ApConnected.
                                    let sta_freq = wpa::WpaClient::connect()
                                        .ok()
                                        .and_then(|w| w.connected_frequency())
                                        .unwrap_or(0);
                                    let ap_freq = AP_FREQ_MHZ.load(Ordering::Relaxed);
                                    info!("Network: AP up co-channel, recording ConnectedWithAp");
                                    state = NetState::ApConnected {
                                        ssid: ssid.clone(),
                                        ip: read_interface_ip("wlan0").unwrap_or_default(),
                                        signal: read_wifi_signal().unwrap_or(0),
                                        sta_freq,
                                        ap_freq,
                                    };
                                    state_changed = true;
                                } else if demand && ap_reconcile_cooldown == 0 {
                                    // AP demanded but down — reconcile co-channel (re-home
                                    // off DFS first if needed). ap_reconciling suppresses
                                    // the AP-health/TX-reset path during the brief STA
                                    // churn; the cooldown throttles retries.
                                    let (ssid_o, sta_freq0) =
                                        if let NetState::WifiOnly { ssid, sta_freq, .. } = &state {
                                            (ssid.clone(), *sta_freq)
                                        } else {
                                            (ssid.clone(), 0u32)
                                        };
                                    info!("Network: AP demand raised, bringing up AP co-channel");
                                    let ssid_c = ssid_o.clone();
                                    ap_reconciling.store(true, Ordering::Relaxed);
                                    let (got_ap, new_freq) = tokio::task::spawn_blocking(move || {
                                        match wpa::WpaClient::connect() {
                                            Ok(w) => reconcile_ap(&ssid_c, sta_freq0, true, &w),
                                            Err(_) => (false, sta_freq0),
                                        }
                                    })
                                    .await
                                    .unwrap_or((false, sta_freq0));
                                    ap_reconciling.store(false, Ordering::Relaxed);
                                    ap_reconcile_cooldown = AP_RECONCILE_TICKS;
                                    self.ap_active.store(got_ap, Ordering::Relaxed);
                                    if got_ap {
                                        let ap_freq = AP_FREQ_MHZ.load(Ordering::Relaxed);
                                        state = NetState::ApConnected {
                                            ssid: ssid_o,
                                            ip: read_interface_ip("wlan0").unwrap_or_default(),
                                            signal: read_wifi_signal().unwrap_or(0),
                                            sta_freq: new_freq,
                                            ap_freq,
                                        };
                                    } else if let NetState::WifiOnly { ref mut sta_freq, .. } = state {
                                        *sta_freq = new_freq;
                                    }
                                    state_changed = true;
                                } else if !demand && ap_up {
                                    // AP is up but no longer wanted — stop it; STA keeps
                                    // the radio.
                                    info!("Network: AP up but not demanded, stopping");
                                    let _ = tokio::task::spawn_blocking(ap::stop_ap).await;
                                    self.ap_active.store(false, Ordering::Relaxed);
                                    state_changed = true;
                                } else {
                                    // Update signal/ip.
                                    let new_signal = read_wifi_signal().unwrap_or(0);
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
                                // The connect path already set the AP to match
                                // demand; land in ApConnected only if it is really
                                // beaconing, otherwise STA-only (WifiOnly).
                                let ap_up = tokio::task::spawn_blocking(ap::is_ap_running)
                                    .await
                                    .unwrap_or(false);
                                let signal = read_wifi_signal().unwrap_or(0);
                                state = if ap_up {
                                    NetState::ApConnected {
                                        ssid: ssid.clone(),
                                        ip,
                                        signal,
                                        sta_freq,
                                        ap_freq,
                                    }
                                } else {
                                    NetState::WifiOnly {
                                        ssid: ssid.clone(),
                                        ip,
                                        signal,
                                        sta_freq,
                                    }
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
                                    let ap_wanted = explicit_ap_demand.load(Ordering::Relaxed);
                                    tokio::task::spawn_blocking(move || {
                                        wc.store(true, Ordering::Relaxed);
                                        let r = connect_wifi_blocking(&ssid_c, &pw_c, &hn_c, ap_wanted);
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

                    // AP lifecycle is reconciled in the ApConnected/WifiOnly arms
                    // above, driven by explicit_ap_demand — no NTP/keep-alive gate.

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
                            // Mirror stock "system:wifi-setup" — hold the setup animation
                            // on the ring while the AP is up for provisioning.
                            self.send_led(crate::led::LedCmd::PlayBin {
                                name: "L_302_d_wifisetup".into(),
                                repeat: true,
                            });
                            wifi_setup_led = true;
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

                            // NOTE: Do NOT call ensure_ap() here — connect_wifi_user()
                            // owns the AP for the attempt: it frees the radio, and on
                            // success reconciles the AP to current demand (co-channel,
                            // re-homing off DFS if needed) or restores it on failure.

                            // Connect WiFi in a blocking task with result feedback
                            let ws_tx = self.ws_tx.clone();
                            let ap_active = self.ap_active.clone();
                            let hn = dhcp_name(&self.hostname);
                            let cache = self.wifi_result_cache.clone();
                            let wc = wifi_connecting.clone();
                            let ap_wanted = explicit_ap_demand.load(Ordering::Relaxed);
                            tokio::task::spawn_blocking(move || {
                                wc.store(true, Ordering::Relaxed);
                                let result = connect_wifi_user(&ssid, &password, &hn, ap_wanted);
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
                        Some(NetworkCmd::SetApRequested(on)) => {
                            info!("Network: SetApRequested({})", on);
                            explicit_ap_demand.store(on, Ordering::Relaxed);
                            // The monitor's Online arms reconcile the AP to this
                            // demand (single reconcile point — no racing the monitor).
                            // Clear the cooldown so it acts on the next tick.
                            ap_reconcile_cooldown = 0;
                            let proto = self.to_protocol_state(&state);
                            self.broadcast_state(&proto);
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
                        Some(NetworkCmd::SetSpotifyZone { advertise, name }) => {
                            // The group coordinator owns the single Spotify Connect
                            // identity. Rename or remove the `_spotify-connect` mDNS
                            // entry, then restart the responder (register_mdns is a
                            // no-op while one exists, so unregister first — mirrors
                            // SetDeviceName).
                            let svc_type = "_spotify-connect._tcp";
                            let current = self
                                .mdns_services
                                .iter()
                                .find(|s| s.service_type == svc_type)
                                .map(|s| s.instance_name.clone());
                            let desired = if advertise { Some(name.clone()) } else { None };
                            if current != desired {
                                info!("Network: spotify zone {:?} -> {:?}", current, desired);
                                self.mdns_services.retain(|s| s.service_type != svc_type);
                                if let Some(n) = desired {
                                    self.mdns_services.push(mdns::MdnsService {
                                        service_type: svc_type.into(),
                                        instance_name: n,
                                        port: crate::spotify::ZEROCONF_PORT,
                                        txt: vec!["VERSION=1.0".into(), "CPath=/".into()],
                                    });
                                }
                                self.unregister_mdns();
                                if matches!(state, NetState::ApConnected { .. } | NetState::WifiOnly { .. }) {
                                    self.register_mdns();
                                }
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

/// Drive the AP to match demand for a connected STA. Single radio (SD8887): the
/// uAP must share the STA's exact non-DFS channel, so when the AP is wanted but
/// the STA is on a DFS channel, re-home the STA to a non-DFS BSS of the same
/// network first. Returns `(ap_up, sta_freq)` — `ap_up` reflects real beaconing.
fn reconcile_ap(ssid: &str, sta_freq: u32, want_ap: bool, wpa: &WpaClient) -> (bool, u32) {
    if !want_ap {
        let _ = ap::stop_ap();
        let _ = std::process::Command::new("ifconfig")
            .args(["p2p0", "down"])
            .status();
        return (false, sta_freq);
    }

    let mut freq = sta_freq;
    if freq == 0 || wpa::is_dfs_freq(freq) {
        // STA on DFS (or unknown) — re-home to a non-DFS BSS so the uAP can follow.
        if wpa.scan().is_err() {
            return (false, freq);
        }
        std::thread::sleep(std::time::Duration::from_secs(3));
        let results = match wpa.scan_results() {
            Ok(r) => r,
            Err(_) => return (false, freq),
        };
        let nd = wpa::non_dfs_freqs(&results, ssid);
        if nd.is_empty() {
            warn!(
                "Network: '{}' only on DFS — cannot host AP co-channel; leaving AP down",
                ssid
            );
            return (false, freq);
        }
        let list = nd
            .iter()
            .map(|f| f.to_string())
            .collect::<Vec<_>>()
            .join(" ");
        info!(
            "Network: AP wanted but STA on {}MHz (DFS/unknown) — re-homing to non-DFS {:?}",
            freq, nd
        );
        let _ = wpa.set_active_freq_list(&list);
        let _ = wpa.reassociate();
        // Wait for the STA to land on a non-DFS channel (up to ~10s).
        for _ in 0..20 {
            std::thread::sleep(std::time::Duration::from_millis(500));
            if let Some(f) = wpa.connected_frequency() {
                if !wpa::is_dfs_freq(f) {
                    freq = f;
                    break;
                }
            }
        }
        if freq == 0 || wpa::is_dfs_freq(freq) {
            warn!("Network: STA did not leave DFS; leaving AP down");
            return (false, freq);
        }
    }

    let started = ap::start_ap_on_band(freq);
    let running = ap::is_ap_running();
    info!(
        "Network: reconcile AP start({}MHz) -> started_ok={} is_ap_running={}",
        freq,
        started.is_ok(),
        running
    );
    if started.is_ok() && running {
        (true, freq)
    } else {
        warn!("Network: AP bring-up on {}MHz did not beacon", freq);
        (false, freq)
    }
}

/// Perform blocking WiFi connection (called from spawn_blocking).
/// `dhcp_hostname` is sent to the router via DHCP `-h` so it can register
/// the name in its local DNS (e.g. "encore" → router resolves "encore" or "encore.lan").
/// `scan_first`: if true, scan for the SSID before connecting (user-initiated).
/// `ap_wanted`: if true, steer the STA onto a non-DFS channel and bring the AP up
/// co-channel after connecting; if false, the STA is unconstrained and the AP
/// stays off (single radio — the STA gets full capability).
///
/// Flow: free the radio (single radio — a running AP starves the STA during
/// scan/association) → STA connect → DHCP → reconcile the AP to demand. On any
/// failure the AP is restored so the device stays reachable.
fn connect_wifi_blocking(
    ssid: &str,
    password: &str,
    dhcp_hostname: &str,
    ap_wanted: bool,
) -> WifiConnectResult {
    connect_wifi_inner(ssid, password, dhcp_hostname, false, ap_wanted)
}

/// User-initiated WiFi connection with pre-connect scan gate.
fn connect_wifi_user(
    ssid: &str,
    password: &str,
    dhcp_hostname: &str,
    ap_wanted: bool,
) -> WifiConnectResult {
    connect_wifi_inner(ssid, password, dhcp_hostname, true, ap_wanted)
}

fn connect_wifi_inner(
    ssid: &str,
    password: &str,
    dhcp_hostname: &str,
    scan_first: bool,
    ap_wanted: bool,
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

    // Scan to (a) gate user connects on SSID visibility before committing to a
    // 15s association timeout, and (b) when the AP is wanted, steer the STA onto a
    // non-DFS channel so the single-radio uAP can coexist on it. When the AP is
    // not wanted, leave the STA unconstrained for maximum capability (DFS allowed).
    let mut freq_list: Vec<u32> = Vec::new();
    if let Err(e) = wpa.scan() {
        debug!(
            "Network: pre-connect scan failed: {} (proceeding without steer)",
            e
        );
    } else {
        // Wait for scan to complete (typical: 2-4s)
        std::thread::sleep(std::time::Duration::from_secs(3));
        match wpa.scan_results() {
            Ok(results) => {
                let matches: Vec<&_> = results.iter().filter(|r| r.ssid == ssid).collect();
                if matches.is_empty() {
                    if scan_first {
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
                    // Boot/retry path: SSID may not be visible yet (router still
                    // coming up). Proceed unconstrained; wpa_supplicant keeps trying.
                    debug!(
                        "Network: SSID '{}' not yet in scan, proceeding (no steer)",
                        ssid
                    );
                } else {
                    let all_freqs: Vec<u32> = matches.iter().map(|r| r.frequency).collect();
                    info!(
                        "Network: SSID '{}' found in scan on {:?} MHz",
                        ssid, all_freqs
                    );
                    if ap_wanted {
                        let nd = wpa::non_dfs_freqs(&results, ssid);
                        if nd.is_empty() {
                            warn!(
                                "Network: AP wanted but '{}' only on DFS {:?} — connecting STA-only, AP cannot coexist",
                                ssid, all_freqs
                            );
                        } else {
                            info!("Network: AP wanted — steering STA to non-DFS {:?}", nd);
                            freq_list = nd;
                        }
                    }
                }
            }
            Err(e) => {
                debug!(
                    "Network: scan_results failed: {} (proceeding without steer)",
                    e
                );
            }
        }
    }

    if let Err(e) = wpa.connect_network(ssid, password, &freq_list) {
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

    // Reconcile the AP to current demand. The radio was freed for association; if
    // the AP is wanted we steered the STA onto a non-DFS channel above, so the uAP
    // can come up co-channel. reconcile_ap also covers the case where the STA still
    // landed on DFS (re-home), and the not-wanted case (leave AP off).
    let sta_freq = wpa.connected_frequency().unwrap_or(0);
    let (ap_up, _freq) = reconcile_ap(ssid, sta_freq, ap_wanted, &wpa);
    if ap_up {
        info!("Network: AP up co-channel with STA after connect");
    } else if ap_wanted {
        warn!("Network: AP wanted but could not be brought up co-channel");
    } else {
        info!("Network: AP off after WiFi connect, device is on the LAN (STA owns the radio)");
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
