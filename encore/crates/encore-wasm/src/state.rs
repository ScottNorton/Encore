//! Global application state.
//!
//! Single Rc<RefCell<AppState>> stored in a thread_local. Pages read
//! state via `with()` and mutate via `with_mut()`. The WebSocket
//! dispatcher writes new data here, then calls the active page's
//! `update()` method to patch the DOM.

use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::rc::Rc;

use wasm_bindgen::JsValue;
use encore_common::protocol::*;

thread_local! {
    static STATE: RefCell<Option<Rc<RefCell<AppState>>>> = RefCell::new(None);
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
    pub master_volume: u8,

    // ── UI state ──
    pub active_page: String,
    pub panel_open: Option<String>,
    pub config_dirty: bool,
    pub connected: bool,
    /// Stored `beforeinstallprompt` event for PWA install.
    pub install_prompt: Option<JsValue>,

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
    /// Pre-scanned speakers from mDNS discovery during boot (name, host).
    pub boot_speakers: Vec<(String, String)>,
    /// Set to true when ConfigLoaded message arrives via WebSocket.
    pub boot_config_received: bool,
    /// Set to true when first SystemStatus message arrives via WebSocket.
    pub boot_system_received: bool,
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
            master_volume: 30,
            active_page: "dashboard".into(),
            panel_open: None,
            config_dirty: false,
            connected: false,
            install_prompt: None,
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
            boot_speakers: Vec::new(),
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
        let result = f(&inner);
        result
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
        let result = f(&mut inner);
        result
    })
}
