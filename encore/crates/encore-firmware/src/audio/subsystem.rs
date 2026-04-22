//! Audio subsystem — manages playback pipeline, mixer, and volume.
//!
//! Integrates AlsaPcm, MixerSlots, IO Expander (mute/unmute), DAC, and DSP
//! into a single Subsystem. The mixer runs on a dedicated OS thread for
//! real-time audio timing (not on tokio's thread pool).

use crate::audio::alsa_ctl::AlsaCtl;
use crate::audio::capture::{CaptureChannel, CaptureConsumer, CaptureManager};
use crate::audio::mixer::MixerSlot;
use crate::audio::pcm::{AlsaPcm, PcmConfig};
use crate::mcu::dac::Dac;
use crate::mcu::dsp::Dsp;
use crate::mcu::io_expander::IoExpander;
use crate::subsystem::{Subsystem, SubsystemContext};
use anyhow::{Context, Result};
use encore_common::protocol::SubsystemState;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU8, AtomicU32, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

/// Audio hardware power state.
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioPowerState {
    /// Full power: DAC active, AMP unmuted, PCM writing audio.
    Active = 0,
    /// AMP muted, still writing silence to PCM (fast resume ~150ms).
    Idle = 1,
    /// DAC in standby, PCM closed, mixer parked (resume ~200ms).
    Standby = 2,
}

impl AudioPowerState {
    fn from_u8(v: u8) -> Self {
        match v {
            0 => Self::Active,
            1 => Self::Idle,
            _ => Self::Standby,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Idle => "idle",
            Self::Standby => "standby",
        }
    }
}

/// Cross-thread bridge between the tokio command loop and the OS mixer thread.
/// Uses atomics for lock-free state sharing and a condvar for parking the mixer.
pub struct MixerBridge {
    /// Current power state (read by mixer, written by command loop).
    power_state: AtomicU8,
    /// Mixer sets this when it detects audio data and state != Active.
    wake_request: AtomicBool,
    /// When true, mixer should park (close PCM, wait on condvar).
    parked: AtomicBool,
    /// Consecutive silence periods counted by the mixer thread.
    silence_count: AtomicU32,
    /// Condvar + mutex for parking/unparking the mixer thread.
    park_mutex: Mutex<bool>,
    park_condvar: Condvar,
}

impl MixerBridge {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            power_state: AtomicU8::new(AudioPowerState::Active as u8),
            wake_request: AtomicBool::new(false),
            parked: AtomicBool::new(false),
            silence_count: AtomicU32::new(0),
            park_mutex: Mutex::new(false),
            park_condvar: Condvar::new(),
        })
    }

    /// Get current power state.
    fn state(&self) -> AudioPowerState {
        AudioPowerState::from_u8(self.power_state.load(Ordering::Acquire))
    }

    /// Set power state (called from tokio command loop).
    fn set_state(&self, state: AudioPowerState) {
        self.power_state.store(state as u8, Ordering::Release);
    }

    /// Check and clear wake request (called from tokio command loop).
    fn take_wake_request(&self) -> bool {
        self.wake_request.swap(false, Ordering::AcqRel)
    }

    /// Get silence period count.
    fn silence_periods(&self) -> u32 {
        self.silence_count.load(Ordering::Acquire)
    }

    /// Check if mixer is parked.
    fn is_parked(&self) -> bool {
        self.parked.load(Ordering::Acquire)
    }

    /// Request mixer to park (called from tokio command loop).
    fn request_park(&self) {
        self.parked.store(true, Ordering::Release);
    }

    /// Unpark the mixer thread (called from tokio command loop).
    fn unpark(&self) {
        self.parked.store(false, Ordering::Release);
        // Wake the mixer thread from condvar wait
        let mut guard = self.park_mutex.lock().unwrap();
        *guard = true;
        self.park_condvar.notify_one();
        drop(guard);
    }
}

/// Messages from mixer thread to tokio bridge task.
#[derive(Debug)]
enum MixerMsg {
    Levels(f32, f32, f32, f32),
    Spectrum { bins: [f32; 32], waveform: [f32; 256] },
}

/// Shared DRC parameters readable by the mixer thread (lock-free atomics).
/// Values stored as integers for atomic access; converted to f32 in the mixer.
pub struct SharedDrcParams {
    /// DRC enabled
    pub enabled: AtomicBool,
    /// Threshold in dB (negative, e.g. -20)
    pub threshold_db: AtomicI32,
    /// Ratio × 10 (e.g. 20 = 2.0:1)
    pub ratio_x10: AtomicI32,
    /// Attack time in ms
    pub attack_ms: AtomicI32,
    /// Release time in ms
    pub release_ms: AtomicI32,
}

impl SharedDrcParams {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            enabled: AtomicBool::new(false),
            threshold_db: AtomicI32::new(-20),
            ratio_x10: AtomicI32::new(10), // 1.0:1 = passthrough
            attack_ms: AtomicI32::new(10),
            release_ms: AtomicI32::new(200),
        })
    }

    /// Update from DRC state (uses "mid" band as the single-band compressor source).
    fn update_from_state(&self, state: &encore_common::protocol::DrcState) {
        self.enabled.store(state.enabled, Ordering::Release);
        // Use the mid band for the single-band software compressor
        let mid = &state.bands[1];
        self.threshold_db.store(mid.threshold_db as i32, Ordering::Release);
        self.ratio_x10.store(mid.ratio_x10 as i32, Ordering::Release);
        self.attack_ms.store(mid.attack_ms as i32, Ordering::Release);
        self.release_ms.store(mid.release_ms as i32, Ordering::Release);
    }
}

/// Audio commands sent to the subsystem.
#[derive(Debug)]
pub enum AudioCmd {
    SetVolume(u8),
    Mute,
    Unmute,
    // EQ
    SetEqBand { band: u8, config: encore_common::protocol::EqBand },
    SetEqPreset(encore_common::protocol::EqPreset),
    SetEqEnabled(bool),
    // DRC
    SetDrc { band: encore_common::protocol::DrcBand, config: encore_common::protocol::DrcBandConfig },
    SetDrcCrossover { low_mid_hz: u16, mid_high_hz: u16 },
    SetDrcEnabled(bool),
    SetDrcPreset(encore_common::protocol::DrcPreset),
    // DSP
    SetDspVolume(u8),
    SetMicMute(bool),
    // Explorer (oneshot reply channels for async responses)
    DacRegRead { page: u8, reg: u8, reply: tokio::sync::oneshot::Sender<anyhow::Result<u8>> },
    DacRegWrite { page: u8, reg: u8, value: u8 },
    DspSpiSend { msg_type: u16, data: Vec<u8>, reply: tokio::sync::oneshot::Sender<anyhow::Result<Vec<u8>>> },
    DspMemoryDump { start_page: u16, num_pages: u16, reply: tokio::sync::oneshot::Sender<anyhow::Result<Vec<u8>>> },
    DspDumpToFile { path: String, reply: tokio::sync::oneshot::Sender<anyhow::Result<usize>> },
    DspPollEvents { reply: tokio::sync::oneshot::Sender<anyhow::Result<Vec<String>>> },
    // Mic test
    StartMicTest,
    StopMicTest,
}

/// Handles returned when registering an audio source.
pub struct AudioSource {
    pub slot: Arc<MixerSlot>,
}

/// Audio subsystem managing the full playback pipeline.
pub struct AudioSubsystem {
    cmd_rx: mpsc::Receiver<AudioCmd>,
    sources: Vec<Arc<MixerSlot>>,
    mixer_running: Arc<AtomicBool>,
    capture_mgr: Option<CaptureManager>,
    eq_state: encore_common::protocol::EqState,
    drc_state: encore_common::protocol::DrcState,
    mic_muted: bool,
    dsp_volume: u8,
    dsp_version: String,
    ws_tx: Option<tokio::sync::broadcast::Sender<String>>,
    shared_drc: Arc<SharedDrcParams>,
    /// Network tap: post-DRC audio is pushed here when active (for group leader streaming).
    network_tap: Option<Arc<MixerSlot>>,
    /// Controls whether the network tap is active.
    tap_active: Option<Arc<AtomicBool>>,
    /// Cross-thread bridge for power state management.
    bridge: Arc<MixerBridge>,
    /// Seconds of silence before transitioning Active → Idle.
    idle_timeout_secs: u32,
    /// Seconds in Idle before transitioning to Standby.
    standby_timeout_secs: u32,
    /// Whether to power-gate the DSP in Standby (adds ~3.5s resume).
    dsp_power_gate: bool,
    /// Shared flag: mic test active (mic monitor task reads this).
    mic_test_active: Arc<AtomicBool>,
}

impl AudioSubsystem {
    pub fn new(cmd_rx: mpsc::Receiver<AudioCmd>) -> Self {
        Self {
            cmd_rx,
            sources: Vec::new(),
            mixer_running: Arc::new(AtomicBool::new(false)),
            capture_mgr: Some(CaptureManager::new()),
            eq_state: encore_common::protocol::EqState::default(),
            drc_state: encore_common::protocol::DrcState::default(),
            mic_muted: false,
            dsp_volume: 50,
            dsp_version: String::new(),
            ws_tx: None,
            shared_drc: SharedDrcParams::new(),
            network_tap: None,
            tap_active: None,
            bridge: MixerBridge::new(),
            idle_timeout_secs: 5,
            standby_timeout_secs: 60,
            dsp_power_gate: false,
            mic_test_active: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Apply power management config from EncoreConfigFile.
    pub fn apply_power_config(&mut self, audio: &encore_common::config::AudioConfig) {
        self.idle_timeout_secs = audio.idle_timeout_secs;
        self.standby_timeout_secs = audio.standby_timeout_secs;
        self.dsp_power_gate = audio.dsp_power_gate;
    }

    /// Set the network tap for group audio streaming.
    pub fn set_network_tap(&mut self, tap: Arc<MixerSlot>, active: Arc<AtomicBool>) {
        self.network_tap = Some(tap);
        self.tap_active = Some(active);
    }

    /// Set the WebSocket broadcast channel for state updates to dashboard.
    pub fn set_ws_tx(&mut self, tx: tokio::sync::broadcast::Sender<String>) {
        self.ws_tx = Some(tx);
    }

    /// Register an audio source and get a MixerSlot for pushing samples.
    pub fn add_source(&mut self) -> AudioSource {
        let slot = MixerSlot::new();
        self.sources.push(slot.clone());
        AudioSource { slot }
    }

    /// Get a clone of the mic test active flag (shared with mic monitor task).
    pub fn mic_test_flag(&self) -> Arc<AtomicBool> {
        self.mic_test_active.clone()
    }

    /// Register a capture consumer for a specific mic channel.
    /// Returns a `CaptureConsumer` with an mpsc receiver for 16kHz mono S16 chunks.
    /// Must be called before the audio subsystem is started.
    pub fn add_capture_consumer(&mut self, channel: CaptureChannel) -> CaptureConsumer {
        self.capture_mgr
            .as_mut()
            .expect("add_capture_consumer called after audio subsystem started")
            .add_consumer(channel)
    }

    /// Broadcast a ServerMsg to all connected WebSocket clients.
    fn broadcast(&self, msg: &encore_common::protocol::ServerMsg) {
        if let Some(ref tx) = self.ws_tx {
            if let Ok(json) = serde_json::to_string(msg) {
                let _ = tx.send(json);
            }
        }
    }

    /// Broadcast current EQ state to dashboard.
    fn broadcast_eq(&self) {
        self.broadcast(&encore_common::protocol::ServerMsg::EqState(self.eq_state.clone()));
    }

    /// Broadcast current DRC state to dashboard.
    fn broadcast_drc(&self) {
        self.broadcast(&encore_common::protocol::ServerMsg::DrcState(self.drc_state.clone()));
    }

    /// Broadcast current power state to dashboard.
    fn broadcast_power_state(&self) {
        let state = self.bridge.state();
        self.broadcast(&encore_common::protocol::ServerMsg::AudioPowerState {
            state: state.as_str().to_string(),
        });
    }

    /// Transition: Active → Idle.
    ///
    /// Stock firmware never mutes amp/DAC after silence — it stays fully
    /// active at all times. We keep the state for dashboard reporting but
    /// do NOT touch hardware (no amp mute, no DAC standby). This matches
    /// stock behavior and avoids the TAS5756M standby register (0x02)
    /// which the stock firmware never writes.
    fn transition_to_idle(&self, _io: &mut IoExpander) {
        info!("Audio: Active → Idle");
        self.bridge.set_state(AudioPowerState::Idle);
        self.broadcast_power_state();
    }

    /// Transition: Idle → Standby.
    ///
    /// DOES NOT put the DAC in standby or mute amp/DAC. Stock firmware
    /// never uses TAS5756M register 0x02 (power/standby), and writing it
    /// can leave the DAC in a state it cannot recover from. We only
    /// power-gate the DSP if explicitly configured.
    fn transition_to_standby(&self, io: &mut IoExpander, _dac: &mut Dac, dsp: &mut Dsp) {
        info!("Audio: Idle → Standby");

        if self.dsp_power_gate {
            io.mute_dac().ok();
            self.bridge.request_park(); // close PCM only when DSP is gated
            io.dsp_power_off().ok();
            dsp.reset_fw_state();
            info!("Audio: DSP power gated, mixer parked");
        }

        self.bridge.set_state(AudioPowerState::Standby);
        self.broadcast_power_state();
    }

    /// Transition: Idle/Standby → Active (full resume).
    async fn transition_to_active(&self, io: &mut IoExpander, dac: &mut Dac, dsp: &mut Dsp) {
        let prev = self.bridge.state();
        info!("Audio: {} → Active", prev.as_str());

        if prev == AudioPowerState::Standby && self.dsp_power_gate {
            // Re-power DSP if it was gated (mixer is parked, PCM closed)
            if self.bridge.is_parked() {
                io.dsp_power_on().ok();
                if let Err(e) = dsp.upload_firmware(io) {
                    warn!("Audio: DSP firmware re-upload failed: {}", e);
                }
                // Unpark mixer (reopens PCM, restores I2S clocks)
                self.bridge.unpark();
                // Give DSP time to sync to restored I2S clocks
                tokio::time::sleep(std::time::Duration::from_millis(200)).await;
            }

            // Unmute DAC after DSP restore
            io.unmute_dac().ok();
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }

        // Ensure DAC is active (undo any prior standby from older firmware)
        dac.exit_standby().ok();

        self.bridge.set_state(AudioPowerState::Active);
        self.bridge.silence_count.store(0, Ordering::Release);
        self.broadcast_power_state();
    }

    /// Broadcast current DSP info to dashboard.
    fn broadcast_dsp(&self) {
        self.broadcast(&encore_common::protocol::ServerMsg::DspInfo(encore_common::protocol::DspInfo {
            version: self.dsp_version.clone(),
            hybridflow: 6,
            mic_muted: self.mic_muted,
            dsp_volume: self.dsp_volume,
        }));
    }
}

#[async_trait::async_trait]
impl Subsystem for AudioSubsystem {
    fn name(&self) -> &'static str {
        "audio"
    }

    async fn run(&mut self, mut ctx: SubsystemContext) -> Result<()> {
        ctx.health.set_state(SubsystemState::Running);

        // Initialize hardware
        let mut io = IoExpander::open().context("IO Expander init failed")?;
        io.init()?;

        let mut dac = Dac::open().context("DAC init failed")?;
        dac.init()?;

        // NOTE: dac.init_eq() NOT called at boot — selecting HybridFlow 6
        // causes a CPU hang on some boots (I2C bus contention with DSP).
        // DAC EQ can be enabled at runtime via dashboard command.

        // Enable the SoC audio PLL/MCLK early.
        Dsp::enable_audio_clock()?;

        // Upload DSP firmware BEFORE opening PCM (matches stock boot order).
        // AVIO registers are now read-modify-write to preserve kernel's I2S routing.
        let mut dsp = Dsp::open().context("DSP init failed")?;
        dsp.upload_firmware(&mut io)?;

        // Query DSP firmware version after upload
        self.dsp_version = match dsp.query_version() {
            Ok(bytes) => {
                let ver = bytes.iter().map(|b| format!("{:02X}", b)).collect::<Vec<_>>().join(" ");
                info!("Audio: DSP firmware version: {}", ver);
                ver
            }
            Err(e) => {
                warn!("Audio: DSP version query failed: {}", e);
                String::new()
            }
        };

        // Configure WM8904 codec mixer levels before unmuting.
        init_wm8904_mixer();

        // Wait for DSP firmware to fully boot and configure its I2S interface.
        info!("Audio: waiting 2s for DSP boot...");
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;

        // Set DSP volume to maximum — the DSP's internal processing (crossover,
        // speaker EQ, bass enhancement) works best at full scale. Volume control
        // is handled downstream by the DAC digital registers (0x3D/0x3E).
        if let Err(e) = dsp.set_volume(100) {
            warn!("Audio: initial DSP volume set failed (non-fatal): {}", e);
        }

        io.unmute()?;
        info!("Audio: hardware initialized, unmuted (DSP volume=100)");

        // Open PCM after DSP is fully booted (matches stock boot order).
        let mut pcm = AlsaPcm::open(&PcmConfig::default())
            .context("PCM open failed")?;

        // Broadcast initial state to any connected dashboard clients
        self.broadcast_eq();
        self.broadcast_drc();
        self.broadcast_dsp();

        let period_size = pcm.period_size();
        let channels = pcm.channels();
        let samples_per_period = (period_size * channels) as usize;

        // Spawn mixer thread (dedicated OS thread for real-time audio)
        let sources = self.sources.clone();
        let running = self.mixer_running.clone();
        running.store(true, Ordering::Release);

        // Channel for audio levels + spectrum: mixer thread → tokio task → WebSocket
        let (levels_tx, mut levels_rx) = mpsc::channel::<MixerMsg>(8);
        if let Some(ref ws) = self.ws_tx {
            let ws = ws.clone();
            tokio::spawn(async move {
                while let Some(mixer_msg) = levels_rx.recv().await {
                    match mixer_msg {
                        MixerMsg::Levels(l_rms, r_rms, l_peak, r_peak) => {
                            let msg = encore_common::protocol::ServerMsg::AudioLevels {
                                left_rms: l_rms, right_rms: r_rms,
                                left_peak: l_peak, right_peak: r_peak,
                            };
                            if let Ok(json) = serde_json::to_string(&msg) {
                                let _ = ws.send(json);
                            }
                        }
                        MixerMsg::Spectrum { bins, waveform } => {
                            let msg = encore_common::protocol::ServerMsg::AudioSpectrum {
                                bins: bins.to_vec(),
                            };
                            if let Ok(json) = serde_json::to_string(&msg) {
                                let _ = ws.send(json);
                            }
                            let msg2 = encore_common::protocol::ServerMsg::AudioWaveform {
                                samples: waveform.to_vec(),
                            };
                            if let Ok(json) = serde_json::to_string(&msg2) {
                                let _ = ws.send(json);
                            }
                        }
                    }
                }
            });
        }

        let shared_drc = self.shared_drc.clone();
        let network_tap = self.network_tap.clone();
        let tap_active = self.tap_active.clone();
        let mixer_bridge = self.bridge.clone();
        let mixer_handle = std::thread::Builder::new()
            .name("encore-mixer".into())
            .spawn(move || {
                mixer_thread(pcm, sources, running, samples_per_period, levels_tx, shared_drc, network_tap, tap_active, mixer_bridge);
            })
            .context("failed to spawn mixer thread")?;

        // Spawn capture thread if consumers were registered
        let capture_state = if let Some(mgr) = self.capture_mgr.take() {
            if mgr.has_consumers() {
                match mgr.start(1, 0) {
                    Ok((running, handle)) => {
                        info!("Audio: capture thread started");
                        Some((running, handle))
                    }
                    Err(e) => {
                        warn!("Audio: capture thread failed to start: {}", e);
                        None
                    }
                }
            } else {
                info!("Audio: no capture consumers, skipping capture thread");
                None
            }
        } else {
            None
        };

        // Power state management
        let mut power_tick = tokio::time::interval(std::time::Duration::from_secs(1));
        power_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut idle_elapsed_secs = 0u32;

        // PCM period rate: 48000Hz / 512 frames = ~94 periods/sec
        let periods_per_sec = 94u32;

        // Command loop
        loop {
            tokio::select! {
                _ = power_tick.tick() => {
                    let current_state = self.bridge.state();

                    // Check for wake request from mixer (audio detected)
                    if self.bridge.take_wake_request() {
                        if current_state != AudioPowerState::Active {
                            self.transition_to_active(&mut io, &mut dac, &mut dsp).await;
                            idle_elapsed_secs = 0;
                        }
                        continue;
                    }

                    // Check if any source is actively streaming (e.g., BT connected).
                    // This prevents power-down transitions while a source is active,
                    // even if the mixer hasn't seen data yet (codec warmup latency).
                    let any_source_active = self.sources.iter().any(|s| s.is_active());

                    // In Standby, wake up if any source is active
                    if current_state == AudioPowerState::Standby {
                        if any_source_active {
                            self.transition_to_active(&mut io, &mut dac, &mut dsp).await;
                            idle_elapsed_secs = 0;
                        }
                        continue;
                    }

                    let silence_periods = self.bridge.silence_periods();
                    let silence_secs = silence_periods / periods_per_sec;

                    match current_state {
                        AudioPowerState::Active => {
                            // Don't go Idle if any source is actively streaming
                            if silence_secs >= self.idle_timeout_secs && !any_source_active {
                                self.transition_to_idle(&mut io);
                                idle_elapsed_secs = 0;
                            }
                        }
                        AudioPowerState::Idle => {
                            if any_source_active {
                                // Source became active while in Idle — wake up
                                self.transition_to_active(&mut io, &mut dac, &mut dsp).await;
                                idle_elapsed_secs = 0;
                            } else {
                                idle_elapsed_secs += 1;
                                if idle_elapsed_secs >= self.standby_timeout_secs {
                                    self.transition_to_standby(&mut io, &mut dac, &mut dsp);
                                    idle_elapsed_secs = 0;
                                }
                            }
                        }
                        AudioPowerState::Standby => {} // handled above
                    }
                }
                cmd = self.cmd_rx.recv() => {
                    match cmd {
                        Some(AudioCmd::SetVolume(level)) => {
                            let level = level.clamp(0, 100);
                            // Volume control uses DAC hardware registers (0x3D/0x3E),
                            // NOT the DSP SPI volume command. This matches the stock
                            // Python driver which adjusts DAC registers 0x00-0xA0.
                            let reg = crate::mcu::dac::volume_to_reg(level);
                            if let Err(e) = dac.set_volume(reg) {
                                warn!("Audio: DAC volume failed: {}", e);
                            }

                            let now = std::time::SystemTime::now()
                                .duration_since(std::time::UNIX_EPOCH)
                                .unwrap_or_default()
                                .as_secs();
                            ctx.health.beat(now);
                            ctx.health.inc_msg();
                        }
                        Some(AudioCmd::Mute) => {
                            info!("Audio: muting");
                            io.mute().ok();
                        }
                        Some(AudioCmd::Unmute) => {
                            info!("Audio: unmuting");
                            io.unmute().ok();
                        }
                        Some(AudioCmd::SetEqBand { band, config }) => {
                            if (band as usize) < 10 {
                                self.eq_state.bands[band as usize] = config;
                                self.eq_state.preset = None;
                                if let Err(e) = dac.program_eq(&self.eq_state.bands, self.eq_state.enabled) {
                                    warn!("Audio: EQ program failed: {}", e);
                                }
                                self.broadcast_eq();
                            }
                            ctx.health.inc_msg();
                        }
                        Some(AudioCmd::SetEqPreset(preset)) => {
                            self.eq_state = eq_preset_to_state(preset);
                            if let Err(e) = dac.program_eq(&self.eq_state.bands, self.eq_state.enabled) {
                                warn!("Audio: EQ preset failed: {}", e);
                            }
                            self.broadcast_eq();
                            ctx.health.inc_msg();
                        }
                        Some(AudioCmd::SetEqEnabled(enabled)) => {
                            self.eq_state.enabled = enabled;
                            if let Err(e) = dac.program_eq(&self.eq_state.bands, self.eq_state.enabled) {
                                warn!("Audio: EQ enable toggle failed: {}", e);
                            }
                            self.broadcast_eq();
                            ctx.health.inc_msg();
                        }
                        Some(AudioCmd::SetDrc { band, config }) => {
                            let idx = match band {
                                encore_common::protocol::DrcBand::Low => 0,
                                encore_common::protocol::DrcBand::Mid => 1,
                                encore_common::protocol::DrcBand::High => 2,
                            };
                            self.drc_state.bands[idx] = config;
                            self.drc_state.preset = None;
                            self.shared_drc.update_from_state(&self.drc_state);
                            info!("Audio: DRC band {:?} updated", band);
                            self.broadcast_drc();
                            ctx.health.inc_msg();
                        }
                        Some(AudioCmd::SetDrcCrossover { low_mid_hz, mid_high_hz }) => {
                            self.drc_state.low_mid_hz = low_mid_hz;
                            self.drc_state.mid_high_hz = mid_high_hz;
                            info!("Audio: DRC crossover = {}/{} Hz", low_mid_hz, mid_high_hz);
                            self.broadcast_drc();
                            ctx.health.inc_msg();
                        }
                        Some(AudioCmd::SetDrcEnabled(enabled)) => {
                            self.drc_state.enabled = enabled;
                            self.shared_drc.update_from_state(&self.drc_state);
                            info!("Audio: DRC enabled = {}", enabled);
                            self.broadcast_drc();
                            ctx.health.inc_msg();
                        }
                        Some(AudioCmd::SetDrcPreset(preset)) => {
                            self.drc_state = drc_preset_to_state(preset);
                            self.shared_drc.update_from_state(&self.drc_state);
                            info!("Audio: DRC preset = {:?}", preset);
                            self.broadcast_drc();
                            ctx.health.inc_msg();
                        }
                        Some(AudioCmd::SetDspVolume(level)) => {
                            self.dsp_volume = level;
                            if let Err(e) = dsp.set_volume(level) {
                                warn!("Audio: DSP volume failed: {}", e);
                            }
                            self.broadcast_dsp();
                            ctx.health.inc_msg();
                        }
                        Some(AudioCmd::SetMicMute(muted)) => {
                            self.mic_muted = muted;
                            if let Err(e) = dsp.set_mic_mute(muted) {
                                warn!("Audio: DSP mic mute failed: {}", e);
                            }
                            self.broadcast_dsp();
                            ctx.health.inc_msg();
                        }
                        Some(AudioCmd::DacRegRead { page, reg, reply }) => {
                            let result = dac.read_paged(page, reg);
                            let _ = reply.send(result);
                        }
                        Some(AudioCmd::DacRegWrite { page, reg, value }) => {
                            if let Err(e) = dac.write_paged(page, reg, value) {
                                warn!("Audio: DAC reg write failed: {}", e);
                            }
                        }
                        Some(AudioCmd::DspSpiSend { msg_type, data, reply }) => {
                            let result = dsp.send_raw(msg_type, &data);
                            let _ = reply.send(result);
                        }
                        Some(AudioCmd::DspMemoryDump { start_page, num_pages, reply }) => {
                            info!("Audio: DSP memory dump pages 0x{:04X}..+{}", start_page, num_pages);
                            let result = dsp.dump_memory_range(start_page, num_pages);
                            let _ = reply.send(result);
                        }
                        Some(AudioCmd::DspDumpToFile { path, reply }) => {
                            info!("Audio: DSP full memory dump to {}", path);
                            let result = dsp.dump_all_to_file(&path);
                            let _ = reply.send(result);
                        }
                        Some(AudioCmd::DspPollEvents { reply }) => {
                            // Read one response via zero-transfer (no GPIO needed)
                            let result = dsp.send_raw(0x0000, &[0x08]).map(|rx| {
                                match Dsp::parse_event(&rx) {
                                    Some(ev) => vec![format!("{:?}", ev)],
                                    None => vec![],
                                }
                            });
                            let _ = reply.send(result);
                        }
                        Some(AudioCmd::StartMicTest) => {
                            self.mic_test_active.store(true, Ordering::Relaxed);
                            info!("Audio: mic test started");
                        }
                        Some(AudioCmd::StopMicTest) => {
                            self.mic_test_active.store(false, Ordering::Relaxed);
                            info!("Audio: mic test stopped");
                        }
                        None => {
                            info!("Audio: command channel closed");
                            break;
                        }
                    }
                }
                _ = ctx.shutdown.recv() => {
                    info!("Audio: shutdown");
                    break;
                }
            }
        }

        // Exit standby if needed for clean hardware state
        if self.bridge.state() == AudioPowerState::Standby {
            dac.exit_standby().ok();
        }

        // Stop mixer thread — unpark first so it can exit
        self.mixer_running.store(false, Ordering::Release);
        self.bridge.unpark(); // wake from condvar if parked
        if let Err(e) = mixer_handle.join() {
            warn!("Audio: mixer thread panicked: {:?}", e);
        }

        // Stop capture thread
        if let Some((capture_running, capture_handle)) = capture_state {
            capture_running.store(false, Ordering::Release);
            if let Err(e) = capture_handle.join() {
                warn!("Audio: capture thread panicked: {:?}", e);
            }
            info!("Audio: capture thread stopped");
        }

        // Mute before exit
        io.mute().ok();
        info!("Audio: muted, shutdown complete");

        Ok(())
    }
}

/// Set WM8904 codec ALSA mixer controls to optimal levels.
///
/// The SoC's I2S output goes through a WM8904 codec before reaching the
/// DAC/amplifier chain. The kernel defaults are far too low,
///  but current setup could be too loud for the hardware:
///   Master:    16/100 → 90/100 (near 0 dB)
///   Headphone: 34/63  → 50/63  (-7 dB)
///   DAC OSRx2: off    → on     (2x oversampling, better quality)
fn init_wm8904_mixer() {
    let ctl = match AlsaCtl::open(1) {
        Ok(c) => c,
        Err(e) => {
            warn!("WM8904: failed to open control device: {}", e);
            return;
        }
    };

    // Control names from WM8904 ALSA driver (kernel uses full names,
    // not the short forms that amixer's `sset` accepts).
    if let Err(e) = ctl.set_integer("Master Volume", &[90, 90]) {
        warn!("WM8904: Master Volume failed: {}", e);
    } else {
        info!("WM8904: Master Volume = 90,90");
    }

    if let Err(e) = ctl.set_integer("Headphone Volume", &[50, 50]) {
        warn!("WM8904: Headphone Volume failed: {}", e);
    } else {
        info!("WM8904: Headphone Volume = 50,50");
    }

    if let Err(e) = ctl.set_bool("DAC OSRx2 Switch", true) {
        warn!("WM8904: DAC OSRx2 Switch failed: {}", e);
    } else {
        info!("WM8904: DAC OSRx2 Switch = on");
    }
}

/// Convert EQ preset to full EqState with 10 band configurations.
fn eq_preset_to_state(preset: encore_common::protocol::EqPreset) -> encore_common::protocol::EqState {
    use encore_common::protocol::{EqBand, EqPreset, EqState, FilterType};
    let bands = match preset {
        EqPreset::Flat => [EqBand::default(); 10],
        EqPreset::BassBoost => {
            let mut b = [EqBand::default(); 10];
            b[0] = EqBand { freq_hz: 60, gain_cb: 80, q_x10: 7, filter_type: FilterType::LowShelf };
            b[1] = EqBand { freq_hz: 150, gain_cb: 40, q_x10: 10, filter_type: FilterType::Peak };
            b
        }
        EqPreset::VocalClarity => {
            let mut b = [EqBand::default(); 10];
            b[0] = EqBand { freq_hz: 200, gain_cb: -30, q_x10: 8, filter_type: FilterType::Peak };
            b[1] = EqBand { freq_hz: 2500, gain_cb: 50, q_x10: 12, filter_type: FilterType::Peak };
            b[2] = EqBand { freq_hz: 5000, gain_cb: 30, q_x10: 10, filter_type: FilterType::Peak };
            b
        }
        EqPreset::Warm => {
            let mut b = [EqBand::default(); 10];
            b[0] = EqBand { freq_hz: 80, gain_cb: 40, q_x10: 7, filter_type: FilterType::LowShelf };
            b[1] = EqBand { freq_hz: 3000, gain_cb: -20, q_x10: 10, filter_type: FilterType::Peak };
            b[2] = EqBand { freq_hz: 10000, gain_cb: -40, q_x10: 7, filter_type: FilterType::HighShelf };
            b
        }
        EqPreset::LateNight => {
            let mut b = [EqBand::default(); 10];
            b[0] = EqBand { freq_hz: 60, gain_cb: -60, q_x10: 7, filter_type: FilterType::LowShelf };
            b[1] = EqBand { freq_hz: 1000, gain_cb: 30, q_x10: 8, filter_type: FilterType::Peak };
            b[2] = EqBand { freq_hz: 8000, gain_cb: -40, q_x10: 7, filter_type: FilterType::HighShelf };
            b
        }
    };
    EqState { bands, preset: Some(preset), enabled: true }
}

/// Convert DRC preset to full DrcState.
fn drc_preset_to_state(preset: encore_common::protocol::DrcPreset) -> encore_common::protocol::DrcState {
    use encore_common::protocol::{DrcBandConfig, DrcPreset, DrcState};
    match preset {
        DrcPreset::Off => DrcState::default(),
        DrcPreset::Gentle => DrcState {
            bands: [
                DrcBandConfig { threshold_db: -25, ratio_x10: 20, attack_ms: 20, release_ms: 300 },
                DrcBandConfig { threshold_db: -20, ratio_x10: 20, attack_ms: 15, release_ms: 250 },
                DrcBandConfig { threshold_db: -20, ratio_x10: 20, attack_ms: 10, release_ms: 200 },
            ],
            low_mid_hz: 200, mid_high_hz: 2000,
            preset: Some(DrcPreset::Gentle), enabled: true,
        },
        DrcPreset::LateNight => DrcState {
            bands: [
                DrcBandConfig { threshold_db: -35, ratio_x10: 60, attack_ms: 5, release_ms: 500 },
                DrcBandConfig { threshold_db: -25, ratio_x10: 30, attack_ms: 10, release_ms: 300 },
                DrcBandConfig { threshold_db: -20, ratio_x10: 20, attack_ms: 10, release_ms: 200 },
            ],
            low_mid_hz: 150, mid_high_hz: 2500,
            preset: Some(DrcPreset::LateNight), enabled: true,
        },
        DrcPreset::Protect => DrcState {
            bands: [
                DrcBandConfig { threshold_db: -10, ratio_x10: 100, attack_ms: 1, release_ms: 100 },
                DrcBandConfig { threshold_db: -10, ratio_x10: 100, attack_ms: 1, release_ms: 100 },
                DrcBandConfig { threshold_db: -10, ratio_x10: 100, attack_ms: 1, release_ms: 100 },
            ],
            low_mid_hz: 200, mid_high_hz: 2000,
            preset: Some(DrcPreset::Protect), enabled: true,
        },
    }
}

/// Software DRC (dynamic range compressor) state for the mixer thread.
/// Single-band feed-forward compressor with peak detection envelope.
struct DrcProcessor {
    envelope: f32,         // current envelope level (0.0 - 1.0)
    threshold: f32,        // threshold in linear scale
    ratio: f32,            // compression ratio (e.g. 2.0)
    attack_coeff: f32,     // smoothing coefficient for attack
    release_coeff: f32,    // smoothing coefficient for release
    enabled: bool,
}

impl DrcProcessor {
    fn new() -> Self {
        Self {
            envelope: 0.0,
            threshold: 1.0,
            ratio: 1.0,
            attack_coeff: 0.0,
            release_coeff: 0.0,
            enabled: false,
        }
    }

    /// Reload parameters from shared atomics.
    fn sync_params(&mut self, params: &SharedDrcParams) {
        self.enabled = params.enabled.load(Ordering::Acquire);
        if !self.enabled { return; }

        let thresh_db = params.threshold_db.load(Ordering::Relaxed) as f32;
        let ratio_x10 = params.ratio_x10.load(Ordering::Relaxed).max(10) as f32;
        let attack_ms = params.attack_ms.load(Ordering::Relaxed).max(1) as f32;
        let release_ms = params.release_ms.load(Ordering::Relaxed).max(1) as f32;

        // Convert threshold from dBFS to linear
        self.threshold = 10.0_f32.powf(thresh_db / 20.0);
        self.ratio = ratio_x10 / 10.0;

        // Compute smoothing coefficients: α = 1 - e^(-1 / (fs * t))
        // At 48kHz with 512-sample frames, we process per-sample.
        let fs = 48000.0_f32;
        self.attack_coeff = 1.0 - (-1.0 / (fs * attack_ms / 1000.0)).exp();
        self.release_coeff = 1.0 - (-1.0 / (fs * release_ms / 1000.0)).exp();
    }

    /// Process stereo-interleaved i32 samples in-place.
    fn process(&mut self, buf: &mut [i32]) {
        if !self.enabled || self.ratio <= 1.0 { return; }

        let frames = buf.len() / 2;
        let inv_max = 1.0 / (i32::MAX as f32);

        for i in 0..frames {
            let l = buf[i * 2] as f32 * inv_max;
            let r = buf[i * 2 + 1] as f32 * inv_max;

            // Peak detection (stereo max)
            let peak = l.abs().max(r.abs());

            // Envelope follower (attack/release)
            let coeff = if peak > self.envelope { self.attack_coeff } else { self.release_coeff };
            self.envelope += coeff * (peak - self.envelope);

            // Compute gain reduction
            if self.envelope > self.threshold {
                let over_db = 20.0 * (self.envelope / self.threshold).log10();
                let reduced_db = over_db / self.ratio;
                let gain = 10.0_f32.powf((reduced_db - over_db) / 20.0);

                buf[i * 2] = ((l * gain) * i32::MAX as f32) as i32;
                buf[i * 2 + 1] = ((r * gain) * i32::MAX as f32) as i32;
            }
        }
    }
}

/// Dedicated mixer thread — reads from all active slots, sums, writes to ALSA.
/// Runs on a real OS thread (not tokio) for deterministic timing.
/// Supports parking (standby mode): closes PCM and waits on condvar until unparked.
fn mixer_thread(
    pcm: AlsaPcm,
    sources: Vec<Arc<MixerSlot>>,
    running: Arc<AtomicBool>,
    samples_per_period: usize,
    levels_tx: mpsc::Sender<MixerMsg>,
    shared_drc: Arc<SharedDrcParams>,
    network_tap: Option<Arc<MixerSlot>>,
    tap_active: Option<Arc<AtomicBool>>,
    bridge: Arc<MixerBridge>,
) {
    info!("Mixer thread started (period={} samples)", samples_per_period);

    let mut pcm: Option<AlsaPcm> = Some(pcm);
    let mut mix_buf = vec![0i32; samples_per_period];
    let mut logged_first_audio = false;

    // Software DRC processor
    let mut drc = DrcProcessor::new();
    let mut drc_sync_counter = 0u32;

    // VU meter state: accumulate over ~100ms (~10 periods at 512 frames/period)
    // then send a levels update. 48000 / 512 ≈ 94 periods/sec → every 9 periods ≈ 10Hz.
    // Send levels every ~47 periods ≈ 2Hz (was 9 ≈ 10Hz, which flooded
    // the WebSocket and caused WiFi backpressure → tokio starvation).
    const LEVELS_INTERVAL: u32 = 47;
    let mut levels_counter = 0u32;
    let mut sum_sq_l = 0.0f64;
    let mut sum_sq_r = 0.0f64;
    let mut peak_l = 0.0f32;
    let mut peak_r = 0.0f32;
    let mut sample_count = 0u32;

    // FFT state: accumulate 512 mono samples, run FFT every ~24 periods (~4Hz)
    const FFT_INTERVAL: u32 = 24;
    let mut fft_buf: [f32; 512] = [0.0; 512];
    let mut fft_pos: usize = 0;
    let mut fft_counter: u32 = 0;
    let mut waveform_snap: [f32; 256] = [0.0; 256];

    while running.load(Ordering::Acquire) {
        // Check if we should park (standby mode)
        if bridge.is_parked() {
            // Close PCM to stop I2S clocks and reduce power
            if let Some(p) = pcm.take() {
                p.drop_pcm().ok();
                debug!("Mixer: PCM closed for standby");
            }

            // Wait on condvar until unparked or shutdown
            let mut guard = bridge.park_mutex.lock().unwrap();
            while bridge.is_parked() && running.load(Ordering::Acquire) {
                guard = bridge.park_condvar.wait(guard).unwrap();
            }
            drop(guard);

            if !running.load(Ordering::Acquire) {
                break;
            }

            // Reopen PCM after unpark
            match AlsaPcm::open(&PcmConfig::default()) {
                Ok(new_pcm) => {
                    debug!("Mixer: PCM reopened after standby");
                    pcm = Some(new_pcm);
                }
                Err(e) => {
                    warn!("Mixer: PCM reopen failed: {}", e);
                    // Try again next iteration
                    std::thread::sleep(std::time::Duration::from_millis(100));
                    continue;
                }
            }
        }

        // Zero the mix buffer
        for s in mix_buf.iter_mut() {
            *s = 0;
        }

        // Sum all active sources
        let mut any_active = false;
        for slot in &sources {
            if slot.is_active() && slot.available() >= samples_per_period {
                slot.read_add(&mut mix_buf);
                any_active = true;
            }
        }

        if any_active {
            bridge.silence_count.store(0, Ordering::Release);

            // Signal wake if not in Active state
            if bridge.state() != AudioPowerState::Active {
                bridge.wake_request.store(true, Ordering::Release);
            }

            // Sync DRC params from shared atomics ~10x/sec (not every period)
            drc_sync_counter += 1;
            if drc_sync_counter >= LEVELS_INTERVAL {
                drc.sync_params(&shared_drc);
                drc_sync_counter = 0;
            }

            // Log first non-silent mixer output for diagnostics
            if !logged_first_audio {
                let max_abs = mix_buf.iter().map(|s| s.unsigned_abs()).max().unwrap_or(0);
                if max_abs > 0 {
                    info!("Mixer: first audio output, max_abs={} ({:.1}dBFS)",
                        max_abs, 20.0 * (max_abs as f64 / i32::MAX as f64).log10());
                    logged_first_audio = true;
                }
            }

            // Apply software DRC (in-place, before VU metering)
            drc.process(&mut mix_buf);

            // Push post-DRC audio to the network tap for group streaming (leader mode)
            if let (Some(ref tap), Some(ref active)) = (&network_tap, &tap_active) {
                if active.load(Ordering::Relaxed) {
                    tap.push(&mix_buf);
                }
            }

            // Compute per-channel levels (stereo interleaved: L R L R ...)
            let frames = samples_per_period / 2;
            for i in 0..frames {
                let l = (mix_buf[i * 2] as f32) / (i32::MAX as f32);
                let r = (mix_buf[i * 2 + 1] as f32) / (i32::MAX as f32);
                sum_sq_l += (l as f64) * (l as f64);
                sum_sq_r += (r as f64) * (r as f64);
                let al = l.abs();
                let ar = r.abs();
                if al > peak_l { peak_l = al; }
                if ar > peak_r { peak_r = ar; }

                // Accumulate mono samples for FFT
                let mono = (l + r) * 0.5;
                if fft_pos < 512 {
                    fft_buf[fft_pos] = mono;
                    fft_pos += 1;
                }
                // Also capture waveform snapshot (first 256 samples of accumulation)
                if fft_pos <= 256 {
                    waveform_snap[fft_pos.saturating_sub(1)] = mono;
                }
            }
            sample_count += frames as u32;

            levels_counter += 1;
            if levels_counter >= LEVELS_INTERVAL {
                let n = sample_count.max(1) as f64;
                let rms_l = (sum_sq_l / n).sqrt() as f32;
                let rms_r = (sum_sq_r / n).sqrt() as f32;
                let _ = levels_tx.try_send(MixerMsg::Levels(rms_l, rms_r, peak_l, peak_r));
                // Reset accumulators
                sum_sq_l = 0.0;
                sum_sq_r = 0.0;
                peak_l = 0.0;
                peak_r = 0.0;
                sample_count = 0;
                levels_counter = 0;
            }

            // FFT processing (~4Hz)
            fft_counter += 1;
            if fft_counter >= FFT_INTERVAL && fft_pos >= 512 {
                // Apply Hann window
                for i in 0..512 {
                    let w = 0.5 * (1.0 - (std::f32::consts::TAU * i as f32 / 512.0).cos());
                    fft_buf[i] *= w;
                }

                // Run real FFT (512 samples → 257 complex bins)
                let spectrum = microfft::real::rfft_512(&mut fft_buf);

                // Map 257 bins to 32 log-spaced magnitude bins (20Hz–20kHz)
                let bin_hz = 48000.0_f32 / 512.0; // 93.75 Hz per bin
                let mut bins_32 = [0.0f32; 32];
                // Log-spaced center frequencies from 20Hz to 20kHz
                let log_min = 20.0_f32.ln();
                let log_max = 20000.0_f32.ln();
                for b in 0..32 {
                    let t0 = b as f32 / 32.0;
                    let t1 = (b + 1) as f32 / 32.0;
                    let f0 = (log_min + t0 * (log_max - log_min)).exp();
                    let f1 = (log_min + t1 * (log_max - log_min)).exp();
                    let bin_start = (f0 / bin_hz).floor() as usize;
                    let bin_end = (f1 / bin_hz).ceil() as usize;
                    let bin_start = bin_start.max(1).min(256);
                    let bin_end = bin_end.max(bin_start + 1).min(257);

                    let mut max_mag = 0.0f32;
                    for i in bin_start..bin_end {
                        let re = spectrum[i].re;
                        let im = spectrum[i].im;
                        let mag = (re * re + im * im).sqrt();
                        if mag > max_mag { max_mag = mag; }
                    }

                    // Convert to dBFS and map [-90, 0] to [0.0, 1.0]
                    let db = if max_mag > 1e-10 { 20.0 * max_mag.log10() } else { -90.0 };
                    bins_32[b] = ((db + 90.0) / 90.0).clamp(0.0, 1.0);
                }

                let _ = levels_tx.try_send(MixerMsg::Spectrum {
                    bins: bins_32,
                    waveform: waveform_snap,
                });

                fft_pos = 0;
                fft_counter = 0;
            }

            // Write i32 samples directly as S32_LE to PCM
            if let Some(ref mut p) = pcm {
                if let Err(e) = p.write_frames(&mix_buf) {
                    warn!("Mixer: PCM write error: {}", e);
                }
            }
        } else {
            // Increment silence count atomically
            bridge.silence_count.fetch_add(1, Ordering::Release);

            // Reset VU meter to zero during silence
            if levels_counter > 0 || peak_l > 0.0 || peak_r > 0.0 {
                let _ = levels_tx.try_send(MixerMsg::Levels(0.0, 0.0, 0.0, 0.0));
                sum_sq_l = 0.0;
                sum_sq_r = 0.0;
                peak_l = 0.0;
                peak_r = 0.0;
                sample_count = 0;
                levels_counter = 0;
            }

            // Always write silence to keep WM8904 I2S clocks active.
            // The DSP only generates I2S output to the DAC when it sees
            // continuous I2S input. If we stop writing, the WM8904 stops
            // generating clocks and the DSP stops its output.
            for s in mix_buf.iter_mut() { *s = 0; }
            if let Some(ref mut p) = pcm {
                if let Err(e) = p.write_frames(&mix_buf) {
                    warn!("Mixer: PCM silence write error: {}", e);
                }
            }
        }
    }

    if let Some(p) = pcm {
        p.drop_pcm().ok();
    }
    info!("Mixer thread stopped");
}
