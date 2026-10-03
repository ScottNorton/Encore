//! Global application state.
//!
//! Single Rc<RefCell<AppState>> stored in a thread_local. Pages read
//! state via `with()` and mutate via `with_mut()`. The WebSocket
//! dispatcher writes new data here, then calls the active page's
//! `update()` method to patch the DOM.

use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::rc::Rc;

use encore_common::protocol::*;

thread_local! {
    static STATE: RefCell<Option<Rc<RefCell<AppState>>>> = const { RefCell::new(None) };
}

/// All dashboard state in one place.
pub struct AppState {
    // ── Live telemetry (1Hz) ──
    pub system: Option<SystemSnapshot>,
    pub subsystems: HashMap<String, SubsystemSnapshot>,

    // ── Event-driven ──
    pub track: Option<TrackInfo>,
    pub spotify_status: Option<SpotifyStatus>,
    pub network: Option<NetworkState>,
    pub led: Option<LedAnimation>,
    pub config: Option<EncoreConfig>,
    pub bt_devices: Vec<BtEvent>,
    pub bt_status: Option<BtStatus>,
    /// AVRCP now-playing metadata for the connected source (None when idle).
    pub bt_track: Option<BtTrack>,
    /// AVRCP playback position/duration (None when idle/unknown).
    pub bt_playstatus: Option<BtPlayStatus>,
    pub master_volume: u8,

    // ── UI state ──
    pub active_page: String,
    pub config_dirty: bool,
    pub connected: bool,

    // ── Audio levels for VU meter ──
    pub audio_left_rms: f32,
    pub audio_right_rms: f32,
    pub audio_left_peak: f32,
    pub audio_right_peak: f32,

    // ── Mic test levels ──
    pub mic_left_rms: f32,
    pub mic_right_rms: f32,
    pub mic_left_peak: f32,
    pub mic_right_peak: f32,
    pub mic_testing: bool,

    // ── Audio visualization ──
    pub audio_spectrum: Option<[f32; 32]>,
    pub audio_waveform: Option<Vec<f32>>,
    pub viz_mode: String,

    // ── EQ/DRC/DSP state ──
    pub eq_state: Option<EqState>,
    pub drc_state: Option<DrcState>,
    pub dsp_info: Option<DspInfo>,
    /// Last DAC register read result (for explorer)
    pub dac_reg_result: Option<(u8, u8, u8)>,
    /// Last DSP SPI response (for explorer)
    pub dsp_spi_result: Option<Vec<u8>>,
    /// WiFi connection result (shown as banner on network page)
    pub wifi_connect_result: Option<WifiConnectResult>,
    /// Currently selected EQ band index (UI only)
    pub eq_selected_band: Option<usize>,
    /// Setup wizard step (0=welcome, 1=wifi, 2=name, 3=success)
    pub setup_step: u8,
    /// Setup is complete (config exists)
    pub setup_complete: bool,

    // ── History for client-side delta computation ──
    pub cpu_history: VecDeque<u8>,
    pub prev_cores: Vec<CpuCoreSnapshot>,
    pub prev_net: HashMap<String, (u64, u64)>,
    /// Total RX bytes/sec history for network sparkline (last 60 samples).
    pub net_rx_history: VecDeque<u64>,

    // ── Group (multi-speaker sync) ──
    pub group_status: Option<GroupStatus>,

    // ── Audio power state ──
    /// Current audio power state: "active", "idle", or "standby".
    pub audio_power_state: String,

    // ── Safe mode (OTA crash fallback) ──
    /// True when running fallback binary after OTA binary crashed.
    pub safe_mode: bool,
    /// True after the one-time safe mode modal has been dismissed.
    pub safe_mode_modal_shown: bool,
    /// Boot source: "next", "lsync", "rootfs", "dev", or "unknown".
    pub boot_source: String,

    // ── Standalone app (Tauri) ──
    /// Speaker host/IP for standalone mode (e.g. "192.168.43.1").
    pub speaker_host: Option<String>,

    // ── Boot sequence ──
    /// Set to true when ConfigLoaded message arrives via WebSocket.
    pub boot_config_received: bool,
    /// Set to true when first SystemStatus message arrives via WebSocket.
    pub boot_system_received: bool,
}

/// Derive the active audio source for the Tuner indicator and mini-bar.
///
/// Pure function of state — there is NO source-switch protocol message (see the
/// design v2 resolution). Precedence: Spotify if playing, then a connected
/// Bluetooth device (last event wins over `bt_devices`), then Voice if the
/// `wyoming` subsystem is Running. Defaults to "spotify" when idle.
pub fn active_source(s: &AppState) -> &'static str {
    if s.spotify_status
        .as_ref()
        .map(|st| st.is_playing)
        .unwrap_or(false)
    {
        return "spotify";
    }
    // Last-event-wins: a later DeviceConnected for a different addr replaces the
    // current one; a DeviceDisconnected for the connected addr clears it.
    let mut connected: Option<&str> = None;
    for ev in &s.bt_devices {
        match ev {
            BtEvent::DeviceConnected { addr, .. } => connected = Some(addr.as_str()),
            BtEvent::DeviceDisconnected { addr } => {
                if connected == Some(addr.as_str()) {
                    connected = None;
                }
            }
            _ => {}
        }
    }
    // Fall back to the authoritative live status: a dashboard opened (or WS-
    // reconnected) mid-stream gets a BluetoothStatus with `connected` set but no
    // DeviceConnected event, so the event log above is empty even though a device
    // is connected. Mirrors the precedence the Bluetooth settings page uses.
    if connected.is_some()
        || s.bt_status
            .as_ref()
            .is_some_and(|st| st.connected.is_some())
    {
        return "bluetooth";
    }
    if s.subsystems
        .get("wyoming")
        .map(|sub| sub.state == SubsystemState::Running)
        .unwrap_or(false)
    {
        return "voice";
    }
    "spotify"
}

impl Default for AppState {
    fn default() -> Self {
        Self {
            system: None,
            subsystems: HashMap::new(),
            track: None,
            spotify_status: None,
            network: None,
            led: None,
            config: None,
            bt_devices: Vec::new(),
            bt_status: None,
            bt_track: None,
            bt_playstatus: None,
            master_volume: 30,
            active_page: "home".into(),
            config_dirty: false,
            connected: false,
            audio_left_rms: 0.0,
            audio_right_rms: 0.0,
            audio_left_peak: 0.0,
            audio_right_peak: 0.0,
            mic_left_rms: 0.0,
            mic_right_rms: 0.0,
            mic_left_peak: 0.0,
            mic_right_peak: 0.0,
            mic_testing: false,
            audio_spectrum: None,
            audio_waveform: None,
            viz_mode: "spectrum".into(),
            eq_state: None,
            drc_state: None,
            dsp_info: None,
            dac_reg_result: None,
            dsp_spi_result: None,
            wifi_connect_result: None,
            eq_selected_band: None,
            setup_step: 0,
            setup_complete: false,
            cpu_history: VecDeque::with_capacity(60),
            prev_cores: Vec::new(),
            prev_net: HashMap::new(),
            net_rx_history: VecDeque::with_capacity(60),
            group_status: None,
            audio_power_state: "active".into(),
            safe_mode: false,
            safe_mode_modal_shown: false,
            boot_source: String::new(),
            speaker_host: None,
            boot_config_received: false,
            boot_system_received: false,
        }
    }
}

/// Initialize global state. Call once at startup.
pub fn init() -> Rc<RefCell<AppState>> {
    let state = Rc::new(RefCell::new(AppState::default()));
    STATE.with(|s| {
        *s.borrow_mut() = Some(state.clone());
    });
    state
}

/// Read access to global state.
pub fn with<F, R>(f: F) -> R
where
    F: FnOnce(&AppState) -> R,
{
    STATE.with(|s| {
        let borrow = s.borrow();
        let state = borrow.as_ref().expect("state not initialized");
        let inner = state.borrow();
        f(&inner)
    })
}

/// Write access to global state.
pub fn with_mut<F, R>(f: F) -> R
where
    F: FnOnce(&mut AppState) -> R,
{
    STATE.with(|s| {
        let borrow = s.borrow();
        let state = borrow.as_ref().expect("state not initialized");
        let mut inner = state.borrow_mut();
        f(&mut inner)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use encore_common::protocol::{
        BtEvent, DebugMode, SpotifyStatus, SubsystemSnapshot, SubsystemState,
    };

    fn playing(is_playing: bool) -> SpotifyStatus {
        SpotifyStatus {
            is_playing,
            shuffle: false,
            repeat_context: false,
            repeat_track: false,
            volume: 50,
            position_ms: 0,
            duration_ms: 0,
            track: None,
            connected_user: None,
        }
    }

    fn wyoming_running() -> SubsystemSnapshot {
        SubsystemSnapshot {
            name: "wyoming".into(),
            state: SubsystemState::Running,
            debug_mode: DebugMode::Production,
            restart_count: 0,
            msg_count: 0,
            uptime_secs: 1,
        }
    }

    #[test]
    fn spotify_playing_wins() {
        let mut s = AppState {
            spotify_status: Some(playing(true)),
            ..Default::default()
        };
        s.bt_devices.push(BtEvent::DeviceConnected {
            name: "Phone".into(),
            addr: "AA".into(),
        });
        assert_eq!(active_source(&s), "spotify");
    }

    #[test]
    fn bluetooth_when_connected_and_not_playing() {
        let mut s = AppState {
            spotify_status: Some(playing(false)),
            ..Default::default()
        };
        s.bt_devices.push(BtEvent::DeviceConnected {
            name: "Phone".into(),
            addr: "AA".into(),
        });
        assert_eq!(active_source(&s), "bluetooth");
    }

    #[test]
    fn bluetooth_ignored_after_disconnect() {
        let mut s = AppState::default();
        s.bt_devices.push(BtEvent::DeviceConnected {
            name: "Phone".into(),
            addr: "AA".into(),
        });
        s.bt_devices
            .push(BtEvent::DeviceDisconnected { addr: "AA".into() });
        s.subsystems.insert("wyoming".into(), wyoming_running());
        assert_eq!(active_source(&s), "voice");
    }

    #[test]
    fn voice_when_wyoming_running() {
        let mut s = AppState::default();
        s.subsystems.insert("wyoming".into(), wyoming_running());
        assert_eq!(active_source(&s), "voice");
    }

    #[test]
    fn defaults_to_spotify_when_idle() {
        let s = AppState::default();
        assert_eq!(active_source(&s), "spotify");
    }
}
