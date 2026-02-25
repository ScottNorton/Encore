//! Spotify Connect subsystem via librespot.
//!
//! Runs an in-process Spotify Connect device. Decoded PCM audio is pushed
//! into a MixerSlot ring buffer for the audio subsystem's mixer thread.

use crate::audio::mixer::MixerSlot;
use crate::subsystem::{Subsystem, SubsystemContext};
use anyhow::{Context, Result};
use futures::StreamExt;
use encore_common::protocol::{ServerMsg, SpotifyStatus, SubsystemState, TrackInfo};
use librespot_connect::{Spirc, ConnectConfig};
use librespot_core::config::DeviceType;
use librespot_core::cache::Cache;
use librespot_core::config::SessionConfig;
use librespot_core::session::Session;
use librespot_discovery::Discovery;
use librespot_metadata::audio::item::UniqueFields;
use librespot_playback::audio_backend::{Sink, SinkResult};
use librespot_playback::config::{AudioFormat, PlayerConfig};
use librespot_playback::convert::Converter;
use librespot_playback::decoder::AudioPacket;
use librespot_playback::mixer::{self, MixerConfig};
use librespot_playback::player::{Player, PlayerEvent};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{broadcast, mpsc};
use tracing::info;

const CACHE_DIR: &str = "/lsync/encore/spotify";
const DEFAULT_NAME: &str = "Invoke";
/// Fixed port for the Spotify Connect zeroconf HTTP server.
/// Our mDNS responder advertises this port in SRV records for
/// `_spotify-connect._tcp.local.`, replacing libmdns (which has a
/// strict DNS parser that rejects queries with the AD bit set).
pub const ZEROCONF_PORT: u16 = 48144;

/// Mutable playback state for broadcasting SpotifyStatus.
struct SpotifyPlayState {
    is_playing: bool,
    shuffle: bool,
    repeat_context: bool,
    repeat_track: bool,
    volume: u16,
    position_ms: u32,
    duration_ms: u32,
    track: Option<TrackInfo>,
    connected_user: Option<String>,
}

impl SpotifyPlayState {
    fn new() -> Self {
        Self {
            is_playing: false,
            shuffle: false,
            repeat_context: false,
            repeat_track: false,
            volume: u16::MAX / 2,
            position_ms: 0,
            duration_ms: 0,
            track: None,
            connected_user: None,
        }
    }

    fn to_status(&self) -> SpotifyStatus {
        SpotifyStatus {
            is_playing: self.is_playing,
            shuffle: self.shuffle,
            repeat_context: self.repeat_context,
            repeat_track: self.repeat_track,
            volume: self.volume,
            position_ms: self.position_ms,
            duration_ms: self.duration_ms,
            track: self.track.clone(),
            connected_user: self.connected_user.clone(),
        }
    }
}

/// Spotify Connect subsystem.
pub struct SpotifySubsystem {
    slot: Arc<MixerSlot>,
    device_name: String,
    cmd_rx: Option<mpsc::Receiver<encore_common::protocol::SpotifyAction>>,
    ws_tx: Option<broadcast::Sender<String>>,
    suspend_rx: Option<mpsc::Receiver<bool>>,
    group_cmd_tx: Option<mpsc::Sender<crate::group::GroupCmd>>,
}

impl SpotifySubsystem {
    pub fn new(
        slot: Arc<MixerSlot>,
        device_name: Option<String>,
        cmd_rx: Option<mpsc::Receiver<encore_common::protocol::SpotifyAction>>,
        ws_tx: Option<broadcast::Sender<String>>,
    ) -> Self {
        Self {
            slot,
            device_name: device_name.unwrap_or_else(|| DEFAULT_NAME.to_string()),
            cmd_rx,
            ws_tx,
            suspend_rx: None,
            group_cmd_tx: None,
        }
    }

    /// Set the suspend channel for group sync (follower mode pauses Spotify).
    pub fn set_suspend_rx(&mut self, rx: mpsc::Receiver<bool>) {
        self.suspend_rx = Some(rx);
    }

    /// Set the group command channel for notifying the group subsystem.
    pub fn set_group_tx(&mut self, tx: mpsc::Sender<crate::group::GroupCmd>) {
        self.group_cmd_tx = Some(tx);
    }
}

#[async_trait::async_trait]
impl Subsystem for SpotifySubsystem {
    fn name(&self) -> &'static str {
        "spotify"
    }

    async fn run(&mut self, mut ctx: SubsystemContext) -> Result<()> {
        ctx.health.set_state(SubsystemState::Running);

        let session_config = SessionConfig::default();
        // Credentials on NAND, audio cache on tmpfs (RAM-backed, won't fill NAND)
        let cache = Cache::new(
            Some(CACHE_DIR),       // credentials/volume state on NAND
            None,                  // no separate volume cache dir
            Some("/tmp/spotify"),  // audio cache on tmpfs (RAM)
            Some(20 * 1024 * 1024), // 20MB cap (tmpfs is 32MB total)
        )
        .ok();

        // Start Spotify Connect discovery (zeroconf/mDNS)
        // Bind to wlan0 IP only — the default picks up ap0 (192.168.43.1)
        // which is unreachable from the home WiFi network.
        let device_id = session_config.device_id.clone();
        let wlan_ip = crate::network::get_wlan_ip();
        info!("Spotify: starting discovery as '{}' (bind: {:?})", self.device_name, wlan_ip);

        let mut builder = Discovery::builder(device_id, session_config.client_id.clone())
            .name(self.device_name.clone())
            .device_type(DeviceType::Speaker)
            .port(ZEROCONF_PORT);
        if let Some(ip) = wlan_ip {
            builder = builder.zeroconf_ip(vec![ip]);
        }
        let mut discovery = builder
            .launch()
            .context("failed to start Spotify discovery")?;

        // Wait for a Spotify client to authenticate via Connect
        let credentials = loop {
            tokio::select! {
                event = discovery.next() => {
                    match event {
                        Some(creds) => {
                            info!("Spotify: authenticated via Connect");
                            break creds;
                        }
                        None => {
                            anyhow::bail!("Spotify discovery stream ended");
                        }
                    }
                }
                _ = ctx.shutdown.recv() => {
                    return Ok(());
                }
            }
        };

        // Create session (Spirc::new handles session.connect() internally)
        let session = Session::new(session_config, cache);

        // Create mixer (software volume)
        let mixer_config = MixerConfig::default();
        let mixer_fn = mixer::find(None).context("no mixer found")?;
        let soft_mixer = mixer_fn(mixer_config)
            .context("failed to create software mixer")?;
        let volume_getter = soft_mixer.get_soft_volume();

        // Load audio quality settings from config
        let cfg = encore_common::config::EncoreConfigFile::load(
            std::path::Path::new("/lsync/encore/config.toml"),
        )
        .unwrap_or_default();
        let bitrate = match cfg.spotify.bitrate.as_str() {
            "96" => librespot_playback::config::Bitrate::Bitrate96,
            "160" => librespot_playback::config::Bitrate::Bitrate160,
            _ => librespot_playback::config::Bitrate::Bitrate320,
        };

        // Create player with custom sink
        let slot = self.slot.clone();
        let player_config = PlayerConfig {
            bitrate,
            gapless: cfg.spotify.gapless,
            normalisation: cfg.spotify.normalisation,
            normalisation_type: match cfg.spotify.normalisation_type.as_str() {
                "album" => librespot_playback::config::NormalisationType::Album,
                "track" => librespot_playback::config::NormalisationType::Track,
                _ => librespot_playback::config::NormalisationType::Auto,
            },
            normalisation_pregain_db: cfg.spotify.normalisation_pregain_db as f64,
            position_update_interval: Some(Duration::from_secs(1)),
            ..PlayerConfig::default()
        };

        let player = Player::new(
            player_config,
            session.clone(),
            volume_getter,
            move || -> Box<dyn Sink> {
                Box::new(EncoreSink {
                    slot: slot.clone(),
                    format: AudioFormat::S32,
                    resampler: Resampler::new(44100, 48000, 2),
                })
            },
        );
        let mut events = player.get_player_event_channel();

        // Create Spirc (Spotify Connect protocol handler)
        let connect_config = ConnectConfig {
            name: self.device_name.clone(),
            device_type: DeviceType::Speaker,
            initial_volume: u16::MAX / 2, // ~50%
            ..ConnectConfig::default()
        };

        let (spirc, spirc_task) = Spirc::new(
            connect_config,
            session.clone(),
            credentials,
            player,
            soft_mixer,
        ).await
        .context("Spotify Spirc creation failed")?;

        // Spawn the Spirc background task
        let spirc_handle = tokio::spawn(spirc_task);

        info!("Spotify: connected and ready — device '{}' active", self.device_name);

        // Mutable playback state for broadcasting
        let mut play_state = SpotifyPlayState::new();
        let ws_tx = self.ws_tx.clone();

        // Event loop — take command/suspend receivers out of self for use in select!
        let group_cmd_tx = self.group_cmd_tx.clone();
        let has_cmds = self.cmd_rx.is_some();
        let (_dummy_tx, dummy_rx) = mpsc::channel::<encore_common::protocol::SpotifyAction>(1);
        let mut cmd_rx = self.cmd_rx.take().unwrap_or(dummy_rx);
        let has_suspend = self.suspend_rx.is_some();
        let (_suspend_dummy_tx, suspend_dummy_rx) = mpsc::channel::<bool>(1);
        let mut suspend_rx = self.suspend_rx.take().unwrap_or(suspend_dummy_rx);
        let mut suspended = false;
        loop {
            tokio::select! {
                event = events.recv() => {
                    match event {
                        Some(event) => {
                            handle_player_event(&event, &ctx, &mut play_state, &ws_tx, &group_cmd_tx);
                        }
                        None => {
                            info!("Spotify: event channel closed");
                            break;
                        }
                    }
                }
                Some(suspend) = suspend_rx.recv(), if has_suspend => {
                    if suspend && !suspended {
                        info!("Spotify: suspended by group (follower mode)");
                        suspended = true;
                        let _ = spirc.pause();
                        self.slot.set_active(false);
                    } else if !suspend && suspended {
                        info!("Spotify: resumed by group");
                        suspended = false;
                        self.slot.set_active(true);
                    }
                }
                Some(action) = cmd_rx.recv(), if has_cmds => {
                    use encore_common::protocol::SpotifyAction;
                    match action {
                        SpotifyAction::Play => {
                            if let Err(e) = spirc.play() { tracing::warn!("Spotify: play failed: {}", e); }
                            else { info!("Spotify: play (remote)"); }
                        }
                        SpotifyAction::Pause => {
                            if let Err(e) = spirc.pause() { tracing::warn!("Spotify: pause failed: {}", e); }
                            else { info!("Spotify: pause (remote)"); }
                        }
                        SpotifyAction::Next => {
                            if let Err(e) = spirc.next() { tracing::warn!("Spotify: next failed: {}", e); }
                            else { info!("Spotify: next (remote)"); }
                        }
                        SpotifyAction::Previous => {
                            if let Err(e) = spirc.prev() { tracing::warn!("Spotify: prev failed: {}", e); }
                            else { info!("Spotify: prev (remote)"); }
                        }
                        SpotifyAction::SetVolume { level } => {
                            let vol = (level as u32 * u16::MAX as u32 / 100).min(u16::MAX as u32) as u16;
                            if let Err(e) = spirc.set_volume(vol) { tracing::warn!("Spotify: set_volume failed: {}", e); }
                            else {
                                info!("Spotify: volume set to {}% (raw {})", level, vol);
                                play_state.volume = vol;
                                broadcast_status(&play_state, &ws_tx);
                            }
                        }
                        SpotifyAction::Seek { position_ms } => {
                            if let Err(e) = spirc.set_position_ms(position_ms) { tracing::warn!("Spotify: seek failed: {}", e); }
                            else {
                                info!("Spotify: seek to {}ms", position_ms);
                                play_state.position_ms = position_ms;
                                broadcast_status(&play_state, &ws_tx);
                            }
                        }
                        SpotifyAction::Shuffle { enabled } => {
                            if let Err(e) = spirc.shuffle(enabled) { tracing::warn!("Spotify: shuffle failed: {}", e); }
                            else {
                                info!("Spotify: shuffle = {}", enabled);
                                play_state.shuffle = enabled;
                                broadcast_status(&play_state, &ws_tx);
                            }
                        }
                        SpotifyAction::Repeat { enabled } => {
                            if let Err(e) = spirc.repeat(enabled) { tracing::warn!("Spotify: repeat failed: {}", e); }
                            else {
                                info!("Spotify: repeat context = {}", enabled);
                                play_state.repeat_context = enabled;
                                broadcast_status(&play_state, &ws_tx);
                            }
                        }
                        SpotifyAction::RepeatTrack { enabled } => {
                            if let Err(e) = spirc.repeat_track(enabled) { tracing::warn!("Spotify: repeat_track failed: {}", e); }
                            else {
                                info!("Spotify: repeat track = {}", enabled);
                                play_state.repeat_track = enabled;
                                broadcast_status(&play_state, &ws_tx);
                            }
                        }
                    }
                }
                _ = ctx.shutdown.recv() => {
                    info!("Spotify: shutdown");
                    break;
                }
            }
        }

        let _ = spirc.shutdown();
        spirc_handle.abort();

        self.slot.set_active(false);
        self.slot.clear();

        info!("Spotify: stopped");
        Ok(())
    }
}

/// Broadcast current SpotifyStatus as a JSON ServerMsg over WebSocket.
fn broadcast_status(state: &SpotifyPlayState, ws_tx: &Option<broadcast::Sender<String>>) {
    if let Some(tx) = ws_tx {
        let msg = ServerMsg::SpotifyStatus(state.to_status());
        if let Ok(json) = serde_json::to_string(&msg) {
            let _ = tx.send(json);
        }
    }
}

fn handle_player_event(
    event: &PlayerEvent,
    ctx: &SubsystemContext,
    state: &mut SpotifyPlayState,
    ws_tx: &Option<broadcast::Sender<String>>,
    group_cmd_tx: &Option<mpsc::Sender<crate::group::GroupCmd>>,
) {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    match event {
        PlayerEvent::TrackChanged { audio_item } => {
            let artist = match &audio_item.unique_fields {
                UniqueFields::Track { artists, .. } => {
                    artists.0.iter().map(|a| a.name.as_str()).collect::<Vec<_>>().join(", ")
                }
                UniqueFields::Episode { show_name, .. } => show_name.clone(),
                _ => String::new(),
            };
            let album = match &audio_item.unique_fields {
                UniqueFields::Track { album, .. } => album.clone(),
                _ => String::new(),
            };
            let cover_url = audio_item.covers.first().map(|c| c.url.clone()).unwrap_or_default();
            let track = TrackInfo {
                title: audio_item.name.clone(),
                artist,
                album,
                duration_ms: audio_item.duration_ms,
                position_ms: 0,
                cover_url,
                uri: audio_item.uri.clone(),
                is_explicit: audio_item.is_explicit,
            };
            info!("Spotify: track changed — {} by {}", track.title, track.artist);
            state.duration_ms = audio_item.duration_ms;
            state.position_ms = 0;
            state.track = Some(track.clone());

            // Also broadcast TrackChanged for backward compat
            if let Some(tx) = ws_tx {
                let msg = ServerMsg::TrackChanged(track);
                if let Ok(json) = serde_json::to_string(&msg) {
                    let _ = tx.send(json);
                }
            }
            broadcast_status(state, ws_tx);
            ctx.health.inc_msg();
        }
        PlayerEvent::Playing { position_ms, .. } => {
            let was_playing = state.is_playing;
            info!("Spotify: playing at {}ms", position_ms);
            state.is_playing = true;
            state.position_ms = *position_ms;
            broadcast_status(state, ws_tx);
            if !was_playing {
                if let Some(ref tx) = group_cmd_tx {
                    let _ = tx.try_send(crate::group::GroupCmd::LocalAudioStarted { source: "spotify".into() });
                }
            }
            ctx.health.beat(now);
            ctx.health.inc_msg();
        }
        PlayerEvent::Paused { position_ms, .. } => {
            info!("Spotify: paused at {}ms", position_ms);
            state.is_playing = false;
            state.position_ms = *position_ms;
            broadcast_status(state, ws_tx);
            if let Some(ref tx) = group_cmd_tx {
                let _ = tx.try_send(crate::group::GroupCmd::LocalAudioStopped { source: "spotify".into() });
            }
            ctx.health.beat(now);
        }
        PlayerEvent::Stopped { .. } => {
            info!("Spotify: stopped");
            state.is_playing = false;
            state.track = None;
            state.position_ms = 0;
            state.duration_ms = 0;
            broadcast_status(state, ws_tx);
            if let Some(ref tx) = group_cmd_tx {
                let _ = tx.try_send(crate::group::GroupCmd::LocalAudioStopped { source: "spotify".into() });
            }
            ctx.health.beat(now);
        }
        PlayerEvent::PositionChanged { position_ms, .. } => {
            // Update internal state only — DON'T broadcast.
            // The client interpolates position at 1Hz locally.
            // Broadcasting here causes 1Hz full-page re-renders.
            state.position_ms = *position_ms;
        }
        PlayerEvent::Seeked { position_ms, .. } => {
            state.position_ms = *position_ms;
            broadcast_status(state, ws_tx);
        }
        PlayerEvent::VolumeChanged { volume } => {
            info!("Spotify: volume changed to {}", volume);
            state.volume = *volume;
            broadcast_status(state, ws_tx);
        }
        PlayerEvent::ShuffleChanged { shuffle } => {
            info!("Spotify: shuffle = {}", shuffle);
            state.shuffle = *shuffle;
            broadcast_status(state, ws_tx);
        }
        PlayerEvent::RepeatChanged { context, track } => {
            info!("Spotify: repeat context={} track={}", context, track);
            state.repeat_context = *context;
            state.repeat_track = *track;
            broadcast_status(state, ws_tx);
        }
        PlayerEvent::SessionConnected { user_name, .. } => {
            info!("Spotify: session connected (user: {})", user_name);
            // Don't overwrite connected_user here — the canonical username
            // is a numeric ID for newer accounts (e.g. "12122522170").
            // SessionClientChanged fires shortly after with the friendly name.
            if state.connected_user.is_none() {
                // Fallback in case SessionClientChanged never fires
                state.connected_user = Some(user_name.clone());
                broadcast_status(state, ws_tx);
            }
        }
        PlayerEvent::SessionClientChanged { client_name, client_brand_name, client_model_name, .. } => {
            let client_display = if !client_name.is_empty() {
                client_name.clone()
            } else if !client_brand_name.is_empty() {
                format!("{} {}", client_brand_name, client_model_name)
            } else {
                return; // no useful info
            };
            info!("Spotify: client changed — {}", client_display);
            state.connected_user = Some(client_display);
            broadcast_status(state, ws_tx);
        }
        PlayerEvent::SessionDisconnected { .. } => {
            info!("Spotify: session disconnected");
            state.connected_user = None;
            state.is_playing = false;
            state.track = None;
            state.position_ms = 0;
            state.duration_ms = 0;
            broadcast_status(state, ws_tx);
        }
        _ => {}
    }
}

/// Linear interpolation resampler: 44100 Hz → 48000 Hz (stereo S32).
///
/// The hardware DAC is clocked at 48kHz only. Spotify/librespot decodes at
/// 44.1kHz. Without resampling, playback is ~8.8% too fast and pitch-shifted up.
struct Resampler {
    /// Input samples consumed per output sample: 44100 / 48000 ≈ 0.91875
    step: f64,
    channels: usize,
    /// Fractional position carried across chunk boundaries.
    phase: f64,
}

impl Resampler {
    fn new(from_rate: u32, to_rate: u32, channels: u32) -> Self {
        Self {
            step: from_rate as f64 / to_rate as f64,
            channels: channels as usize,
            phase: 0.0,
        }
    }

    /// Resample a chunk of interleaved S32 samples. Returns the resampled output.
    fn process(&mut self, input: &[i32]) -> Vec<i32> {
        let ch = self.channels;
        let in_frames = input.len() / ch;
        if in_frames == 0 {
            return Vec::new();
        }

        // Output has more frames than input (upsampling).
        let est_out = ((in_frames as f64 / self.step) as usize) + 2;
        let mut output = Vec::with_capacity(est_out * ch);

        let mut pos = self.phase;
        while pos < (in_frames - 1) as f64 {
            let idx = pos as usize;
            let frac = pos - idx as f64;
            for c in 0..ch {
                let a = input[idx * ch + c] as f64;
                let b = input[(idx + 1) * ch + c] as f64;
                output.push((a + (b - a) * frac) as i32);
            }
            pos += self.step;
        }

        // Carry fractional position to next chunk.
        self.phase = pos - (in_frames - 1) as f64;
        output
    }
}

/// Custom librespot Sink that pushes decoded PCM into a MixerSlot ring buffer.
struct EncoreSink {
    slot: Arc<MixerSlot>,
    #[allow(dead_code)]
    format: AudioFormat,
    resampler: Resampler,
}

impl Sink for EncoreSink {
    fn start(&mut self) -> SinkResult<()> {
        self.slot.set_active(true);
        self.slot.clear();
        self.resampler.phase = 0.0;
        Ok(())
    }

    fn stop(&mut self) -> SinkResult<()> {
        self.slot.set_active(false);
        Ok(())
    }

    fn write(&mut self, packet: AudioPacket, converter: &mut Converter) -> SinkResult<()> {
        match packet {
            AudioPacket::Samples(samples) => {
                let s32 = converter.f64_to_s32(&samples);
                // Resample 44100 → 48000 Hz
                let resampled = self.resampler.process(&s32);
                // Backpressure: block until ALL samples are written to the ring buffer.
                // Without this, librespot decodes at full CPU speed, overflows the buffer,
                // drops samples, and "finishes" a 3-minute track in ~6 seconds.
                let mut offset = 0;
                while offset < resampled.len() {
                    let written = self.slot.push(&resampled[offset..]);
                    offset += written;
                    if offset < resampled.len() {
                        // Ring buffer full — sleep to let mixer thread drain.
                        // 5ms ≈ half an ALSA period (10.67ms at 256 frames/48kHz).
                        std::thread::sleep(std::time::Duration::from_millis(5));
                    }
                }
            }
            AudioPacket::Raw(_) => {
                // Raw passthrough — not used with S32 format
            }
        }
        Ok(())
    }
}
