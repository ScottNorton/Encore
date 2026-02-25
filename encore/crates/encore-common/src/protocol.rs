//! WebSocket protocol — all messages between firmware and dashboard.
//!
//! [`ServerMsg`] is firmware → dashboard (JSON over WebSocket).
//! [`ClientMsg`] is dashboard → firmware.
//!
//! All types derive both serde (JSON for WebSocket) and rkyv (zero-copy
//! for potential future IPC). Tagged enums use `{"type": "...", "data": ...}`.

use rkyv::{Archive, Deserialize, Serialize};

use crate::config::EncoreConfigFile;

/// Identifies an audio source in the mixer
#[derive(Archive, Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[derive(serde::Serialize, serde::Deserialize)]
pub enum SourceId {
    Spotify,
    Bluetooth,
    Wyoming,
    System,
}

/// Spotify playback actions
#[derive(Archive, Serialize, Deserialize, Debug, Clone)]
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind")]
pub enum SpotifyAction {
    Play,
    Pause,
    Next,
    Previous,
    SetVolume { level: u8 },
    Seek { position_ms: u32 },
    Shuffle { enabled: bool },
    Repeat { enabled: bool },
    RepeatTrack { enabled: bool },
}

/// Bluetooth control actions
#[derive(Archive, Serialize, Deserialize, Debug, Clone)]
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind")]
pub enum BtAction {
    StartDiscovery,
    StopDiscovery,
    Pair { addr: String },
    Connect { addr: String },
    Disconnect { addr: String },
    Forget { addr: String },
}

/// Single LED animation frame (13 LEDs: 12 ring + 1 center).
#[derive(Archive, Serialize, Deserialize, Debug, Clone)]
#[derive(serde::Serialize, serde::Deserialize)]
pub struct LedFrame {
    pub colors: [(u8, u8, u8); 13],
    pub duration_ms: u16,
}

/// LED animation presets
#[derive(Archive, Serialize, Deserialize, Debug, Clone)]
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind")]
pub enum LedAnimation {
    Off,
    Solid { r: u8, g: u8, b: u8 },
    Breathe { r: u8, g: u8, b: u8, period_ms: u16 },
    Spin { r: u8, g: u8, b: u8, speed: u8 },
    Pulse { r: u8, g: u8, b: u8 },
    VolumeArc { level: u8 },
    BootSurge,
    SafeMode,
    Custom { frames: Vec<LedFrame> },
}

/// Biquad filter shape for parametric EQ
#[derive(Archive, Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[derive(serde::Serialize, serde::Deserialize)]
pub enum FilterType {
    Peak,
    LowShelf,
    HighShelf,
    Notch,
}

/// Single parametric EQ band configuration
#[derive(Archive, Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[derive(serde::Serialize, serde::Deserialize)]
pub struct EqBand {
    pub freq_hz: u16,
    pub gain_cb: i16,
    pub q_x10: u16,
    pub filter_type: FilterType,
}

impl Default for EqBand {
    fn default() -> Self {
        Self { freq_hz: 1000, gain_cb: 0, q_x10: 10, filter_type: FilterType::Peak }
    }
}

/// Built-in EQ presets
#[derive(Archive, Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[derive(serde::Serialize, serde::Deserialize)]
pub enum EqPreset {
    Flat,
    BassBoost,
    VocalClarity,
    Warm,
    LateNight,
}

/// Full EQ state broadcast to dashboard
#[derive(Archive, Serialize, Deserialize, Debug, Clone)]
#[derive(serde::Serialize, serde::Deserialize)]
pub struct EqState {
    pub bands: [EqBand; 10],
    pub preset: Option<EqPreset>,
    pub enabled: bool,
}

impl Default for EqState {
    fn default() -> Self {
        Self {
            bands: [EqBand::default(); 10],
            preset: Some(EqPreset::Flat),
            enabled: true,
        }
    }
}

/// DRC frequency band identifier
#[derive(Archive, Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[derive(serde::Serialize, serde::Deserialize)]
pub enum DrcBand {
    Low,
    Mid,
    High,
}

/// Single DRC band configuration
#[derive(Archive, Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[derive(serde::Serialize, serde::Deserialize)]
pub struct DrcBandConfig {
    pub threshold_db: i8,
    pub ratio_x10: u8,
    pub attack_ms: u16,
    pub release_ms: u16,
}

impl Default for DrcBandConfig {
    fn default() -> Self {
        Self { threshold_db: -20, ratio_x10: 10, attack_ms: 10, release_ms: 200 }
    }
}

/// Built-in DRC presets
#[derive(Archive, Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[derive(serde::Serialize, serde::Deserialize)]
pub enum DrcPreset {
    Off,
    Gentle,
    LateNight,
    Protect,
}

/// Full DRC state broadcast to dashboard
#[derive(Archive, Serialize, Deserialize, Debug, Clone)]
#[derive(serde::Serialize, serde::Deserialize)]
pub struct DrcState {
    pub bands: [DrcBandConfig; 3],
    pub low_mid_hz: u16,
    pub mid_high_hz: u16,
    pub preset: Option<DrcPreset>,
    pub enabled: bool,
}

impl Default for DrcState {
    fn default() -> Self {
        Self {
            bands: [DrcBandConfig::default(); 3],
            low_mid_hz: 200,
            mid_high_hz: 2000,
            preset: Some(DrcPreset::Off),
            enabled: false,
        }
    }
}

/// DSP engine status broadcast to dashboard
#[derive(Archive, Serialize, Deserialize, Debug, Clone)]
#[derive(serde::Serialize, serde::Deserialize)]
pub struct DspInfo {
    pub version: String,
    pub hybridflow: u8,
    pub mic_muted: bool,
    pub dsp_volume: u8,
}

/// Debug mode per subsystem
#[derive(Archive, Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[derive(serde::Serialize, serde::Deserialize)]
pub enum DebugMode {
    Production,
    Hold,
    Trace,
}

/// WiFi credentials for network configuration
#[derive(Archive, Serialize, Deserialize, Debug, Clone)]
#[derive(serde::Serialize, serde::Deserialize)]
pub struct WifiCredentials {
    pub ssid: String,
    pub password: String,
}

/// Track information from Spotify
#[derive(Archive, Serialize, Deserialize, Debug, Clone)]
#[derive(serde::Serialize, serde::Deserialize)]
pub struct TrackInfo {
    pub title: String,
    pub artist: String,
    pub album: String,
    pub duration_ms: u32,
    pub position_ms: u32,
    #[serde(default)]
    pub cover_url: String,
    #[serde(default)]
    pub uri: String,
    #[serde(default)]
    pub is_explicit: bool,
}

/// Spotify playback state (broadcast on every state change + 1Hz position updates)
#[derive(Archive, Serialize, Deserialize, Debug, Clone)]
#[derive(serde::Serialize, serde::Deserialize)]
pub struct SpotifyStatus {
    pub is_playing: bool,
    pub shuffle: bool,
    pub repeat_context: bool,
    pub repeat_track: bool,
    pub volume: u16,
    pub position_ms: u32,
    pub duration_ms: u32,
    pub track: Option<TrackInfo>,
    #[serde(default)]
    pub connected_user: Option<String>,
}

/// Bluetooth event
#[derive(Archive, Serialize, Deserialize, Debug, Clone)]
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind")]
pub enum BtEvent {
    DeviceConnected { name: String, addr: String },
    DeviceDisconnected { addr: String },
    DiscoveryResult { name: String, addr: String, rssi: i16 },
}

/// Network state
#[derive(Archive, Serialize, Deserialize, Debug, Clone, PartialEq)]
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind")]
pub enum NetworkState {
    Disconnected,
    Connecting { ssid: String },
    Connected { ssid: String, ip: String, signal: i8, #[serde(default)] hostname: String, #[serde(default)] frequency_mhz: u32 },
    ApMode { ssid: String, clients: u8, #[serde(default)] ap_frequency_mhz: u32 },
    ConnectedWithAp { ssid: String, ip: String, signal: i8, hostname: String, #[serde(default)] frequency_mhz: u32, ap_ssid: String, ap_clients: u8, #[serde(default)] ap_frequency_mhz: u32 },
}

/// Per-core CPU jiffies snapshot (client computes delta for %)
#[derive(Archive, Serialize, Deserialize, Debug, Clone)]
#[derive(serde::Serialize, serde::Deserialize)]
pub struct CpuCoreSnapshot {
    pub user: u64,
    pub nice: u64,
    pub system: u64,
    pub idle: u64,
    pub iowait: u64,
    pub irq: u64,
    pub softirq: u64,
    /// Current clock speed in kHz (from scaling_cur_freq), 0 if unavailable.
    #[serde(default)]
    pub freq_khz: u32,
}

/// Disk usage for a single mount point
#[derive(Archive, Serialize, Deserialize, Debug, Clone)]
#[derive(serde::Serialize, serde::Deserialize)]
pub struct DiskUsage {
    pub mount: String,
    pub total_kb: u64,
    pub used_kb: u64,
}

/// Network interface traffic snapshot
#[derive(Archive, Serialize, Deserialize, Debug, Clone)]
#[derive(serde::Serialize, serde::Deserialize)]
pub struct NetInterfaceSnapshot {
    pub name: String,
    pub rx_bytes: u64,
    pub tx_bytes: u64,
}

/// System-wide snapshot (1Hz telemetry)
#[derive(Archive, Serialize, Deserialize, Debug, Clone)]
#[derive(serde::Serialize, serde::Deserialize)]
pub struct SystemSnapshot {
    pub uptime_secs: u64,
    pub cpu_percent: u8,
    pub ram_used_kb: u32,
    pub ram_total_kb: u32,
    pub cores: Vec<CpuCoreSnapshot>,
    pub load_avg: [f32; 3],
    pub ram_free_kb: u32,
    pub ram_buffers_kb: u32,
    pub ram_cached_kb: u32,
    pub temperature_mc: Option<i32>,
    pub disks: Vec<DiskUsage>,
    pub net_interfaces: Vec<NetInterfaceSnapshot>,
    pub process_count: u16,
}

fn default_health() -> u8 { 100 }

/// Status of a peer in the speaker group.
#[derive(Archive, Serialize, Deserialize, Debug, Clone)]
#[derive(serde::Serialize, serde::Deserialize)]
pub struct PeerInfo {
    pub peer_id: String,
    pub name: String,
    pub address: String,
    pub channel: String,
    pub role: String,
    pub latency_us: i64,
    pub connected: bool,
    #[serde(default)]
    pub packet_loss_pct: f32,
    #[serde(default)]
    pub clock_offset_us: i64,
    #[serde(default = "default_health")]
    pub buffer_health: u8,
    #[serde(default)]
    pub hop_count: u8,
    #[serde(default)]
    pub is_relay: bool,
    #[serde(default)]
    pub instability_score: u32,
}

/// Status of the multi-speaker group.
#[derive(Archive, Serialize, Deserialize, Debug, Clone)]
#[derive(serde::Serialize, serde::Deserialize)]
pub struct GroupStatus {
    pub enabled: bool,
    pub group_name: String,
    pub role: String,
    pub peers: Vec<PeerInfo>,
    pub buffer_ms: u16,
    pub channel: String,
    #[serde(default)]
    pub party_mode: bool,
    #[serde(default)]
    pub volume: u8,
}

/// Firmware -> Dashboard messages
#[derive(Archive, Serialize, Deserialize, Debug, Clone)]
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum ServerMsg {
    SystemStatus(SystemSnapshot),
    SubsystemStatus(SubsystemSnapshot),
    TrackChanged(TrackInfo),
    BluetoothEvent(BtEvent),
    VolumeChanged { source: SourceId, level: u8 },
    NetworkChanged(NetworkState),
    LedStateChanged(LedAnimation),
    CrashReport(CrashSummary),
    ConfigLoaded(Box<EncoreConfig>),
    LogEntries(Vec<LogEntry>),
    AudioLevels { left_rms: f32, right_rms: f32, left_peak: f32, right_peak: f32 },
    SpotifyStatus(SpotifyStatus),
    EqState(EqState),
    DrcState(DrcState),
    DspInfo(DspInfo),
    DacRegValue { page: u8, reg: u8, value: u8 },
    DspSpiResponse { data: Vec<u8> },
    DspMemoryDump { start_page: u16, data: Vec<u8> },
    DspEvent { description: String },
    WifiConnectResult(WifiConnectResult),
    TimeSynced { timestamp_secs: u64 },
    AudioSpectrum { bins: Vec<f32> },
    AudioWaveform { samples: Vec<f32> },
    GroupStatus(GroupStatus),
    BootMode { safe_mode: bool, #[serde(default)] boot_source: String },
    AudioPowerState { state: String },
}

/// Dashboard -> Firmware messages
#[derive(Archive, Serialize, Deserialize, Debug, Clone)]
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum ClientMsg {
    SetVolume { source: SourceId, level: u8 },
    SetMasterVolume(u8),
    SpotifyControl(SpotifyAction),
    BluetoothControl(BtAction),
    SetLed(LedAnimation),
    SetWifi(WifiCredentials),
    SetDebugMode { subsystem: String, mode: DebugMode },
    RestartSubsystem(String),
    RequestConfig,
    SaveConfig(Box<EncoreConfig>),
    // EQ controls
    SetEqBand { band: u8, config: EqBand },
    SetEqPreset(EqPreset),
    SetEqEnabled(bool),
    // DRC controls
    SetDrc { band: DrcBand, config: DrcBandConfig },
    SetDrcCrossover { low_mid_hz: u16, mid_high_hz: u16 },
    SetDrcEnabled(bool),
    SetDrcPreset(DrcPreset),
    // DSP controls
    SetDspVolume(u8),
    SetMicMute(bool),
    // Hardware explorers
    DacRegRead { page: u8, reg: u8 },
    DacRegWrite { page: u8, reg: u8, value: u8 },
    DspSpiSend { msg_type: u16, data: Vec<u8> },
    DspMemoryDump { start_page: u16, num_pages: u16 },
    DspDumpToFile { path: String },
    DspPollEvents,
    RequestNetworkState,
    SetCustomAnimation { frames: Vec<LedFrame> },
    // Group controls
    SetGroupEnabled(bool),
    SetGroupChannel(String),
    SetGroupBufferMs(u16),
    SetGroupName(String),
    SetGroupVolume(u8),
    SetPartyMode(bool),
    RequestGroupStatus,
}

/// Full config mirror for dashboard editor
#[derive(Archive, Serialize, Deserialize, Debug, Clone)]
#[derive(serde::Serialize, serde::Deserialize)]
pub struct EncoreConfig {
    pub device_name: String,
    pub master_volume: u8,
    pub spotify_volume: u8,
    pub bluetooth_volume: u8,
    pub tts_duck_percent: u8,
    pub volume_ring_step: u8,
    pub spotify_enabled: bool,
    pub spotify_bitrate: String,
    pub spotify_gapless: bool,
    pub spotify_normalisation: bool,
    pub spotify_normalisation_type: String,
    pub spotify_normalisation_pregain_db: f32,
    pub bluetooth_enabled: bool,
    pub bluetooth_discoverable: bool,
    pub homeassistant_enabled: bool,
    pub mqtt_host: Option<String>,
    pub mqtt_port: Option<u16>,
    pub mqtt_user: Option<String>,
    pub mqtt_password: Option<String>,
    pub wyoming_enabled: bool,
    pub wyoming_host: Option<String>,
    pub wyoming_port: Option<u16>,
    pub wifi_ssid: Option<String>,
    pub wifi_password: Option<String>,
    pub ap_keep_alive: bool,
    pub ap_ssid: Option<String>,
    pub ap_password: Option<String>,
    pub vpn_enabled: bool,
    pub vpn_private_key: Option<String>,
    pub vpn_address: Option<String>,
    pub vpn_peer_public_key: Option<String>,
    pub vpn_peer_preshared_key: Option<String>,
    pub vpn_peer_endpoint: Option<String>,
    pub vpn_peer_allowed_ips: Option<String>,
    pub vpn_persistent_keepalive: u16,
    pub group_enabled: bool,
    pub group_name: String,
    pub group_channel: String,
    pub group_buffer_ms: u16,
    #[serde(default)]
    pub group_peers: Vec<String>,
    pub debug_mode: String,
}

impl Default for EncoreConfig {
    /// Matches EncoreConfigFile::default() — the canonical source of truth.
    fn default() -> Self {
        Self::from_file(&crate::config::EncoreConfigFile::default())
    }
}

impl EncoreConfig {
    /// Convert from EncoreConfigFile to protocol EncoreConfig
    pub fn from_file(cfg: &EncoreConfigFile) -> Self {
        Self {
            device_name: cfg.device.name.clone(),
            master_volume: cfg.audio.master_volume,
            spotify_volume: cfg.audio.spotify_volume,
            bluetooth_volume: cfg.audio.bluetooth_volume,
            tts_duck_percent: cfg.audio.tts_duck_percent,
            volume_ring_step: cfg.audio.volume_ring_step,
            spotify_enabled: cfg.spotify.enabled,
            spotify_bitrate: cfg.spotify.bitrate.clone(),
            spotify_gapless: cfg.spotify.gapless,
            spotify_normalisation: cfg.spotify.normalisation,
            spotify_normalisation_type: cfg.spotify.normalisation_type.clone(),
            spotify_normalisation_pregain_db: cfg.spotify.normalisation_pregain_db,
            bluetooth_enabled: cfg.bluetooth.enabled,
            bluetooth_discoverable: cfg.bluetooth.discoverable,
            homeassistant_enabled: cfg.homeassistant.enabled,
            mqtt_host: cfg.homeassistant.mqtt_host.clone(),
            mqtt_port: cfg.homeassistant.mqtt_port,
            mqtt_user: cfg.homeassistant.mqtt_user.clone(),
            mqtt_password: cfg.homeassistant.mqtt_password.clone(),
            wyoming_enabled: cfg.wyoming.enabled,
            wyoming_host: cfg.wyoming.server_host.clone(),
            wyoming_port: cfg.wyoming.server_port,
            wifi_ssid: cfg.network.wifi_ssid.clone(),
            wifi_password: cfg.network.wifi_password.clone(),
            ap_keep_alive: cfg.network.ap_keep_alive,
            ap_ssid: cfg.network.ap_ssid.clone(),
            ap_password: cfg.network.ap_password.clone(),
            vpn_enabled: cfg.vpn.enabled,
            vpn_private_key: cfg.vpn.private_key.clone(),
            vpn_address: cfg.vpn.address.clone(),
            vpn_peer_public_key: cfg.vpn.peer_public_key.clone(),
            vpn_peer_preshared_key: cfg.vpn.peer_preshared_key.clone(),
            vpn_peer_endpoint: cfg.vpn.peer_endpoint.clone(),
            vpn_peer_allowed_ips: cfg.vpn.peer_allowed_ips.clone(),
            vpn_persistent_keepalive: cfg.vpn.persistent_keepalive,
            group_enabled: cfg.group.enabled,
            group_name: cfg.group.group_name.clone(),
            group_channel: cfg.group.channel.clone(),
            group_buffer_ms: cfg.group.buffer_ms,
            group_peers: cfg.group.peers.clone(),
            debug_mode: cfg.debug.default_mode.clone(),
        }
    }

    /// Convert protocol EncoreConfig back to EncoreConfigFile for saving.
    /// Merges onto an existing file config to preserve fields not exposed
    /// in the protocol (e.g. spotify.cache_path, debug.overrides).
    pub fn to_file(&self) -> EncoreConfigFile {
        self.to_file_merge(&crate::config::EncoreConfigFile::default())
    }

    /// Merge protocol config onto an existing EncoreConfigFile, preserving
    /// fields that the dashboard doesn't edit (cache_path, overrides).
    pub fn to_file_merge(&self, existing: &EncoreConfigFile) -> EncoreConfigFile {
        use crate::config::*;
        EncoreConfigFile {
            device: DeviceConfig { name: self.device_name.clone() },
            audio: AudioConfig {
                master_volume: self.master_volume,
                spotify_volume: self.spotify_volume,
                bluetooth_volume: self.bluetooth_volume,
                tts_duck_percent: self.tts_duck_percent,
                volume_ring_step: self.volume_ring_step,
                // Preserve power management config (not dashboard-editable)
                idle_timeout_secs: existing.audio.idle_timeout_secs,
                standby_timeout_secs: existing.audio.standby_timeout_secs,
                dsp_power_gate: existing.audio.dsp_power_gate,
            },
            eq: existing.eq.clone(),
            drc: existing.drc.clone(),
            spotify: SpotifyConfig {
                enabled: self.spotify_enabled,
                cache_path: existing.spotify.cache_path.clone(),
                bitrate: self.spotify_bitrate.clone(),
                gapless: self.spotify_gapless,
                normalisation: self.spotify_normalisation,
                normalisation_type: self.spotify_normalisation_type.clone(),
                normalisation_pregain_db: self.spotify_normalisation_pregain_db,
            },
            bluetooth: BluetoothConfig {
                enabled: self.bluetooth_enabled,
                discoverable: self.bluetooth_discoverable,
            },
            homeassistant: HomeAssistantConfig {
                enabled: self.homeassistant_enabled,
                mqtt_host: self.mqtt_host.clone(),
                mqtt_port: self.mqtt_port,
                mqtt_user: self.mqtt_user.clone(),
                mqtt_password: self.mqtt_password.clone(),
            },
            wyoming: WyomingConfig {
                enabled: self.wyoming_enabled,
                server_host: self.wyoming_host.clone(),
                server_port: self.wyoming_port,
            },
            network: NetworkConfig {
                wifi_ssid: self.wifi_ssid.clone(),
                wifi_password: self.wifi_password.clone(),
                ap_keep_alive: self.ap_keep_alive,
                ap_ssid: self.ap_ssid.clone(),
                ap_password: self.ap_password.clone(),
            },
            vpn: VpnConfig {
                enabled: self.vpn_enabled,
                private_key: self.vpn_private_key.clone(),
                address: self.vpn_address.clone(),
                peer_public_key: self.vpn_peer_public_key.clone(),
                peer_preshared_key: self.vpn_peer_preshared_key.clone(),
                peer_endpoint: self.vpn_peer_endpoint.clone(),
                peer_allowed_ips: self.vpn_peer_allowed_ips.clone(),
                persistent_keepalive: self.vpn_persistent_keepalive,
            },
            group: GroupConfig {
                enabled: self.group_enabled,
                group_name: self.group_name.clone(),
                channel: self.group_channel.clone(),
                buffer_ms: self.group_buffer_ms,
                peer_id: existing.group.peer_id.clone(),
                peers: self.group_peers.clone(),
                party_mode: existing.group.party_mode,
            },
            debug: DebugConfig {
                default_mode: self.debug_mode.clone(),
                overrides: existing.debug.overrides.clone(),
            },
        }
    }
}

/// Crash summary for dashboard display
#[derive(Archive, Serialize, Deserialize, Debug, Clone)]
#[derive(serde::Serialize, serde::Deserialize)]
pub struct CrashSummary {
    pub subsystem: String,
    pub message: String,
    pub backtrace: String,
    pub timestamp_secs: u64,
    pub restart_count: u8,
}

/// Per-subsystem status snapshot
#[derive(Archive, Serialize, Deserialize, Debug, Clone)]
#[derive(serde::Serialize, serde::Deserialize)]
pub struct SubsystemSnapshot {
    pub name: String,
    pub state: SubsystemState,
    pub debug_mode: DebugMode,
    pub restart_count: u8,
    pub msg_count: u64,
    pub uptime_secs: u64,
}

/// Subsystem lifecycle state
#[derive(Archive, Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[derive(serde::Serialize, serde::Deserialize)]
pub enum SubsystemState {
    Starting,
    Running,
    Degraded,
    Held,
    Crashed,
    Stopped,
}

impl SubsystemState {
    /// Convert from atomic u8 storage back to enum.
    pub fn from_u8(v: u8) -> Self {
        match v {
            0 => Self::Starting,
            1 => Self::Running,
            2 => Self::Degraded,
            3 => Self::Held,
            4 => Self::Crashed,
            _ => Self::Stopped,
        }
    }
}

/// Log entry for real-time log viewer
#[derive(Archive, Serialize, Deserialize, Debug, Clone)]
#[derive(serde::Serialize, serde::Deserialize)]
pub struct LogEntry {
    pub timestamp_ms: u64,
    pub level: String,
    pub target: String,
    pub message: String,
}

/// Result of a WiFi connection attempt
#[derive(Archive, Serialize, Deserialize, Debug, Clone)]
#[derive(serde::Serialize, serde::Deserialize)]
pub struct WifiConnectResult {
    pub ssid: String,
    pub success: bool,
    pub error: Option<String>,
}

/// WiFi network from scan results
#[derive(Archive, Serialize, Deserialize, Debug, Clone)]
#[derive(serde::Serialize, serde::Deserialize)]
pub struct WifiNetwork {
    pub ssid: String,
    pub bssid: String,
    pub signal_dbm: i16,
    pub security: String,
    #[serde(default)]
    pub frequency_mhz: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper macro: rkyv round-trip serialize then deserialize
    macro_rules! rkyv_rt {
        ($val:expr, $ty:ty) => {{
            let bytes = rkyv::to_bytes::<rkyv::rancor::Error>($val).unwrap();
            rkyv::from_bytes::<$ty, rkyv::rancor::Error>(&bytes).unwrap()
        }};
    }

    /// Helper macro: JSON round-trip serialize then deserialize
    macro_rules! json_rt {
        ($val:expr, $ty:ty) => {{
            let json = serde_json::to_string($val).unwrap();
            let rt: $ty = serde_json::from_str(&json).unwrap();
            rt
        }};
    }

    #[test]
    fn source_id_round_trip() {
        for id in [SourceId::Spotify, SourceId::Bluetooth, SourceId::Wyoming, SourceId::System] {
            let rt: SourceId = rkyv_rt!(&id, SourceId);
            assert_eq!(rt, id);
            let jrt: SourceId = json_rt!(&id, SourceId);
            assert_eq!(jrt, id);
        }
    }

    #[test]
    fn subsystem_state_round_trip() {
        for state in [
            SubsystemState::Starting,
            SubsystemState::Running,
            SubsystemState::Degraded,
            SubsystemState::Held,
            SubsystemState::Crashed,
            SubsystemState::Stopped,
        ] {
            let rt: SubsystemState = rkyv_rt!(&state, SubsystemState);
            assert_eq!(rt, state);
            let jrt: SubsystemState = json_rt!(&state, SubsystemState);
            assert_eq!(jrt, state);
        }
    }

    #[test]
    fn debug_mode_round_trip() {
        for mode in [DebugMode::Production, DebugMode::Hold, DebugMode::Trace] {
            let rt: DebugMode = rkyv_rt!(&mode, DebugMode);
            assert_eq!(rt, mode);
            let jrt: DebugMode = json_rt!(&mode, DebugMode);
            assert_eq!(jrt, mode);
        }
    }

    #[test]
    fn spotify_action_json_round_trip() {
        let actions = vec![
            SpotifyAction::Play,
            SpotifyAction::Pause,
            SpotifyAction::Next,
            SpotifyAction::Previous,
            SpotifyAction::SetVolume { level: 75 },
            SpotifyAction::Seek { position_ms: 30000 },
            SpotifyAction::Shuffle { enabled: true },
            SpotifyAction::Repeat { enabled: true },
            SpotifyAction::RepeatTrack { enabled: false },
        ];
        for action in &actions {
            let _: SpotifyAction = rkyv_rt!(action, SpotifyAction);
            let _: SpotifyAction = json_rt!(action, SpotifyAction);
        }
    }

    #[test]
    fn bt_action_json_round_trip() {
        let actions = vec![
            BtAction::StartDiscovery,
            BtAction::StopDiscovery,
            BtAction::Pair { addr: "AA:BB:CC:DD:EE:FF".into() },
            BtAction::Connect { addr: "AA:BB:CC:DD:EE:FF".into() },
            BtAction::Disconnect { addr: "AA:BB:CC:DD:EE:FF".into() },
            BtAction::Forget { addr: "AA:BB:CC:DD:EE:FF".into() },
        ];
        for action in &actions {
            let _: BtAction = rkyv_rt!(action, BtAction);
            let _: BtAction = json_rt!(action, BtAction);
        }
    }

    #[test]
    fn led_animation_json_round_trip() {
        let anims = vec![
            LedAnimation::Off,
            LedAnimation::Solid { r: 255, g: 0, b: 128 },
            LedAnimation::Breathe { r: 0, g: 255, b: 0, period_ms: 2000 },
            LedAnimation::Spin { r: 128, g: 128, b: 128, speed: 5 },
            LedAnimation::Pulse { r: 255, g: 255, b: 255 },
            LedAnimation::VolumeArc { level: 80 },
            LedAnimation::BootSurge,
            LedAnimation::SafeMode,
            LedAnimation::Custom { frames: vec![
                LedFrame { colors: [(255,0,0); 13], duration_ms: 100 },
                LedFrame { colors: [(0,255,0); 13], duration_ms: 200 },
            ]},
        ];
        for anim in &anims {
            let _: LedAnimation = rkyv_rt!(anim, LedAnimation);
            let _: LedAnimation = json_rt!(anim, LedAnimation);
        }
    }

    #[test]
    fn track_info_round_trip() {
        let track = TrackInfo {
            title: "Bohemian Rhapsody".into(),
            artist: "Queen".into(),
            album: "A Night at the Opera".into(),
            duration_ms: 354000,
            position_ms: 120000,
            cover_url: "https://i.scdn.co/image/abc123".into(),
            uri: "spotify:track:abc123".into(),
            is_explicit: true,
        };
        let rt: TrackInfo = rkyv_rt!(&track, TrackInfo);
        assert_eq!(rt.title, "Bohemian Rhapsody");
        assert_eq!(rt.artist, "Queen");
        assert_eq!(rt.duration_ms, 354000);
        assert_eq!(rt.cover_url, "https://i.scdn.co/image/abc123");
        assert_eq!(rt.uri, "spotify:track:abc123");
        assert!(rt.is_explicit);
        let jrt: TrackInfo = json_rt!(&track, TrackInfo);
        assert_eq!(jrt.title, "Bohemian Rhapsody");
        assert_eq!(jrt.artist, "Queen");
        assert_eq!(jrt.duration_ms, 354000);
        assert_eq!(jrt.cover_url, "https://i.scdn.co/image/abc123");
        assert!(jrt.is_explicit);
    }

    #[test]
    fn spotify_status_round_trip() {
        let status = SpotifyStatus {
            is_playing: true,
            shuffle: true,
            repeat_context: false,
            repeat_track: true,
            volume: 32768,
            position_ms: 45000,
            duration_ms: 210000,
            track: Some(TrackInfo {
                title: "Test Track".into(),
                artist: "Test Artist".into(),
                album: "Test Album".into(),
                duration_ms: 210000,
                position_ms: 45000,
                cover_url: "https://example.com/cover.jpg".into(),
                uri: "spotify:track:xyz".into(),
                is_explicit: false,
            }),
            connected_user: Some("testuser".into()),
        };
        let rt: SpotifyStatus = rkyv_rt!(&status, SpotifyStatus);
        assert!(rt.is_playing);
        assert!(rt.shuffle);
        assert!(!rt.repeat_context);
        assert!(rt.repeat_track);
        assert_eq!(rt.volume, 32768);
        assert_eq!(rt.position_ms, 45000);
        assert!(rt.track.is_some());
        assert_eq!(rt.connected_user.as_deref(), Some("testuser"));
        let jrt: SpotifyStatus = json_rt!(&status, SpotifyStatus);
        assert!(jrt.is_playing);
        assert!(jrt.shuffle);
        assert_eq!(jrt.volume, 32768);
        assert_eq!(jrt.track.as_ref().unwrap().title, "Test Track");
        assert_eq!(jrt.connected_user.as_deref(), Some("testuser"));

        // Also test with no track
        let empty = SpotifyStatus {
            is_playing: false,
            shuffle: false,
            repeat_context: false,
            repeat_track: false,
            volume: 0,
            position_ms: 0,
            duration_ms: 0,
            track: None,
            connected_user: None,
        };
        let rt2: SpotifyStatus = rkyv_rt!(&empty, SpotifyStatus);
        assert!(!rt2.is_playing);
        assert!(rt2.track.is_none());
        assert!(rt2.connected_user.is_none());
        let jrt2: SpotifyStatus = json_rt!(&empty, SpotifyStatus);
        assert!(jrt2.track.is_none());
    }

    #[test]
    fn network_state_json_round_trip() {
        let states = vec![
            NetworkState::Disconnected,
            NetworkState::Connecting { ssid: "MyWifi".into() },
            NetworkState::Connected {
                ssid: "MyWifi".into(),
                ip: "192.168.1.42".into(),
                signal: -45,
                hostname: "encore.local".into(),
                frequency_mhz: 5180,
            },
            NetworkState::ApMode {
                ssid: "HK-Invoke".into(),
                clients: 2,
                ap_frequency_mhz: 2437,
            },
            NetworkState::ConnectedWithAp {
                ssid: "MyWifi".into(),
                ip: "192.168.1.42".into(),
                signal: -45,
                hostname: "encore.local".into(),
                frequency_mhz: 5180,
                ap_ssid: "HK-Invoke".into(),
                ap_clients: 1,
                ap_frequency_mhz: 2437,
            },
        ];
        for state in &states {
            let _: NetworkState = rkyv_rt!(state, NetworkState);
            let _: NetworkState = json_rt!(state, NetworkState);
        }
    }

    #[test]
    fn crash_summary_rkyv_round_trip() {
        let crash = CrashSummary {
            subsystem: "bluetooth".into(),
            message: "D-Bus connection lost".into(),
            backtrace: "at bluetooth/mod.rs:42".into(),
            timestamp_secs: 1739600000,
            restart_count: 2,
        };
        let rt: CrashSummary = rkyv_rt!(&crash, CrashSummary);
        assert_eq!(rt.subsystem, "bluetooth");
        assert_eq!(rt.restart_count, 2);
    }

    #[test]
    fn crash_summary_serde_json_round_trip() {
        let crash = CrashSummary {
            subsystem: "audio".into(),
            message: "PCM open failed".into(),
            backtrace: String::new(),
            timestamp_secs: 1739600000,
            restart_count: 1,
        };
        let json = serde_json::to_string(&crash).unwrap();
        let rt: CrashSummary = serde_json::from_str(&json).unwrap();
        assert_eq!(rt.subsystem, "audio");
        assert_eq!(rt.message, "PCM open failed");
    }

    #[test]
    fn server_msg_json_round_trip() {
        let msgs: Vec<ServerMsg> = vec![
            ServerMsg::SystemStatus(SystemSnapshot {
                uptime_secs: 3600,
                cpu_percent: 45,
                ram_used_kb: 128000,
                ram_total_kb: 256000,
                cores: vec![],
                load_avg: [0.5, 0.3, 0.1],
                ram_free_kb: 100000,
                ram_buffers_kb: 8000,
                ram_cached_kb: 20000,
                temperature_mc: Some(45200),
                disks: vec![],
                net_interfaces: vec![],
                process_count: 42,
            }),
            ServerMsg::VolumeChanged {
                source: SourceId::Spotify,
                level: 80,
            },
            ServerMsg::TrackChanged(TrackInfo {
                title: "Test".into(),
                artist: "Artist".into(),
                album: "Album".into(),
                duration_ms: 180000,
                position_ms: 0,
                cover_url: String::new(),
                uri: String::new(),
                is_explicit: false,
            }),
            ServerMsg::NetworkChanged(NetworkState::Disconnected),
            ServerMsg::LedStateChanged(LedAnimation::Off),
            ServerMsg::SubsystemStatus(SubsystemSnapshot {
                name: "audio".into(),
                state: SubsystemState::Running,
                debug_mode: DebugMode::Production,
                restart_count: 0,
                msg_count: 10,
                uptime_secs: 500,
            }),
            ServerMsg::CrashReport(CrashSummary {
                subsystem: "bluetooth".into(),
                message: "panic".into(),
                backtrace: String::new(),
                timestamp_secs: 100,
                restart_count: 1,
            }),
            ServerMsg::ConfigLoaded(Box::new(EncoreConfig::default())),
            ServerMsg::SpotifyStatus(SpotifyStatus {
                is_playing: true,
                shuffle: false,
                repeat_context: true,
                repeat_track: false,
                volume: 40000,
                position_ms: 60000,
                duration_ms: 240000,
                track: Some(TrackInfo {
                    title: "Song".into(),
                    artist: "Band".into(),
                    album: "LP".into(),
                    duration_ms: 240000,
                    position_ms: 60000,
                    cover_url: String::new(),
                    uri: "spotify:track:test".into(),
                    is_explicit: false,
                }),
                connected_user: Some("user123".into()),
            }),
            ServerMsg::EqState(EqState::default()),
            ServerMsg::DrcState(DrcState::default()),
            ServerMsg::DspInfo(DspInfo { version: "1.0".into(), hybridflow: 6, mic_muted: false, dsp_volume: 80 }),
            ServerMsg::DacRegValue { page: 0, reg: 0x2B, value: 6 },
            ServerMsg::DspSpiResponse { data: vec![0x00, 0x08, 0x01, 0x02] },
            ServerMsg::DspMemoryDump { start_page: 0, data: vec![0xDE, 0xAD] },
            ServerMsg::DspEvent { description: "TriggerFound".into() },
            ServerMsg::WifiConnectResult(WifiConnectResult { ssid: "Test".into(), success: true, error: None }),
            ServerMsg::TimeSynced { timestamp_secs: 1739600000 },
            ServerMsg::AudioSpectrum { bins: vec![0.0; 32] },
            ServerMsg::AudioWaveform { samples: vec![0.0; 256] },
            ServerMsg::BootMode { safe_mode: true, boot_source: "rootfs".into() },
            ServerMsg::BootMode { safe_mode: false, boot_source: "next".into() },
            ServerMsg::AudioPowerState { state: "active".into() },
            ServerMsg::AudioPowerState { state: "idle".into() },
            ServerMsg::AudioPowerState { state: "standby".into() },
        ];
        for msg in &msgs {
            let _: ServerMsg = rkyv_rt!(msg, ServerMsg);
            let json = serde_json::to_string(msg).unwrap();
            let _rt: ServerMsg = serde_json::from_str(&json).unwrap();
            // Verify tagged dispatch structure
            let val: serde_json::Value = serde_json::from_str(&json).unwrap();
            assert!(val.get("type").is_some(), "ServerMsg must have 'type' tag");
        }
    }

    #[test]
    fn client_msg_json_round_trip() {
        let msgs: Vec<ClientMsg> = vec![
            ClientMsg::SetVolume {
                source: SourceId::Bluetooth,
                level: 50,
            },
            ClientMsg::SetMasterVolume(75),
            ClientMsg::SpotifyControl(SpotifyAction::Play),
            ClientMsg::BluetoothControl(BtAction::StartDiscovery),
            ClientMsg::SetLed(LedAnimation::Solid { r: 255, g: 0, b: 0 }),
            ClientMsg::SetWifi(WifiCredentials {
                ssid: "TestNet".into(),
                password: "pass123".into(),
            }),
            ClientMsg::SetDebugMode {
                subsystem: "audio".into(),
                mode: DebugMode::Hold,
            },
            ClientMsg::RestartSubsystem("bluetooth".into()),
            ClientMsg::RequestConfig,
            ClientMsg::SaveConfig(Box::new(EncoreConfig::default())),
            ClientMsg::SetEqBand { band: 0, config: EqBand::default() },
            ClientMsg::SetEqPreset(EqPreset::Flat),
            ClientMsg::SetEqEnabled(true),
            ClientMsg::SetDrc { band: DrcBand::Low, config: DrcBandConfig::default() },
            ClientMsg::SetDrcCrossover { low_mid_hz: 200, mid_high_hz: 2000 },
            ClientMsg::SetDrcEnabled(false),
            ClientMsg::SetDrcPreset(DrcPreset::Off),
            ClientMsg::SetDspVolume(80),
            ClientMsg::SetMicMute(true),
            ClientMsg::DacRegRead { page: 0, reg: 0x2B },
            ClientMsg::DacRegWrite { page: 0, reg: 0x3D, value: 0x30 },
            ClientMsg::DspSpiSend { msg_type: 0x0000, data: vec![0x08] },
            ClientMsg::DspMemoryDump { start_page: 0, num_pages: 1 },
            ClientMsg::DspPollEvents,
            ClientMsg::RequestNetworkState,
            ClientMsg::SetCustomAnimation { frames: vec![LedFrame { colors: [(255,0,0); 13], duration_ms: 100 }] },
        ];
        for msg in &msgs {
            let _: ClientMsg = rkyv_rt!(msg, ClientMsg);
            let json = serde_json::to_string(msg).unwrap();
            let _rt: ClientMsg = serde_json::from_str(&json).unwrap();
            // Verify tagged dispatch structure
            let val: serde_json::Value = serde_json::from_str(&json).unwrap();
            assert!(val.get("type").is_some(), "ClientMsg must have 'type' tag");
        }
    }

    #[test]
    fn subsystem_snapshot_round_trip() {
        let snap = SubsystemSnapshot {
            name: "spotify".into(),
            state: SubsystemState::Running,
            debug_mode: DebugMode::Production,
            restart_count: 0,
            msg_count: 42,
            uptime_secs: 1200,
        };
        let rt: SubsystemSnapshot = rkyv_rt!(&snap, SubsystemSnapshot);
        assert_eq!(rt.name, "spotify");
        assert_eq!(rt.state, SubsystemState::Running);
        assert_eq!(rt.msg_count, 42);
        let jrt: SubsystemSnapshot = json_rt!(&snap, SubsystemSnapshot);
        assert_eq!(jrt.name, "spotify");
        assert_eq!(jrt.state, SubsystemState::Running);
        assert_eq!(jrt.msg_count, 42);
    }

    #[test]
    fn encore_config_default_matches_file_default() {
        let cfg = EncoreConfig::default();
        let file_cfg = crate::config::EncoreConfigFile::default();
        let from_file = EncoreConfig::from_file(&file_cfg);
        // EncoreConfig::default() must match from_file() for consistency
        assert_eq!(cfg.device_name, from_file.device_name);
        assert_eq!(cfg.device_name, "Encore");
        assert_eq!(cfg.spotify_enabled, from_file.spotify_enabled);
        assert!(cfg.spotify_enabled);
        assert_eq!(cfg.spotify_bitrate, "320");
        assert!(cfg.spotify_gapless);
        assert!(!cfg.spotify_normalisation);
        assert_eq!(cfg.spotify_normalisation_type, "auto");
        assert_eq!(cfg.spotify_normalisation_pregain_db, 0.0);
        assert!(cfg.bluetooth_enabled);
        assert!(cfg.bluetooth_discoverable);
        assert_eq!(cfg.master_volume, 70);
        assert!(cfg.ap_keep_alive);
        assert_eq!(cfg.debug_mode, "production");
        // Round-trip through rkyv and JSON
        let rt: EncoreConfig = rkyv_rt!(&cfg, EncoreConfig);
        assert_eq!(rt.device_name, "Encore");
        assert!(rt.spotify_enabled);
        let jrt: EncoreConfig = json_rt!(&cfg, EncoreConfig);
        assert_eq!(jrt.device_name, "Encore");
        assert!(jrt.spotify_enabled);
    }

    #[test]
    fn encore_config_file_conversion_round_trip() {
        let file_cfg = crate::config::EncoreConfigFile {
            device: crate::config::DeviceConfig { name: "Test".into() },
            audio: crate::config::AudioConfig {
                master_volume: 50,
                spotify_volume: 80,
                bluetooth_volume: 60,
                tts_duck_percent: 40,
                volume_ring_step: 3,
                idle_timeout_secs: 10,
                standby_timeout_secs: 120,
                dsp_power_gate: true,
            },
            eq: crate::config::EqConfig::default(),
            drc: crate::config::DrcConfig::default(),
            spotify: crate::config::SpotifyConfig {
                enabled: true,
                bitrate: "160".into(),
                normalisation: true,
                normalisation_pregain_db: -2.5,
                ..Default::default()
            },
            bluetooth: crate::config::BluetoothConfig { enabled: true, discoverable: false },
            homeassistant: crate::config::HomeAssistantConfig {
                enabled: true,
                mqtt_host: Some("mqtt.local".into()),
                mqtt_port: Some(1883),
                mqtt_user: Some("user".into()),
                mqtt_password: Some("pass".into()),
            },
            wyoming: crate::config::WyomingConfig {
                enabled: true,
                server_host: Some("ha.local".into()),
                server_port: Some(10300),
            },
            network: crate::config::NetworkConfig {
                wifi_ssid: Some("MyWifi".into()),
                wifi_password: Some("secret".into()),
                ap_keep_alive: false,
                ..Default::default()
            },
            vpn: crate::config::VpnConfig::default(),
            group: crate::config::GroupConfig {
                peers: vec!["192.168.1.50".into(), "192.168.1.51".into()],
                ..Default::default()
            },
            debug: crate::config::DebugConfig {
                default_mode: "trace".into(),
                ..Default::default()
            },
        };

        let proto = EncoreConfig::from_file(&file_cfg);
        assert_eq!(proto.device_name, "Test");
        assert_eq!(proto.master_volume, 50);
        assert!(proto.spotify_enabled);
        assert_eq!(proto.spotify_bitrate, "160");
        assert!(proto.spotify_normalisation);
        assert!((proto.spotify_normalisation_pregain_db - (-2.5)).abs() < f32::EPSILON);
        assert!(!proto.bluetooth_discoverable);
        assert_eq!(proto.mqtt_host.as_deref(), Some("mqtt.local"));
        assert_eq!(proto.wyoming_port, Some(10300));
        assert!(!proto.ap_keep_alive);
        assert_eq!(proto.group_peers, vec!["192.168.1.50", "192.168.1.51"]);

        let back = proto.to_file();
        assert_eq!(back.device.name, "Test");
        assert_eq!(back.audio.master_volume, 50);
        assert!(back.spotify.enabled);
        assert_eq!(back.spotify.bitrate, "160");
        assert!(back.spotify.normalisation);
        assert!((back.spotify.normalisation_pregain_db - (-2.5)).abs() < f32::EPSILON);
        assert!(!back.bluetooth.discoverable);
        assert_eq!(back.homeassistant.mqtt_host.as_deref(), Some("mqtt.local"));
        assert_eq!(back.wyoming.server_port, Some(10300));
        assert!(!back.network.ap_keep_alive);
        assert_eq!(back.debug.default_mode, "trace");
        assert_eq!(back.group.peers, vec!["192.168.1.50", "192.168.1.51"]);
    }

    #[test]
    fn group_status_round_trip() {
        let status = GroupStatus {
            enabled: true,
            group_name: "Living Room".into(),
            role: "leader".into(),
            peers: vec![PeerInfo {
                peer_id: "abc-123".into(),
                name: "Kitchen".into(),
                address: "192.168.1.50".into(),
                channel: "stereo".into(),
                role: "follower".into(),
                latency_us: 1200,
                connected: true,
                packet_loss_pct: 0.5,
                clock_offset_us: -200,
                buffer_health: 95,
                hop_count: 1,
                is_relay: false,
                instability_score: 3,
            }],
            buffer_ms: 80,
            channel: "stereo".into(),
            party_mode: true,
            volume: 65,
        };
        let rt: GroupStatus = rkyv_rt!(&status, GroupStatus);
        assert_eq!(rt.group_name, "Living Room");
        assert!(rt.party_mode);
        assert_eq!(rt.volume, 65);
        let jrt: GroupStatus = json_rt!(&status, GroupStatus);
        assert!(jrt.party_mode);
        assert_eq!(jrt.volume, 65);
    }

    #[test]
    fn bt_event_json_round_trip() {
        let events = vec![
            BtEvent::DeviceConnected { name: "Speaker".into(), addr: "AA:BB:CC:DD:EE:FF".into() },
            BtEvent::DeviceDisconnected { addr: "AA:BB:CC:DD:EE:FF".into() },
            BtEvent::DiscoveryResult { name: "Phone".into(), addr: "11:22:33:44:55:66".into(), rssi: -60 },
        ];
        for event in &events {
            let _: BtEvent = rkyv_rt!(event, BtEvent);
            let _: BtEvent = json_rt!(event, BtEvent);
        }
    }

    #[test]
    fn wifi_credentials_json_round_trip() {
        let creds = WifiCredentials { ssid: "MyNet".into(), password: "secret".into() };
        let _: WifiCredentials = rkyv_rt!(&creds, WifiCredentials);
        let jrt: WifiCredentials = json_rt!(&creds, WifiCredentials);
        assert_eq!(jrt.ssid, "MyNet");
        assert_eq!(jrt.password, "secret");
    }

    #[test]
    fn system_snapshot_json_round_trip() {
        let snap = SystemSnapshot {
            uptime_secs: 86400,
            cpu_percent: 23,
            ram_used_kb: 100000,
            ram_total_kb: 256000,
            cores: vec![
                CpuCoreSnapshot { user: 1000, nice: 10, system: 500, idle: 8000, iowait: 50, irq: 5, softirq: 3, freq_khz: 1300000 },
                CpuCoreSnapshot { user: 800, nice: 5, system: 400, idle: 8500, iowait: 30, irq: 2, softirq: 1, freq_khz: 1300000 },
            ],
            load_avg: [1.23, 0.85, 0.42],
            ram_free_kb: 80000,
            ram_buffers_kb: 12000,
            ram_cached_kb: 64000,
            temperature_mc: Some(48200),
            disks: vec![
                DiskUsage { mount: "/".into(), total_kb: 44000, used_kb: 44000 },
                DiskUsage { mount: "/lsync".into(), total_kb: 32000, used_kb: 8000 },
            ],
            net_interfaces: vec![
                NetInterfaceSnapshot { name: "wlan0".into(), rx_bytes: 123456, tx_bytes: 78901 },
            ],
            process_count: 67,
        };
        let jrt: SystemSnapshot = json_rt!(&snap, SystemSnapshot);
        assert_eq!(jrt.uptime_secs, 86400);
        assert_eq!(jrt.cpu_percent, 23);
        assert_eq!(jrt.cores.len(), 2);
        assert_eq!(jrt.cores[0].user, 1000);
        assert_eq!(jrt.load_avg[0], 1.23);
        assert_eq!(jrt.ram_free_kb, 80000);
        assert_eq!(jrt.ram_buffers_kb, 12000);
        assert_eq!(jrt.ram_cached_kb, 64000);
        assert_eq!(jrt.temperature_mc, Some(48200));
        assert_eq!(jrt.disks.len(), 2);
        assert_eq!(jrt.disks[1].mount, "/lsync");
        assert_eq!(jrt.net_interfaces.len(), 1);
        assert_eq!(jrt.net_interfaces[0].rx_bytes, 123456);
        assert_eq!(jrt.process_count, 67);

        // rkyv round-trip
        let rt: SystemSnapshot = rkyv_rt!(&snap, SystemSnapshot);
        assert_eq!(rt.cores.len(), 2);
        assert_eq!(rt.temperature_mc, Some(48200));
    }

    #[test]
    fn log_entry_round_trip() {
        let entry = LogEntry {
            timestamp_ms: 1739600000000,
            level: "INFO".into(),
            target: "encore::web".into(),
            message: "Server started on port 80".into(),
        };
        let rt: LogEntry = rkyv_rt!(&entry, LogEntry);
        assert_eq!(rt.timestamp_ms, 1739600000000);
        assert_eq!(rt.level, "INFO");
        assert_eq!(rt.target, "encore::web");
        assert_eq!(rt.message, "Server started on port 80");
        let jrt: LogEntry = json_rt!(&entry, LogEntry);
        assert_eq!(jrt.timestamp_ms, 1739600000000);
        assert_eq!(jrt.level, "INFO");
        assert_eq!(jrt.target, "encore::web");
        assert_eq!(jrt.message, "Server started on port 80");
    }

    #[test]
    fn wifi_connect_result_round_trip() {
        let success = WifiConnectResult { ssid: "TestNet".into(), success: true, error: None };
        let rt: WifiConnectResult = rkyv_rt!(&success, WifiConnectResult);
        assert_eq!(rt.ssid, "TestNet");
        assert!(rt.success);
        assert!(rt.error.is_none());
        let jrt: WifiConnectResult = json_rt!(&success, WifiConnectResult);
        assert_eq!(jrt.ssid, "TestNet");
        assert!(jrt.success);

        let fail = WifiConnectResult { ssid: "BadNet".into(), success: false, error: Some("timeout".into()) };
        let rt2: WifiConnectResult = rkyv_rt!(&fail, WifiConnectResult);
        assert!(!rt2.success);
        assert_eq!(rt2.error.as_deref(), Some("timeout"));
        let jrt2: WifiConnectResult = json_rt!(&fail, WifiConnectResult);
        assert!(!jrt2.success);
        assert_eq!(jrt2.error.as_deref(), Some("timeout"));
    }

    #[test]
    fn wifi_network_round_trip() {
        let net = WifiNetwork {
            ssid: "MyNetwork".into(),
            bssid: "AA:BB:CC:DD:EE:FF".into(),
            signal_dbm: -65,
            security: "WPA2-PSK".into(),
            frequency_mhz: 5180,
        };
        let rt: WifiNetwork = rkyv_rt!(&net, WifiNetwork);
        assert_eq!(rt.ssid, "MyNetwork");
        assert_eq!(rt.bssid, "AA:BB:CC:DD:EE:FF");
        assert_eq!(rt.signal_dbm, -65);
        assert_eq!(rt.security, "WPA2-PSK");
        assert_eq!(rt.frequency_mhz, 5180);
        let jrt: WifiNetwork = json_rt!(&net, WifiNetwork);
        assert_eq!(jrt.ssid, "MyNetwork");
        assert_eq!(jrt.bssid, "AA:BB:CC:DD:EE:FF");
        assert_eq!(jrt.signal_dbm, -65);
        assert_eq!(jrt.security, "WPA2-PSK");
        assert_eq!(jrt.frequency_mhz, 5180);
    }

    #[test]
    fn server_msg_log_entries_round_trip() {
        let msg = ServerMsg::LogEntries(vec![
            LogEntry {
                timestamp_ms: 1000,
                level: "DEBUG".into(),
                target: "encore::audio".into(),
                message: "PCM opened".into(),
            },
            LogEntry {
                timestamp_ms: 2000,
                level: "WARN".into(),
                target: "encore::bt".into(),
                message: "No controller".into(),
            },
        ]);
        let _: ServerMsg = rkyv_rt!(&msg, ServerMsg);
        let json = serde_json::to_string(&msg).unwrap();
        let rt: ServerMsg = serde_json::from_str(&json).unwrap();
        let val: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(val["type"].as_str(), Some("LogEntries"));
        match rt {
            ServerMsg::LogEntries(entries) => {
                assert_eq!(entries.len(), 2);
                assert_eq!(entries[0].level, "DEBUG");
                assert_eq!(entries[1].message, "No controller");
            }
            _ => panic!("Expected LogEntries variant"),
        }
    }

    #[test]
    fn server_msg_audio_levels_round_trip() {
        let msg = ServerMsg::AudioLevels {
            left_rms: 0.42,
            right_rms: 0.38,
            left_peak: 0.95,
            right_peak: 0.87,
        };
        let _: ServerMsg = rkyv_rt!(&msg, ServerMsg);
        let json = serde_json::to_string(&msg).unwrap();
        let rt: ServerMsg = serde_json::from_str(&json).unwrap();
        let val: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(val["type"].as_str(), Some("AudioLevels"));
        match rt {
            ServerMsg::AudioLevels { left_rms, right_rms, left_peak, right_peak } => {
                assert!((left_rms - 0.42).abs() < f32::EPSILON);
                assert!((right_rms - 0.38).abs() < f32::EPSILON);
                assert!((left_peak - 0.95).abs() < f32::EPSILON);
                assert!((right_peak - 0.87).abs() < f32::EPSILON);
            }
            _ => panic!("Expected AudioLevels variant"),
        }
    }

    #[test]
    fn filter_type_round_trip() {
        for ft in [FilterType::Peak, FilterType::LowShelf, FilterType::HighShelf, FilterType::Notch] {
            let _: FilterType = rkyv_rt!(&ft, FilterType);
            let _: FilterType = json_rt!(&ft, FilterType);
        }
    }

    #[test]
    fn eq_band_round_trip() {
        let band = EqBand { freq_hz: 1000, gain_cb: 60, q_x10: 14, filter_type: FilterType::Peak };
        let rt: EqBand = rkyv_rt!(&band, EqBand);
        assert_eq!(rt.freq_hz, 1000);
        assert_eq!(rt.gain_cb, 60);
        let jrt: EqBand = json_rt!(&band, EqBand);
        assert_eq!(jrt.q_x10, 14);
    }

    #[test]
    fn eq_state_round_trip() {
        let state = EqState::default();
        let rt: EqState = rkyv_rt!(&state, EqState);
        assert!(rt.enabled);
        assert_eq!(rt.bands.len(), 10);
        assert_eq!(rt.preset, Some(EqPreset::Flat));
        let jrt: EqState = json_rt!(&state, EqState);
        assert!(jrt.enabled);
    }

    #[test]
    fn drc_state_round_trip() {
        let state = DrcState::default();
        let rt: DrcState = rkyv_rt!(&state, DrcState);
        assert!(!rt.enabled);
        assert_eq!(rt.low_mid_hz, 200);
        assert_eq!(rt.mid_high_hz, 2000);
        let jrt: DrcState = json_rt!(&state, DrcState);
        assert_eq!(jrt.bands.len(), 3);
    }

    #[test]
    fn dsp_info_round_trip() {
        let info = DspInfo { version: "1.2.3".into(), hybridflow: 6, mic_muted: false, dsp_volume: 80 };
        let rt: DspInfo = rkyv_rt!(&info, DspInfo);
        assert_eq!(rt.version, "1.2.3");
        assert_eq!(rt.hybridflow, 6);
        let jrt: DspInfo = json_rt!(&info, DspInfo);
        assert_eq!(jrt.dsp_volume, 80);
    }
}
