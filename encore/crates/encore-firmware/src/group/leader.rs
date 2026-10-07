//! Leader streaming task — reads from the mixer network tap and
//! sends audio chunks to followers via UDP.

use super::clock;
use super::wire::{self, GroupPacket};
use crate::audio::mixer::MixerSlot;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tokio::net::UdpSocket;

/// A live, mutable set of follower UDP targets shared between the group event
/// loop and the leader streaming task.
///
/// The event loop replaces the whole set whenever membership changes (a peer
/// connects, disconnects, or is evicted as dead). The streaming task reads a
/// fresh [`snapshot`](SharedTargets::snapshot) every tick, so a peer that left
/// stops receiving audio immediately instead of the task holding a frozen list.
#[derive(Clone, Default)]
pub struct SharedTargets(Arc<Mutex<Vec<std::net::SocketAddr>>>);

impl SharedTargets {
    pub fn new() -> Self {
        Self(Arc::new(Mutex::new(Vec::new())))
    }

    /// Replace the entire target set (called on every membership change).
    pub fn set(&self, addrs: Vec<std::net::SocketAddr>) {
        *self.0.lock().unwrap() = addrs;
    }

    /// Read the current set of targets.
    pub fn snapshot(&self) -> Vec<std::net::SocketAddr> {
        self.0.lock().unwrap().clone()
    }
}

/// Frames per audio chunk sent to followers. Defined in `wire` so sender and
/// receiver agree and the encoded datagram fits one MTU — an oversized chunk
/// fragments and shreds on a lossy WiFi link (the multi-room "static" bug).
const CHUNK_FRAMES: usize = wire::AUDIO_CHUNK_FRAMES;
/// Samples per chunk (stereo).
const CHUNK_SAMPLES: usize = CHUNK_FRAMES * 2;
/// Duration of one chunk in microseconds (144 frames / 48kHz = 3ms).
const CHUNK_DUR_US: u64 = CHUNK_FRAMES as u64 * 1_000_000 / 48_000;

/// Monotonic, sample-counted playout timeline for a leader stream.
///
/// Each chunk's `play_at` is derived from a fixed epoch plus its index times
/// the exact chunk duration, so send-time jitter (bursty mixer drains) can
/// never collapse the spacing between consecutive chunks. The timeline emits
/// strictly `CHUNK_DUR_US`-spaced timestamps regardless of when chunks are
/// actually drained and sent.
struct StreamTimeline {
    /// Reference clock for chunk 0 (leader clock, in us).
    epoch_us: u64,
    /// Playout lead added to every chunk (followers hold until play_at).
    lead_us: u64,
    /// Index of the next chunk to be stamped.
    chunk_index: u64,
    /// EMA-smoothed grid-vs-production error (see [`observe`]).
    err_ema: f64,
}

/// Epoch servo deadband: the grid may sit this far from production time before
/// correction engages. Wider than the drain tick's natural ±10ms burst phase,
/// so steady state never chases jitter; also the bound on how far the grid —
/// and with it the follower playout phase — can drift from the leader's.
const SERVO_DEADBAND_US: i64 = 10_000;
/// Minimum epoch correction per tick once engaged. 20µs per 10ms tick =
/// 2000ppm of authority, comfortably above real crystal drift (<200ppm). A
/// follower sees chunk spacing of 3000±slew µs, inside its gap/stale
/// tolerances (gap-fill floors at one whole chunk; stale needs spacing ≤ 0).
const SERVO_SLEW_MIN_US: i64 = 20;
/// Ceiling on the per-tick correction: a gross error (a stall, a recovering
/// pause) walks back at up to 1ms/tick (~100ms/s) — spacing stays 3000±1000µs,
/// still follower-safe on both sides.
const SERVO_SLEW_MAX_US: i64 = 1_000;
/// Proportional divisor: slew = excess-over-deadband / this, clamped above.
const SERVO_SLEW_DIV: i64 = 64;
/// Past this (raw, not smoothed) the grid is not drifting, it is broken — a
/// long source pause froze production while the clock ran on. Re-anchor to now
/// like a fresh stream; a follower treats the stamp jump as a discontinuity
/// and re-anchors too.
const SERVO_REANCHOR_US: i64 = 1_000_000;

impl StreamTimeline {
    fn new(epoch_us: u64, lead_us: u64) -> Self {
        Self {
            epoch_us,
            lead_us,
            chunk_index: 0,
            err_ema: 0.0,
        }
    }

    /// Return the `play_at` for the next chunk and advance by exactly one chunk.
    fn next_play_at(&mut self) -> u64 {
        let play_at = self.epoch_us + self.chunk_index * CHUNK_DUR_US + self.lead_us;
        self.chunk_index += 1;
        play_at
    }

    /// Long-stream epoch servo, called once per drain tick.
    ///
    /// The grid is anchored once, but chunk production is paced by the DAC
    /// crystal while the grid advances on CLOCK_MONOTONIC — over a movie-length
    /// stream the two drift apart by hundreds of ms (ppm-level clock error),
    /// eroding the followers' lead (late side) or bloating their jitter buffers
    /// into skip-corrections (early side). Whenever the smoothed error leaves
    /// the deadband, nudge the epoch a bounded step back toward "grid ≈ now".
    fn observe(&mut self, now_us: u64) {
        let next_stamp = self.epoch_us + self.chunk_index * CHUNK_DUR_US;
        let err = next_stamp as i64 - now_us as i64; // >0: grid ahead of production
        if err < -SERVO_REANCHOR_US {
            // A paused source froze the grid far in the past; resume fresh.
            self.epoch_us = now_us.saturating_sub(self.chunk_index * CHUNK_DUR_US);
            self.err_ema = 0.0;
            return;
        }
        self.err_ema += (err as f64 - self.err_ema) / 8.0;
        let e = self.err_ema as i64;
        let excess = e.abs() - SERVO_DEADBAND_US;
        if excess <= 0 {
            return;
        }
        // Proportional slew: gentle near the deadband (drift tracking), up to
        // the ceiling for gross errors (pause recovery) — never past the error.
        let slew = (excess / SERVO_SLEW_DIV)
            .clamp(SERVO_SLEW_MIN_US, SERVO_SLEW_MAX_US)
            .min(excess) as u64;
        if e > 0 {
            self.epoch_us = self.epoch_us.saturating_sub(slew);
        } else {
            self.epoch_us += slew;
        }
    }
}

/// Read from the mixer tap and send audio chunks to followers via UDP.
///
/// Runs on a dedicated tokio task. Reads from `network_tap` whenever
/// enough samples are available, wraps in AudioChunk with a play_at
/// timestamp, and sends via UDP unicast to each follower.
///
/// `play_at = now + buffer_ms` gives followers timing control: they hold
/// audio in the jitter buffer until play_at arrives, matching the leader's
/// pipeline delay for synchronized playback (Snapcast model).
pub async fn leader_stream_task(
    network_tap: Arc<MixerSlot>,
    tap_active: Arc<AtomicBool>,
    udp_socket: Arc<UdpSocket>,
    targets: SharedTargets,
    leader_id: u32,
    buffer_ms: u64,
) {
    let mut read_buf = vec![0i32; CHUNK_SAMPLES];
    let mut interval = tokio::time::interval(std::time::Duration::from_millis(10));
    let mut seq: u32 = 0;
    // Anchor the playout timeline once, at stream start. Each chunk's play_at
    // then advances by exactly one chunk duration, immune to send jitter.
    let mut timeline = StreamTimeline::new(clock::now_us(), buffer_ms * 1000);

    while tap_active.load(Ordering::Relaxed) {
        interval.tick().await;

        if !tap_active.load(Ordering::Relaxed) {
            break;
        }

        // Re-read the live follower set each tick so a peer that left stops
        // receiving audio immediately.
        let follower_addrs = targets.snapshot();

        // Drain all available audio from the tap in chunks
        while network_tap.available() >= CHUNK_SAMPLES {
            let read = network_tap.read_copy(&mut read_buf);
            if read < CHUNK_SAMPLES {
                break;
            }

            let play_at_us = timeline.next_play_at();

            let packet = GroupPacket::AudioChunk {
                seq,
                leader_id,
                play_at_us,
                frame_count: CHUNK_FRAMES as u16,
                hop_count: 0,
                pcm: read_buf[..CHUNK_SAMPLES].to_vec(),
            };

            // The same `seq` lands in both the header and the AudioChunk payload.
            // The follower keys timing on the payload `seq`; the header copy is
            // informational for this type (see `wire::encode`).
            let data = wire::encode(&packet, seq);
            seq = seq.wrapping_add(1);

            // Fire-and-forget UDP unicast to each follower
            for addr in &follower_addrs {
                let _ = udp_socket.send_to(&data, addr).await;
            }
        }

        // Keep the stamp grid tracking DAC-paced production over long streams.
        timeline.observe(clock::now_us());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn play_at_advances_exactly_one_chunk_regardless_of_send_jitter() {
        let mut tl = StreamTimeline::new(/*epoch_us*/ 1_000_000, /*lead_us*/ 80_000);
        let a = tl.next_play_at(); // chunk 0
        let b = tl.next_play_at(); // chunk 1
        let c = tl.next_play_at(); // chunk 2
        assert_eq!(b - a, CHUNK_DUR_US); // exactly one chunk apart, jitter-immune
        assert_eq!(c - b, CHUNK_DUR_US);
        assert_eq!(a, 1_000_000 + 80_000); // epoch + lead for chunk 0
    }

    /// Simulate a stream where chunk production is paced by a drifted DAC:
    /// per simulated tick, `now` advances `tick_us` but only `produced` chunks
    /// come off the tap. Returns the final grid-vs-now error.
    fn run_drifted_stream(drift_ppm: f64, ticks: u64) -> i64 {
        let tick_us = 10_000u64;
        // Realistic large epoch (the real anchor is CLOCK_MONOTONIC µs).
        let mut tl = StreamTimeline::new(1_000_000_000_000, 0);
        let mut now = 1_000_000_000_000u64;
        let mut produced_frac = 0.0f64;
        for _ in 0..ticks {
            now += tick_us;
            // DAC-paced production: nominal chunks per tick, scaled by drift.
            produced_frac += (tick_us as f64 / CHUNK_DUR_US as f64) * (1.0 + drift_ppm * 1e-6);
            while produced_frac >= 1.0 {
                let _ = tl.next_play_at();
                produced_frac -= 1.0;
            }
            tl.observe(now);
        }
        (tl.epoch_us + tl.chunk_index * CHUNK_DUR_US) as i64 - now as i64
    }

    #[test]
    fn epoch_servo_bounds_grid_drift_over_long_streams() {
        // 200ppm slow DAC over 2 simulated hours (720k ticks) would drift the
        // grid 1.44s from production without the servo. It must stay within
        // the deadband plus a little regulation slack.
        let err = run_drifted_stream(-200.0, 720_000);
        assert!(
            err.abs() < SERVO_DEADBAND_US + 5_000,
            "grid drifted {}us from production",
            err
        );
        // Fast DAC drifts the other way.
        let err = run_drifted_stream(200.0, 720_000);
        assert!(err.abs() < SERVO_DEADBAND_US + 5_000, "err {}us", err);
    }

    #[test]
    fn epoch_servo_leaves_a_clean_stream_alone() {
        let mut tl = StreamTimeline::new(0, 80_000);
        let mut now = 0u64;
        let mut produced_frac = 0.0f64;
        let mut stamps = Vec::new();
        for _ in 0..1000 {
            now += 10_000;
            // Perfect production: exactly 10ms of audio per 10ms tick.
            produced_frac += 10_000f64 / CHUNK_DUR_US as f64;
            while produced_frac >= 1.0 {
                stamps.push(tl.next_play_at());
                produced_frac -= 1.0;
            }
            tl.observe(now);
        }
        // Zero drift: every stamp still exactly one chunk apart (no wobble).
        assert!(
            stamps.windows(2).all(|w| w[1] - w[0] == CHUNK_DUR_US),
            "servo must not perturb an aligned stream"
        );
    }

    #[test]
    fn epoch_servo_keeps_stamp_spacing_follower_safe_under_drift() {
        // Under heavy drift the servo slews the epoch between stamps; the
        // spacing a follower sees must stay within 3000±SERVO_SLEW_US — never
        // ≤0 (stale-reject) and never a whole extra chunk (gap-fill).
        let tick_us = 10_000u64;
        let mut tl = StreamTimeline::new(1_000_000_000_000, 0);
        let mut now = 1_000_000_000_000u64;
        let mut produced_frac = 0.0f64;
        let mut last_stamp: Option<u64> = None;
        for _ in 0..100_000 {
            now += tick_us;
            produced_frac += (tick_us as f64 / CHUNK_DUR_US as f64) * (1.0 - 500.0 * 1e-6);
            while produced_frac >= 1.0 {
                let s = tl.next_play_at();
                if let Some(prev) = last_stamp {
                    let gap = s as i64 - prev as i64;
                    assert!(
                        gap > 0 && gap <= (CHUNK_DUR_US as i64 + SERVO_SLEW_MAX_US),
                        "stamp spacing {} breaks follower tolerances",
                        gap
                    );
                }
                last_stamp = Some(s);
                produced_frac -= 1.0;
            }
            tl.observe(now);
        }
    }

    #[test]
    fn epoch_servo_recovers_after_a_long_pause() {
        let mut tl = StreamTimeline::new(0, 0);
        let mut now = 0u64;
        let mut produced_frac = 0.0f64;
        let mut stream = |tl: &mut StreamTimeline, now: &mut u64, ticks: u64, producing: bool| {
            for _ in 0..ticks {
                *now += 10_000;
                if producing {
                    produced_frac += 10_000f64 / CHUNK_DUR_US as f64;
                    while produced_frac >= 1.0 {
                        let _ = tl.next_play_at();
                        produced_frac -= 1.0;
                    }
                }
                tl.observe(*now);
            }
        };
        stream(&mut tl, &mut now, 100, true); // stream a bit
        stream(&mut tl, &mut now, 6000, false); // 60s pause: clock runs, grid freezes
        stream(&mut tl, &mut now, 6000, true); // resume for 60s
                                               // Between the raw-error re-anchor (bounds the pause damage at ~1s) and
                                               // the fast slew (~20ms/s), the grid must be back near production.
        let next = tl.epoch_us + tl.chunk_index * CHUNK_DUR_US;
        let err = next as i64 - now as i64;
        assert!(
            err.abs() < SERVO_DEADBAND_US + 10_000,
            "grid must recover after a pause, still {}us off",
            err
        );
    }

    #[test]
    fn chunk_dur_us_divides_evenly() {
        // The chunk duration must land on an exact integer-microsecond grid, or
        // the sample-counted playout timeline accumulates rounding drift.
        assert_eq!(CHUNK_DUR_US * 48_000, CHUNK_FRAMES as u64 * 1_000_000);
    }

    fn addr(ip: &str) -> std::net::SocketAddr {
        std::net::SocketAddr::new(ip.parse().unwrap(), super::super::peer::GROUP_AUDIO_PORT)
    }

    #[test]
    fn leader_targets_reflect_live_membership() {
        let targets = SharedTargets::new();
        targets.set(vec![addr("10.0.0.1"), addr("10.0.0.2")]);
        assert_eq!(targets.snapshot().len(), 2);
        targets.set(vec![addr("10.0.0.1")]); // a peer left
        assert_eq!(targets.snapshot().len(), 1);
    }
}
