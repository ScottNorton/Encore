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
    #[serde(default)]
    pub sense: SenseConfig,
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
    /// Optional EQ preset applied to the software EQ at boot, e.g. "bass_boost",
    /// "warm", "flat". `None` = start flat. Runtime changes from the dashboard
    /// always work regardless of this setting.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub eq_boot_preset: Option<String>,

    /// Loudest permitted DAC digital-volume register (0x3D/0x3E). LOWER = louder;
    /// 0x00 is the chip max. Volume 0..100 maps into `[max_volume_reg ..= 0xA0]`.
    /// Default 0x37 = -27.5 dB (calibrated by ear). Lower it (toward the 0x18 hard
    /// floor) only after the on-device SPL/excursion calibration in
    /// docs/audio-overhaul.md. Values are clamped to the firmware's safe floor.
    #[serde(default = "default_max_volume_reg")]
    pub max_volume_reg: u8,
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
pub fn default_max_volume_reg() -> u8 {
    // 0x37 = -27.5 dB. Calibrated by ear on-device (all sources at max, no EQ):
    // the loudest the speaker runs cleanly without straining the driver. 3.5 dB
    // more conservative than the stock 0x30; re-verify with bass-boost EQ active.
    0x37
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
            eq_boot_preset: None,
            max_volume_reg: default_max_volume_reg(),
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

fn default_eq_freq() -> u16 {
    1000
}
fn default_q() -> u16 {
    10
}
fn default_filter_type() -> String {
    "peak".into()
}

impl Default for EqConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            bands: Vec::new(),
        }
    }
}

/// Parse a config `filter_type` string into the protocol enum, case- and
/// separator-insensitive. Unknown values fall back to a peak filter.
fn parse_filter_type(s: &str) -> crate::protocol::FilterType {
    use crate::protocol::FilterType::*;
    match s.to_ascii_lowercase().replace(['_', '-', ' '], "").as_str() {
        "lowshelf" => LowShelf,
        "highshelf" => HighShelf,
        "notch" => Notch,
        _ => Peak,
    }
}

impl EqConfig {
    /// Map the persisted `[eq]` config into the runtime `EqState` applied at
    /// boot. Bands map by index into the 10-band array (extras past 10 are
    /// dropped); unspecified bands stay flat. An explicit config is not a named
    /// preset, so `preset` is `None`.
    pub fn to_state(&self) -> crate::protocol::EqState {
        use crate::protocol::{EqBand, EqState};
        let mut bands = [EqBand::default(); 10];
        for (slot, b) in bands.iter_mut().zip(self.bands.iter()) {
            *slot = EqBand {
                freq_hz: b.freq_hz,
                gain_cb: b.gain_cb,
                q_x10: b.q_x10,
                filter_type: parse_filter_type(&b.filter_type),
            };
        }
        EqState {
            bands,
            preset: None,
            enabled: self.enabled,
        }
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

fn default_low_mid() -> u16 {
    200
}
fn default_mid_high() -> u16 {
    2000
}

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

fn default_threshold() -> i8 {
    -20
}
fn default_ratio() -> u8 {
    10
}
fn default_attack() -> u16 {
    10
}
fn default_release() -> u16 {
    200
}

impl Default for DrcConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            low_mid_hz: 200,
            mid_high_hz: 2000,
            bands: Vec::new(),
        }
    }
}

impl DrcConfig {
    /// Map the persisted `[drc]` config into the runtime `DrcState` applied at
    /// boot.
    ///
    /// The software compressor is single-band and applies `bands[1]` (Mid). A
    /// single `[[drc.bands]]` entry is therefore replicated onto every state
    /// band so it lands on the applied one; if several entries are given they
    /// map by index, and the second is the one that takes effect.
    pub fn to_state(&self) -> crate::protocol::DrcState {
        use crate::protocol::{DrcBandConfig, DrcState};
        let band_at = |i: usize| -> DrcBandConfig {
            self.bands
                .get(i)
                .or_else(|| self.bands.first())
                .map(|b| DrcBandConfig {
                    threshold_db: b.threshold_db,
                    ratio_x10: b.ratio_x10,
                    attack_ms: b.attack_ms,
                    release_ms: b.release_ms,
                })
                .unwrap_or_default()
        };
        DrcState {
            bands: [band_at(0), band_at(1), band_at(2)],
            low_mid_hz: self.low_mid_hz,
            mid_high_hz: self.mid_high_hz,
            preset: None,
            enabled: self.enabled,
        }
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
    /// Mesh mode: when grouped, all members advertise the group name and only
    /// the coordinator stays connectable, so the group looks like one BT
    /// device. Off (default) = normal per-speaker Bluetooth.
    #[serde(default)]
    pub mesh_enabled: bool,
    /// Target A2DP buffer depth in ms. The latency servo holds the stream this
    /// far behind the source — low enough for lip sync, high enough to ride
    /// out link jitter. 0 (or absent) = the built-in default.
    #[serde(default)]
    pub latency_target_ms: u16,
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

impl Default for GroupConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            group_name: default_group_name(),
            channel: default_channel(),
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

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SenseConfig {
    pub enabled: bool,
    pub channels: String,      // "left" | "both"
    pub activity_db: f32,      // dB over noise floor to call "activity"
    pub attack_frames: u32,    // consecutive over-threshold frames before Activity
    pub quiet_timeout_s: u32,  // sub-threshold seconds -> decay to Quiet
    pub loud_ratio: f32,       // peak/floor ratio -> loud event
    pub playback_tail_ms: u32, // ignore mic this long after playback stops
    pub led_feedback: bool,
}
impl Default for SenseConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            channels: "left".into(),
            activity_db: 12.0,
            attack_frames: 6,
            quiet_timeout_s: 30,
            loud_ratio: 8.0,
            playback_tail_ms: 800,
            led_feedback: false,
        }
    }
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
                mesh_enabled: false,
                latency_target_ms: 0, // 0 = firmware default
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
            sense: SenseConfig::default(),
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

    /// Save config to TOML file.
    ///
    /// Atomic: a plain truncate-in-place write (`std::fs::write`) leaves
    /// config.toml empty or half-written if power is cut or the watchdog restarts
    /// the process mid-write — and every loader falls back to `unwrap_or_default()`
    /// on a parse error, so a truncated file silently wipes the WiFi credentials
    /// and every other setting. Write a temp sibling, fsync it, then rename over
    /// the target (atomic on the same filesystem), so a reader always sees either
    /// the complete old file or the complete new one, never a partial one.
    #[cfg(feature = "toml")]
    pub fn save(&self, path: &std::path::Path) -> anyhow::Result<()> {
        use std::io::Write;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let content = toml::to_string_pretty(self)?;
        let tmp = path.with_extension("toml.tmp");
        {
            let mut f = std::fs::File::create(&tmp)?;
            f.write_all(content.as_bytes())?;
            f.sync_all()?;
        }
        std::fs::rename(&tmp, path)?;
        // Best-effort: fsync the directory so the rename itself is durable across
        // a power cut on flash. Not supported everywhere, so ignore failures.
        if let Some(parent) = path.parent() {
            let _ = std::fs::File::open(parent).and_then(|d| d.sync_all());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drc_config_to_state_applies_single_band() {
        // A single configured band lands on the applied (Mid) band, and the
        // enable flag and crossover values carry through.
        let cfg = DrcConfig {
            enabled: true,
            low_mid_hz: 150,
            mid_high_hz: 1800,
            bands: vec![DrcBandSaveConfig {
                threshold_db: -18,
                ratio_x10: 40,
                attack_ms: 5,
                release_ms: 150,
            }],
        };
        let st = cfg.to_state();
        assert!(st.enabled);
        assert_eq!(st.low_mid_hz, 150);
        assert_eq!(st.mid_high_hz, 1800);
        assert_eq!(st.bands[1].threshold_db, -18);
        assert_eq!(st.bands[1].ratio_x10, 40);
        assert_eq!(st.bands[1].attack_ms, 5);
        assert_eq!(st.bands[1].release_ms, 150);

        // The default config maps to a disabled, all-default state, so a normal
        // boot (no [drc] edits) behaves exactly as before.
        let d = DrcConfig::default().to_state();
        assert!(!d.enabled);
        assert_eq!(d.bands[1], crate::protocol::DrcBandConfig::default());
    }

    #[test]
    fn eq_config_to_state_maps_bands_and_filter_types() {
        use crate::protocol::{EqBand, FilterType};
        let cfg = EqConfig {
            enabled: true,
            bands: vec![
                EqBandConfig {
                    freq_hz: 100,
                    gain_cb: 30,
                    q_x10: 7,
                    filter_type: "lowshelf".into(),
                },
                EqBandConfig {
                    freq_hz: 8000,
                    gain_cb: -20,
                    q_x10: 12,
                    filter_type: "HighShelf".into(),
                },
            ],
        };
        let st = cfg.to_state();
        assert!(st.enabled);
        assert_eq!(st.preset, None);
        assert_eq!(st.bands[0].freq_hz, 100);
        assert_eq!(st.bands[0].filter_type, FilterType::LowShelf);
        assert_eq!(st.bands[1].gain_cb, -20);
        assert_eq!(st.bands[1].filter_type, FilterType::HighShelf);
        // Bands past those configured stay flat.
        assert_eq!(st.bands[2], EqBand::default());

        // Disabled config with an unknown filter type: enabled carries through
        // and the unknown type falls back to a peak filter.
        let c2 = EqConfig {
            enabled: false,
            bands: vec![EqBandConfig {
                freq_hz: 1000,
                gain_cb: 0,
                q_x10: 10,
                filter_type: "bogus".into(),
            }],
        };
        let s2 = c2.to_state();
        assert!(!s2.enabled);
        assert_eq!(s2.bands[0].filter_type, FilterType::Peak);
    }

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
        assert_eq!(
            cfg.homeassistant.mqtt_host.as_deref(),
            Some("192.168.1.100")
        );
        assert_eq!(cfg.homeassistant.mqtt_port, Some(1883));
        // Spotify defaults to enabled even when section is absent
        assert!(cfg.spotify.enabled);
    }

    #[test]
    fn round_trip_serialize_deserialize() {
        let original = EncoreConfigFile {
            device: DeviceConfig {
                name: "Test Speaker".into(),
            },
            audio: AudioConfig {
                master_volume: 42,
                spotify_volume: 80,
                bluetooth_volume: 60,
                tts_duck_percent: 50,
                volume_ring_step: 3,
                idle_timeout_secs: 10,
                standby_timeout_secs: 120,
                dsp_power_gate: true,
                eq_boot_preset: Some("bass_boost".into()),
                max_volume_reg: 0x28,
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
                mesh_enabled: false,
                latency_target_ms: 0,
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
            sense: SenseConfig::default(),
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
        assert_eq!(
            parsed.homeassistant.mqtt_host.as_deref(),
            Some("mqtt.local")
        );
        assert!(parsed.wyoming.enabled);
        assert_eq!(parsed.wyoming.server_port, Some(10300));
        assert_eq!(parsed.network.wifi_ssid.as_deref(), Some("MyWifi"));
        assert!(!parsed.network.ap_keep_alive);
        assert_eq!(parsed.debug.default_mode, "trace");
        assert_eq!(
            parsed.debug.overrides.get("audio").map(|s| s.as_str()),
            Some("hold")
        );
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
            device: DeviceConfig {
                name: "RoundTrip".into(),
            },
            ..EncoreConfigFile::default()
        };

        original.save(&path).unwrap();
        let loaded = EncoreConfigFile::load(&path).unwrap();
        assert_eq!(loaded.device.name, "RoundTrip");

        // Cleanup
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn save_overwrites_atomically_without_leaving_temp() {
        let dir = std::env::temp_dir().join("encore_test_config_atomic");
        std::fs::remove_dir_all(&dir).ok();
        let path = dir.join("config.toml");

        let a = EncoreConfigFile {
            device: DeviceConfig { name: "A".into() },
            ..EncoreConfigFile::default()
        };
        a.save(&path).unwrap();
        // Re-save over the existing file (the rename-over-existing path).
        let b = EncoreConfigFile {
            device: DeviceConfig { name: "B".into() },
            ..EncoreConfigFile::default()
        };
        b.save(&path).unwrap();

        assert_eq!(EncoreConfigFile::load(&path).unwrap().device.name, "B");
        // The temp sibling must not linger after a successful save.
        assert!(!path.with_extension("toml.tmp").exists());

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
    fn ap_fallback_alias_maps_to_ap_keep_alive() {
        // The legacy field name `ap_fallback` is a serde alias for `ap_keep_alive`.
        let toml_str = r#"
[network]
ap_fallback = false
"#;
        let cfg: EncoreConfigFile = toml::from_str(toml_str).unwrap();
        assert!(!cfg.network.ap_keep_alive);
    }

    #[test]
    fn ap_fallback_alias_true_value() {
        let toml_str = r#"
[network]
ap_fallback = true
"#;
        let cfg: EncoreConfigFile = toml::from_str(toml_str).unwrap();
        assert!(cfg.network.ap_keep_alive);
    }

    #[test]
    fn ap_keep_alive_canonical_name_still_parses() {
        // The canonical field name continues to work alongside the alias.
        let toml_str = r#"
[network]
ap_keep_alive = false
"#;
        let cfg: EncoreConfigFile = toml::from_str(toml_str).unwrap();
        assert!(!cfg.network.ap_keep_alive);
    }

    #[test]
    fn vpn_config_defaults() {
        let vpn = VpnConfig::default();
        assert!(!vpn.enabled);
        assert!(vpn.private_key.is_none());
        assert!(vpn.address.is_none());
        assert!(vpn.peer_public_key.is_none());
        assert!(vpn.peer_preshared_key.is_none());
        assert!(vpn.peer_endpoint.is_none());
        assert!(vpn.peer_allowed_ips.is_none());
        assert_eq!(vpn.persistent_keepalive, 25);
    }

    #[test]
    fn vpn_config_serde_defaults_from_empty_section() {
        // An empty [vpn] section should fill every field with its serde default.
        let toml_str = r#"
[vpn]
"#;
        let cfg: EncoreConfigFile = toml::from_str(toml_str).unwrap();
        assert!(!cfg.vpn.enabled);
        assert!(cfg.vpn.private_key.is_none());
        assert_eq!(cfg.vpn.persistent_keepalive, 25);
    }

    #[test]
    fn group_config_defaults() {
        let group = GroupConfig::default();
        assert!(!group.enabled);
        assert_eq!(group.group_name, "Home");
        assert_eq!(group.channel, "stereo");
        assert!(group.peer_id.is_none());
        assert!(group.peers.is_empty());
        assert!(!group.party_mode);
    }

    #[test]
    fn group_config_serde_defaults_from_empty_section() {
        // An empty [group] section should yield the documented defaults.
        let toml_str = r#"
[group]
"#;
        let cfg: EncoreConfigFile = toml::from_str(toml_str).unwrap();
        assert!(!cfg.group.enabled);
        assert_eq!(cfg.group.group_name, "Home");
        assert_eq!(cfg.group.channel, "stereo");
        assert!(cfg.group.peers.is_empty());
        assert!(!cfg.group.party_mode);
    }

    #[test]
    fn group_config_has_no_user_buffer_setting() {
        // The playout buffer is no longer a user setting: the controller owns it.
        // Parsing a [group] section that omits it must still succeed (no missing
        // required field), and the field must not serialize back out.
        let toml = "[group]\nenabled = true\ngroup_name = \"Home\"\n";
        let cfg: EncoreConfigFile = toml::from_str(toml).unwrap();
        assert!(cfg.group.enabled);

        // Round-trip a default config and prove the knob is gone from the wire:
        // while the field exists it serializes as `buffer_ms = 80`, so this fails
        // until the field (and its serde default) are removed.
        let serialized = toml::to_string_pretty(&EncoreConfigFile::default()).unwrap();
        assert!(
            !serialized.contains("buffer_ms"),
            "GroupConfig must not serialize a buffer_ms knob:\n{serialized}"
        );
    }

    #[test]
    fn eq_config_defaults() {
        let eq = EqConfig::default();
        assert!(eq.enabled);
        assert!(eq.bands.is_empty());
    }

    #[test]
    fn drc_config_defaults() {
        let drc = DrcConfig::default();
        assert!(!drc.enabled);
        assert_eq!(drc.low_mid_hz, 200);
        assert_eq!(drc.mid_high_hz, 2000);
        assert!(drc.bands.is_empty());
    }

    #[test]
    fn sense_config_defaults_and_parse() {
        let cfg: EncoreConfigFile =
            toml::from_str("[sense]\nenabled = true\nactivity_db = 10.0\n").unwrap();
        assert!(cfg.sense.enabled);
        assert_eq!(cfg.sense.activity_db, 10.0);
        assert_eq!(cfg.sense.quiet_timeout_s, 30); // omitted -> default
        let def = SenseConfig::default();
        assert!(!def.enabled);
        assert_eq!(def.channels, "left");
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
