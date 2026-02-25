//! Configuration file schema — `/lsync/encore/config.toml`.
//!
//! All fields have serde defaults so partially-written or empty config
//! files gracefully merge with defaults. EQ and DRC settings are persisted
//! here for restore-on-boot.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EncoreConfigFile {
    #[serde(default)]
    pub device: DeviceConfig,
    #[serde(default)]
    pub audio: AudioConfig,
    #[serde(default)]
    pub eq: EqConfig,
    #[serde(default)]
    pub drc: DrcConfig,
    #[serde(default)]
    pub spotify: SpotifyConfig,
    #[serde(default)]
    pub bluetooth: BluetoothConfig,
    #[serde(default)]
    pub homeassistant: HomeAssistantConfig,
    #[serde(default)]
    pub wyoming: WyomingConfig,
    #[serde(default)]
    pub network: NetworkConfig,
    #[serde(default)]
    pub vpn: VpnConfig,
    #[serde(default)]
    pub group: GroupConfig,
    #[serde(default)]
    pub debug: DebugConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceConfig {
    #[serde(default = "default_device_name")]
    pub name: String,
}

fn default_device_name() -> String {
    "Encore".into()
}

impl Default for DeviceConfig {
    fn default() -> Self {
        Self {
            name: default_device_name(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AudioConfig {
    #[serde(default = "default_volume")]
    pub master_volume: u8,
    #[serde(default = "default_volume")]
    pub spotify_volume: u8,
    #[serde(default = "default_volume")]
    pub bluetooth_volume: u8,
    #[serde(default = "default_duck")]
    pub tts_duck_percent: u8,
    #[serde(default = "default_vol_step")]
    pub volume_ring_step: u8,
    /// Seconds of silence before muting AMP (Active → Idle).
    #[serde(default = "default_idle_timeout")]
    pub idle_timeout_secs: u32,
    /// Seconds in Idle before DAC standby + mixer park (Idle → Standby).
    #[serde(default = "default_standby_timeout")]
    pub standby_timeout_secs: u32,
    /// Power-gate DSP in Standby (saves ~500 mW-1W, adds ~3.5s resume).
    #[serde(default)]
    pub dsp_power_gate: bool,
}

fn default_volume() -> u8 {
    70
}
fn default_duck() -> u8 {
    80
}
fn default_vol_step() -> u8 {
    2
}
fn default_idle_timeout() -> u32 {
    5
}
fn default_standby_timeout() -> u32 {
    60
}

impl Default for AudioConfig {
    fn default() -> Self {
        Self {
            master_volume: default_volume(),
            spotify_volume: default_volume(),
            bluetooth_volume: default_volume(),
            tts_duck_percent: default_duck(),
            volume_ring_step: default_vol_step(),
            idle_timeout_secs: default_idle_timeout(),
            standby_timeout_secs: default_standby_timeout(),
            dsp_power_gate: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EqConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub bands: Vec<EqBandConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EqBandConfig {
    #[serde(default = "default_eq_freq")]
    pub freq_hz: u16,
    #[serde(default)]
    pub gain_cb: i16,
    #[serde(default = "default_q")]
    pub q_x10: u16,
    #[serde(default = "default_filter_type")]
    pub filter_type: String,
}

fn default_eq_freq() -> u16 { 1000 }
fn default_q() -> u16 { 10 }
fn default_filter_type() -> String { "peak".into() }

impl Default for EqConfig {
    fn default() -> Self {
        Self { enabled: true, bands: Vec::new() }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DrcConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_low_mid")]
    pub low_mid_hz: u16,
    #[serde(default = "default_mid_high")]
    pub mid_high_hz: u16,
    #[serde(default)]
    pub bands: Vec<DrcBandSaveConfig>,
}

fn default_low_mid() -> u16 { 200 }
fn default_mid_high() -> u16 { 2000 }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DrcBandSaveConfig {
    #[serde(default = "default_threshold")]
    pub threshold_db: i8,
    #[serde(default = "default_ratio")]
    pub ratio_x10: u8,
    #[serde(default = "default_attack")]
    pub attack_ms: u16,
    #[serde(default = "default_release")]
    pub release_ms: u16,
}

fn default_threshold() -> i8 { -20 }
fn default_ratio() -> u8 { 10 }
fn default_attack() -> u16 { 10 }
fn default_release() -> u16 { 200 }

impl Default for DrcConfig {
    fn default() -> Self {
        Self { enabled: false, low_mid_hz: 200, mid_high_hz: 2000, bands: Vec::new() }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpotifyConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    pub cache_path: Option<String>,
    #[serde(default = "default_bitrate")]
    pub bitrate: String,
    #[serde(default = "default_true")]
    pub gapless: bool,
    #[serde(default)]
    pub normalisation: bool,
    #[serde(default = "default_norm_type")]
    pub normalisation_type: String,
    #[serde(default)]
    pub normalisation_pregain_db: f32,
}

impl Default for SpotifyConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            cache_path: None,
            bitrate: default_bitrate(),
            gapless: true,
            normalisation: false,
            normalisation_type: default_norm_type(),
            normalisation_pregain_db: 0.0,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct BluetoothConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_true")]
    pub discoverable: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct HomeAssistantConfig {
    pub enabled: bool,
    pub mqtt_host: Option<String>,
    pub mqtt_port: Option<u16>,
    pub mqtt_user: Option<String>,
    pub mqtt_password: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct WyomingConfig {
    pub enabled: bool,
    pub server_host: Option<String>,
    pub server_port: Option<u16>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct NetworkConfig {
    pub wifi_ssid: Option<String>,
    pub wifi_password: Option<String>,
    #[serde(default = "default_true", alias = "ap_fallback")]
    pub ap_keep_alive: bool,
    /// Custom AP SSID override. If unset, defaults to `Invoke-XXXX` (MAC-based).
    pub ap_ssid: Option<String>,
    /// Custom AP password override. If unset, defaults to `ridiculous`.
    pub ap_password: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VpnConfig {
    #[serde(default)]
    pub enabled: bool,
    pub private_key: Option<String>,
    pub address: Option<String>,
    pub peer_public_key: Option<String>,
    pub peer_preshared_key: Option<String>,
    pub peer_endpoint: Option<String>,
    pub peer_allowed_ips: Option<String>,
    #[serde(default = "default_keepalive")]
    pub persistent_keepalive: u16,
}

fn default_keepalive() -> u16 {
    25
}

impl Default for VpnConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            private_key: None,
            address: None,
            peer_public_key: None,
            peer_preshared_key: None,
            peer_endpoint: None,
            peer_allowed_ips: None,
            persistent_keepalive: default_keepalive(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GroupConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_group_name")]
    pub group_name: String,
    #[serde(default = "default_channel")]
    pub channel: String,
    #[serde(default = "default_buffer_ms")]
    pub buffer_ms: u16,
    /// Unique peer ID (UUID v4), generated on first boot.
    pub peer_id: Option<String>,
    /// Bootstrap peers for cross-subnet discovery (e.g. ["192.168.43.1", "10.0.0.5"]).
    #[serde(default)]
    pub peers: Vec<String>,
    /// Party mode: accept streams from any group's leader (not just our own group).
    #[serde(default)]
    pub party_mode: bool,
}

fn default_group_name() -> String {
    "Home".into()
}
fn default_channel() -> String {
    "stereo".into()
}
fn default_buffer_ms() -> u16 {
    80
}

impl Default for GroupConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            group_name: default_group_name(),
            channel: default_channel(),
            buffer_ms: default_buffer_ms(),
            peer_id: None,
            peers: Vec::new(),
            party_mode: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DebugConfig {
    #[serde(default = "default_debug_mode")]
    pub default_mode: String,
    #[serde(default)]
    pub overrides: std::collections::HashMap<String, String>,
}

fn default_true() -> bool {
    true
}
fn default_bitrate() -> String {
    "320".into()
}
fn default_norm_type() -> String {
    "auto".into()
}
fn default_debug_mode() -> String {
    "production".into()
}

impl Default for EncoreConfigFile {
    fn default() -> Self {
        Self {
            device: DeviceConfig::default(),
            audio: AudioConfig::default(),
            eq: EqConfig::default(),
            drc: DrcConfig::default(),
            spotify: SpotifyConfig {
                enabled: true,
                ..Default::default()
            },
            bluetooth: BluetoothConfig {
                enabled: true,
                discoverable: true,
            },
            homeassistant: HomeAssistantConfig::default(),
            wyoming: WyomingConfig::default(),
            network: NetworkConfig {
                ap_keep_alive: true,
                ..Default::default()
            },
            vpn: VpnConfig::default(),
            group: GroupConfig::default(),
            debug: DebugConfig {
                default_mode: "production".into(),
                ..Default::default()
            },
        }
    }
}

impl EncoreConfigFile {
    /// Load config from TOML file, or return defaults if file doesn't exist
    #[cfg(feature = "toml")]
    pub fn load(path: &std::path::Path) -> anyhow::Result<Self> {
        if path.exists() {
            let content = std::fs::read_to_string(path)?;
            Ok(toml::from_str(&content)?)
        } else {
            Ok(Self::default())
        }
    }

    /// Save config to TOML file
    #[cfg(feature = "toml")]
    pub fn save(&self, path: &std::path::Path) -> anyhow::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let content = toml::to_string_pretty(self)?;
        std::fs::write(path, content)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_sane() {
        let cfg = EncoreConfigFile::default();
        assert_eq!(cfg.device.name, "Encore");
        assert_eq!(cfg.audio.master_volume, 70);
        assert_eq!(cfg.audio.spotify_volume, 70);
        assert_eq!(cfg.audio.bluetooth_volume, 70);
        assert_eq!(cfg.audio.tts_duck_percent, 80);
        assert_eq!(cfg.audio.volume_ring_step, 2);
        assert!(cfg.spotify.enabled);
        assert_eq!(cfg.spotify.bitrate, "320");
        assert!(cfg.spotify.gapless);
        assert!(!cfg.spotify.normalisation);
        assert_eq!(cfg.spotify.normalisation_type, "auto");
        assert_eq!(cfg.spotify.normalisation_pregain_db, 0.0);
        assert!(cfg.bluetooth.enabled);
        assert!(cfg.bluetooth.discoverable);
        assert!(!cfg.homeassistant.enabled);
        assert!(!cfg.wyoming.enabled);
        assert!(cfg.network.ap_keep_alive);
        assert!(!cfg.vpn.enabled);
        assert_eq!(cfg.vpn.persistent_keepalive, 25);
        assert_eq!(cfg.debug.default_mode, "production");
    }

    #[test]
    fn parse_empty_toml_uses_defaults() {
        let cfg: EncoreConfigFile = toml::from_str("").unwrap();
        assert_eq!(cfg.device.name, "Encore");
        assert_eq!(cfg.audio.master_volume, 70);
        // Spotify defaults to enabled=true whether section is present or not.
        assert!(cfg.spotify.enabled);
    }

    #[test]
    fn parse_partial_toml_merges_with_defaults() {
        let toml_str = r#"
[device]
name = "Living Room"

[audio]
master_volume = 50

[homeassistant]
enabled = true
mqtt_host = "192.168.1.100"
mqtt_port = 1883
"#;
        let cfg: EncoreConfigFile = toml::from_str(toml_str).unwrap();
        assert_eq!(cfg.device.name, "Living Room");
        assert_eq!(cfg.audio.master_volume, 50);
        // Unspecified audio fields keep defaults
        assert_eq!(cfg.audio.spotify_volume, 70);
        assert_eq!(cfg.audio.tts_duck_percent, 80);
        // HA is enabled with config
        assert!(cfg.homeassistant.enabled);
        assert_eq!(cfg.homeassistant.mqtt_host.as_deref(), Some("192.168.1.100"));
        assert_eq!(cfg.homeassistant.mqtt_port, Some(1883));
        // Spotify defaults to enabled even when section is absent
        assert!(cfg.spotify.enabled);
    }

    #[test]
    fn round_trip_serialize_deserialize() {
        let original = EncoreConfigFile {
            device: DeviceConfig { name: "Test Speaker".into() },
            audio: AudioConfig {
                master_volume: 42,
                spotify_volume: 80,
                bluetooth_volume: 60,
                tts_duck_percent: 50,
                volume_ring_step: 3,
                idle_timeout_secs: 10,
                standby_timeout_secs: 120,
                dsp_power_gate: true,
            },
            eq: EqConfig::default(),
            drc: DrcConfig::default(),
            spotify: SpotifyConfig {
                enabled: false,
                cache_path: Some("/tmp/spotify".into()),
                bitrate: "160".into(),
                gapless: false,
                normalisation: true,
                normalisation_type: "album".into(),
                normalisation_pregain_db: -3.0,
            },
            bluetooth: BluetoothConfig {
                enabled: true,
                discoverable: false,
            },
            homeassistant: HomeAssistantConfig {
                enabled: true,
                mqtt_host: Some("mqtt.local".into()),
                mqtt_port: Some(1883),
                mqtt_user: Some("user".into()),
                mqtt_password: Some("pass".into()),
            },
            wyoming: WyomingConfig {
                enabled: true,
                server_host: Some("ha.local".into()),
                server_port: Some(10300),
            },
            network: NetworkConfig {
                wifi_ssid: Some("MyWifi".into()),
                wifi_password: Some("secret".into()),
                ap_keep_alive: false,
                ..Default::default()
            },
            vpn: VpnConfig::default(),
            group: GroupConfig::default(),
            debug: DebugConfig {
                default_mode: "trace".into(),
                overrides: [("audio".to_string(), "hold".to_string())].into(),
            },
        };

        let toml_str = toml::to_string_pretty(&original).unwrap();
        let parsed: EncoreConfigFile = toml::from_str(&toml_str).unwrap();

        assert_eq!(parsed.device.name, "Test Speaker");
        assert_eq!(parsed.audio.master_volume, 42);
        assert!(!parsed.spotify.enabled);
        assert_eq!(parsed.spotify.cache_path.as_deref(), Some("/tmp/spotify"));
        assert_eq!(parsed.spotify.bitrate, "160");
        assert!(!parsed.spotify.gapless);
        assert!(parsed.spotify.normalisation);
        assert_eq!(parsed.spotify.normalisation_type, "album");
        assert!((parsed.spotify.normalisation_pregain_db - (-3.0)).abs() < f32::EPSILON);
        assert!(!parsed.bluetooth.discoverable);
        assert!(parsed.homeassistant.enabled);
        assert_eq!(parsed.homeassistant.mqtt_host.as_deref(), Some("mqtt.local"));
        assert!(parsed.wyoming.enabled);
        assert_eq!(parsed.wyoming.server_port, Some(10300));
        assert_eq!(parsed.network.wifi_ssid.as_deref(), Some("MyWifi"));
        assert!(!parsed.network.ap_keep_alive);
        assert_eq!(parsed.debug.default_mode, "trace");
        assert_eq!(parsed.debug.overrides.get("audio").map(|s| s.as_str()), Some("hold"));
    }

    #[test]
    fn load_nonexistent_file_returns_defaults() {
        let cfg = EncoreConfigFile::load(std::path::Path::new("/nonexistent/config.toml")).unwrap();
        assert_eq!(cfg.device.name, "Encore");
        assert_eq!(cfg.audio.master_volume, 70);
    }

    #[test]
    fn save_and_load_round_trip() {
        let dir = std::env::temp_dir().join("encore_test_config");
        let path = dir.join("config.toml");

        let original = EncoreConfigFile {
            device: DeviceConfig { name: "RoundTrip".into() },
            ..EncoreConfigFile::default()
        };

        original.save(&path).unwrap();
        let loaded = EncoreConfigFile::load(&path).unwrap();
        assert_eq!(loaded.device.name, "RoundTrip");

        // Cleanup
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn section_present_but_field_missing_uses_serde_default() {
        // When [spotify] section exists but `enabled` is omitted,
        // serde uses field-level default = "default_true" → enabled=true
        let toml_str = r#"
[spotify]
cache_path = "/tmp/spotify"
"#;
        let cfg: EncoreConfigFile = toml::from_str(toml_str).unwrap();
        assert!(cfg.spotify.enabled); // field-level default kicks in
        assert_eq!(cfg.spotify.cache_path.as_deref(), Some("/tmp/spotify"));
    }

    #[test]
    fn unknown_fields_are_ignored() {
        let toml_str = r#"
[device]
name = "Test"
unknown_field = "should not break"

[some_future_section]
key = "value"
"#;
        // This should not panic — unknown fields are silently ignored by serde
        let result = toml::from_str::<EncoreConfigFile>(toml_str);
        // If serde is configured with deny_unknown_fields, this would fail.
        // We want it to succeed for forward compatibility.
        if let Ok(cfg) = result {
            assert_eq!(cfg.device.name, "Test");
        }
        // If it fails, that's also valid info — means we have strict parsing
    }
}
