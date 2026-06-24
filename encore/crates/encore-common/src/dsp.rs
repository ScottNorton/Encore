//! Pure DSP math shared by firmware and dashboard.
//!
//! Biquad filter design (Robert Bristow-Johnson Audio EQ Cookbook) and
//! frequency-response evaluation. No allocation, no platform deps — usable in
//! the real-time mixer thread and in the WASM dashboard's EQ curve renderer.

use crate::protocol::{EqBand, FilterType};

/// Maximum number of EQ bands (matches `protocol::EqState`).
pub const MAX_EQ_BANDS: usize = 10;

/// A normalized biquad section (a0 = 1):
/// `y[n] = b0 x[n] + b1 x[n-1] + b2 x[n-2] - a1 y[n-1] - a2 y[n-2]`
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Biquad {
    pub b0: f32,
    pub b1: f32,
    pub b2: f32,
    pub a1: f32,
    pub a2: f32,
}

impl Biquad {
    /// Flat passthrough (0 dB at all frequencies).
    pub const IDENTITY: Biquad = Biquad {
        b0: 1.0,
        b1: 0.0,
        b2: 0.0,
        a1: 0.0,
        a2: 0.0,
    };
}

/// Per-channel delay state for one [`Biquad`] (Transposed Direct Form II).
#[derive(Debug, Clone, Copy, Default)]
pub struct BiquadState {
    z1: f32,
    z2: f32,
}

impl Biquad {
    /// Design a biquad from RBJ Audio EQ Cookbook parameters. `gain_db` is
    /// ignored for `Notch`. Returns a normalized biquad (a0 divided out).
    pub fn design(
        freq_hz: f32,
        gain_db: f32,
        q: f32,
        filter: FilterType,
        sample_rate: f32,
    ) -> Biquad {
        let q = q.max(0.01);
        let w0 = 2.0 * std::f32::consts::PI * freq_hz / sample_rate;
        let (sin_w0, cos_w0) = w0.sin_cos();
        let alpha = sin_w0 / (2.0 * q);
        let a = 10.0_f32.powf(gain_db / 40.0); // shelf/peak amplitude

        let (b0, b1, b2, a0, a1, a2) = match filter {
            FilterType::Peak => (
                1.0 + alpha * a,
                -2.0 * cos_w0,
                1.0 - alpha * a,
                1.0 + alpha / a,
                -2.0 * cos_w0,
                1.0 - alpha / a,
            ),
            FilterType::LowShelf => {
                let tsa = 2.0 * a.sqrt() * alpha;
                (
                    a * ((a + 1.0) - (a - 1.0) * cos_w0 + tsa),
                    2.0 * a * ((a - 1.0) - (a + 1.0) * cos_w0),
                    a * ((a + 1.0) - (a - 1.0) * cos_w0 - tsa),
                    (a + 1.0) + (a - 1.0) * cos_w0 + tsa,
                    -2.0 * ((a - 1.0) + (a + 1.0) * cos_w0),
                    (a + 1.0) + (a - 1.0) * cos_w0 - tsa,
                )
            }
            FilterType::HighShelf => {
                let tsa = 2.0 * a.sqrt() * alpha;
                (
                    a * ((a + 1.0) + (a - 1.0) * cos_w0 + tsa),
                    -2.0 * a * ((a - 1.0) + (a + 1.0) * cos_w0),
                    a * ((a + 1.0) + (a - 1.0) * cos_w0 - tsa),
                    (a + 1.0) - (a - 1.0) * cos_w0 + tsa,
                    2.0 * ((a - 1.0) - (a + 1.0) * cos_w0),
                    (a + 1.0) - (a - 1.0) * cos_w0 - tsa,
                )
            }
            FilterType::Notch => (
                1.0,
                -2.0 * cos_w0,
                1.0,
                1.0 + alpha,
                -2.0 * cos_w0,
                1.0 - alpha,
            ),
        };

        Biquad {
            b0: b0 / a0,
            b1: b1 / a0,
            b2: b2 / a0,
            a1: a1 / a0,
            a2: a2 / a0,
        }
    }

    /// Process one sample through this biquad (Transposed Direct Form II).
    /// `state` carries the two delay elements between calls.
    #[inline]
    pub fn process(&self, x: f32, state: &mut BiquadState) -> f32 {
        let y = self.b0 * x + state.z1;
        state.z1 = self.b1 * x - self.a1 * y + state.z2;
        state.z2 = self.b2 * x - self.a2 * y;
        y
    }

    /// Magnitude response in dB at `freq_hz` for the given `sample_rate`.
    pub fn magnitude_db(&self, freq_hz: f32, sample_rate: f32) -> f32 {
        let w = 2.0 * std::f32::consts::PI * freq_hz / sample_rate;
        let (sw, cw) = w.sin_cos();
        let (s2w, c2w) = (2.0 * w).sin_cos();
        let num_re = self.b0 + self.b1 * cw + self.b2 * c2w;
        let num_im = -(self.b1 * sw + self.b2 * s2w);
        let den_re = 1.0 + self.a1 * cw + self.a2 * c2w;
        let den_im = -(self.a1 * sw + self.a2 * s2w);
        let num2 = num_re * num_re + num_im * num_im;
        let den2 = den_re * den_re + den_im * den_im;
        10.0 * (num2 / den2).log10()
    }
}

/// Allocation-free stereo multi-band parametric EQ for the real-time mixer.
/// Holds up to [`MAX_EQ_BANDS`] cascaded biquads plus independent L/R delay
/// state, and an optional `pre_gain` for headroom management.
#[derive(Debug, Clone)]
pub struct StereoEq {
    sample_rate: f32,
    biquads: [Biquad; MAX_EQ_BANDS],
    state_l: [BiquadState; MAX_EQ_BANDS],
    state_r: [BiquadState; MAX_EQ_BANDS],
    active: usize,
    pre_gain: f32,
}

impl StereoEq {
    pub fn new(sample_rate: f32) -> Self {
        Self {
            sample_rate,
            biquads: [Biquad::IDENTITY; MAX_EQ_BANDS],
            state_l: [BiquadState::default(); MAX_EQ_BANDS],
            state_r: [BiquadState::default(); MAX_EQ_BANDS],
            active: 0,
            pre_gain: 1.0,
        }
    }

    /// Recompute the filter cascade from band configs. Bands with zero gain are
    /// skipped. When `enabled` is false the EQ becomes a true passthrough
    /// (pre-gain reset to unity). Delay state is preserved across updates for
    /// click-free changes.
    pub fn set_bands(&mut self, bands: &[EqBand], enabled: bool) {
        if !enabled {
            // A disabled EQ must be a bit-exact passthrough on its own, not only
            // when the caller also re-runs apply_headroom(). Drop any reserved
            // headroom pre-gain, or a stale attenuation from a prior boost would
            // quietly lower the level with no compensating boost.
            self.active = 0;
            self.pre_gain = 1.0;
            return;
        }
        let mut n = 0;
        for b in bands {
            if n >= MAX_EQ_BANDS {
                break;
            }
            // A zero-gain peak/shelf is a unity passthrough — skip it. A notch
            // is defined by Q, not gain, so it is never skipped.
            if b.gain_cb == 0 && b.filter_type != FilterType::Notch {
                continue;
            }
            self.biquads[n] = Biquad::design(
                b.freq_hz as f32,
                b.gain_cb as f32 / 10.0,
                b.q_x10 as f32 / 10.0,
                b.filter_type,
                self.sample_rate,
            );
            n += 1;
        }
        self.active = n;
    }

    /// Set the input pre-gain (linear) applied before the cascade (headroom).
    pub fn set_pre_gain(&mut self, gain: f32) {
        self.pre_gain = gain;
    }

    /// Automatically reserve headroom: attenuate the input by the cascade's
    /// worst-case boost so the EQ can never push the signal past 0 dBFS.
    /// A cut-only or flat EQ leaves the gain at unity.
    pub fn apply_headroom(&mut self) {
        let peak = self.peak_gain_db();
        self.pre_gain = if peak > 0.0 {
            10.0f32.powf(-peak / 20.0)
        } else {
            1.0
        };
    }

    /// Worst-case total boost of the active cascade, in dB, scanned across a
    /// log-spaced 20 Hz–20 kHz grid. Used to reserve headroom so the EQ cannot
    /// clip. Returns >= 0; a cut-only EQ yields ~0.
    pub fn peak_gain_db(&self) -> f32 {
        if self.active == 0 {
            return 0.0;
        }
        // Dense log grid: a high-Q (narrow) boost peaks almost entirely between
        // coarse grid points, so too few points under-read its true gain and the
        // reserved headroom falls short — the EQ then hard-clips at the resonant
        // frequency on full-scale input. Only runs on an EQ change, never in the
        // per-sample path, so the extra points are free.
        let n = 720;
        let f_lo = 20.0f32;
        let f_hi = 20_000.0f32.min(self.sample_rate * 0.49);
        let ratio = (f_hi / f_lo).powf(1.0 / (n - 1) as f32);
        let mut max_db = 0.0f32;
        let mut f = f_lo;
        for _ in 0..n {
            let mut total = 0.0f32;
            for k in 0..self.active {
                total += self.biquads[k].magnitude_db(f, self.sample_rate);
            }
            max_db = max_db.max(total);
            f *= ratio;
        }
        max_db
    }

    /// Process a stereo-interleaved i32 buffer (`[L, R, L, R, ...]`) in place.
    pub fn process_interleaved(&mut self, buf: &mut [i32]) {
        // Bit-exact passthrough when there is nothing to do.
        if self.active == 0 && self.pre_gain == 1.0 {
            return;
        }
        let inv = 1.0 / i32::MAX as f32;
        let scale = i32::MAX as f32;
        let frames = buf.len() / 2;
        for i in 0..frames {
            let mut l = buf[i * 2] as f32 * inv * self.pre_gain;
            let mut r = buf[i * 2 + 1] as f32 * inv * self.pre_gain;
            for k in 0..self.active {
                l = self.biquads[k].process(l, &mut self.state_l[k]);
                r = self.biquads[k].process(r, &mut self.state_r[k]);
            }
            buf[i * 2] = (l.clamp(-1.0, 1.0) * scale) as i32;
            buf[i * 2 + 1] = (r.clamp(-1.0, 1.0) * scale) as i32;
        }
    }
}

/// Volume-adaptive "house curve" low-shelf bass gain, in dB, for equal-loudness
/// compensation. At low listening levels the ear is far less sensitive to bass
/// (Fletcher-Munson) and there is ample amp headroom, so the low shelf is boosted;
/// as volume rises the bass is already loud and headroom is scarce, so the boost
/// tapers to zero by HIGH_PCT. Pure mapping — the caller turns this into a shelf
/// biquad and owns when to apply it.
pub fn house_curve_bass_db(volume_pct: u8, max_boost_db: f32) -> f32 {
    const LOW_PCT: f32 = 20.0; // at/below this: full boost
    const HIGH_PCT: f32 = 75.0; // at/above this: no boost
    let v = volume_pct.min(100) as f32;
    if v <= LOW_PCT {
        max_boost_db
    } else if v >= HIGH_PCT {
        0.0
    } else {
        max_boost_db * (HIGH_PCT - v) / (HIGH_PCT - LOW_PCT)
    }
}

/// Number of log-spaced display bands the spectrum analyzer renders.
pub const SPECTRUM_BANDS: usize = 32;

/// Map a real-FFT magnitude spectrum to `SPECTRUM_BANDS` log-spaced bands in [0,1].
///
/// `mags[i]` is the linear magnitude `|X[i]| = sqrt(re^2 + im^2)` of FFT bin `i`
/// (`mags.len() == fft_n / 2`; bin 0 is DC and is skipped). `bin_hz` is the width
/// of one FFT bin in Hz (`sample_rate / fft_n`).
///
/// This is the spectrum-analyzer band mapping, kept pure so the two bugs it fixes
/// are pinned by tests:
///  - **Clipping.** A raw FFT magnitude is unnormalized: a full-scale tone through
///    a Hann window peaks near `N/4`, not 1.0, so treating magnitude 1.0 as 0 dBFS
///    saturates every band to 1.0. We normalize amplitude as `mag * 4 / N` (Hann
///    coherent gain 0.5) so 0 dBFS corresponds to a full-scale tone.
///  - **Duplicate low bands.** Below the FFT resolution (`bin_hz`) several log bands
///    fall inside the same FFT bin and read an identical value. Any band spanning
///    less than one whole bin is linearly interpolated at its geometric-center
///    frequency, so adjacent sub-bin bands get distinct values.
pub fn map_log_bins(mags: &[f32], bin_hz: f32, fft_n: usize) -> [f32; SPECTRUM_BANDS] {
    const F_MIN: f32 = 20.0;
    const F_MAX: f32 = 20_000.0;
    const DB_FLOOR: f32 = -80.0;
    const DB_CEIL: f32 = 0.0;

    let mut out = [0.0f32; SPECTRUM_BANDS];
    if mags.len() < 2 || bin_hz <= 0.0 || fft_n == 0 {
        return out;
    }
    let max_bin = mags.len() - 1; // highest valid index
    let norm = 4.0 / fft_n as f32; // |X| -> amplitude (Hann coherent gain 0.5)
    let log_min = F_MIN.ln();
    let log_max = F_MAX.ln();

    for (b, slot) in out.iter_mut().enumerate() {
        let t0 = b as f32 / SPECTRUM_BANDS as f32;
        let t1 = (b + 1) as f32 / SPECTRUM_BANDS as f32;
        let f0 = (log_min + t0 * (log_max - log_min)).exp();
        let f1 = (log_min + t1 * (log_max - log_min)).exp();
        let p0 = f0 / bin_hz; // fractional bin positions
        let p1 = f1 / bin_hz;

        let mag = if (p1 - p0) >= 1.0 {
            // Spans >=1 whole bin: take the band peak (lively, like a real analyzer).
            let lo = (p0.ceil() as usize).clamp(1, max_bin);
            let hi = (p1.floor() as usize).clamp(1, max_bin);
            let mut m = 0.0f32;
            for &v in &mags[lo..=hi] {
                if v > m {
                    m = v;
                }
            }
            m
        } else {
            // Sub-bin band: interpolate at the geometric center so adjacent bands
            // that land in the same FFT bin still get distinct values.
            let fc = (f0 * f1).sqrt();
            let pos = (fc / bin_hz).clamp(1.0, max_bin as f32);
            let i = pos.floor() as usize;
            let frac = pos - i as f32;
            let m1 = mags[(i + 1).min(max_bin)];
            mags[i] * (1.0 - frac) + m1 * frac
        };

        let amp = mag * norm;
        let db = if amp > 1e-9 {
            20.0 * amp.log10()
        } else {
            DB_FLOOR
        };
        *slot = ((db - DB_FLOOR) / (DB_CEIL - DB_FLOOR)).clamp(0.0, 1.0);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn house_curve_tapers_bass_with_volume() {
        let m = 6.0;
        // Quiet: full boost; loud: none.
        assert_eq!(house_curve_bass_db(0, m), 6.0);
        assert_eq!(house_curve_bass_db(20, m), 6.0); // low knee
        assert_eq!(house_curve_bass_db(75, m), 0.0); // high knee
        assert_eq!(house_curve_bass_db(100, m), 0.0);
        // Monotonic, strictly between the knees.
        let mid = house_curve_bass_db(47, m);
        assert!(mid > 0.0 && mid < 6.0, "mid boost {mid} not between knees");
        assert!(house_curve_bass_db(30, m) > house_curve_bass_db(60, m));
    }

    #[test]
    fn magnitude_of_constant_gain_biquad() {
        // b0 = 2.0 is a flat 2x gain = +6.0206 dB at every frequency.
        let bq = Biquad {
            b0: 2.0,
            ..Biquad::IDENTITY
        };
        let mag = bq.magnitude_db(1000.0, 48000.0);
        assert!((mag - 6.0206).abs() < 0.01, "expected +6.02 dB, got {mag}");
    }

    #[test]
    fn peak_filter_boosts_center_by_gain() {
        let bq = Biquad::design(1000.0, 6.0, 1.0, FilterType::Peak, 48000.0);
        let mag = bq.magnitude_db(1000.0, 48000.0);
        assert!(
            (mag - 6.0).abs() < 0.1,
            "expected +6 dB at center, got {mag}"
        );
    }

    #[test]
    fn peak_cut_attenuates_center() {
        let bq = Biquad::design(1000.0, -10.0, 1.0, FilterType::Peak, 48000.0);
        let mag = bq.magnitude_db(1000.0, 48000.0);
        assert!(
            (mag + 10.0).abs() < 0.2,
            "expected -10 dB at center, got {mag}"
        );
    }

    #[test]
    fn low_shelf_boosts_lows_not_highs() {
        let bq = Biquad::design(80.0, 6.0, 0.7, FilterType::LowShelf, 48000.0);
        let lo = bq.magnitude_db(20.0, 48000.0);
        let hi = bq.magnitude_db(15000.0, 48000.0);
        assert!((lo - 6.0).abs() < 0.6, "low end expected ~+6 dB, got {lo}");
        assert!(hi.abs() < 0.5, "high end expected ~0 dB, got {hi}");
    }

    #[test]
    fn high_shelf_boosts_highs_not_lows() {
        let bq = Biquad::design(8000.0, 6.0, 0.7, FilterType::HighShelf, 48000.0);
        let hi = bq.magnitude_db(20000.0, 48000.0);
        let lo = bq.magnitude_db(50.0, 48000.0);
        assert!((hi - 6.0).abs() < 0.6, "high end expected ~+6 dB, got {hi}");
        assert!(lo.abs() < 0.5, "low end expected ~0 dB, got {lo}");
    }

    #[test]
    fn notch_cuts_center_and_passes_elsewhere() {
        let bq = Biquad::design(1000.0, 0.0, 5.0, FilterType::Notch, 48000.0);
        assert!(
            bq.magnitude_db(1000.0, 48000.0) < -20.0,
            "expected deep cut at center"
        );
        assert!(
            bq.magnitude_db(100.0, 48000.0).abs() < 1.0,
            "expected flat passband"
        );
    }

    #[test]
    fn identity_biquad_passes_signal_unchanged() {
        let bq = Biquad::IDENTITY;
        let mut st = BiquadState::default();
        for &x in &[0.1f32, -0.5, 0.9, 0.3, -0.7] {
            let y = bq.process(x, &mut st);
            assert!((y - x).abs() < 1e-6, "identity should pass {x}, got {y}");
        }
    }

    fn tone_gain_db(eq: &mut StereoEq, f: f32, fs: f32, amp: f32) -> (f32, f32) {
        let n = 4800;
        let inv = 1.0 / i32::MAX as f32;
        let (mut max_l, mut max_r) = (0.0f32, 0.0f32);
        for i in 0..n {
            let s = amp * (2.0 * std::f32::consts::PI * f * i as f32 / fs).sin();
            let si = (s * i32::MAX as f32) as i32;
            let mut buf = [si, si];
            eq.process_interleaved(&mut buf);
            if i > n / 2 {
                max_l = max_l.max((buf[0] as f32 * inv).abs());
                max_r = max_r.max((buf[1] as f32 * inv).abs());
            }
        }
        (20.0 * (max_l / amp).log10(), 20.0 * (max_r / amp).log10())
    }

    #[test]
    fn stereo_eq_boosts_tone_on_both_channels() {
        let mut eq = StereoEq::new(48000.0);
        let band = EqBand {
            freq_hz: 1000,
            gain_cb: 60, // +6.0 dB
            q_x10: 10,
            filter_type: FilterType::Peak,
        };
        eq.set_bands(&[band], true);
        let (gl, gr) = tone_gain_db(&mut eq, 1000.0, 48000.0, 0.4);
        assert!((gl - 6.0).abs() < 0.4, "L gain {gl}");
        assert!((gr - 6.0).abs() < 0.4, "R gain {gr}");
    }

    #[test]
    fn peak_gain_reflects_boost() {
        let mut eq = StereoEq::new(48000.0);
        let band = EqBand {
            freq_hz: 1000,
            gain_cb: 90, // +9 dB
            q_x10: 10,
            filter_type: FilterType::Peak,
        };
        eq.set_bands(&[band], true);
        let pk = eq.peak_gain_db();
        assert!((pk - 9.0).abs() < 0.5, "expected ~+9 dB peak, got {pk}");
    }

    #[test]
    fn peak_gain_is_zero_for_cuts_only() {
        let mut eq = StereoEq::new(48000.0);
        let band = EqBand {
            freq_hz: 1000,
            gain_cb: -90, // -9 dB cut
            q_x10: 10,
            filter_type: FilterType::Peak,
        };
        eq.set_bands(&[band], true);
        assert!(
            eq.peak_gain_db() <= 0.1,
            "cut-only EQ must not report boost"
        );
    }

    #[test]
    fn peak_gain_captures_high_q_boost() {
        // A narrow (high-Q) boost peaks almost entirely between coarse grid
        // points. The headroom scan must still read close to its true gain, else
        // apply_headroom under-reserves and the EQ hard-clips at the resonant
        // frequency on full-scale input. (A 120-point grid read only ~+10.7 dB.)
        let mut eq = StereoEq::new(48000.0);
        let band = EqBand {
            freq_hz: 1000,
            gain_cb: 120, // +12 dB
            q_x10: 100,   // Q = 10 (narrow)
            filter_type: FilterType::Peak,
        };
        eq.set_bands(&[band], true);
        let pk = eq.peak_gain_db();
        assert!(
            (pk - 12.0).abs() < 0.3,
            "scan missed the high-Q peak: saw {pk} dB, true +12"
        );
    }

    #[test]
    fn headroom_offsets_boost_to_unity_at_peak() {
        let mut eq = StereoEq::new(48000.0);
        let band = EqBand {
            freq_hz: 1000,
            gain_cb: 120, // +12 dB
            q_x10: 10,
            filter_type: FilterType::Peak,
        };
        eq.set_bands(&[band], true);
        eq.apply_headroom();
        // With headroom, the +12 dB peak is offset by ~-12 dB pre-gain, so the
        // net gain at the peak frequency is ~0 dB (cannot clip).
        let (gl, gr) = tone_gain_db(&mut eq, 1000.0, 48000.0, 0.5);
        assert!(
            gl.abs() < 0.5,
            "expected ~0 dB at peak with headroom, got {gl}"
        );
        assert!(
            gr.abs() < 0.5,
            "expected ~0 dB at peak with headroom, got {gr}"
        );
    }

    #[test]
    fn headroom_is_unity_for_cuts() {
        let mut eq = StereoEq::new(48000.0);
        let band = EqBand {
            freq_hz: 1000,
            gain_cb: -120, // -12 dB cut, no boost anywhere
            q_x10: 10,
            filter_type: FilterType::Peak,
        };
        eq.set_bands(&[band], true);
        eq.apply_headroom();
        // Passband (away from the cut) must stay at unity — no needless attenuation.
        let (gl, _) = tone_gain_db(&mut eq, 100.0, 48000.0, 0.5);
        assert!(
            gl.abs() < 0.3,
            "cut-only EQ must not attenuate passband, got {gl}"
        );
    }

    #[test]
    fn disabled_eq_is_transparent() {
        let mut eq = StereoEq::new(48000.0);
        let band = EqBand {
            freq_hz: 1000,
            gain_cb: 120,
            q_x10: 10,
            filter_type: FilterType::Peak,
        };
        eq.set_bands(&[band], false); // disabled
        let mut buf = [1000i32, -2000, 3000, -4000];
        let orig = buf;
        eq.process_interleaved(&mut buf);
        assert_eq!(buf, orig, "disabled EQ must be bit-exact passthrough");
    }

    #[test]
    fn disabling_eq_clears_reserved_headroom() {
        // Disabling must be a TRUE passthrough even after headroom was reserved.
        // Otherwise the pre-gain attenuation reserved for a boost lingers with no
        // compensating boost, so "EQ off" would quietly lower the level.
        let mut eq = StereoEq::new(48000.0);
        let band = EqBand {
            freq_hz: 1000,
            gain_cb: 120, // +12 dB boost -> apply_headroom reserves ~-12 dB pre-gain
            q_x10: 10,
            filter_type: FilterType::Peak,
        };
        eq.set_bands(&[band], true);
        eq.apply_headroom(); // pre_gain now well below 1.0
        eq.set_bands(&[band], false); // user turns the EQ off
        let mut buf = [1000i32, -2000, 3000, -4000];
        let orig = buf;
        eq.process_interleaved(&mut buf);
        assert_eq!(
            buf, orig,
            "disabled EQ must be bit-exact passthrough even after apply_headroom"
        );
    }

    #[test]
    fn process_steady_state_matches_magnitude_response() {
        // A 1 kHz sine through a +6 dB peak at 1 kHz should settle to ~2x amplitude.
        let fs = 48000.0;
        let f = 1000.0;
        let bq = Biquad::design(f, 6.0, 1.0, FilterType::Peak, fs);
        let mut st = BiquadState::default();
        let mut max_out = 0.0f32;
        let n = 4800;
        for i in 0..n {
            let x = (2.0 * std::f32::consts::PI * f * i as f32 / fs).sin();
            let y = bq.process(x, &mut st);
            if i > n / 2 {
                max_out = max_out.max(y.abs());
            }
        }
        let gain_db = 20.0 * max_out.log10();
        assert!(
            (gain_db - 6.0).abs() < 0.3,
            "expected ~+6 dB, got {gain_db}"
        );
    }

    // --- spectrum band mapping ---

    const FFT_N: usize = 2048;
    fn bin_hz() -> f32 {
        48_000.0 / FFT_N as f32
    }

    #[test]
    fn spectrum_silence_is_zero() {
        let mags = vec![0.0f32; FFT_N / 2];
        let bins = map_log_bins(&mags, bin_hz(), FFT_N);
        assert!(bins.iter().all(|&b| b == 0.0), "silence must be all zero");
    }

    #[test]
    fn spectrum_normalization_does_not_clip_real_signals() {
        // A flat magnitude of 5.0 is far above 1.0 — the old (unnormalized) mapping
        // saturated every band to 1.0. With Hann-normalized dBFS it lands mid-scale.
        let mags = vec![5.0f32; FFT_N / 2];
        let bins = map_log_bins(&mags, bin_hz(), FFT_N);
        for (i, &b) in bins.iter().enumerate() {
            assert!(
                b > 0.4 && b < 0.6,
                "band {i} = {b}: a 5.0 magnitude must not clip (expected ~0.5)"
            );
        }
        // A genuine full-scale tone (|X| = N/4) reads ~1.0 (0 dBFS).
        let full = vec![(FFT_N as f32) / 4.0; FFT_N / 2];
        let fbins = map_log_bins(&full, bin_hz(), FFT_N);
        assert!(
            fbins.iter().all(|&b| b > 0.98),
            "a full-scale tone should read ~1.0"
        );
    }

    #[test]
    fn spectrum_low_bands_are_distinct() {
        // Rising magnitude per bin. The old floor/ceil+max mapping collapsed the
        // lowest ~10 log bands onto FFT bin 1, making them identical. Interpolating
        // at each band's center frequency gives strictly increasing low bands.
        let mags: Vec<f32> = (0..FFT_N / 2).map(|i| i as f32).collect();
        let bins = map_log_bins(&mags, bin_hz(), FFT_N);
        for i in 0..4 {
            assert!(
                bins[i + 1] > bins[i],
                "low bands must be distinct: band {} ({}) <= band {} ({})",
                i + 1,
                bins[i + 1],
                i,
                bins[i]
            );
        }
    }
}
