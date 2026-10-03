//! Spotify Connect subsystem via librespot.
//!
//! Runs an in-process Spotify Connect device. Decoded PCM audio is pushed
//! into a MixerSlot ring buffer for the audio subsystem's mixer thread.

use crate::audio::mixer::MixerSlot;
use crate::subsystem::{Subsystem, SubsystemContext};
use anyhow::{Context, Result};
use encore_common::protocol::{ServerMsg, SpotifyStatus, SubsystemState, TrackInfo};
use futures::StreamExt;
use librespot_connect::{ConnectConfig, Spirc};
use librespot_core::cache::Cache;
use librespot_core::config::DeviceType;
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
use tokio::sync::{broadcast, mpsc, watch};
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
    suspend_rx: Option<watch::Receiver<bool>>,
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
    pub fn set_suspend_rx(&mut self, rx: watch::Receiver<bool>) {
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

        // Take command receiver early so it's available during discovery
        let (_dummy_tx, dummy_rx) = mpsc::channel::<encore_common::protocol::SpotifyAction>(1);
        let mut cmd_rx = self.cmd_rx.take().unwrap_or(dummy_rx);
        let has_cmds = true; // always true after take (dummy still works)

        let session_config = SessionConfig::default();
        // Credentials on NAND, audio cache on tmpfs (RAM-backed, won't fill NAND)
        let cache = Cache::new(
            Some(CACHE_DIR),        // credentials/volume state on NAND
            None,                   // no separate volume cache dir
            Some("/run/spotify"),   // audio cache on tmpfs (RAM) — /run is 150MB vs /tmp's 32MB
            Some(64 * 1024 * 1024), // 64MB cap
        )
        .ok();

        // Channels are taken from `self` ONCE and reused across reconnect cycles.
        let group_cmd_tx = self.group_cmd_tx.clone();
        let has_suspend = self.suspend_rx.is_some();
        let (_suspend_dummy_tx, suspend_dummy_rx) = watch::channel(false);
        let mut suspend_rx = self.suspend_rx.take().unwrap_or(suspend_dummy_rx);

        // Keep librespot's Discovery (the zeroconf HTTP server on port 48144) ALIVE
        // for the speaker's whole life. Dropping it after the first auth — the old
        // behaviour — made the device vanish from the Spotify picker the instant a
        // phone connected. It is launched lazily, relaunched if its mDNS backend
        // dies, and a new phone claiming the running device yields fresh credentials
        // via discovery.next() so we hand the session over. Only a user-disable or a
        // process shutdown returns from run().
        let device_id = session_config.device_id.clone();
        let mut discovery: Option<Discovery> = None;
        let mut pending_creds = None;
        loop {
            // Obtain credentials: a pending hand-off from a new claimant, otherwise
            // (re)launch discovery and wait for a phone to claim this device. The
            // Discovery object lives in `discovery` across iterations so 48144 stays up.
            let credentials = match pending_creds.take() {
                Some(c) => c,
                None => loop {
                    if discovery.is_none() {
                        let wlan_ip = loop {
                            if let Some(ip) = crate::network::get_wlan_ip() {
                                break ip;
                            }
                            tokio::select! {
                                _ = tokio::time::sleep(Duration::from_secs(2)) => {}
                                Some(action) = cmd_rx.recv() => {
                                    if matches!(action, encore_common::protocol::SpotifyAction::SetEnabled { enabled: false }) {
                                        info!("Spotify: disabled by user (waiting for WiFi)");
                                        return Ok(());
                                    }
                                }
                                _ = ctx.shutdown.recv() => { return Ok(()); }
                            }
                        };
                        info!(
                            "Spotify: discovery listening as '{}' (bind: {})",
                            self.device_name, wlan_ip
                        );
                        match Discovery::builder(
                            device_id.clone(),
                            session_config.client_id.clone(),
                        )
                        .name(self.device_name.clone())
                        .device_type(DeviceType::Speaker)
                        .port(ZEROCONF_PORT)
                        .zeroconf_ip(vec![wlan_ip])
                        .launch()
                        {
                            Ok(d) => discovery = Some(d),
                            Err(e) => {
                                tracing::warn!(
                                    "Spotify: discovery launch failed: {}, retrying in 5s",
                                    e
                                );
                                tokio::select! {
                                    _ = tokio::time::sleep(Duration::from_secs(5)) => { continue; }
                                    _ = ctx.shutdown.recv() => { return Ok(()); }
                                }
                            }
                        }
                    }
                    tokio::select! {
                        event = discovery.as_mut().unwrap().next() => {
                            match event {
                                Some(creds) => {
                                    info!("Spotify: authenticated via Connect");
                                    break creds;
                                }
                                None => {
                                    tracing::warn!("Spotify: discovery stream ended, relaunching in 5s");
                                    discovery = None;
                                    tokio::select! {
                                        _ = tokio::time::sleep(Duration::from_secs(5)) => { continue; }
                                        _ = ctx.shutdown.recv() => { return Ok(()); }
                                    }
                                }
                            }
                        }
                        Some(action) = cmd_rx.recv() => {
                            if matches!(action, encore_common::protocol::SpotifyAction::SetEnabled { enabled: false }) {
                                info!("Spotify: disabled by user (during discovery)");
                                return Ok(());
                            }
                        }
                        _ = ctx.shutdown.recv() => {
                            return Ok(());
                        }
                    }
                },
            };

            // Create session (Spirc::new handles session.connect() internally).
            // Clone the config/cache so the same device_id is reused every reconnect.
            let session = Session::new(session_config.clone(), cache.clone());

            // Create mixer (software volume)
            let mixer_config = MixerConfig::default();
            let mixer_fn = mixer::find(None).context("no mixer found")?;
            let soft_mixer = mixer_fn(mixer_config).context("failed to create software mixer")?;
            let volume_getter = soft_mixer.get_soft_volume();

            // Load audio quality settings from config
            let cfg = encore_common::config::EncoreConfigFile::load(std::path::Path::new(
                "/lsync/encore/config.toml",
            ))
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
            )
            .await
            .context("Spotify Spirc creation failed")?;

            // Spawn the Spirc background task
            let spirc_handle = tokio::spawn(spirc_task);

            info!(
                "Spotify: connected and ready — device '{}' active",
                self.device_name
            );

            // Mutable playback state for broadcasting
            let mut play_state = SpotifyPlayState::new();
            let ws_tx = self.ws_tx.clone();

            // Per-session state. `exit` = user-disable/shutdown (return from run).
            // `handoff` carries credentials when another phone claims the running
            // device; `discovery_dead` flags that the mDNS backend died mid-session.
            let mut suspended = false;
            let mut exit = false;
            let mut handoff = None;
            let mut discovery_dead = false;
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
                    // Hand-off: another phone claimed this still-running device. Tear the
                    // current session down (below) and rebuild with the new credentials.
                    claim = discovery.as_mut().unwrap().next() => {
                        match claim {
                            Some(creds) => {
                                info!("Spotify: new client claimed the device — handing off");
                                handoff = Some(creds);
                                break;
                            }
                            None => {
                                tracing::warn!("Spotify: discovery stream ended during session");
                                discovery_dead = true;
                                break;
                            }
                        }
                    }
                    Ok(()) = suspend_rx.changed(), if has_suspend => {
                        let suspend = *suspend_rx.borrow_and_update();
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
                            SpotifyAction::SetEnabled { enabled } => {
                                if !enabled {
                                    info!("Spotify: disabled by user");
                                    exit = true;
                                    break;
                                }
                            }
                        }
                    }
                    _ = ctx.shutdown.recv() => {
                        info!("Spotify: shutdown");
                        exit = true;
                        break;
                    }
                }
            }

            let _ = spirc.shutdown();
            spirc_handle.abort();
            self.slot.set_active(false);
            self.slot.clear();

            if exit {
                info!("Spotify: stopped");
                return Ok(());
            }
            // The Discovery listener (port 48144) stays alive across the session loop,
            // so the device never disappears from the picker. If its backend died
            // mid-session, force a relaunch; then hand off to the new claimant if any,
            // otherwise loop back and keep listening for the next connection.
            if discovery_dead {
                discovery = None;
            }
            pending_creds = handoff;
            if pending_creds.is_none() {
                info!("Spotify: session ended — still listening for the next connection");
            }
        } // session loop
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
                UniqueFields::Track { artists, .. } => artists
                    .0
                    .iter()
                    .map(|a| a.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", "),
                UniqueFields::Episode { show_name, .. } => show_name.clone(),
                _ => String::new(),
            };
            let album = match &audio_item.unique_fields {
                UniqueFields::Track { album, .. } => album.clone(),
                _ => String::new(),
            };
            let cover_url = audio_item
                .covers
                .first()
                .map(|c| c.url.clone())
                .unwrap_or_default();
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
            info!(
                "Spotify: track changed — {} by {}",
                track.title, track.artist
            );
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
                    let _ = tx.try_send(crate::group::GroupCmd::LocalAudioStarted {
                        source: "spotify".into(),
                    });
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
                let _ = tx.try_send(crate::group::GroupCmd::LocalAudioStopped {
                    source: "spotify".into(),
                });
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
                let _ = tx.try_send(crate::group::GroupCmd::LocalAudioStopped {
                    source: "spotify".into(),
                });
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
        PlayerEvent::SessionClientChanged {
            client_name,
            client_brand_name,
            client_model_name,
            ..
        } => {
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
    /// Final input frame of the previous chunk (one frame = `channels` samples),
    /// prepended to the next chunk so linear interpolation spans the boundary
    /// continuously. Empty before the first chunk and after a reset.
    hist: Vec<i32>,
}

impl Resampler {
    fn new(from_rate: u32, to_rate: u32, channels: u32) -> Self {
        Self {
            step: from_rate as f64 / to_rate as f64,
            channels: channels as usize,
            phase: 0.0,
            hist: Vec::new(),
        }
    }

    /// Reset to a clean stream start (called on playback start / after a flush):
    /// drop the carried phase and boundary-history frame so the next chunk begins
    /// fresh with no stale sample bleeding across the discontinuity.
    fn reset(&mut self) {
        self.phase = 0.0;
        self.hist.clear();
    }

    /// Resample a chunk of interleaved S32 samples. Returns the resampled output.
    ///
    /// The previous chunk's final frame is prepended (via `hist`) so interpolation
    /// spans the chunk boundary continuously. Without it, the output at the seam
    /// between two chunks is skipped — dropping ~1 sample and slipping phase by one
    /// input frame per chunk (a `1/chunk_size` pitch error plus a click). With the
    /// carry the result is identical whether the stream arrives in one chunk or many.
    fn process(&mut self, input: &[i32]) -> Vec<i32> {
        let ch = self.channels;
        let in_frames = input.len() / ch;
        if in_frames == 0 {
            return Vec::new();
        }

        // Virtual buffer = carried boundary frame (if any) then this input, so index
        // 0 is the previous chunk's last frame and interpolation is seamless.
        let mut combined: Vec<i32> = Vec::with_capacity(self.hist.len() + input.len());
        combined.extend_from_slice(&self.hist);
        combined.extend_from_slice(input);
        let n = combined.len() / ch;

        // Output has more frames than input (upsampling).
        let est_out = ((n as f64 / self.step) as usize) + 2;
        let mut output = Vec::with_capacity(est_out * ch);

        let mut pos = self.phase;
        while pos < (n - 1) as f64 {
            let idx = pos as usize;
            let frac = pos - idx as f64;
            for c in 0..ch {
                let a = combined[idx * ch + c] as f64;
                let b = combined[(idx + 1) * ch + c] as f64;
                output.push((a + (b - a) * frac) as i32);
            }
            pos += self.step;
        }

        // Carry the fractional position and this chunk's final frame forward.
        self.phase = pos - (n - 1) as f64;
        self.hist.clear();
        self.hist
            .extend_from_slice(&input[(in_frames - 1) * ch..in_frames * ch]);
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
        self.resampler.reset();
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

#[cfg(test)]
mod resampler_tests {
    use super::*;

    #[test]
    fn resampling_is_chunk_invariant() {
        // The same stream must resample identically whether delivered whole or
        // split into many chunks — i.e. no sample is dropped or phase slipped at a
        // chunk seam. (The old code dropped ~1 sample per boundary.)
        let frames = 4096;
        let mut input = Vec::with_capacity(frames * 2);
        for i in 0..frames {
            let s = ((i as f64 * 0.07).sin() * 1.0e8) as i32;
            input.push(s); // L
            input.push(s / 2); // R (distinct channel)
        }

        let mut whole_r = Resampler::new(44100, 48000, 2);
        let whole = whole_r.process(&input);

        let mut split_r = Resampler::new(44100, 48000, 2);
        let mut split = Vec::new();
        for chunk in input.chunks(2 * 137) {
            // 137 frames per chunk
            split.extend(split_r.process(chunk));
        }

        assert!(
            (whole.len() as i64 - split.len() as i64).abs() <= 2,
            "chunking changed output length: whole {} vs split {}",
            whole.len(),
            split.len()
        );
        let n = whole.len().min(split.len());
        let max_diff = (0..n)
            .map(|i| (whole[i] as i64 - split[i] as i64).abs())
            .max()
            .unwrap_or(0);
        assert!(
            max_diff <= 2,
            "chunking changed samples (max diff {max_diff})"
        );
    }

    #[test]
    fn resampling_preserves_a_linear_ramp() {
        // Linear interpolation is exact for a linear input: resampling a ramp must
        // yield a ramp whose per-output-frame step equals slope * (in_rate/out_rate),
        // with no kinks. Guards that the rewrite still resamples correctly.
        let ch = 2usize;
        let frames = 2000;
        let mut input = Vec::with_capacity(frames * ch);
        for i in 0..frames {
            input.push((i as i32) * 1000); // L ramp
            input.push((i as i32) * 1000); // R ramp
        }
        let mut r = Resampler::new(44100, 48000, ch as u32);
        let out = r.process(&input);
        let expected_step = 1000.0 * 44100.0 / 48000.0;
        let of = out.len() / ch;
        for i in 1..of - 1 {
            let d = (out[i * ch] - out[(i - 1) * ch]) as f64;
            assert!(
                (d - expected_step).abs() < 2.0,
                "ramp not linear at out frame {i}: step {d}, expected {expected_step}"
            );
        }
    }
}
