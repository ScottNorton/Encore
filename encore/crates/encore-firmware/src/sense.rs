//! Acoustic sensing subsystem.
//!
//! Taps a `Left` capture consumer and runs a pure [`SenseDetector`]
//! (EWMA noise floor → activity state machine + transient/loud detection)
//! **only when playback is idle**. Emits [`SenseUpdate`] over an mpsc channel
//! to the Home Assistant bridge, which publishes `encore/<device>/sense/*`,
//! and optionally reflects activity on the LED ring.

use crate::audio::capture::CaptureConsumer;
use crate::led::LedCmd;
use crate::subsystem::{Subsystem, SubsystemContext};
use anyhow::Result;
use encore_common::config::SenseConfig;
use encore_common::protocol::{LedAnimation, SubsystemState};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::sync::mpsc;

// ── Pure detector ────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SenseState {
    Quiet,
    Activity,
}

#[derive(Debug, Clone)]
pub enum SenseEvent {
    StateChanged(SenseState),
    LoudEvent { level: f32 },
}

#[derive(Debug, Clone)]
pub struct DetectorCfg {
    pub activity_db: f32,
    pub attack_frames: u32,
    pub quiet_frames: u32,
    pub loud_ratio: f32,
}

pub struct SenseDetector {
    cfg: DetectorCfg,
    floor: f32, // EWMA of RMS (linear)
    over: u32,
    quiet: u32,
    state: SenseState,
    primed: bool,
}

impl SenseDetector {
    pub fn new(cfg: DetectorCfg) -> Self {
        Self {
            cfg,
            floor: 1e-4,
            over: 0,
            quiet: 0,
            state: SenseState::Quiet,
            primed: false,
        }
    }

    /// Feed one chunk's RMS + peak (both linear, 0..1). Returns an event on change.
    pub fn feed(&mut self, rms: f32, peak: f32) -> Option<SenseEvent> {
        // Loud = an impulsive transient: peak well above THIS chunk's RMS (a high crest
        // factor), not merely a sustained signal above the floor — otherwise any activity
        // trips it every frame and masks the Activity transition. Crest separates
        // glass/slam/clap from sustained speech and music.
        let loud = self.primed && peak > rms.max(1e-5) * self.cfg.loud_ratio;

        // Adaptive floor: fast-track down, slow up (so the floor tracks quiet, not bursts).
        let a = if rms < self.floor { 0.2 } else { 0.02 };
        self.floor = self.floor + a * (rms - self.floor);
        self.primed = true;

        let thresh = self.floor.max(1e-5) * 10f32.powf(self.cfg.activity_db / 20.0);
        let active = rms > thresh;
        if active {
            self.over += 1;
            self.quiet = 0;
        } else {
            self.quiet += 1;
            self.over = 0;
        }

        let mut ev = None;
        match self.state {
            SenseState::Quiet if self.over >= self.cfg.attack_frames => {
                self.state = SenseState::Activity;
                ev = Some(SenseEvent::StateChanged(SenseState::Activity));
            }
            SenseState::Activity if self.quiet >= self.cfg.quiet_frames => {
                self.state = SenseState::Quiet;
                ev = Some(SenseEvent::StateChanged(SenseState::Quiet));
            }
            _ => {}
        }
        // Loud event is independent of the activity state machine; prefer it if both.
        if loud {
            return Some(SenseEvent::LoudEvent { level: peak });
        }
        ev
    }

    pub fn state(&self) -> SenseState {
        self.state
    }
    /// 0-100 level relative to floor (for the retained `level` topic).
    pub fn level(&self, rms: f32) -> u8 {
        let db = 20.0 * (rms.max(1e-6) / self.floor.max(1e-6)).log10();
        (db.clamp(0.0, 40.0) * 2.5) as u8 // 0..40 dB -> 0..100
    }
}

// ── Subsystem (tap + idle gate + detector + emit) ─────────────────────────

#[derive(Debug, Clone)]
pub enum SenseUpdate {
    State { state: SenseState, level: u8 },
    Loud { level: f32 },
}

pub struct SenseSubsystem {
    cfg: SenseConfig,
    mic: Option<CaptureConsumer>,
    playback_active: Arc<AtomicBool>,
    out: mpsc::Sender<SenseUpdate>,
    led_tx: Option<mpsc::Sender<LedCmd>>,
}

impl SenseSubsystem {
    pub fn new(
        cfg: SenseConfig,
        mic: CaptureConsumer,
        playback_active: Arc<AtomicBool>,
        out: mpsc::Sender<SenseUpdate>,
    ) -> Self {
        Self {
            cfg,
            mic: Some(mic),
            playback_active,
            out,
            led_tx: None,
        }
    }
    pub fn set_led_tx(&mut self, tx: mpsc::Sender<LedCmd>) {
        self.led_tx = Some(tx);
    }
}

#[async_trait::async_trait]
impl Subsystem for SenseSubsystem {
    fn name(&self) -> &'static str {
        "sense"
    }
    async fn run(&mut self, mut ctx: SubsystemContext) -> Result<()> {
        ctx.health.set_state(SubsystemState::Running);
        let mut rx = match self.mic.take() {
            Some(c) => c.rx,
            None => return Ok(()),
        };
        let dcfg = DetectorCfg {
            activity_db: self.cfg.activity_db,
            attack_frames: self.cfg.attack_frames,
            // ~170 chunks/s; convert quiet_timeout_s to frames
            quiet_frames: (self.cfg.quiet_timeout_s.saturating_mul(170)).max(1),
            loud_ratio: self.cfg.loud_ratio,
        };
        let mut det = SenseDetector::new(dcfg);
        let tail = std::time::Duration::from_millis(self.cfg.playback_tail_ms as u64);
        let mut last_play = std::time::Instant::now();
        loop {
            tokio::select! {
                maybe = rx.recv() => {
                    let Some(chunk) = maybe else { break };
                    if self.playback_active.load(Ordering::Relaxed) {
                        last_play = std::time::Instant::now();
                        continue;
                    }
                    if last_play.elapsed() < tail { continue; } // ring-out guard
                    let (rms, peak) = rms_peak(&chunk);
                    if let Some(ev) = det.feed(rms, peak) {
                        ctx.health.inc_msg();
                        match ev {
                            SenseEvent::StateChanged(st) => {
                                let _ = self.out.try_send(SenseUpdate::State { state: st, level: det.level(rms) });
                                self.led(st);
                            }
                            SenseEvent::LoudEvent { level } => {
                                let _ = self.out.try_send(SenseUpdate::Loud { level });
                            }
                        }
                    }
                }
                _ = ctx.shutdown.recv() => break,
            }
        }
        Ok(())
    }
}

fn rms_peak(samples: &[i16]) -> (f32, f32) {
    let n = samples.len().max(1) as f64;
    let scale = 1.0 / 32768.0;
    let (mut s, mut p) = (0.0f64, 0.0f64);
    for &v in samples {
        let x = v as f64 * scale;
        s += x * x;
        p = p.max(x.abs());
    }
    ((s / n).sqrt() as f32, p as f32)
}

impl SenseSubsystem {
    fn led(&self, st: SenseState) {
        if !self.cfg.led_feedback {
            return;
        }
        if let Some(ref tx) = self.led_tx {
            let anim = match st {
                SenseState::Activity => LedAnimation::Breathe {
                    r: 0,
                    g: 120,
                    b: 90,
                    period_ms: 2000,
                },
                SenseState::Quiet => LedAnimation::Off,
            };
            let _ = tx.try_send(LedCmd::Animate(anim));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn cfg() -> DetectorCfg {
        DetectorCfg {
            activity_db: 12.0,
            attack_frames: 3,
            quiet_frames: 5,
            loud_ratio: 8.0,
        }
    }
    #[test]
    fn rising_level_triggers_activity_after_attack() {
        let mut d = SenseDetector::new(cfg());
        for _ in 0..50 {
            d.feed(0.001, 0.002);
        } // settle floor low
        let mut got = None;
        for _ in 0..3 {
            got = d.feed(0.05, 0.06).or(got);
        } // ~34 dB over floor
        assert!(matches!(
            got,
            Some(SenseEvent::StateChanged(SenseState::Activity))
        ));
    }
    #[test]
    fn transient_spike_emits_loud_event() {
        let mut d = SenseDetector::new(cfg());
        for _ in 0..50 {
            d.feed(0.001, 0.002);
        }
        let ev = d.feed(0.002, 0.5); // peak 250x floor
        assert!(matches!(ev, Some(SenseEvent::LoudEvent { .. })));
    }
    #[test]
    fn decays_to_quiet_after_quiet_frames() {
        let mut d = SenseDetector::new(cfg());
        for _ in 0..50 {
            d.feed(0.001, 0.002);
        }
        for _ in 0..3 {
            d.feed(0.05, 0.06);
        } // -> Activity
        let mut got = None;
        for _ in 0..6 {
            got = d.feed(0.001, 0.002).or(got);
        } // quiet
        assert!(matches!(
            got,
            Some(SenseEvent::StateChanged(SenseState::Quiet))
        ));
    }
}
