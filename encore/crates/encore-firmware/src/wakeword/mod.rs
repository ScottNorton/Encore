//! Wake word detection framework and voice session state machine.
//!
//! Provides a pluggable `WakeWordDetector` trait fed by the capture pipeline's
//! 16kHz mono S16 recognition stream. The `VoiceSession` manages the lifecycle
//! from wake word detection through Wyoming pipeline completion.
//!
//! Currently ships with `StubDetector` (never triggers). A real engine
//! (tract + openWakeWord ONNX) drops in by implementing the trait.

pub mod detector;

#[cfg(target_os = "linux")]
use crate::audio::capture::CaptureConsumer;
#[cfg(target_os = "linux")]
use crate::led;
#[cfg(target_os = "linux")]
use tokio::sync::mpsc;
#[cfg(target_os = "linux")]
use tracing::{debug, info, warn};

/// A wake word detection result.
pub struct Detection {
    /// Wake word name (e.g. "hey_invoke").
    pub name: String,
    /// How many samples ago the wake word started (for buffer rewind).
    pub samples_before: usize,
}

/// Pluggable wake word detector. Feed it 16kHz mono S16 audio.
pub trait WakeWordDetector: Send {
    /// Feed audio samples. Returns `Some(Detection)` when a wake word is found.
    fn feed(&mut self, samples: &[i16]) -> Option<Detection>;
    /// Reset internal state (e.g. after a session completes).
    fn reset(&mut self);
}

/// Voice session states for the invocation lifecycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VoiceState {
    /// Listening for wake word, all audio flows normally.
    Idle,
    /// Wake word detected, audio suppressed from comms, routing to Wyoming.
    Active,
    /// Wyoming TTS playing response, waiting for completion or follow-up.
    Responding,
}

/// Events emitted by the voice session.
#[derive(Debug)]
pub enum VoiceEvent {
    /// Wake word detected.
    WakeDetected(String),
    /// Voice pipeline session started.
    SessionStart,
    /// Voice pipeline session ended.
    SessionEnd,
}

/// Commands into the voice session (from Wyoming protocol events).
#[derive(Debug)]
pub enum VoiceCmd {
    /// Wyoming pipeline started (detection event received from HA).
    PipelineStarted,
    /// HA is playing TTS.
    TtsStarted,
    /// HA finished TTS.
    TtsDone,
    /// Pipeline complete — resume normal audio flow.
    PipelineComplete,
    /// Error / timeout.
    Abort,
}

/// Voice session task — runs wake word detection and manages session state.
///
/// Receives audio from the capture pipeline, feeds the wake word detector,
/// and coordinates with Wyoming via command/event channels.
#[cfg(target_os = "linux")]
pub async fn voice_session_task(
    mut mic_rx: CaptureConsumer,
    mut cmd_rx: mpsc::Receiver<VoiceCmd>,
    event_tx: mpsc::Sender<VoiceEvent>,
    led_tx: Option<mpsc::Sender<led::LedCmd>>,
    mut detector: Box<dyn WakeWordDetector>,
) {
    let mut state = VoiceState::Idle;
    let timeout_duration = tokio::time::Duration::from_secs(30);

    info!("VoiceSession: started (state=Idle)");

    loop {
        match state {
            VoiceState::Idle => {
                // Listen for wake word from mic audio
                tokio::select! {
                    samples = mic_rx.rx.recv() => {
                        match samples {
                            Some(chunk) => {
                                if let Some(detection) = detector.feed(&chunk) {
                                    info!("VoiceSession: wake word '{}' detected", detection.name);
                                    state = VoiceState::Active;
                                    let _ = event_tx.try_send(VoiceEvent::WakeDetected(detection.name));
                                    let _ = event_tx.try_send(VoiceEvent::SessionStart);

                                    if let Some(ref tx) = led_tx {
                                        let _ = tx.try_send(led::LedCmd::PlayBin {
                                            name: "L_101_c_listening".into(),
                                            repeat: true,
                                        });
                                    }
                                }
                            }
                            None => {
                                info!("VoiceSession: mic channel closed, exiting");
                                return;
                            }
                        }
                    }
                    cmd = cmd_rx.recv() => {
                        match cmd {
                            Some(VoiceCmd::PipelineStarted) => {
                                debug!("VoiceSession: pipeline started while idle (external trigger)");
                                state = VoiceState::Active;
                                let _ = event_tx.try_send(VoiceEvent::SessionStart);
                            }
                            None => {
                                info!("VoiceSession: cmd channel closed, exiting");
                                return;
                            }
                            _ => {}
                        }
                    }
                }
            }

            VoiceState::Active => {
                // Session active — wait for TTS or timeout
                tokio::select! {
                    cmd = cmd_rx.recv() => {
                        match cmd {
                            Some(VoiceCmd::TtsStarted) => {
                                debug!("VoiceSession: TTS started");
                                state = VoiceState::Responding;
                            }
                            Some(VoiceCmd::PipelineComplete) | Some(VoiceCmd::Abort) => {
                                info!("VoiceSession: session ended (active)");
                                state = VoiceState::Idle;
                                detector.reset();
                                let _ = event_tx.try_send(VoiceEvent::SessionEnd);
                            }
                            None => return,
                            _ => {}
                        }
                    }
                    // Drain mic samples during active state (keeps capture flowing)
                    _ = mic_rx.rx.recv() => {}
                    _ = tokio::time::sleep(timeout_duration) => {
                        warn!("VoiceSession: timeout in Active state, aborting");
                        state = VoiceState::Idle;
                        detector.reset();
                        let _ = event_tx.try_send(VoiceEvent::SessionEnd);
                    }
                }
            }

            VoiceState::Responding => {
                // TTS playing — wait for completion or timeout
                tokio::select! {
                    cmd = cmd_rx.recv() => {
                        match cmd {
                            Some(VoiceCmd::TtsDone) | Some(VoiceCmd::PipelineComplete) => {
                                info!("VoiceSession: session complete");
                                state = VoiceState::Idle;
                                detector.reset();
                                let _ = event_tx.try_send(VoiceEvent::SessionEnd);
                            }
                            Some(VoiceCmd::Abort) => {
                                warn!("VoiceSession: aborted during TTS");
                                state = VoiceState::Idle;
                                detector.reset();
                                let _ = event_tx.try_send(VoiceEvent::SessionEnd);
                            }
                            None => return,
                            _ => {}
                        }
                    }
                    // Drain mic samples during response
                    _ = mic_rx.rx.recv() => {}
                    _ = tokio::time::sleep(timeout_duration) => {
                        warn!("VoiceSession: timeout in Responding state, aborting");
                        state = VoiceState::Idle;
                        detector.reset();
                        let _ = event_tx.try_send(VoiceEvent::SessionEnd);
                    }
                }
            }
        }
    }
}
