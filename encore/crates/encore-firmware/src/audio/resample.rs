//! Linear interpolation resampler for speech audio.
//!
//! TTS output is typically 16kHz or 22050Hz mono; the mixer runs at 48kHz.
//! Linear interpolation is sufficient quality for speech.

/// Resample i16 PCM using linear interpolation.
///
/// Passthrough (clone) when `src_rate == dst_rate`.
pub fn resample_linear(input: &[i16], src_rate: u32, dst_rate: u32) -> Vec<i16> {
    // A zero rate (e.g. a Wyoming audio-start carrying "rate": 0) would make the
    // ratio 0 and out_len = (len / 0).ceil() saturate to usize::MAX, panicking
    // Vec::with_capacity. Treat any degenerate rate as passthrough.
    if src_rate == dst_rate || src_rate == 0 || dst_rate == 0 || input.is_empty() {
        return input.to_vec();
    }

    let ratio = src_rate as f64 / dst_rate as f64;
    let out_len = ((input.len() as f64) / ratio).ceil() as usize;
    let mut output = Vec::with_capacity(out_len);

    for i in 0..out_len {
        let src_pos = i as f64 * ratio;
        let idx = src_pos as usize;
        let frac = src_pos - idx as f64;

        let sample = if idx + 1 < input.len() {
            let a = input[idx] as f64;
            let b = input[idx + 1] as f64;
            (a + (b - a) * frac) as i16
        } else {
            input[input.len() - 1]
        };

        output.push(sample);
    }

    output
}

/// Resample stereo interleaved i32 PCM using linear interpolation.
///
/// Input and output are interleaved `[L, R, L, R, ...]`. Passthrough when
/// `src_rate == dst_rate`.
pub fn resample_i32_stereo(input: &[i32], src_rate: u32, dst_rate: u32) -> Vec<i32> {
    // See resample_linear: a zero rate would saturate out_frames to usize::MAX.
    if src_rate == dst_rate || src_rate == 0 || dst_rate == 0 || input.len() < 2 {
        return input.to_vec();
    }

    let in_frames = input.len() / 2;
    let ratio = src_rate as f64 / dst_rate as f64;
    let out_frames = ((in_frames as f64) / ratio).ceil() as usize;
    let mut output = Vec::with_capacity(out_frames * 2);

    for i in 0..out_frames {
        let src_pos = i as f64 * ratio;
        let idx = src_pos as usize;
        let frac = src_pos - idx as f64;

        if idx + 1 < in_frames {
            let a_l = input[idx * 2] as f64;
            let b_l = input[(idx + 1) * 2] as f64;
            output.push((a_l + (b_l - a_l) * frac) as i32);

            let a_r = input[idx * 2 + 1] as f64;
            let b_r = input[(idx + 1) * 2 + 1] as f64;
            output.push((a_r + (b_r - a_r) * frac) as i32);
        } else if idx < in_frames {
            output.push(input[idx * 2]);
            output.push(input[idx * 2 + 1]);
        }
    }

    output
}

/// Streaming adaptive-ratio linear resampler for stereo interleaved i32 PCM.
///
/// Unlike [`resample_i32_stereo`] (stateless, fixed ratio, per-chunk), this
/// carries the fractional phase and the last input frame across chunks (no
/// boundary discontinuity) and lets a latency servo trim the effective ratio
/// by a few thousand ppm, so a clock mismatch between the producer (e.g. a BT
/// source's 44.1 kHz crystal) and the consumer (the local DAC) is steered out
/// continuously instead of accumulating in the buffer.
pub struct AdaptiveResampler {
    /// Input frames consumed per output frame at zero correction.
    base_step: f64,
    /// Correction in parts-per-million. Positive consumes input faster (fewer
    /// output samples per input second → a downstream buffer drains).
    corr_ppm: f64,
    /// Fractional position between `prev` and the next input frame, in [0, 1)
    /// plus any whole-frame stride pending consumption.
    phase: f64,
    /// Last consumed input frame, carried across chunks for interpolation.
    prev: Option<[i32; 2]>,
}

impl AdaptiveResampler {
    pub fn new(src_rate: u32, dst_rate: u32) -> Self {
        // Degenerate rates fall back to 1:1 (see resample_linear's zero-rate note).
        let base_step = if src_rate == 0 || dst_rate == 0 {
            1.0
        } else {
            src_rate as f64 / dst_rate as f64
        };
        Self {
            base_step,
            corr_ppm: 0.0,
            phase: 0.0,
            prev: None,
        }
    }

    /// Set the servo correction. The caller clamps; this applies it verbatim.
    pub fn set_correction_ppm(&mut self, ppm: f64) {
        self.corr_ppm = ppm;
    }

    /// Resample `input` (stereo interleaved), appending output to `out`.
    pub fn process(&mut self, input: &[i32], out: &mut Vec<i32>) {
        let in_frames = input.len() / 2;
        if in_frames == 0 {
            return;
        }
        let step = self.base_step * (1.0 + self.corr_ppm * 1e-6);
        let frame = |i: usize| [input[i * 2], input[i * 2 + 1]];

        let mut cursor = 0usize;
        let mut prev = match self.prev {
            Some(p) => p,
            None => {
                // First chunk ever: anchor exactly on the first input frame.
                cursor = 1;
                self.phase = 0.0;
                frame(0)
            }
        };

        loop {
            // Consume input frames the output cursor has stridden past.
            while self.phase >= 1.0 {
                if cursor >= in_frames {
                    self.prev = Some(prev);
                    return;
                }
                prev = frame(cursor);
                cursor += 1;
                self.phase -= 1.0;
            }
            if cursor >= in_frames {
                self.prev = Some(prev);
                return;
            }
            let next = frame(cursor);
            let l = prev[0] as f64 + (next[0] - prev[0]) as f64 * self.phase;
            let r = prev[1] as f64 + (next[1] - prev[1]) as f64 * self.phase;
            out.push(l as i32);
            out.push(r as i32);
            self.phase += step;
        }
    }
}

/// Interleaved stereo samples per millisecond at the 48 kHz mixer rate.
const SAMPLES_PER_MS: f64 = 96.0;

/// What the latency servo asks the producer to do with the next chunk.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ServoVerdict {
    /// Buffered audio is catastrophically over target: drop the chunk outright.
    /// Repeated until the buffer drains back to target — catch-up at real-time
    /// rate (the "device got really behind" recovery).
    Skip,
    /// Steady state: apply this resample correction (ppm) so the buffer fill
    /// converges on the target instead of drifting with clock error.
    Correct(f64),
}

/// Buffer-fill latency servo for a producer resampling into a `MixerSlot`.
///
/// The producer calls [`update`](LatencyServo::update) with the slot's fill
/// before pushing each decoded chunk. The servo EMA-smooths the fill (burst
/// jitter must not thrash it), compares to the target, and either engages a
/// skip latch (way over target → drop whole chunks until back at target) or
/// returns a proportional ratio trim, clamped to an inaudible slew.
pub struct LatencyServo {
    /// Target fill in interleaved samples.
    target: f64,
    /// EMA-smoothed fill; negative = unprimed (first sample seeds it).
    ema: f64,
    /// Skip latch (hysteresis: engage well above target, release near it).
    skipping: bool,
    /// Total chunks dropped by the skip latch (telemetry).
    pub skipped_chunks: u64,
}

/// Skip latch engages this far over target (interleaved samples ≈ 250 ms).
const SKIP_ENGAGE: f64 = 250.0 * SAMPLES_PER_MS;
/// Skip latch releases this close to target (≈ 30 ms over).
const SKIP_RELEASE: f64 = 30.0 * SAMPLES_PER_MS;
/// Proportional gain: correction removes the error over ~this many seconds.
/// (err_us / TAU_S is already in ppm: µs-per-second = 1e-6/s.)
const TAU_S: f64 = 20.0;
/// Correction clamp. ±0.8% is a momentary pitch shift well under audibility
/// for movie/music content; steady state sits at the real clock drift
/// (typically well under 1000 ppm).
const MAX_PPM: f64 = 8000.0;

impl LatencyServo {
    pub fn new(target_ms: u32) -> Self {
        Self {
            target: target_ms as f64 * SAMPLES_PER_MS,
            ema: -1.0,
            skipping: false,
            skipped_chunks: 0,
        }
    }

    /// Feed the current slot fill (interleaved samples); returns what to do
    /// with the chunk about to be pushed.
    pub fn update(&mut self, avail: usize) -> ServoVerdict {
        let avail = avail as f64;
        if self.ema < 0.0 {
            self.ema = avail;
        } else {
            self.ema += (avail - self.ema) / 16.0;
        }
        let over = self.ema - self.target;
        if self.skipping {
            if over <= SKIP_RELEASE {
                self.skipping = false;
            }
        } else if over >= SKIP_ENGAGE {
            self.skipping = true;
        }
        if self.skipping {
            self.skipped_chunks += 1;
            return ServoVerdict::Skip;
        }
        let err_us = over * (1000.0 / SAMPLES_PER_MS);
        ServoVerdict::Correct((err_us / TAU_S).clamp(-MAX_PPM, MAX_PPM))
    }

    /// Smoothed buffered-audio depth in milliseconds (telemetry).
    pub fn latency_ms(&self) -> u32 {
        (self.ema.max(0.0) / SAMPLES_PER_MS) as u32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn passthrough_same_rate() {
        let input = vec![100, 200, 300];
        let out = resample_linear(&input, 48000, 48000);
        assert_eq!(out, input);
    }

    #[test]
    fn upsample_16k_to_48k() {
        let input = vec![0, 3000, 6000];
        let out = resample_linear(&input, 16000, 48000);
        // 3x upsample: 3 input → 9 output
        assert_eq!(out.len(), 9);
        // First and last should match input boundaries
        assert_eq!(out[0], 0);
        assert_eq!(out[out.len() - 1], 6000);
    }

    #[test]
    fn upsample_22050_to_48000() {
        let input = vec![0, 1000];
        let out = resample_linear(&input, 22050, 48000);
        // Ratio ~2.177x, so 2 samples → ~4 output
        assert!(out.len() >= 4);
        assert_eq!(out[0], 0);
    }

    #[test]
    fn empty_input() {
        let out = resample_linear(&[], 16000, 48000);
        assert!(out.is_empty());
    }

    #[test]
    fn zero_rate_does_not_overflow() {
        // A degenerate rate (e.g. a Wyoming audio-start with "rate": 0) must not
        // saturate out_len to usize::MAX and panic Vec::with_capacity.
        let input = vec![100i16, 200, 300];
        assert_eq!(resample_linear(&input, 0, 48000), input);
        assert_eq!(resample_linear(&input, 16000, 0), input);
        let stereo = vec![1i32, 2, 3, 4];
        assert_eq!(resample_i32_stereo(&stereo, 0, 48000), stereo);
        assert_eq!(resample_i32_stereo(&stereo, 44100, 0), stereo);
    }

    // ── i32 stereo resampler tests ──

    #[test]
    fn i32_stereo_passthrough_same_rate() {
        let input = vec![100i32, 200, 300, 400]; // 2 stereo frames
        let out = resample_i32_stereo(&input, 48000, 48000);
        assert_eq!(out, input);
    }

    #[test]
    fn i32_stereo_44100_to_48000_length() {
        // 100 stereo frames at 44100 → should produce ~109 frames at 48000
        let input: Vec<i32> = (0..200).map(|i| i * 1000).collect();
        let out = resample_i32_stereo(&input, 44100, 48000);
        let out_frames = out.len() / 2;
        // Expected: ceil(100 * 48000/44100) = ceil(108.84) = 109
        assert_eq!(out_frames, 109);
        // Output length must be even (stereo pairs)
        assert_eq!(out.len() % 2, 0);
    }

    #[test]
    fn i32_stereo_preserves_channel_separation() {
        // L=1000, R=2000 for all frames
        let input = vec![1000i32, 2000, 1000, 2000, 1000, 2000, 1000, 2000]; // 4 frames
        let out = resample_i32_stereo(&input, 44100, 48000);
        // All left samples should be 1000, all right should be 2000
        for (i, &s) in out.iter().enumerate() {
            if i % 2 == 0 {
                assert_eq!(s, 1000, "L sample at {}", i);
            } else {
                assert_eq!(s, 2000, "R sample at {}", i);
            }
        }
    }

    #[test]
    fn i32_stereo_empty_input() {
        let out = resample_i32_stereo(&[], 44100, 48000);
        assert!(out.is_empty());
    }

    #[test]
    fn i32_stereo_single_frame() {
        let input = vec![500i32, -500];
        let out = resample_i32_stereo(&input, 44100, 48000);
        // 1 frame in → at least 1 frame out
        assert!(out.len() >= 2);
        assert_eq!(out.len() % 2, 0);
        assert_eq!(out[0], 500);
        assert_eq!(out[1], -500);
    }

    // ── adaptive resampler ──

    /// Feed `chunks` chunks of `frames` frames each; return total output samples.
    fn run_adaptive(rs: &mut AdaptiveResampler, chunks: usize, frames: usize) -> usize {
        let input: Vec<i32> = (0..frames * 2).map(|i| (i as i32) * 3).collect();
        let mut total = 0;
        let mut out = Vec::new();
        for _ in 0..chunks {
            out.clear();
            rs.process(&input, &mut out);
            assert_eq!(out.len() % 2, 0, "output must be whole stereo frames");
            total += out.len();
        }
        total
    }

    #[test]
    fn adaptive_matches_nominal_ratio_over_many_chunks() {
        let mut rs = AdaptiveResampler::new(44100, 48000);
        // 1000 chunks × 672 frames at 44.1k → expect ≈ 672000 * 48000/44100 frames.
        let out_samples = run_adaptive(&mut rs, 1000, 672);
        let expected = (672_000f64 * 48000.0 / 44100.0) * 2.0;
        let err = (out_samples as f64 - expected).abs();
        // Cross-chunk continuity means no per-chunk rounding accumulation:
        // total must be within a frame or two of exact.
        assert!(
            err < 8.0,
            "output {} vs expected {} (err {})",
            out_samples,
            expected,
            err
        );
    }

    #[test]
    fn adaptive_positive_ppm_produces_fewer_samples() {
        let mut nominal = AdaptiveResampler::new(44100, 48000);
        let mut fast = AdaptiveResampler::new(44100, 48000);
        fast.set_correction_ppm(8000.0);
        let base = run_adaptive(&mut nominal, 200, 672);
        let corrected = run_adaptive(&mut fast, 200, 672);
        // +8000ppm consumes input ~0.8% faster → ~0.8% fewer output samples.
        let ratio = corrected as f64 / base as f64;
        assert!((0.990..0.995).contains(&ratio), "ratio {}", ratio);
    }

    #[test]
    fn adaptive_is_continuous_across_chunk_boundaries() {
        // A pure ramp resampled in many small chunks must stay monotonic —
        // a phase reset at a boundary would repeat or jump values.
        let mut rs = AdaptiveResampler::new(44100, 48000);
        let ramp: Vec<i32> = (0..2000).flat_map(|i| [i * 100, i * 100]).collect();
        let mut out = Vec::new();
        for chunk in ramp.chunks(34) {
            // odd-sized (17-frame) chunks stress the carry
            rs.process(chunk, &mut out);
        }
        let lefts: Vec<i32> = out.iter().step_by(2).copied().collect();
        assert!(
            lefts.windows(2).all(|w| w[1] >= w[0]),
            "ramp must stay monotonic"
        );
        assert!(
            lefts.len() > 2000,
            "upsampling must yield more frames than input"
        );
    }

    #[test]
    fn adaptive_degenerate_rate_is_identity_step() {
        let mut rs = AdaptiveResampler::new(0, 48000);
        let input = vec![10i32, 20, 30, 40, 50, 60];
        let mut out = Vec::new();
        rs.process(&input, &mut out);
        // 1:1 step: output frame count tracks input (minus the initial anchor).
        assert!(!out.is_empty());
        assert_eq!(out.len() % 2, 0);
    }

    // ── latency servo ──

    #[test]
    fn servo_holds_zero_correction_at_target() {
        let mut servo = LatencyServo::new(100);
        let at_target = 100 * 96;
        for _ in 0..100 {
            match servo.update(at_target) {
                ServoVerdict::Correct(ppm) => assert!(ppm.abs() < 1.0, "ppm {}", ppm),
                ServoVerdict::Skip => panic!("must not skip at target"),
            }
        }
        assert_eq!(servo.latency_ms(), 100);
    }

    #[test]
    fn servo_corrects_toward_target_with_right_sign() {
        // Over target → positive ppm (consume faster, drain the buffer).
        let mut servo = LatencyServo::new(100);
        let mut last = 0.0;
        for _ in 0..200 {
            if let ServoVerdict::Correct(ppm) = servo.update(150 * 96) {
                last = ppm;
            }
        }
        // 50ms over → 50_000us / 20s = 2500 ppm.
        assert!((2000.0..3000.0).contains(&last), "ppm {}", last);

        // Under target → negative ppm (produce more, build the buffer).
        let mut servo = LatencyServo::new(100);
        let mut last = 0.0;
        for _ in 0..200 {
            if let ServoVerdict::Correct(ppm) = servo.update(50 * 96) {
                last = ppm;
            }
        }
        assert!((-3000.0..-2000.0).contains(&last), "ppm {}", last);
    }

    #[test]
    fn servo_clamps_correction() {
        let mut servo = LatencyServo::new(100);
        // 200ms over target (below the 250ms skip engage) → clamped to MAX_PPM.
        for _ in 0..500 {
            match servo.update(300 * 96) {
                ServoVerdict::Correct(ppm) => assert!(ppm <= 8000.0),
                ServoVerdict::Skip => panic!("must not skip below the engage threshold"),
            }
        }
    }

    #[test]
    fn servo_skip_latch_engages_and_releases_with_hysteresis() {
        let mut servo = LatencyServo::new(100);
        // Prime the EMA at a catastrophic backlog → skip engages.
        let mut skipped = false;
        for _ in 0..200 {
            if servo.update(400 * 96) == ServoVerdict::Skip {
                skipped = true;
                break;
            }
        }
        assert!(skipped, "must engage skip way over target");
        // Still skipping at moderate overshoot (inside hysteresis)...
        assert_eq!(servo.update(200 * 96), ServoVerdict::Skip);
        // ...until the fill EMA is back near target.
        let mut released = false;
        for _ in 0..500 {
            if let ServoVerdict::Correct(_) = servo.update(100 * 96) {
                released = true;
                break;
            }
        }
        assert!(released, "skip latch must release near target");
        assert!(servo.skipped_chunks > 0);
    }

    #[test]
    fn servo_ema_smooths_burst_jitter() {
        let mut servo = LatencyServo::new(100);
        for _ in 0..100 {
            servo.update(100 * 96);
        }
        // One wild outlier sample must not flip the latch.
        assert_ne!(servo.update(500 * 96), ServoVerdict::Skip);
    }
}
