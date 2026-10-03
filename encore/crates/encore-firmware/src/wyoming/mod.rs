//! Wyoming voice satellite subsystem.
//!
//! TCP server on port 10700. Home Assistant connects and routes
//! audio through its voice pipeline (openwakeword -> whisper ->
//! LLM -> piper). Mic audio comes from the native ALSA capture
//! pipeline (16kHz mono S16) and is streamed as `audio-chunk` events.
//! TTS responses play via MixerSlot.

pub mod protocol;

#[cfg(target_os = "linux")]
use crate::audio::capture::CaptureConsumer;
#[cfg(target_os = "linux")]
use crate::audio::mixer::MixerSlot;
#[cfg(target_os = "linux")]
use crate::audio::resample::resample_linear;
#[cfg(target_os = "linux")]
use crate::subsystem::{Subsystem, SubsystemContext};
#[cfg(target_os = "linux")]
use crate::wakeword::VoiceCmd;
#[cfg(target_os = "linux")]
use anyhow::{Context, Result};
#[cfg(target_os = "linux")]
use encore_common::protocol::SubsystemState;
#[cfg(target_os = "linux")]
use protocol::{read_event, write_event, WyomingEvent};
#[cfg(target_os = "linux")]
use serde_json::json;
#[cfg(target_os = "linux")]
use std::sync::Arc;
#[cfg(target_os = "linux")]
use tokio::io::BufReader;
#[cfg(target_os = "linux")]
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
#[cfg(target_os = "linux")]
use tracing::{debug, info, warn};

#[cfg(target_os = "linux")]
const DEFAULT_PORT: u16 = 10700;
#[cfg(target_os = "linux")]
const DEFAULT_NAME: &str = "Invoke";

// Mic capture settings (from capture pipeline — 16kHz mono S16)
#[cfg(target_os = "linux")]
const MIC_RATE: u32 = 16000;
#[cfg(target_os = "linux")]
const MIC_WIDTH: u16 = 2; // S16_LE
#[cfg(target_os = "linux")]
const MIC_CHANNELS: u16 = 1;

// Mixer output rate (must match audio subsystem)
#[cfg(target_os = "linux")]
const MIXER_RATE: u32 = 48000;

/// Wyoming voice satellite subsystem.
#[cfg(target_os = "linux")]
pub struct WyomingSubsystem {
    port: u16,
    name: String,
    slot: Arc<MixerSlot>,
    mic_rx: Option<CaptureConsumer>,
    voice_cmd_tx: Option<tokio::sync::mpsc::Sender<VoiceCmd>>,
    led_tx: Option<tokio::sync::mpsc::Sender<crate::led::LedCmd>>,
    trigger_rx: Option<tokio::sync::mpsc::Receiver<()>>,
    suspend_rx: Option<tokio::sync::watch::Receiver<bool>>,
}

#[cfg(target_os = "linux")]
impl WyomingSubsystem {
    pub fn new(
        slot: Arc<MixerSlot>,
        mic_rx: CaptureConsumer,
        voice_cmd_tx: Option<tokio::sync::mpsc::Sender<VoiceCmd>>,
        port: Option<u16>,
        name: Option<String>,
        led_tx: Option<tokio::sync::mpsc::Sender<crate::led::LedCmd>>,
        trigger_rx: Option<tokio::sync::mpsc::Receiver<()>>,
    ) -> Self {
        Self {
            port: port.unwrap_or(DEFAULT_PORT),
            name: name.unwrap_or_else(|| DEFAULT_NAME.to_string()),
            slot,
            mic_rx: Some(mic_rx),
            voice_cmd_tx,
            led_tx,
            trigger_rx,
            suspend_rx: None,
        }
    }

    /// Set the suspend channel for group sync (follower mode pauses Wyoming).
    pub fn set_suspend_rx(&mut self, rx: tokio::sync::watch::Receiver<bool>) {
        self.suspend_rx = Some(rx);
    }
}

/// Enable TCP keepalive on an accepted satellite connection so a silently dead
/// peer (half-open: the HA host vanished with no FIN/RST — e.g. the satellite's
/// link flips from USB-RNDIS to Wi-Fi) is detected by the kernel within ~35s.
/// Without this the single-session accept loop blocks forever in `read_event` on
/// the dead socket and never accepts HA's reconnect — the socket sits LISTEN with
/// a stuck backlog while HA retries every 10s in vain.
#[cfg(target_os = "linux")]
fn set_tcp_keepalive(stream: &tokio::net::TcpStream) {
    use std::os::unix::io::AsRawFd;
    let fd = stream.as_raw_fd();
    let on: libc::c_int = 1;
    let idle: libc::c_int = 20; // begin probing after 20s idle
    let intvl: libc::c_int = 5; // probe every 5s
    let cnt: libc::c_int = 3; // drop after 3 missed probes (~35s total)
    let set = |level: libc::c_int, name: libc::c_int, val: &libc::c_int| unsafe {
        libc::setsockopt(
            fd,
            level,
            name,
            val as *const libc::c_int as *const libc::c_void,
            std::mem::size_of::<libc::c_int>() as libc::socklen_t,
        )
    };
    set(libc::SOL_SOCKET, libc::SO_KEEPALIVE, &on);
    set(libc::IPPROTO_TCP, libc::TCP_KEEPIDLE, &idle);
    set(libc::IPPROTO_TCP, libc::TCP_KEEPINTVL, &intvl);
    set(libc::IPPROTO_TCP, libc::TCP_KEEPCNT, &cnt);
}

#[cfg(target_os = "linux")]
#[async_trait::async_trait]
impl Subsystem for WyomingSubsystem {
    fn name(&self) -> &'static str {
        "wyoming"
    }

    async fn run(&mut self, mut ctx: SubsystemContext) -> Result<()> {
        ctx.health.set_state(SubsystemState::Running);

        let socket = tokio::net::TcpSocket::new_v4().context("create Wyoming socket")?;
        socket.set_reuseaddr(true).ok();
        socket
            .bind(std::net::SocketAddr::from(([0, 0, 0, 0], self.port)))
            .context("bind Wyoming TCP")?;
        let listener = socket.listen(128).context("listen Wyoming TCP")?;

        info!("Wyoming: listening on port {}", self.port);
        let has_suspend = self.suspend_rx.is_some();
        let (_suspend_dummy_tx, suspend_dummy_rx) = tokio::sync::watch::channel(false);
        let mut suspend_rx = self.suspend_rx.take().unwrap_or(suspend_dummy_rx);
        let mut suspended = false;

        loop {
            tokio::select! {
                accept = listener.accept(), if !suspended => {
                    match accept {
                        Ok((stream, addr)) => {
                            info!("Wyoming: client connected from {}", addr);
                            // Detect a half-open HA connection in ~35s instead of never.
                            set_tcp_keepalive(&stream);
                            let (read_half, write_half) = stream.into_split();
                            let mut reader = BufReader::new(read_half);

                            let trigger_rx = self.trigger_rx.take();
                            let mic_rx = self.mic_rx.take();
                            let mut session = Session::new(
                                &self.name, self.slot.clone(),
                                mic_rx, self.voice_cmd_tx.clone(),
                                self.led_tx.clone(), trigger_rx,
                            );
                            match session.run(&mut reader, write_half, &ctx).await {
                                Ok(()) => info!("Wyoming: session ended normally"),
                                Err(e) => warn!("Wyoming: session error: {}", e),
                            }
                            // Recover owned resources for next session
                            self.trigger_rx = session.trigger_rx.take();
                            self.mic_rx = session.mic_rx.take();
                            ctx.health.inc_msg();
                        }
                        Err(e) => {
                            warn!("Wyoming: accept error: {}", e);
                        }
                    }
                }
                Ok(()) = suspend_rx.changed(), if has_suspend => {
                    let suspend = *suspend_rx.borrow_and_update();
                    if suspend && !suspended {
                        info!("Wyoming: suspended by group (follower mode)");
                        suspended = true;
                        self.slot.set_active(false);
                    } else if !suspend && suspended {
                        info!("Wyoming: resumed by group");
                        suspended = false;
                        // No set_active(true): the voice slot is session-scoped
                        // (activated with TTS data per session), unlike the
                        // persistent BT/Spotify stream slots.
                    }
                }
                _ = ctx.shutdown.recv() => {
                    info!("Wyoming: shutdown");
                    break;
                }
            }
        }

        Ok(())
    }
}

/// A single HA client session through the full voice cycle.
///
/// Lifecycle:
///   1. Handshake: describe -> info, wait for run-satellite
///   2. Streaming: mic audio -> HA, HA events <- handled
///   3. Cleanup: stop playback
#[cfg(target_os = "linux")]
struct Session {
    name: String,
    slot: Arc<MixerSlot>,
    mic_rx: Option<CaptureConsumer>,
    voice_cmd_tx: Option<tokio::sync::mpsc::Sender<VoiceCmd>>,
    led_tx: Option<tokio::sync::mpsc::Sender<crate::led::LedCmd>>,
    trigger_rx: Option<tokio::sync::mpsc::Receiver<()>>,
    tts_rate: u32,
    muted: bool,
    streaming: bool,
}

#[cfg(target_os = "linux")]
impl Session {
    fn new(
        name: &str,
        slot: Arc<MixerSlot>,
        mic_rx: Option<CaptureConsumer>,
        voice_cmd_tx: Option<tokio::sync::mpsc::Sender<VoiceCmd>>,
        led_tx: Option<tokio::sync::mpsc::Sender<crate::led::LedCmd>>,
        trigger_rx: Option<tokio::sync::mpsc::Receiver<()>>,
    ) -> Self {
        Self {
            name: name.to_string(),
            slot,
            mic_rx,
            voice_cmd_tx,
            led_tx,
            trigger_rx,
            tts_rate: 22050,
            muted: false,
            streaming: false,
        }
    }

    fn send_led(&self, cmd: crate::led::LedCmd) {
        if let Some(ref tx) = self.led_tx {
            let _ = tx.try_send(cmd);
        }
    }

    async fn run(
        &mut self,
        reader: &mut BufReader<OwnedReadHalf>,
        mut writer: OwnedWriteHalf,
        ctx: &SubsystemContext,
    ) -> Result<()> {
        // Phase 1: Handshake
        if !self.handshake(reader, &mut writer).await? {
            return Ok(());
        }

        // Phase 2: Streaming loop
        let result = self.streaming_loop(reader, &mut writer, ctx).await;

        // Phase 3: Cleanup — always runs, even if the streaming loop errored, so the
        // mixer slot / LED / voice-cmd state never leaks into the next session.
        self.cleanup();

        result
    }

    // ── Handshake ──

    async fn handshake(
        &self,
        reader: &mut BufReader<OwnedReadHalf>,
        writer: &mut OwnedWriteHalf,
    ) -> Result<bool> {
        loop {
            let event = match read_event(reader).await? {
                Some(e) => e,
                None => return Ok(false),
            };

            match event.event_type.as_str() {
                "describe" => {
                    self.send_info(writer).await?;
                }
                "run-satellite" => {
                    info!("Wyoming: run-satellite received");
                    return Ok(true);
                }
                "ping" => {
                    write_event(writer, &WyomingEvent::new("pong", event.data)).await?;
                }
                other => {
                    debug!("Wyoming: handshake ignoring {}", other);
                }
            }
        }
    }

    /// Respond with satellite capabilities.
    ///
    /// Empty wake/asr/tts lists tell HA to use its own pipeline
    /// services for all processing including wake word detection.
    async fn send_info(&self, writer: &mut OwnedWriteHalf) -> Result<()> {
        let info = WyomingEvent::new(
            "info",
            json!({
                "satellite": {
                    "name": self.name,
                    // HA's wyoming `Satellite` dataclass requires these (verified via
                    // Info.from_event introspection + a parse test): without
                    // attribution/installed, Info.from_event raises and the integration
                    // setup fails ("missing required arguments 'attribution' and 'installed'").
                    "attribution": { "name": "Encore", "url": "https://github.com/ScottNorton/Encore" },
                    "installed": true,
                    "description": null,
                    "version": null,
                    "area": "",
                    "has_vad": false,
                    "active_wake_words": null,
                    "max_active_wake_words": null,
                    "supports_trigger": false,
                },
                "asr": [],
                "tts": [],
                "handle": [],
                "intent": [],
                "wake": [],
            }),
        );
        write_event(writer, &info).await?;
        info!("Wyoming: sent satellite info (name={})", self.name);
        Ok(())
    }

    /// Send a VoiceCmd to the voice session (if connected).
    fn send_voice_cmd(&self, cmd: VoiceCmd) {
        if let Some(ref tx) = self.voice_cmd_tx {
            let _ = tx.try_send(cmd);
        }
    }

    // ── Streaming ──

    async fn streaming_loop(
        &mut self,
        reader: &mut BufReader<OwnedReadHalf>,
        writer: &mut OwnedWriteHalf,
        ctx: &SubsystemContext,
    ) -> Result<()> {
        self.streaming = true;

        info!("Wyoming: streaming started (capture pipeline)");

        // Extract owned resources to avoid borrow conflict with self in tokio::select!
        let mut trigger_rx = self.trigger_rx.take();
        let mut mic_rx = self.mic_rx.take();

        while self.streaming {
            tokio::select! {
                mic_data = async {
                    if let Some(ref mut consumer) = mic_rx {
                        consumer.rx.recv().await
                    } else {
                        std::future::pending().await
                    }
                } => {
                    match mic_data {
                        Some(samples) if !self.muted => {
                            // Convert i16 samples to raw bytes for Wyoming audio-chunk
                            let bytes: Vec<u8> = samples.iter()
                                .flat_map(|s| s.to_le_bytes())
                                .collect();
                            let event = WyomingEvent::with_payload(
                                "audio-chunk",
                                json!({
                                    "rate": MIC_RATE,
                                    "width": MIC_WIDTH,
                                    "channels": MIC_CHANNELS,
                                }),
                                bytes,
                            );
                            if write_event(writer, &event).await.is_err() {
                                warn!("Wyoming: write failed, disconnecting");
                                break;
                            }
                        }
                        Some(_) => {} // muted, discard
                        None => {
                            warn!("Wyoming: mic capture channel closed");
                            break;
                        }
                    }
                }
                event = read_event(reader) => {
                    match event {
                        Ok(Some(ev)) => {
                            // Never `?` out here: that would skip the mic_rx/trigger_rx
                            // restore below and leave the satellite permanently deaf on
                            // every future reconnect. Log and break so cleanup + restore
                            // run and the accept loop can take a fresh connection.
                            if let Err(e) = self.handle_event(ev, writer, ctx).await {
                                warn!("Wyoming: handle_event error: {}, disconnecting", e);
                                break;
                            }
                        }
                        Ok(None) => {
                            info!("Wyoming: client disconnected");
                            break;
                        }
                        Err(e) => {
                            warn!("Wyoming: read error: {}", e);
                            break;
                        }
                    }
                }
                trigger = async {
                    if let Some(ref mut rx) = trigger_rx {
                        rx.recv().await
                    } else {
                        std::future::pending().await
                    }
                } => {
                    if trigger.is_some() {
                        info!("Wyoming: PTT trigger received, sending run-pipeline");
                        let pipeline = WyomingEvent::new("run-pipeline", json!({
                            "start_stage": "wake",
                            "end_stage": "tts",
                        }));
                        write_event(writer, &pipeline).await.ok();
                    }
                }
            }
        }

        // Restore owned resources for reuse across sessions
        self.trigger_rx = trigger_rx;
        self.mic_rx = mic_rx;

        Ok(())
    }

    // ── Event Handling ──

    async fn handle_event(
        &mut self,
        event: WyomingEvent,
        writer: &mut OwnedWriteHalf,
        ctx: &SubsystemContext,
    ) -> Result<()> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        match event.event_type.as_str() {
            "detection" => {
                let name = event
                    .data
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("?");
                info!("Wyoming: wake word '{}' detected", name);
                ctx.health.beat(now);
                ctx.health.inc_msg();
                self.send_voice_cmd(VoiceCmd::PipelineStarted);
                self.send_led(crate::led::LedCmd::PlayBin {
                    name: "L_101_c_listening".into(),
                    repeat: true,
                });
            }

            "voice-started" => {
                debug!("Wyoming: voice started");
            }

            "voice-stopped" => {
                debug!("Wyoming: voice stopped");
                self.send_led(crate::led::LedCmd::PlayBin {
                    name: "L_104_c_thinking".into(),
                    repeat: true,
                });
            }

            "transcript" => {
                let text = event
                    .data
                    .get("text")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                info!("Wyoming: STT \"{}\"", text);
            }

            "audio-start" => {
                let rate = event
                    .data
                    .get("rate")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(22050) as u32;
                let width = event
                    .data
                    .get("width")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(2) as u16;
                let channels = event
                    .data
                    .get("channels")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(1) as u16;

                info!(
                    "Wyoming: TTS start {}Hz {}bit {}ch",
                    rate,
                    width * 8,
                    channels
                );
                self.muted = true;
                self.start_playback(rate);
                self.send_voice_cmd(VoiceCmd::TtsStarted);
                self.send_led(crate::led::LedCmd::PlayBin {
                    name: "L_105_c_cortanaspeaking".into(),
                    repeat: true,
                });
            }

            "audio-chunk" => {
                if !event.payload.is_empty() {
                    self.write_playback(&event.payload);
                }
            }

            "audio-stop" => {
                info!("Wyoming: TTS done");
                self.stop_playback();
                self.muted = false;
                self.send_voice_cmd(VoiceCmd::TtsDone);
                write_event(writer, &WyomingEvent::new("played", json!({})))
                    .await
                    .ok();
                self.send_led(crate::led::LedCmd::Animate(
                    encore_common::protocol::LedAnimation::Off,
                ));
            }

            "run-pipeline" => {
                let start = event
                    .data
                    .get("start_stage")
                    .and_then(|v| v.as_str())
                    .unwrap_or("?");
                let end = event
                    .data
                    .get("end_stage")
                    .and_then(|v| v.as_str())
                    .unwrap_or("?");
                debug!("Wyoming: pipeline {} -> {}", start, end);
            }

            "ping" => {
                write_event(writer, &WyomingEvent::new("pong", event.data)).await?;
            }

            "pause-satellite" => {
                info!("Wyoming: paused");
                self.streaming = false;
            }

            "run-satellite" => {
                debug!("Wyoming: re-run (already streaming)");
            }

            "describe" => {
                self.send_info(writer).await?;
            }

            "error" => {
                let text = event
                    .data
                    .get("text")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                warn!("Wyoming: HA error: {}", text);
                self.muted = false;
                self.send_voice_cmd(VoiceCmd::Abort);
                // Play the dedicated error animation once (stock state "voice:error"),
                // then playback ends and the ring falls back to the idle animation.
                self.send_led(crate::led::LedCmd::PlayBin {
                    name: "L_108_c_error".into(),
                    repeat: false,
                });
            }

            other => {
                debug!("Wyoming: event {}", other);
            }
        }

        Ok(())
    }

    // ── TTS Playback via MixerSlot ──

    /// Prepare MixerSlot for TTS output.
    fn start_playback(&mut self, rate: u32) {
        self.tts_rate = rate;
        self.slot.clear();
        self.slot.set_active(true);
        debug!(
            "Wyoming: playback started ({}Hz → {}Hz via MixerSlot)",
            rate, MIXER_RATE
        );
    }

    /// Write TTS PCM data to MixerSlot. Mono S16_LE input is resampled to
    /// 48kHz and interleaved to stereo.
    fn write_playback(&self, data: &[u8]) {
        // Parse raw bytes as mono S16_LE samples
        let mono: Vec<i16> = data
            .chunks_exact(2)
            .map(|c| i16::from_le_bytes([c[0], c[1]]))
            .collect();

        // Resample to mixer rate
        let resampled = resample_linear(&mono, self.tts_rate, MIXER_RATE);

        // Mono → stereo interleave, upscale i16 → i32 for 32-bit pipeline
        let mut stereo: Vec<i32> = Vec::with_capacity(resampled.len() * 2);
        for &s in &resampled {
            let s32 = (s as i32) << 16;
            stereo.push(s32);
            stereo.push(s32);
        }

        let written = self.slot.push(&stereo);
        if written < stereo.len() {
            debug!(
                "Wyoming: ring full, dropped {} samples",
                stereo.len() - written
            );
        }
    }

    /// Stop TTS playback.
    fn stop_playback(&self) {
        self.slot.set_active(false);
    }

    // ── Cleanup ──

    fn cleanup(&mut self) {
        self.streaming = false;
        self.stop_playback();
        self.send_voice_cmd(VoiceCmd::PipelineComplete);
        self.send_led(crate::led::LedCmd::Animate(
            encore_common::protocol::LedAnimation::Off,
        ));
    }
}
