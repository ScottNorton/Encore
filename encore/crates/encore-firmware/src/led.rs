//! LED ring controller subsystem.
//!
//! Receives LedAnimation commands, runs animation state machine at ~30fps,
//! and writes 39-byte frames to the MCU via I2C.

use crate::audio::subsystem::AudioCmd;
use crate::mcu::{LED_COUNT, LED_FRAME_SIZE, McuEvent, Mcu};
use crate::subsystem::{Subsystem, SubsystemContext};
use anyhow::Result;
use encore_common::protocol::{LedAnimation, ServerMsg, SubsystemState};
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use tokio::sync::{broadcast, mpsc};
use tokio::time::{interval, Duration};
use tracing::{debug, info};

/// Commands sent to the LED subsystem from the routing task.
pub enum LedCmd {
    /// Set a new persistent animation.
    Animate(LedAnimation),
    /// Set master volume (triggers VolumeArc overlay + forwards to audio).
    SetVolume(u8),
    /// Play a stock .bin animation file by name.
    PlayBin { name: String, repeat: bool },
    /// Signal that all subsystems are ready (triggers boot-complete surge).
    BootComplete,
    /// Signal boot complete in safe mode (amber/blue surge then SafeMode animation).
    SafeBootComplete,
    /// Update the volume ring step size (1-5% per detent click).
    SetVolStep(u8),
}

/// Physical button actions routed from MCU events to main.rs.
pub enum ButtonAction {
    PlayPause,
    VoiceTrigger,
    MicToggle,
    WifiSetup,
    BluetoothToggle,
}

/// Target frame rate for animations.
const FPS: u64 = 30;
const FRAME_INTERVAL: Duration = Duration::from_millis(1000 / FPS);

/// How many frames to show the volume arc after a volume change.
const VOL_ARC_FRAMES: u32 = FPS as u32 * 2; // 2 seconds

/// Default volume at startup (percent).
const DEFAULT_VOLUME: u8 = 30;

/// Cache of stock .bin animation files (loaded once at startup).
struct AnimationCache {
    files: HashMap<String, Vec<[u8; LED_FRAME_SIZE]>>,
}

impl AnimationCache {
    fn load(dir: &Path) -> Self {
        let mut files = HashMap::new();
        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|e| e.to_str()) != Some("bin") {
                    continue;
                }
                let name = path.file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("")
                    .to_string();
                match std::fs::read(&path) {
                    Ok(data) if !data.is_empty() && data.len() % LED_FRAME_SIZE == 0 => {
                        let frames: Vec<[u8; LED_FRAME_SIZE]> = data
                            .chunks_exact(LED_FRAME_SIZE)
                            .map(|c| {
                                let mut f = [0u8; LED_FRAME_SIZE];
                                f.copy_from_slice(c);
                                f
                            })
                            .collect();
                        info!("LED: cached {} ({} frames)", name, frames.len());
                        files.insert(name, frames);
                    }
                    Ok(data) => {
                        info!("LED: skipped {} ({}B, not multiple of {})", name, data.len(), LED_FRAME_SIZE);
                    }
                    Err(e) => {
                        info!("LED: failed to read {}: {}", name, e);
                    }
                }
            }
        }
        info!("LED: loaded {} animation files", files.len());
        AnimationCache { files }
    }

    fn get(&self, name: &str) -> Option<&Vec<[u8; LED_FRAME_SIZE]>> {
        self.files.get(name)
    }
}

/// State for .bin file playback.
struct BinPlayer {
    frames: Vec<[u8; LED_FRAME_SIZE]>,
    position: usize,
    repeat: bool,
    is_first_batch: bool,
}

/// LED controller that runs animations on the 13-LED ring.
/// Also polls MCU events and routes volume changes to the audio subsystem.
pub struct LedSubsystem {
    cmd_rx: mpsc::Receiver<LedCmd>,
    audio_tx: mpsc::Sender<AudioCmd>,
    button_tx: mpsc::Sender<ButtonAction>,
    mcu: Arc<std::sync::Mutex<Mcu>>,
    ws_tx: Option<broadcast::Sender<String>>,
    group_cmd_tx: Option<mpsc::Sender<crate::group::GroupCmd>>,
    brightness: u8,
    volume: u8,
    vol_step: u8,
}

impl LedSubsystem {
    pub fn new(
        cmd_rx: mpsc::Receiver<LedCmd>,
        audio_tx: mpsc::Sender<AudioCmd>,
        button_tx: mpsc::Sender<ButtonAction>,
        mcu: Arc<std::sync::Mutex<Mcu>>,
        vol_step: u8,
    ) -> Self {
        Self {
            cmd_rx,
            audio_tx,
            button_tx,
            mcu,
            ws_tx: None,
            group_cmd_tx: None,
            brightness: 100,
            volume: DEFAULT_VOLUME,
            vol_step: vol_step.clamp(1, 5),
        }
    }

    pub fn set_ws_tx(&mut self, tx: broadcast::Sender<String>) {
        self.ws_tx = Some(tx);
    }

    /// Set the group command channel for outgoing volume sync.
    pub fn set_group_tx(&mut self, tx: mpsc::Sender<crate::group::GroupCmd>) {
        self.group_cmd_tx = Some(tx);
    }

    fn broadcast_led(&self, anim: &LedAnimation) {
        if let Some(ref tx) = self.ws_tx {
            let msg = ServerMsg::LedStateChanged(anim.clone());
            if let Ok(json) = serde_json::to_string(&msg) {
                let _ = tx.send(json);
            }
        }
    }

    fn broadcast_volume(&self) {
        if let Some(ref tx) = self.ws_tx {
            let msg = ServerMsg::VolumeChanged {
                source: encore_common::protocol::SourceId::System,
                level: self.volume,
            };
            if let Ok(json) = serde_json::to_string(&msg) {
                let _ = tx.send(json);
            }
        }
    }
}

#[async_trait::async_trait]
impl Subsystem for LedSubsystem {
    fn name(&self) -> &'static str {
        "led"
    }

    async fn run(&mut self, mut ctx: SubsystemContext) -> Result<()> {
        ctx.health.set_state(SubsystemState::Running);
        info!("LED: controller started ({}fps)", FPS);

        let cache = AnimationCache::load(Path::new("/usr/share/lights"));

        // Set initial volume on audio subsystem
        let _ = self.audio_tx.try_send(AudioCmd::SetVolume(self.volume));

        let mut animation = LedAnimation::BootSurge;
        let mut frame_num: u32 = 0;
        let mut vol_arc_countdown: u32 = 0;
        let mut is_first_frame: bool = true;
        let mut bin_playback: Option<BinPlayer> = None;
        let mut ticker = interval(FRAME_INTERVAL);

        loop {
            tokio::select! {
                _ = ticker.tick() => {
                    // Poll MCU events FIRST so volume changes affect this tick's frame.
                    if let Ok(mut mcu) = self.mcu.lock() {
                        match mcu.read_event() {
                            Ok(Some(McuEvent::VolumeUp(steps))) => {
                                let delta = (steps.max(1) as u16) * (self.vol_step as u16);
                                self.volume = (self.volume as u16 + delta).min(100) as u8;
                                let _ = self.audio_tx.try_send(AudioCmd::SetVolume(self.volume));
                                vol_arc_countdown = VOL_ARC_FRAMES;
                                self.broadcast_volume();
                                self.broadcast_led(&LedAnimation::VolumeArc { level: self.volume });
                                if let Some(ref tx) = self.group_cmd_tx {
                                    let _ = tx.try_send(crate::group::GroupCmd::SetVolume(self.volume));
                                }
                                info!("Volume: {} (+{})", self.volume, steps);
                            }
                            Ok(Some(McuEvent::VolumeDown(steps))) => {
                                let delta = (steps.max(1) as u16) * (self.vol_step as u16);
                                self.volume = self.volume.saturating_sub(delta as u8);
                                let _ = self.audio_tx.try_send(AudioCmd::SetVolume(self.volume));
                                vol_arc_countdown = VOL_ARC_FRAMES;
                                self.broadcast_volume();
                                self.broadcast_led(&LedAnimation::VolumeArc { level: self.volume });
                                if let Some(ref tx) = self.group_cmd_tx {
                                    let _ = tx.try_send(crate::group::GroupCmd::SetVolume(self.volume));
                                }
                                info!("Volume: {} (-{})", self.volume, steps);
                            }
                            Ok(Some(McuEvent::TouchShortPress)) => {
                                let _ = self.button_tx.try_send(ButtonAction::PlayPause);
                            }
                            Ok(Some(McuEvent::TouchLongPress)) => {
                                let _ = self.button_tx.try_send(ButtonAction::VoiceTrigger);
                            }
                            Ok(Some(McuEvent::BluetoothShort)) => {
                                let _ = self.button_tx.try_send(ButtonAction::BluetoothToggle);
                            }
                            Ok(Some(McuEvent::BluetoothLong)) => {
                                debug!("MCU: bluetooth long press (reserved)");
                            }
                            Ok(Some(McuEvent::MicShort)) => {
                                let _ = self.button_tx.try_send(ButtonAction::MicToggle);
                            }
                            Ok(Some(McuEvent::MicLong)) => {
                                let _ = self.button_tx.try_send(ButtonAction::WifiSetup);
                            }
                            Ok(Some(McuEvent::ResetShort)) | Ok(Some(McuEvent::ResetLong)) => {
                                debug!("MCU: reset button (hardware handles this)");
                            }
                            Ok(Some(event)) => {
                                debug!("MCU event: {:?}", event);
                            }
                            Ok(None) => {} // no event pending
                            Err(e) => {
                                debug!("MCU read_event error: {}", e);
                            }
                        }
                    }

                    // Volume arc overlay — render colored arc while countdown active
                    if vol_arc_countdown > 0 {
                        vol_arc_countdown -= 1;
                        let frame = render_frame(&LedAnimation::VolumeArc { level: self.volume }, frame_num, self.brightness);
                        if let Ok(mut mcu) = self.mcu.lock() {
                            if let Err(e) = mcu.set_led_frames(&[frame], is_first_frame) {
                                debug!("LED: I2C write failed: {}", e);
                            }
                            is_first_frame = false;
                        }
                        if vol_arc_countdown == 0 {
                            self.broadcast_led(&animation);
                        }
                    } else {
                        // .bin file playback takes priority over programmatic animations
                        let mut bin_done = false;
                        if let Some(ref mut player) = bin_playback {
                            // Only write a batch every ~8 ticks (8 x 33ms = 264ms, matching stock ~280ms timing)
                            if frame_num % 8 == 0 {
                                let remaining = player.frames.len() - player.position;
                                if remaining == 0 {
                                    if player.repeat {
                                        player.position = 0;
                                        player.is_first_batch = true;
                                    } else {
                                        bin_done = true;
                                    }
                                } else {
                                    let batch_size = remaining.min(10);
                                    let batch = &player.frames[player.position..player.position + batch_size];
                                    if let Ok(mut mcu) = self.mcu.lock() {
                                        let _ = mcu.set_led_frames(batch, player.is_first_batch);
                                        player.is_first_batch = false;
                                    }
                                    player.position += batch_size;
                                }
                            }
                        }
                        if bin_done {
                            bin_playback = None;
                        }

                        if bin_playback.is_none() {
                            // Render either volume arc overlay or normal animation
                            let frame = render_frame(&animation, frame_num, self.brightness);

                            if let Ok(mut mcu) = self.mcu.lock() {
                                if let Err(e) = mcu.set_led_frames(&[frame], is_first_frame) {
                                    debug!("LED: I2C write failed: {}", e);
                                }
                                is_first_frame = false;
                            }
                        }
                    }

                    frame_num = frame_num.wrapping_add(1);

                    if frame_num % (FPS as u32 * 10) == 0 {
                        let now = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_secs();
                        ctx.health.beat(now);
                    }
                }
                cmd = self.cmd_rx.recv() => {
                    match cmd {
                        Some(LedCmd::Animate(new_anim)) => {
                            info!("LED: animation -> {:?}", new_anim);
                            self.broadcast_led(&new_anim);
                            animation = new_anim;
                            frame_num = 0;
                            vol_arc_countdown = 0; // cancel any active volume arc
                            bin_playback = None; // cancel any bin playback
                            is_first_frame = true;
                        }
                        Some(LedCmd::SetVolume(level)) => {
                            self.volume = level.min(100);
                            let _ = self.audio_tx.try_send(AudioCmd::SetVolume(self.volume));
                            vol_arc_countdown = VOL_ARC_FRAMES;
                            self.broadcast_volume();
                            self.broadcast_led(&LedAnimation::VolumeArc { level: self.volume });
                            debug!("Volume: {} (remote)", self.volume);
                        }
                        Some(LedCmd::PlayBin { name, repeat }) => {
                            if let Some(frames) = cache.get(&name) {
                                info!("LED: playing {} ({} frames, repeat={})", name, frames.len(), repeat);
                                bin_playback = Some(BinPlayer {
                                    frames: frames.clone(),
                                    position: 0,
                                    repeat,
                                    is_first_batch: true,
                                });
                            } else {
                                info!("LED: animation '{}' not found in cache", name);
                            }
                        }
                        Some(LedCmd::SetVolStep(step)) => {
                            self.vol_step = step.clamp(1, 5);
                            debug!("LED: vol_step -> {}", self.vol_step);
                        }
                        Some(LedCmd::BootComplete) => {
                            info!("LED: boot complete -- surge sequence");
                            bin_playback = None; // Cancel any bin playback

                            // Phase 2: Surge to 100% gold over ~1 second
                            for surge in 0u32..30 {
                                let intensity = surge as f32 / 29.0;
                                let mut frame = [0u8; LED_FRAME_SIZE];
                                for i in 0..LED_COUNT {
                                    frame[i * 3] = scale_byte(255, intensity);
                                    frame[i * 3 + 1] = scale_byte(180, intensity);
                                    frame[i * 3 + 2] = scale_byte(30, intensity);
                                }
                                if let Ok(mut mcu) = self.mcu.lock() {
                                    let _ = mcu.set_led_frames(&[frame], false);
                                }
                                tokio::time::sleep(FRAME_INTERVAL).await;
                            }
                            // Hold at 100% for 300ms
                            tokio::time::sleep(Duration::from_millis(300)).await;
                            // Phase 3: Fade to black over 2 seconds
                            for fade in 0u32..60 {
                                let intensity = 1.0 - (fade as f32 / 59.0);
                                let mut frame = [0u8; LED_FRAME_SIZE];
                                for i in 0..LED_COUNT {
                                    frame[i * 3] = scale_byte(255, intensity);
                                    frame[i * 3 + 1] = scale_byte(180, intensity);
                                    frame[i * 3 + 2] = scale_byte(30, intensity);
                                }
                                if let Ok(mut mcu) = self.mcu.lock() {
                                    let _ = mcu.set_led_frames(&[frame], false);
                                }
                                tokio::time::sleep(FRAME_INTERVAL).await;
                            }
                            // 500ms delay then show volume
                            tokio::time::sleep(Duration::from_millis(500)).await;
                            animation = LedAnimation::Off;
                            let _ = self.audio_tx.try_send(AudioCmd::SetVolume(self.volume));
                            vol_arc_countdown = VOL_ARC_FRAMES;
                            self.broadcast_volume();
                            self.broadcast_led(&LedAnimation::VolumeArc { level: self.volume });
                            is_first_frame = true; // Reset for normal operation
                        }
                        Some(LedCmd::SafeBootComplete) => {
                            info!("LED: SAFE MODE boot complete -- amber/blue surge");
                            bin_playback = None;

                            // Phase 1: Surge 0→100% with amber/blue sector pattern (~1s)
                            for surge in 0u32..30 {
                                let intensity = surge as f32 / 29.0;
                                let mut frame = [0u8; LED_FRAME_SIZE];
                                let offset = (surge * 15 / FPS as u32) % 12;
                                for i in 0..12u32 {
                                    if (i + offset) % 12 < 6 {
                                        frame[i as usize * 3] = scale_byte(255, intensity);
                                        frame[i as usize * 3 + 1] = scale_byte(160, intensity);
                                        frame[i as usize * 3 + 2] = scale_byte(48, intensity);
                                    } else {
                                        frame[i as usize * 3] = scale_byte(32, intensity);
                                        frame[i as usize * 3 + 1] = scale_byte(96, intensity);
                                        frame[i as usize * 3 + 2] = scale_byte(255, intensity);
                                    }
                                }
                                // Center amber during surge
                                frame[12 * 3] = scale_byte(255, intensity);
                                frame[12 * 3 + 1] = scale_byte(160, intensity);
                                frame[12 * 3 + 2] = scale_byte(48, intensity);
                                if let Ok(mut mcu) = self.mcu.lock() {
                                    let _ = mcu.set_led_frames(&[frame], false);
                                }
                                tokio::time::sleep(FRAME_INTERVAL).await;
                            }
                            // Phase 2: Hold at full brightness for 300ms
                            tokio::time::sleep(Duration::from_millis(300)).await;
                            // Phase 3: Transition into continuous SafeMode animation
                            animation = LedAnimation::SafeMode;
                            frame_num = 0;
                            is_first_frame = true;
                            self.broadcast_led(&animation);
                            // Show volume arc for 2s (same as normal boot)
                            tokio::time::sleep(Duration::from_millis(500)).await;
                            let _ = self.audio_tx.try_send(AudioCmd::SetVolume(self.volume));
                            vol_arc_countdown = VOL_ARC_FRAMES;
                            self.broadcast_volume();
                            self.broadcast_led(&LedAnimation::VolumeArc { level: self.volume });
                        }
                        None => {
                            info!("LED: command channel closed");
                            break;
                        }
                    }
                }
                _ = ctx.shutdown.recv() => {
                    info!("LED: shutdown");
                    break;
                }
            }
        }

        // Turn off LEDs on exit
        if let Ok(mut mcu) = self.mcu.lock() {
            let _ = mcu.set_led_frames(&[[0u8; LED_FRAME_SIZE]], false);
        }

        Ok(())
    }
}

/// Render a single animation frame.
fn render_frame(animation: &LedAnimation, frame_num: u32, brightness: u8) -> [u8; LED_FRAME_SIZE] {
    let mut frame = [0u8; LED_FRAME_SIZE];
    let scale = brightness as f32 / 100.0;

    match animation {
        LedAnimation::Off => {} // all zeros

        LedAnimation::Solid { r, g, b } => {
            for i in 0..LED_COUNT {
                frame[i * 3] = scale_byte(*r, scale);
                frame[i * 3 + 1] = scale_byte(*g, scale);
                frame[i * 3 + 2] = scale_byte(*b, scale);
            }
        }

        LedAnimation::Breathe { r, g, b, period_ms } => {
            // Sinusoidal brightness modulation
            let period = (*period_ms).max(100) as f32;
            let t = (frame_num as f32 * (1000.0 / FPS as f32)) % period;
            let phase = (t / period) * std::f32::consts::TAU;
            // Map sin [-1,1] to [0,1] intensity
            let intensity = (phase.sin() + 1.0) / 2.0;
            let s = scale * intensity;

            for i in 0..LED_COUNT {
                frame[i * 3] = scale_byte(*r, s);
                frame[i * 3 + 1] = scale_byte(*g, s);
                frame[i * 3 + 2] = scale_byte(*b, s);
            }
        }

        LedAnimation::Spin { r, g, b, speed } => {
            // Single lit LED rotating around the 12-LED ring
            let ring_leds = 12u32;
            let speed_val = (*speed).max(1) as u32;
            // Advance position every (FPS / speed) frames
            let pos = (frame_num * speed_val / FPS as u32) % ring_leds;

            // Lit LED at pos, dimmer neighbors
            for i in 0..ring_leds {
                let dist = ring_distance(i, pos, ring_leds);
                let intensity = match dist {
                    0 => 1.0,
                    1 => 0.3,
                    2 => 0.08,
                    _ => 0.0,
                };
                let s = scale * intensity;
                frame[i as usize * 3] = scale_byte(*r, s);
                frame[i as usize * 3 + 1] = scale_byte(*g, s);
                frame[i as usize * 3 + 2] = scale_byte(*b, s);
            }
            // Center LED dim
            let cs = scale * 0.15;
            frame[12 * 3] = scale_byte(*r, cs);
            frame[12 * 3 + 1] = scale_byte(*g, cs);
            frame[12 * 3 + 2] = scale_byte(*b, cs);
        }

        LedAnimation::Pulse { r, g, b } => {
            // Quick flash then fade -- period fixed at 1s
            let t = frame_num % FPS as u32;
            let intensity = if t < 3 {
                1.0 // flash
            } else {
                let fade_t = (t - 3) as f32 / (FPS as f32 - 3.0);
                (1.0 - fade_t).max(0.0)
            };
            let s = scale * intensity;
            for i in 0..LED_COUNT {
                frame[i * 3] = scale_byte(*r, s);
                frame[i * 3 + 1] = scale_byte(*g, s);
                frame[i * 3 + 2] = scale_byte(*b, s);
            }
        }

        LedAnimation::VolumeArc { level } => {
            // Illuminate ring LEDs 0..N proportional to volume level
            // Color matches web UI knob: blue → yellow → red
            let lit_count = (*level as u32 * 12 + 50) / 100; // 0-12
            let (r, g, b) = volume_color(*level);
            for i in 0..12u32 {
                if i < lit_count {
                    frame[i as usize * 3] = scale_byte(r, scale);
                    frame[i as usize * 3 + 1] = scale_byte(g, scale);
                    frame[i as usize * 3 + 2] = scale_byte(b, scale);
                }
            }
            // Center LED at reduced brightness
            let cs = scale * 0.3;
            frame[12 * 3] = scale_byte(r, cs);
            frame[12 * 3 + 1] = scale_byte(g, cs);
            frame[12 * 3 + 2] = scale_byte(b, cs);
        }

        LedAnimation::BootSurge => {
            // Warm gold breathing that peaks at ~70% brightness -- never fully arrives.
            let period = 2000.0f32;
            let t = (frame_num as f32 * (1000.0 / FPS as f32)) % period;
            let phase = (t / period) * std::f32::consts::TAU;
            let intensity = 0.15 + (phase.sin() + 1.0) / 2.0 * 0.55;
            let s = scale * intensity;
            for i in 0..LED_COUNT {
                frame[i * 3] = scale_byte(255, s);
                frame[i * 3 + 1] = scale_byte(180, s);
                frame[i * 3 + 2] = scale_byte(30, s);
            }
        }

        LedAnimation::SafeMode => {
            // Amber/blue alternating sectors rotating at ~0.5 rev/sec.
            // 12 ring LEDs split into two 6-LED sectors.
            let speed = 15u32; // ~0.5 rev/sec at 30fps
            let offset = (frame_num * speed / FPS as u32) % 12;
            let s = scale * 0.7;
            for i in 0..12u32 {
                if (i + offset) % 12 < 6 {
                    // Amber (255, 160, 48)
                    frame[i as usize * 3] = scale_byte(255, s);
                    frame[i as usize * 3 + 1] = scale_byte(160, s);
                    frame[i as usize * 3 + 2] = scale_byte(48, s);
                } else {
                    // Blue (32, 96, 255)
                    frame[i as usize * 3] = scale_byte(32, s);
                    frame[i as usize * 3 + 1] = scale_byte(96, s);
                    frame[i as usize * 3 + 2] = scale_byte(255, s);
                }
            }
            // Center LED: alternates amber/blue every 2 seconds (60 frames)
            let center_amber = (frame_num / 60) % 2 == 0;
            if center_amber {
                frame[12 * 3] = scale_byte(255, s);
                frame[12 * 3 + 1] = scale_byte(160, s);
                frame[12 * 3 + 2] = scale_byte(48, s);
            } else {
                frame[12 * 3] = scale_byte(32, s);
                frame[12 * 3 + 1] = scale_byte(96, s);
                frame[12 * 3 + 2] = scale_byte(255, s);
            }
        }

        LedAnimation::Custom { ref frames } => {
            // Play custom frame sequence — select frame by index wrapping at FPS
            if !frames.is_empty() {
                let idx = frame_num as usize % frames.len();
                let f = &frames[idx];
                for (i, &(r, g, b)) in f.colors.iter().enumerate() {
                    if i >= LED_COUNT {
                        break;
                    }
                    frame[i * 3] = scale_byte(r, scale);
                    frame[i * 3 + 1] = scale_byte(g, scale);
                    frame[i * 3 + 2] = scale_byte(b, scale);
                }
            }
        }
    }

    frame
}

/// Scale a color byte by a float factor.
fn scale_byte(val: u8, factor: f32) -> u8 {
    (val as f32 * factor).round().min(255.0) as u8
}

/// Distance between two positions on a ring.
fn ring_distance(a: u32, b: u32, ring_size: u32) -> u32 {
    let d = if a > b { a - b } else { b - a };
    d.min(ring_size - d)
}

/// Volume level → RGB color (matches web UI knob gradient).
fn volume_color(level: u8) -> (u8, u8, u8) {
    if level > 80 {
        (248, 81, 73)   // red — loud
    } else if level > 60 {
        (227, 179, 65)  // yellow — moderate-high
    } else {
        (88, 166, 255)  // blue — normal
    }
}
