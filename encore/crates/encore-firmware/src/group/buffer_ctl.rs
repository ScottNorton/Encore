//! Self-tuning playout buffer controller.
//!
//! Replaces the static per-node `buffer_ms` setting with a single computed
//! `target_lead_us`: the lead time (how far ahead of "play now" a follower must
//! schedule audio) derived from measured inputs. The controller grows the lead
//! fast on an underrun and decays it slowly when the network stays clean, so it
//! settles just above the real floor instead of carrying a hand-picked margin.
//!
//! Pure logic, no async and no hardware — fully host-testable.

/// Measured inputs to the lead-time formula (all microseconds).
#[derive(Debug, Clone, Copy)]
pub struct Inputs {
    /// 99.9th-percentile network jitter.
    pub jitter_p999_us: u64,
    /// Std-dev of the clock offset estimate.
    pub sigma_offset_us: u64,
    /// Time to drain one batch out of the jitter buffer into ALSA.
    pub drain_us: u64,
    /// One ALSA period (the playout quantum).
    pub alsa_period_us: u64,
    /// Extra hop latency when this node is fed via a relay (0 for direct).
    pub relay_us: u64,
    /// Fixed safety margin.
    pub margin_us: u64,
}

/// Weight on `sigma_offset_us` in the formula (4 sigma ≈ 4*std-dev cover).
const SIGMA_K: u64 = 4;

/// Hard ceiling on the lead (microseconds). Past this the link is unusable.
const CEIL_US: u64 = 500_000;

/// Conservative starting lead (microseconds) before any observation, per the
/// "controller finds the floor" decision: start high, decay down.
const START_LEAD_US: u64 = 35_000;

/// Minimum jump applied on an underrun (microseconds).
const UNDERRUN_MIN_JUMP_US: u64 = 20_000;

/// Step taken toward the formula floor on each clean decay tick (microseconds).
const DECAY_STEP_US: u64 = 2_000;

impl Inputs {
    /// The raw formula value (unclamped): the sum of every measured input with
    /// the sigma term weighted by `SIGMA_K`.
    fn formula_us(&self) -> u64 {
        self.jitter_p999_us
            + SIGMA_K * self.sigma_offset_us
            + self.drain_us
            + self.alsa_period_us
            + self.relay_us
            + self.margin_us
    }
}

/// Self-tuning buffer controller. Holds the current target lead and the most
/// recent formula floor; the lead is never reported below the floor (which is
/// itself never below one ALSA period) nor above [`CEIL_US`].
#[derive(Debug, Clone)]
pub struct BufferCtl {
    /// Current target lead before clamping (microseconds).
    current_us: u64,
    /// Latest formula floor from `observe`/`on_clean_decay_tick` (microseconds).
    floor_us: u64,
}

impl Default for BufferCtl {
    fn default() -> Self {
        Self::new()
    }
}

impl BufferCtl {
    /// New controller seeded with a conservative lead and a minimal floor.
    pub fn new() -> Self {
        Self {
            current_us: START_LEAD_US,
            floor_us: 0,
        }
    }

    /// Feed fresh measurements: sets the formula floor. If the measured floor
    /// has risen above the current lead, the lead follows it up immediately.
    pub fn observe(&mut self, inputs: Inputs) {
        self.floor_us = self.clamp_floor(inputs);
        if self.current_us < self.floor_us {
            self.current_us = self.floor_us;
        }
    }

    /// React to a playout underrun: jump the lead up by the larger of a fixed
    /// minimum and the current formula floor, so a bad link climbs fast.
    pub fn on_underrun(&mut self) {
        let jump = UNDERRUN_MIN_JUMP_US.max(self.floor_us);
        self.current_us = self.current_us.saturating_add(jump);
    }

    /// React to a clean run: step the lead down toward the formula floor by one
    /// `DECAY_STEP_US`, never below the floor. Also refreshes the floor from the
    /// supplied measurements.
    pub fn on_clean_decay_tick(&mut self, inputs: Inputs) {
        self.floor_us = self.clamp_floor(inputs);
        let stepped = self.current_us.saturating_sub(DECAY_STEP_US);
        self.current_us = stepped.max(self.floor_us);
    }

    /// The clamped target lead the playout path should use (microseconds).
    pub fn target_lead_us(&self) -> u64 {
        self.current_us.clamp(self.floor_us, CEIL_US)
    }

    /// Compute the formula floor for these inputs, never below one ALSA period
    /// and never above the ceiling.
    fn clamp_floor(&self, inputs: Inputs) -> u64 {
        inputs.formula_us().clamp(inputs.alsa_period_us, CEIL_US)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A representative "clean network" input set: low jitter/sigma, one 5.333ms
    /// ALSA period (256 frames @ 48kHz), no relay.
    fn clean_inputs() -> Inputs {
        Inputs {
            jitter_p999_us: 1_000,
            sigma_offset_us: 100,
            drain_us: 2_000,
            alsa_period_us: 5_333,
            relay_us: 0,
            margin_us: 5_333,
        }
    }

    #[test]
    fn lead_covers_the_formula_inputs() {
        // Inputs chosen so the formula sum *exceeds* the conservative start lead
        // (START_LEAD_US = 35_000). That forces the reported lead to be driven by
        // the formula rather than by `new()`'s starting constant, so this is a
        // real test of the formula and not of the seed value.
        //
        // sum = 4_000 + 4*5_000 + 2_000 + 5_333 + 0 + 5_333 = 36_666 (> 35_000).
        let mut ctl = BufferCtl::new();
        ctl.observe(Inputs {
            jitter_p999_us: 4_000,
            sigma_offset_us: 5_000,
            drain_us: 2_000,
            alsa_period_us: 5_333,
            relay_us: 0,
            margin_us: 5_333,
        });
        // Hand-computed literal (NOT derived from formula_us(), to avoid a
        // self-referential check). The lead must equal the formula sum exactly:
        // dropping the sigma term, or using any SIGMA_K != 4, or zeroing the
        // formula, drops the sum below START_LEAD_US so the reported lead stays
        // at 35_000 and this assertion fails.
        assert_eq!(
            ctl.target_lead_us(),
            36_666,
            "target_lead must equal the formula sum with sigma weighted by 4"
        );
    }

    #[test]
    fn sigma_offset_is_weighted_by_four() {
        // Two controllers observing inputs that differ ONLY in sigma_offset_us.
        // Both formula sums exceed START_LEAD_US, so the formula drives the lead
        // in each case. The difference in reported lead must equal exactly
        // 4 * (delta sigma), pinning SIGMA_K = 4 without referencing formula_us().
        let base = Inputs {
            jitter_p999_us: 4_000,
            sigma_offset_us: 5_000,
            drain_us: 2_000,
            alsa_period_us: 5_333,
            relay_us: 0,
            margin_us: 5_333,
        };
        let mut hotter = base;
        hotter.sigma_offset_us = 6_000; // +1_000 sigma

        let mut low = BufferCtl::new();
        low.observe(base);
        let mut high = BufferCtl::new();
        high.observe(hotter);

        // Both leads are formula-driven (each sum > START_LEAD_US), so the only
        // moving part is the 4*sigma term: a +1_000 sigma bump must lift the lead
        // by exactly 4_000.
        assert!(
            low.target_lead_us() > START_LEAD_US && high.target_lead_us() > START_LEAD_US,
            "both observations must clear the start lead so the formula drives the value"
        );
        assert_eq!(
            high.target_lead_us() - low.target_lead_us(),
            4 * 1_000,
            "a +1_000us sigma bump must raise the lead by exactly 4*1_000 (SIGMA_K=4)"
        );
    }

    #[test]
    fn underrun_grows_lead_immediately() {
        let mut ctl = BufferCtl::new();
        let before = ctl.target_lead_us();
        ctl.on_underrun();
        assert!(
            ctl.target_lead_us() >= before + 20_000,
            "underrun must jump the lead: {} -> {}",
            before,
            ctl.target_lead_us()
        );
    }

    #[test]
    fn clean_run_decays_toward_floor_slowly() {
        let mut ctl = BufferCtl::new();
        ctl.on_underrun(); // bump up
        let high = ctl.target_lead_us();
        for _ in 0..100 {
            ctl.on_clean_decay_tick(clean_inputs());
        }
        let low = ctl.target_lead_us();
        assert!(low < high, "clean network decays the lead: {high} -> {low}");
        assert!(low >= 5_333, "never below one ALSA period: {low}");
    }

    #[test]
    fn decay_is_gradual_not_instant() {
        // One clean tick must not collapse a high lead straight to the floor:
        // the decay walks down by a single step, proving "slowly".
        let mut ctl = BufferCtl::new();
        ctl.on_underrun();
        let high = ctl.target_lead_us();
        ctl.on_clean_decay_tick(clean_inputs());
        let after_one = ctl.target_lead_us();
        assert_eq!(
            after_one,
            high - 2_000,
            "a single clean tick steps down by exactly one DECAY_STEP_US"
        );
    }

    #[test]
    fn lead_never_decays_below_the_measured_floor() {
        // With a high steady floor, decay ticks must stop at the floor, not at
        // the bare ALSA period.
        let mut ctl = BufferCtl::new();
        let hot = Inputs {
            jitter_p999_us: 20_000,
            sigma_offset_us: 2_000,
            drain_us: 3_000,
            alsa_period_us: 5_333,
            relay_us: 4_000,
            margin_us: 5_333,
        };
        let floor = hot.formula_us();
        ctl.on_underrun();
        for _ in 0..1_000 {
            ctl.on_clean_decay_tick(hot);
        }
        assert_eq!(
            ctl.target_lead_us(),
            floor,
            "decay settles at the formula floor when inputs stay high"
        );
    }

    #[test]
    fn lead_is_clamped_to_the_ceiling() {
        // Repeated underruns must never push the reported lead past CEIL_US.
        let mut ctl = BufferCtl::new();
        for _ in 0..100 {
            ctl.on_underrun();
        }
        assert_eq!(ctl.target_lead_us(), CEIL_US, "lead clamps at the ceiling");
    }
}
