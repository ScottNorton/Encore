//! NTP-like clock synchronization between group peers.
//!
//! Followers send ClockSyncReq to the leader, who responds with timestamps.
//! The follower computes the offset between its local clock and the leader's,
//! using a rolling median filter for outlier rejection.

use std::collections::VecDeque;
use std::time::Instant;

/// Boot-relative microsecond clock (monotonic).
pub fn now_us() -> u64 {
    // Use a lazily-initialized epoch so values fit comfortably in u64
    static EPOCH: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
    let epoch = EPOCH.get_or_init(Instant::now);
    epoch.elapsed().as_micros() as u64
}

/// Compute the median of a slice. Returns 0 for empty slices.
fn median_u64(values: &[u64]) -> u64 {
    if values.is_empty() {
        return 0;
    }
    let mut sorted = values.to_vec();
    sorted.sort_unstable();
    sorted[sorted.len() / 2]
}

/// Rolling sample window capacity.
const MEDIAN_CAPACITY: usize = 60;

/// RTT acceptance-floor window (well-formed samples, accepted or not).
///
/// Deliberately much shorter than the offset window: the floor's min is the
/// acceptance gate, and a floor remembering 60 samples of a quiet-era 3 ms
/// path rejects EVERY sample once streaming load legitimately raises RTTs to
/// tens of ms — observed live as the estimator freezing for a minute mid-
/// stream (zero accepts, sigma pinned, convergence unreachable). At 16 the
/// floor turns over within ~8–30 s of probes, so a new path regime is
/// admitted while the min-RTT anchor still protects the published offset.
const RTT_FLOOR_WINDOW: usize = 16;

/// Fixed slack added to the min-RTT acceptance threshold (microseconds).
///
/// A sample is accepted only if its RTT is within `min_rtt + margin`, where
/// `margin = max(min_rtt, RTT_MARGIN_US)`. The fixed floor keeps very low
/// baseline RTTs from rejecting legitimate same-path samples.
const RTT_MARGIN_US: u64 = 2_000;

/// Hard ceiling on a usable sample's RTT (microseconds).
///
/// The adaptive floor above tracks a genuinely raised path, but it must never
/// legitimize garbage: an exchange that took seconds (observed live when sync
/// rode the TCP control channel under BT/WiFi single-radio contention) carries
/// up to ±RTT/2 of asymmetric-delay error — useless against the ~10ms sync
/// budget, yet once such samples fill the window the floor rises to meet them
/// and they all pass the relative gate. Group peers share a LAN; a real
/// same-network RTT is a few tens of ms even under load, so anything past this
/// ceiling is queueing pathology, not a path to adapt to.
const MAX_ACCEPT_RTT_US: u64 = 150_000;

/// Maximum offset dispersion (microseconds) tolerated for `converged()`.
///
/// Even with enough samples we refuse to call the estimate converged while the
/// offsets are still scattered: a high sigma means the min-RTT anchor and skew
/// fit are not yet trustworthy enough to drive playout timing.
const CONVERGED_SIGMA_US: u64 = 300;

/// Maximum anchor-sample RTT (microseconds) tolerated for `converged()`.
///
/// Low sigma proves the samples agree with each other, not that they are
/// right: a consistently asymmetric path biases every exchange the same way,
/// and no amount of agreement detects it. What timestamps CAN prove is the
/// hard NTP bound — the published offset is within ±RTT/2 of truth for the
/// anchor exchange. Requiring the anchor under 20 ms makes `converged()` a
/// certificate ("provably within ±10 ms", the two-speaker audibility line)
/// instead of an assumption; the achieved bound is exposed via
/// [`bound_us`](ClockSync::bound_us) — typically ±1–3 ms on this LAN.
const CONVERGED_MAX_ANCHOR_RTT_US: u64 = 20_000;

/// Minimum samples required before `converged()` can be true.
const CONVERGED_MIN_SAMPLES: u32 = 3;

/// Number of most-recent samples whose agreement gates `converged()`. Sized so
/// a single stale outlier ages out of the stability check once a steady run of
/// agreeing measurements follows it.
const CONVERGED_WINDOW: usize = 6;

/// Tracks the clock offset between this speaker and a remote peer.
pub struct ClockSync {
    /// Rolling buffer of accepted `(offset_us, rtt_us, local_t_us)` measurements,
    /// kept together so the offset of the minimum-RTT sample can be recovered and
    /// the offset-vs-local-time skew can be regressed.
    samples_buf: VecDeque<(i64, u64, u64)>,
    /// Rolling RTT of every recent (well-formed) sample, accepted or not. The
    /// acceptance floor is the minimum of this window, so it tracks a genuinely
    /// raised path floor and turns over even while offset-outliers are rejected.
    rtt_window: VecDeque<u64>,
    /// Offset of the minimum-RTT sample in the window (least asymmetric path).
    offset_us: i64,
    /// Local timestamp of the minimum-RTT sample — the interpolation anchor `t0`.
    anchor_local_us: u64,
    /// RTT of the anchor sample; `u64::MAX` until a sample is accepted. The
    /// published offset is provably within ±`anchor_rtt_us/2` of truth.
    anchor_rtt_us: u64,
    /// Estimated clock skew in parts-per-million: how fast the offset drifts per
    /// microsecond of local time. Positive means the remote clock runs fast
    /// relative to ours. Stored as a float; `offset_at` applies it continuously.
    skew_ppm: f64,
    /// Median RTT in microseconds across the window.
    rtt_us: u64,
    /// Total number of samples processed.
    samples: u32,
    /// Snapshot of `(offset_us, anchor_local_us, skew_ppm)` taken only while
    /// converged. Playout timing reads this mapping, so a noisy stretch (sync
    /// probes lost or degraded under WiFi/BT contention) extrapolates the last
    /// trusted lock at its measured skew instead of chasing the live wobble.
    stable: Option<(i64, u64, f64)>,
}

impl ClockSync {
    pub fn new() -> Self {
        Self {
            samples_buf: VecDeque::with_capacity(MEDIAN_CAPACITY),
            rtt_window: VecDeque::with_capacity(MEDIAN_CAPACITY),
            offset_us: 0,
            anchor_local_us: 0,
            anchor_rtt_us: u64::MAX,
            skew_ppm: 0.0,
            rtt_us: 0,
            samples: 0,
            stable: None,
        }
    }

    /// Process a clock sync response.
    ///
    /// T1 = originate_us (local send time)
    /// T2 = receive_us (remote receive time)
    /// T3 = transmit_us (remote send time)
    /// T4 = local receive time (now)
    pub fn process_response(&mut self, t1: u64, t2: u64, t3: u64, t4: u64) {
        // The local time at which this measurement was taken is the local
        // receive timestamp (t4).
        self.process_response_at(t1, t2, t3, t4, t4);
    }

    /// As `process_response`, but records `local_t_us` as the local time of the
    /// sample instead of deriving it from `t4`. The skew regression and the
    /// continuous offset interpolation key off this local timestamp.
    pub fn process_response_at(&mut self, t1: u64, t2: u64, t3: u64, t4: u64, local_t_us: u64) {
        // t1..t4 are untrusted microsecond clocks off the wire. Compute the
        // differences in i128 so hostile/corrupt extremes can't overflow i64 —
        // that panics in debug/test builds (tearing down the group loop) and
        // wraps to a garbage offset in release. Bounded clocks fit i128 easily.
        let t1 = t1 as i128;
        let t2 = t2 as i128;
        let t3 = t3 as i128;
        let t4 = t4 as i128;

        let offset_i128 = ((t2 - t1) + (t3 - t4)) / 2;
        let raw_rtt = (t4 - t1) - (t3 - t2);

        // A well-formed NTP exchange yields a non-negative RTT. A negative
        // value means the sample is malformed (backwards clock / reordered
        // timestamps); reject it rather than absolute-valuing a bogus small
        // RTT that could then win min-RTT selection.
        if raw_rtt < 0 {
            return;
        }
        // Reject implausible samples so a hostile/corrupt peer can't inject an
        // out-of-range value, and so the i128 -> i64/u64 narrowing is lossless.
        // The offset is a monotonic-clock difference (bounded by uptime, so it
        // can be large but never absurd); the RTT is real network latency.
        const MAX_OFFSET_US: i128 = 1_000_000_000_000_000; // ~31 years
        const MAX_RTT_US: i128 = 60_000_000; // 60s; real RTT is << this
        if offset_i128.abs() > MAX_OFFSET_US || raw_rtt > MAX_RTT_US {
            return;
        }
        let offset = offset_i128 as i64;
        let rtt = raw_rtt as u64;

        // Queueing pathology, not a measurable path — see MAX_ACCEPT_RTT_US.
        // Rejected before the floor window so a burst of them can't raise the
        // adaptive floor into accepting garbage.
        if rtt > MAX_ACCEPT_RTT_US {
            return;
        }

        // Record the RTT of every well-formed sample so the acceptance floor
        // slides. The floor is the minimum RTT over this window, NOT an all-time
        // monotonic minimum, so a genuinely raised path floor (WiFi roam /
        // single-radio AP-STA channel contention) eventually replaces the stale
        // low values and the gate stops rejecting forever.
        if self.rtt_window.len() >= RTT_FLOOR_WINDOW {
            self.rtt_window.pop_front();
        }
        self.rtt_window.push_back(rtt);

        // Outlier rejection from sample 1: later samples whose RTT exceeds the
        // windowed minimum RTT by more than `margin` carry too much
        // queuing/asymmetry to trust and are kept out of the offset estimate.
        // The first sample (empty window before its own push -> min == itself)
        // seeds the baseline and is always admitted.
        let min_rtt = self.rtt_window.iter().copied().min().unwrap_or(rtt);
        let margin = min_rtt.max(RTT_MARGIN_US);
        if rtt > min_rtt + margin {
            return;
        }

        // Add to the rolling offset window (accepted samples only).
        if self.samples_buf.len() >= MEDIAN_CAPACITY {
            self.samples_buf.pop_front();
        }
        self.samples_buf.push_back((offset, rtt, local_t_us));
        self.samples += 1;

        self.recompute();
    }

    /// Recompute the published offset (min-RTT sample), the interpolation
    /// anchor, the offset-vs-local-time skew, and the median RTT.
    fn recompute(&mut self) {
        // Anchor = the sample with the smallest RTT (least asymmetric path).
        // `min_by_key` returns the first element on ties, keeping the earliest
        // sample when RTTs are equal.
        let anchor = self
            .samples_buf
            .iter()
            .min_by_key(|&&(_, rtt, _)| rtt)
            .copied();
        if let Some((offset, rtt, local_t)) = anchor {
            self.offset_us = offset;
            self.anchor_local_us = local_t;
            self.anchor_rtt_us = rtt;
        } else {
            self.offset_us = 0;
            self.anchor_local_us = 0;
            self.anchor_rtt_us = u64::MAX;
        }

        self.skew_ppm = self.estimate_skew_ppm();

        let rtts: Vec<u64> = self.samples_buf.iter().map(|&(_, r, _)| r).collect();
        self.rtt_us = median_u64(&rtts);

        // Refresh the trusted playout mapping only while the estimate holds
        // together; a noisy window leaves the last lock in place.
        if self.converged() {
            self.stable = Some((self.offset_us, self.anchor_local_us, self.skew_ppm));
        }
    }

    /// Least-squares slope of `offset` vs `local_t` over the window, in ppm
    /// (microseconds of offset drift per microsecond of local time). Returns 0
    /// when there are fewer than two samples or the local timestamps carry no
    /// spread (degenerate fit with zero denominator).
    fn estimate_skew_ppm(&self) -> f64 {
        let n = self.samples_buf.len();
        if n < 2 {
            return 0.0;
        }
        // Center on the anchor's local time to keep the sums well-conditioned.
        let t0 = self.anchor_local_us as f64;
        let mut sum_x = 0.0;
        let mut sum_y = 0.0;
        let mut sum_xx = 0.0;
        let mut sum_xy = 0.0;
        for &(offset, _, local_t) in &self.samples_buf {
            let x = local_t as f64 - t0;
            let y = offset as f64;
            sum_x += x;
            sum_y += y;
            sum_xx += x * x;
            sum_xy += x * y;
        }
        let nf = n as f64;
        let denom = nf * sum_xx - sum_x * sum_x;
        if denom.abs() < f64::EPSILON {
            return 0.0;
        }
        (nf * sum_xy - sum_x * sum_y) / denom
    }

    /// Current estimated offset: remote = local + offset.
    pub fn offset_us(&self) -> i64 {
        self.offset_us
    }

    /// Current estimated RTT in microseconds.
    pub fn rtt_us(&self) -> u64 {
        self.rtt_us
    }

    /// Dispersion (standard deviation) of the accepted offsets in the window,
    /// in microseconds. A low value means the offset estimate is stable.
    pub fn sigma_offset_us(&self) -> u64 {
        self.sigma_offset_over(self.samples_buf.len())
    }

    /// Standard deviation (microseconds) of the offsets in the most recent
    /// `window` samples. Returns 0 for fewer than two samples.
    fn sigma_offset_over(&self, window: usize) -> u64 {
        let total = self.samples_buf.len();
        let take = window.min(total);
        if take < 2 {
            return 0;
        }
        let start = total - take;
        let recent = self.samples_buf.iter().skip(start);
        let sum: i64 = recent.clone().map(|&(o, _, _)| o).sum();
        let mean = sum as f64 / take as f64;
        let var = recent
            .map(|&(o, _, _)| {
                let d = o as f64 - mean;
                d * d
            })
            .sum::<f64>()
            / take as f64;
        var.sqrt() as u64
    }

    /// Estimated clock skew in parts-per-million (offset drift per unit of
    /// local time). Positive means the remote clock runs fast relative to ours.
    pub fn skew_ppm(&self) -> f64 {
        self.skew_ppm
    }

    /// Number of sync samples collected.
    pub fn samples(&self) -> u32 {
        self.samples
    }

    /// Whether we've converged: enough samples, a recent run of offsets that
    /// agree closely, AND an anchor exchange fast enough that the agreement is
    /// backed by a hard accuracy bound (±anchor RTT/2 — see
    /// [`CONVERGED_MAX_ANCHOR_RTT_US`]). A stale outlier earlier in the window
    /// no longer blocks convergence once a steady run of agreeing samples
    /// follows it, but a currently-noisy estimate stays unconverged.
    pub fn converged(&self) -> bool {
        self.samples >= CONVERGED_MIN_SAMPLES
            && self.sigma_offset_over(CONVERGED_WINDOW) < CONVERGED_SIGMA_US
            && self.anchor_rtt_us <= CONVERGED_MAX_ANCHOR_RTT_US
    }

    /// Hard accuracy certificate for the published offset, in microseconds:
    /// the NTP bound guarantees truth is within ±this of the estimate,
    /// regardless of path asymmetry. 0 = no accepted samples yet.
    pub fn bound_us(&self) -> u64 {
        if self.anchor_rtt_us == u64::MAX {
            0
        } else {
            self.anchor_rtt_us / 2
        }
    }

    /// Whether the published mapping rests on a certified anchor (±10 ms or
    /// better, per [`CONVERGED_MAX_ANCHOR_RTT_US`]). This is the trust signal
    /// for playout corrections: unlike [`converged`](Self::converged) it does
    /// not demand that the *latest* samples agree — under streaming load the
    /// radio legitimately delays probes for tens of ms and agreement drops,
    /// but the anchored offset (plus skew) stays certified. Gating buffer
    /// regulation on instantaneous convergence left the follower running with
    /// a near-empty buffer for entire streams (observed live).
    pub fn anchored(&self) -> bool {
        self.anchor_rtt_us <= CONVERGED_MAX_ANCHOR_RTT_US
    }

    /// Interpolated offset at local time `local_t_us`, extrapolating from the
    /// min-RTT anchor at the estimated skew. `remote = local + offset_at(local)`.
    pub fn offset_at(&self, local_t_us: u64) -> i64 {
        let dt = local_t_us as f64 - self.anchor_local_us as f64;
        (self.offset_us as f64 + self.skew_ppm * dt).round() as i64
    }

    /// Convert a remote timestamp to local time using the interpolated offset at
    /// `local_now_us`, so the mapping tracks ongoing skew between syncs.
    pub fn remote_to_local_at(&self, remote_us: u64, local_now_us: u64) -> u64 {
        (remote_us as i64 - self.offset_at(local_now_us)) as u64
    }

    /// Convert a remote timestamp to local time at the current local clock.
    pub fn remote_to_local(&self, remote_us: u64) -> u64 {
        self.remote_to_local_at(remote_us, now_us())
    }

    /// Convert a local timestamp to remote time.
    pub fn local_to_remote(&self, local_us: u64) -> u64 {
        (local_us as i64 + self.offset_at(local_us)) as u64
    }

    /// As [`local_to_remote`](Self::local_to_remote), but through the last
    /// converged mapping (skew-extrapolated) when one exists. Playout timing
    /// uses this so a currently-unconverged estimate — the live offset can
    /// swing by whole seconds while sync probes fight radio contention — never
    /// jerks the release schedule around. Falls back to the live mapping until
    /// first convergence (best effort beats not playing at all).
    pub fn local_to_remote_stable(&self, local_us: u64) -> u64 {
        match self.stable {
            Some((offset, anchor, skew)) => {
                let dt = local_us as f64 - anchor as f64;
                let off = (offset as f64 + skew * dt).round() as i64;
                (local_us as i64 + off) as u64
            }
            None => self.local_to_remote(local_us),
        }
    }

    /// How often to send sync requests (microseconds).
    pub fn sync_interval_us(&self) -> u64 {
        if self.samples < 8 {
            500_000 // 500ms during early convergence
        } else if self.samples < 20 {
            1_000_000 // 1s during convergence
        } else {
            2_000_000 // 2s steady-state
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Synthesize a `(t1, t2, t3, t4)` response that yields the requested
    /// offset and RTT, then feed it to the clock.
    ///
    /// With t1 = 0 and t2 = t3 = mid:
    ///   offset = ((t2 - t1) + (t3 - t4)) / 2 = mid - t4/2
    ///   rtt    = (t4 - t1) - (t3 - t2)       = t4
    /// So pick t4 = rtt_us, mid = offset_us + rtt_us/2.
    fn feed(c: &mut ClockSync, offset_us: i64, rtt_us: u64) {
        let t1: u64 = 0;
        let t4: u64 = rtt_us;
        let mid = (offset_us + (rtt_us / 2) as i64) as u64;
        c.process_response(t1, mid, mid, t4);
    }

    /// `feed` with an explicit local timestamp recorded alongside the sample.
    ///
    /// The NTP arithmetic still produces the requested `offset_us`/`rtt_us`, but
    /// the local time at which the sample was taken is `local_t_us` rather than
    /// being derived from `t4`. To keep the synthesized `t1` non-negative (u64),
    /// the whole NTP exchange is shifted by a large constant base while the
    /// recorded local timestamp stays exactly `local_t_us`.
    fn feed_at(c: &mut ClockSync, local_t_us: u64, offset_us: i64, rtt_us: u64) {
        // Anchor the NTP timestamps well clear of zero so t1 = t4 - rtt >= 0.
        const BASE: u64 = 1_000_000_000;
        let t4 = BASE;
        let t1 = BASE - rtt_us;
        // With t2 = t3 = mid: offset = mid - (t1 + t4)/2  =>  mid = offset + (t1+t4)/2.
        let mid = (offset_us + ((t1 + t4) / 2) as i64) as u64;
        c.process_response_at(t1, mid, mid, t4, local_t_us);
    }

    #[test]
    fn extreme_wire_timestamps_do_not_overflow() {
        // Hostile/corrupt timestamps must not overflow the offset/RTT math (a
        // debug/test panic in the old i64 version) and must not be accepted.
        let mut c = ClockSync::new();
        c.process_response(i64::MIN as u64, i64::MAX as u64, 0, 0);
        c.process_response(u64::MAX, 0, u64::MAX, 0);
        c.process_response(0, u64::MAX, u64::MAX, u64::MAX);
        // All implausible -> rejected; nothing entered the estimator.
        assert_eq!(c.samples(), 0);
    }

    #[test]
    fn skew_is_estimated_from_drifting_offsets() {
        // Synthesize samples whose true offset grows 50us per 1s of local time
        // (=50ppm). After enough samples, remote_to_local at a future local time
        // must track the drift, not the stale offset0.
        let mut c = ClockSync::new();
        feed_at(
            &mut c, /*local_t_us*/ 0, /*offset_us*/ 1000, 2_000,
        );
        feed_at(&mut c, 1_000_000, 1050, 2_000);
        feed_at(&mut c, 2_000_000, 1100, 2_000);
        feed_at(&mut c, 3_000_000, 1150, 2_000);
        // At local t=4s the interpolated offset should be ~1200, so a remote ts R
        // maps to ~ R - 1200, not R - 1000.
        let mapped = c.remote_to_local_at(5_000_000, 4_000_000);
        assert!(
            (mapped as i64 - (5_000_000 - 1200)).abs() < 60,
            "got {mapped}"
        );
    }

    #[test]
    fn skew_is_stable_with_a_large_epoch_baseline() {
        // Realistic production case the basic skew test misses: the two speakers
        // booted hours apart, so the raw offset baseline is ~1e10 us, not ~1000.
        // The least-squares skew fit must still resolve a small drift against that
        // huge baseline (the baseline cancels algebraically; this guards that it
        // stays numerically sound in f64). A fragile fit would mis-estimate the
        // slope and desync the group over a long session.
        let mut c = ClockSync::new();
        const BASE: i64 = 10_000_000_000; // ~2.8 hours of boot-epoch difference
                                          // Offset drifts +50us per 1s of local time (= +50ppm) on top of BASE.
        for i in 0..8u64 {
            feed_at(&mut c, i * 1_000_000, BASE + (i as i64) * 50, 2_000);
        }
        // At local t = 10s the interpolated offset must track BASE + 500, so a
        // remote ts maps through the drifting offset, not the stale anchor.
        let mapped = c.remote_to_local_at((BASE + 20_000_000) as u64, 10_000_000) as i64;
        let expected = (BASE + 20_000_000) - (BASE + 500);
        assert!(
            (mapped - expected).abs() < 80,
            "skew fit unstable at large baseline: mapped {mapped}, expected {expected}"
        );
    }

    #[test]
    fn converged_requires_low_sigma() {
        let mut c = ClockSync::new();
        feed(&mut c, 1000, 2_000);
        feed(&mut c, 9000, 2_000); // noisy
        feed(&mut c, 1000, 2_000);
        assert!(!c.converged(), "high sigma must not count as converged");
        for _ in 0..6 {
            feed(&mut c, 1000, 2_000);
        }
        assert!(c.converged());
    }

    #[test]
    fn offset_uses_min_rtt_sample_not_median() {
        // Three samples: a clean low-RTT one with offset ~1000us, and two high-RTT
        // (queued/asymmetric) ones biased to ~5000us. Min-RTT selection must land
        // near 1000, NOT the median (~5000).
        let mut c = ClockSync::new();
        feed(&mut c, 5000, 40_000);
        feed(&mut c, 1000, 2_000); // cleanest path
        feed(&mut c, 5200, 38_000);
        let off = c.offset_us();
        assert!(
            (off - 1000).abs() < 500,
            "expected ~1000 from min-RTT, got {off}"
        );
    }

    #[test]
    fn sigma_offset_reports_dispersion() {
        let mut c = ClockSync::new();
        for _ in 0..6 {
            feed(&mut c, 1000, 2_000);
        }
        assert!(c.sigma_offset_us() < 200, "stable offsets => low sigma");
        feed(&mut c, 9000, 2_000);
        assert!(c.sigma_offset_us() > 200, "an outlier raises sigma");
    }

    #[test]
    fn first_sample_outlier_is_bounded() {
        // A single wild first sample must not dominate; rtt seed + ceiling applies
        // from sample 1 (today the guard is `samples > 3`).
        let mut c = ClockSync::new();
        feed(&mut c, 100_000, 500_000); // absurd first sample
        feed(&mut c, 1000, 2_000);
        feed(&mut c, 1000, 2_000);
        assert!((c.offset_us() - 1000).abs() < 1500);
    }

    #[test]
    fn raised_path_floor_does_not_freeze_the_gate() {
        // Establish a low-RTT baseline, then let the genuine path floor rise and
        // stay up (WiFi roam / single-radio AP-STA contention). A window-derived
        // min-RTT must let the higher-but-now-typical samples back in so the
        // offset tracks the new path; an all-time monotonic floor would reject
        // every later sample forever and pin a stale offset.
        let mut c = ClockSync::new();
        // Low-RTT era: offset ~1000.
        for _ in 0..MEDIAN_CAPACITY {
            feed(&mut c, 1000, 2_000);
        }
        assert!((c.offset_us() - 1000).abs() < 200);

        // Path floor jumps to ~30ms and stays there; offset truly moved to ~4000.
        // Feed enough so the RTT floor window turns over (gate starts admitting)
        // and then the offset window itself flushes the stale low-RTT samples.
        for _ in 0..(2 * MEDIAN_CAPACITY) {
            feed(&mut c, 4000, 30_000);
        }
        // The gate must have accepted the raised-floor samples and re-converged.
        assert!(
            (c.offset_us() - 4000).abs() < 500,
            "gate froze on stale floor: offset {} did not follow raised path",
            c.offset_us()
        );
    }

    #[test]
    fn convergence_requires_a_certified_anchor_not_just_agreement() {
        // Samples that agree perfectly (sigma 0) but all rode a slow path must
        // NOT converge: agreement proves precision, only a fast anchor bounds
        // accuracy. 30ms RTT → ±15ms possible bias — outside the certificate.
        let mut c = ClockSync::new();
        for _ in 0..10 {
            feed(&mut c, 1000, 30_000);
        }
        assert!(!c.converged(), "agreeing-but-slow samples must not certify");
        assert_eq!(c.bound_us(), 15_000);
        // One clean fast exchange anchors the estimate and certifies it.
        feed(&mut c, 1000, 4_000);
        assert!(c.converged());
        assert_eq!(c.bound_us(), 2_000, "bound = anchor RTT / 2");
    }

    #[test]
    fn bound_is_zero_before_any_sample() {
        let c = ClockSync::new();
        assert_eq!(c.bound_us(), 0);
        assert!(!c.converged());
        assert!(!c.anchored());
    }

    #[test]
    fn anchored_survives_load_noise_that_breaks_convergence() {
        let mut c = ClockSync::new();
        for _ in 0..8 {
            feed(&mut c, 1000, 3_000);
        }
        assert!(c.converged() && c.anchored());
        // Streaming load: scattered offsets on slower (but window-admitted)
        // exchanges. Agreement collapses; the certified anchor does not.
        feed(&mut c, 60_000, 4_000);
        feed(&mut c, -40_000, 4_500);
        assert!(!c.converged(), "noisy run must drop convergence");
        assert!(c.anchored(), "certified anchor must survive the noise");
    }

    #[test]
    fn rtt_floor_recovers_from_a_quiet_era_within_the_short_window() {
        // The live failure: a 3 ms quiet-era floor rejected every 20–70 ms
        // streaming-load sample, freezing the estimator for a minute. With the
        // short floor window, accepts must resume within RTT_FLOOR_WINDOW
        // well-formed probes of the new regime.
        let mut c = ClockSync::new();
        for _ in 0..RTT_FLOOR_WINDOW {
            feed(&mut c, 1000, 3_000);
        }
        let before = c.samples();
        // New regime: 30 ms path. First probes are gate-rejected, but each
        // still enters the floor window; once the 3 ms era ages out the gate
        // admits the new path.
        for _ in 0..RTT_FLOOR_WINDOW {
            feed(&mut c, 2000, 30_000);
        }
        assert!(
            c.samples() > before,
            "estimator must not starve past the floor window"
        );
    }

    #[test]
    fn seconds_long_rtt_never_becomes_the_baseline() {
        // The live failure this guards: sync rode a congested TCP path, every
        // exchange measured seconds of RTT, the adaptive floor rose to meet
        // them, and the estimator "converged" on offsets carrying ±RTT/2 of
        // error. A window of pathological samples must leave the estimator
        // empty rather than adapted.
        let mut c = ClockSync::new();
        for _ in 0..MEDIAN_CAPACITY {
            feed(&mut c, 500_000, 11_000_000); // 11s RTT, garbage offset
        }
        assert_eq!(c.samples(), 0, "pathological RTTs must all be rejected");
        assert!(!c.converged());
        // A real sample afterwards seeds the estimator cleanly.
        feed(&mut c, 1000, 20_000);
        assert_eq!(c.samples(), 1);
        assert!((c.offset_us() - 1000).abs() < 500);
    }

    #[test]
    fn stable_mapping_ignores_unconverged_wobble() {
        let mut c = ClockSync::new();
        // Converge on offset ~1000 (zero skew).
        for _ in 0..8 {
            feed_at(&mut c, 1_000_000, 1000, 2_000);
        }
        assert!(c.converged());
        let locked = c.local_to_remote_stable(2_000_000);
        assert!((locked as i64 - (2_000_000 + 1000)).abs() < 60, "got {locked}");

        // A wild burst (accepted RTT-wise, wildly scattered offsets) breaks
        // convergence and swings the live mapping...
        feed_at(&mut c, 2_000_000, 90_000, 2_000);
        feed_at(&mut c, 2_100_000, -70_000, 1_900);
        assert!(!c.converged(), "scattered offsets must drop convergence");
        // ...but the stable mapping still answers from the last lock.
        let held = c.local_to_remote_stable(2_200_000);
        assert!(
            (held as i64 - (2_200_000 + 1000)).abs() < 60,
            "stable mapping moved with the wobble: {held}"
        );
    }

    #[test]
    fn stable_mapping_falls_back_to_live_before_first_convergence() {
        let mut c = ClockSync::new();
        feed(&mut c, 1000, 2_000);
        // One sample: not converged, no snapshot — must still map via the
        // live estimate rather than identity.
        let mapped = c.local_to_remote_stable(500_000);
        assert!((mapped as i64 - (500_000 + 1000)).abs() < 60, "got {mapped}");
    }

    #[test]
    fn malformed_negative_rtt_is_rejected() {
        let mut c = ClockSync::new();
        feed(&mut c, 1000, 2_000);
        let before = c.offset_us();
        let samples_before = c.samples();
        // raw_rtt = (t4 - t1) - (t3 - t2) = 100 - 200 = -100 (backwards /
        // reordered timestamps). |raw_rtt| = 100 is small enough to slip past
        // the RTT gate and, being below the baseline min, would WIN min-RTT
        // selection if absolute-valued — corrupting the offset to ~50050.
        // Rejecting on the negative raw value keeps the published offset clean.
        c.process_response(0, 50_000, 50_200, 100);
        assert_eq!(
            c.samples(),
            samples_before,
            "malformed sample must be dropped"
        );
        assert_eq!(
            c.offset_us(),
            before,
            "offset must be unchanged by malformed sample"
        );
    }

    #[test]
    fn zero_offset_zero_rtt() {
        let mut cs = ClockSync::new();
        cs.process_response(100, 100, 100, 100);
        assert_eq!(cs.offset_us(), 0);
        assert_eq!(cs.rtt_us(), 0);
        assert_eq!(cs.samples(), 1);
    }

    #[test]
    fn positive_offset() {
        let mut cs = ClockSync::new();
        // Remote clock is 1000us ahead
        cs.process_response(0, 1000, 1000, 0);
        assert_eq!(cs.offset_us(), 1000);
    }

    #[test]
    fn negative_offset() {
        let mut cs = ClockSync::new();
        // Remote clock is 500us behind
        cs.process_response(1000, 500, 500, 1000);
        assert_eq!(cs.offset_us(), -500);
    }

    #[test]
    fn rtt_calculation() {
        let mut cs = ClockSync::new();
        // 2ms RTT
        cs.process_response(0, 1001, 1001, 2000);
        assert_eq!(cs.rtt_us(), 2000);
    }

    #[test]
    fn median_rejects_outliers() {
        let mut cs = ClockSync::new();
        // Feed 5 consistent samples at offset=1000
        for _ in 0..5 {
            cs.process_response(0, 1000, 1000, 0);
        }
        assert_eq!(cs.offset_us(), 1000);

        // Feed one massive outlier — median should barely change
        cs.process_response(0, 50000, 50000, 0);
        // With 6 samples [1000, 1000, 1000, 1000, 1000, 50000], median = 1000
        assert_eq!(cs.offset_us(), 1000);
    }

    #[test]
    fn convergence() {
        let mut cs = ClockSync::new();
        assert!(!cs.converged());
        for _ in 0..3 {
            cs.process_response(0, 0, 0, 0);
        }
        assert!(cs.converged());
    }

    #[test]
    fn remote_to_local_conversion() {
        let mut cs = ClockSync::new();
        cs.process_response(0, 500, 500, 0); // offset = 500
        assert_eq!(cs.remote_to_local(1500), 1000);
    }

    #[test]
    fn local_to_remote_conversion() {
        let mut cs = ClockSync::new();
        cs.process_response(0, 500, 500, 0); // offset = 500
        assert_eq!(cs.local_to_remote(1000), 1500);
    }

    #[test]
    fn sync_interval_decreases_after_convergence() {
        let mut cs = ClockSync::new();
        assert_eq!(cs.sync_interval_us(), 500_000);
        for _ in 0..8 {
            cs.process_response(0, 0, 0, 0);
        }
        assert_eq!(cs.sync_interval_us(), 1_000_000);
        for _ in 0..12 {
            cs.process_response(0, 0, 0, 0);
        }
        assert_eq!(cs.sync_interval_us(), 2_000_000);
    }

    #[test]
    fn high_rtt_samples_rejected() {
        let mut cs = ClockSync::new();
        // Establish baseline: offset=1000, RTT=500
        for _ in 0..5 {
            cs.process_response(0, 1250, 1250, 500);
        }
        let offset_before = cs.offset_us();
        let rtt_before = cs.rtt_us();

        // Feed a sample with RTT=5000 (10x baseline) — should be rejected
        cs.process_response(0, 3500, 3500, 5000);
        assert_eq!(cs.offset_us(), offset_before);
        assert_eq!(cs.rtt_us(), rtt_before);
    }
}
