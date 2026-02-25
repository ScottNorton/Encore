//! NTP-like clock synchronization between group peers.
//!
//! Followers send ClockSyncReq to the leader, who responds with timestamps.
//! The follower computes the offset between its local clock and the leader's,
//! using an exponential moving average for stability.

use std::time::Instant;

/// Boot-relative microsecond clock (monotonic).
pub fn now_us() -> u64 {
    // Use a lazily-initialized epoch so values fit comfortably in u64
    static EPOCH: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
    let epoch = EPOCH.get_or_init(Instant::now);
    epoch.elapsed().as_micros() as u64
}

/// Tracks the clock offset between this speaker and a remote peer.
pub struct ClockSync {
    /// Estimated offset: remote_time = local_time + offset_us.
    offset_us: i64,
    /// Smoothed round-trip time in microseconds.
    rtt_us: u64,
    /// Number of samples collected.
    samples: u32,
    /// EMA alpha for offset smoothing.
    alpha: f64,
}

impl ClockSync {
    pub fn new() -> Self {
        Self {
            offset_us: 0,
            rtt_us: 0,
            samples: 0,
            alpha: 0.2,
        }
    }

    /// Process a clock sync response.
    ///
    /// T1 = originate_us (local send time)
    /// T2 = receive_us (remote receive time)
    /// T3 = transmit_us (remote send time)
    /// T4 = local receive time (now)
    pub fn process_response(&mut self, t1: u64, t2: u64, t3: u64, t4: u64) {
        let t1 = t1 as i64;
        let t2 = t2 as i64;
        let t3 = t3 as i64;
        let t4 = t4 as i64;

        let offset = ((t2 - t1) + (t3 - t4)) / 2;
        let rtt = ((t4 - t1) - (t3 - t2)).unsigned_abs();

        if self.samples == 0 {
            // First sample: use directly
            self.offset_us = offset;
            self.rtt_us = rtt;
        } else {
            // Exponential moving average
            self.offset_us =
                (self.alpha * offset as f64 + (1.0 - self.alpha) * self.offset_us as f64) as i64;
            self.rtt_us =
                (self.alpha * rtt as f64 + (1.0 - self.alpha) * self.rtt_us as f64) as u64;
        }
        self.samples += 1;
    }

    /// Current estimated offset: remote = local + offset.
    pub fn offset_us(&self) -> i64 {
        self.offset_us
    }

    /// Current estimated RTT in microseconds.
    pub fn rtt_us(&self) -> u64 {
        self.rtt_us
    }

    /// Number of sync samples collected.
    pub fn samples(&self) -> u32 {
        self.samples
    }

    /// Whether we've converged (enough samples for reasonable accuracy).
    pub fn converged(&self) -> bool {
        self.samples >= 4
    }

    /// Convert a remote timestamp to local time.
    pub fn remote_to_local(&self, remote_us: u64) -> u64 {
        (remote_us as i64 - self.offset_us) as u64
    }

    /// Convert a local timestamp to remote time.
    pub fn local_to_remote(&self, local_us: u64) -> u64 {
        (local_us as i64 + self.offset_us) as u64
    }

    /// How often to send sync requests (microseconds).
    pub fn sync_interval_us(&self) -> u64 {
        if self.samples < 16 {
            2_000_000 // 2s during convergence
        } else {
            10_000_000 // 10s steady-state
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_offset_zero_rtt() {
        let mut cs = ClockSync::new();
        // Symmetric: T1=100, T2=100, T3=100, T4=100 → offset=0, rtt=0
        cs.process_response(100, 100, 100, 100);
        assert_eq!(cs.offset_us(), 0);
        assert_eq!(cs.rtt_us(), 0);
        assert_eq!(cs.samples(), 1);
    }

    #[test]
    fn positive_offset() {
        let mut cs = ClockSync::new();
        // Remote clock is 1000us ahead
        // T1=0, T2=1000, T3=1000, T4=0 → offset = ((1000-0)+(1000-0))/2 = 1000
        cs.process_response(0, 1000, 1000, 0);
        assert_eq!(cs.offset_us(), 1000);
    }

    #[test]
    fn negative_offset() {
        let mut cs = ClockSync::new();
        // Remote clock is 500us behind
        // T1=1000, T2=500, T3=500, T4=1000 → offset = ((500-1000)+(500-1000))/2 = -500
        cs.process_response(1000, 500, 500, 1000);
        assert_eq!(cs.offset_us(), -500);
    }

    #[test]
    fn rtt_calculation() {
        let mut cs = ClockSync::new();
        // 2ms RTT: T1=0, T2=1001, T3=1001, T4=2000
        // offset = ((1001-0)+(1001-2000))/2 = (1001-999)/2 = 1
        // rtt = (2000-0)-(1001-1001) = 2000
        cs.process_response(0, 1001, 1001, 2000);
        assert_eq!(cs.rtt_us(), 2000);
    }

    #[test]
    fn ema_smoothing() {
        let mut cs = ClockSync::new();
        // First sample: offset=1000
        cs.process_response(0, 1000, 1000, 0);
        assert_eq!(cs.offset_us(), 1000);

        // Second sample: offset=2000, EMA with alpha=0.2
        // expected = 0.2*2000 + 0.8*1000 = 400 + 800 = 1200
        cs.process_response(0, 2000, 2000, 0);
        assert_eq!(cs.offset_us(), 1200);
    }

    #[test]
    fn convergence() {
        let mut cs = ClockSync::new();
        assert!(!cs.converged());
        for _ in 0..4 {
            cs.process_response(0, 0, 0, 0);
        }
        assert!(cs.converged());
    }

    #[test]
    fn remote_to_local_conversion() {
        let mut cs = ClockSync::new();
        cs.process_response(0, 500, 500, 0); // offset = 500
        // remote 1500 → local 1500 - 500 = 1000
        assert_eq!(cs.remote_to_local(1500), 1000);
    }

    #[test]
    fn local_to_remote_conversion() {
        let mut cs = ClockSync::new();
        cs.process_response(0, 500, 500, 0); // offset = 500
        // local 1000 → remote 1000 + 500 = 1500
        assert_eq!(cs.local_to_remote(1000), 1500);
    }

    #[test]
    fn sync_interval_decreases_after_convergence() {
        let mut cs = ClockSync::new();
        assert_eq!(cs.sync_interval_us(), 2_000_000);
        for _ in 0..16 {
            cs.process_response(0, 0, 0, 0);
        }
        assert_eq!(cs.sync_interval_us(), 10_000_000);
    }
}
