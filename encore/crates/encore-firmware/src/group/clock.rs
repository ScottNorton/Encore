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
fn median_i64(values: &[i64]) -> i64 {
    if values.is_empty() {
        return 0;
    }
    let mut sorted = values.to_vec();
    sorted.sort_unstable();
    sorted[sorted.len() / 2]
}

fn median_u64(values: &[u64]) -> u64 {
    if values.is_empty() {
        return 0;
    }
    let mut sorted = values.to_vec();
    sorted.sort_unstable();
    sorted[sorted.len() / 2]
}

/// Rolling median buffer capacity.
const MEDIAN_CAPACITY: usize = 60;

/// Tracks the clock offset between this speaker and a remote peer.
pub struct ClockSync {
    /// Rolling buffer of raw offset measurements.
    offset_samples: VecDeque<i64>,
    /// Rolling buffer of raw RTT measurements.
    rtt_samples: VecDeque<u64>,
    /// Current median offset: remote_time = local_time + offset_us.
    offset_us: i64,
    /// Current median RTT in microseconds.
    rtt_us: u64,
    /// Total number of samples processed.
    samples: u32,
}

impl ClockSync {
    pub fn new() -> Self {
        Self {
            offset_samples: VecDeque::with_capacity(MEDIAN_CAPACITY),
            rtt_samples: VecDeque::with_capacity(MEDIAN_CAPACITY),
            offset_us: 0,
            rtt_us: 0,
            samples: 0,
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

        // Reject measurements with RTT > 2x current median (asymmetric path)
        if self.samples > 3 && self.rtt_us > 0 && rtt > self.rtt_us * 2 {
            return;
        }

        // Add to rolling buffers
        if self.offset_samples.len() >= MEDIAN_CAPACITY {
            self.offset_samples.pop_front();
        }
        if self.rtt_samples.len() >= MEDIAN_CAPACITY {
            self.rtt_samples.pop_front();
        }
        self.offset_samples.push_back(offset);
        self.rtt_samples.push_back(rtt);
        self.samples += 1;

        // Compute medians
        let offsets: Vec<i64> = self.offset_samples.iter().copied().collect();
        let rtts: Vec<u64> = self.rtt_samples.iter().copied().collect();
        self.offset_us = median_i64(&offsets);
        self.rtt_us = median_u64(&rtts);
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
        self.samples >= 3
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
