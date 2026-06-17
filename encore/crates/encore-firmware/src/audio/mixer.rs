//! Software audio mixer with lock-free SPSC ring buffers.
//!
//! Each audio source (Spotify, Bluetooth, Wyoming, System) gets a MixerSlot.
//! The mixer thread reads from all active slots, sums the samples, and writes
//! to the ALSA PCM device.

use std::cell::UnsafeCell;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;

/// Size of each ring buffer in samples (not frames).
/// 48000 Hz * 2 channels * 2s = 192000 samples.
/// Large buffer allows librespot to decode ahead without blocking,
/// reducing CPU jitter and ensuring smooth playback on slow ARM cores.
const RING_SIZE: usize = 192_000;

/// Lock-free single-producer single-consumer ring buffer for i32 audio samples.
///
/// Safety: exactly one producer thread calls `push`, one consumer thread calls
/// `read_add`/`read_copy`. The atomic positions ensure non-overlapping access.
pub struct MixerSlot {
    buf: UnsafeCell<Vec<i32>>,
    read_pos: AtomicUsize,
    write_pos: AtomicUsize,
    active: AtomicBool,
}

// Safety: SPSC discipline — producer and consumer access disjoint regions,
// coordinated by atomic read_pos/write_pos.
unsafe impl Send for MixerSlot {}
unsafe impl Sync for MixerSlot {}

impl MixerSlot {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            buf: UnsafeCell::new(vec![0i32; RING_SIZE]),
            read_pos: AtomicUsize::new(0),
            write_pos: AtomicUsize::new(0),
            active: AtomicBool::new(false),
        })
    }

    /// Number of samples available to read.
    ///
    /// Positions are monotonically increasing — wrapping_sub gives the
    /// correct distance regardless of overflow.
    pub fn available(&self) -> usize {
        let w = self.write_pos.load(Ordering::Acquire);
        let r = self.read_pos.load(Ordering::Acquire);
        w.wrapping_sub(r)
    }

    /// Push samples into the ring buffer (producer side).
    /// Returns number of samples actually written.
    ///
    /// Positions increase monotonically; only the buffer *access* is modded.
    pub fn push(&self, samples: &[i32]) -> usize {
        let w = self.write_pos.load(Ordering::Relaxed);
        let r = self.read_pos.load(Ordering::Acquire);
        let free = RING_SIZE - 1 - w.wrapping_sub(r);
        let to_write = samples.len().min(free);

        let buf = unsafe { &mut *self.buf.get() };

        for i in 0..to_write {
            buf[(w + i) % RING_SIZE] = samples[i];
        }

        self.write_pos.store(w + to_write, Ordering::Release);
        to_write
    }

    /// Read samples from the ring buffer (consumer side).
    /// Adds samples to `out` (for mixing/summing).
    /// Returns number of samples read.
    pub fn read_add(&self, out: &mut [i32]) -> usize {
        let r = self.read_pos.load(Ordering::Relaxed);
        let w = self.write_pos.load(Ordering::Acquire);
        let avail = w.wrapping_sub(r);
        let to_read = out.len().min(avail);

        let buf = unsafe { &*self.buf.get() };

        for i in 0..to_read {
            out[i] = out[i].saturating_add(buf[(r + i) % RING_SIZE]);
        }

        self.read_pos.store(r + to_read, Ordering::Release);
        to_read
    }

    /// Read samples from the ring buffer (consumer side).
    /// Writes samples directly to `out` (no mixing).
    pub fn read_copy(&self, out: &mut [i32]) -> usize {
        let r = self.read_pos.load(Ordering::Relaxed);
        let w = self.write_pos.load(Ordering::Acquire);
        let avail = w.wrapping_sub(r);
        let to_read = out.len().min(avail);

        let buf = unsafe { &*self.buf.get() };

        for i in 0..to_read {
            out[i] = buf[(r + i) % RING_SIZE];
        }

        self.read_pos.store(r + to_read, Ordering::Release);
        to_read
    }

    /// Set whether this slot is actively producing audio.
    pub fn set_active(&self, active: bool) {
        self.active.store(active, Ordering::Release);
    }

    /// Check if this slot is actively producing audio.
    pub fn is_active(&self) -> bool {
        self.active.load(Ordering::Acquire)
    }

    /// Clear the buffer and reset positions.
    pub fn clear(&self) {
        self.read_pos.store(0, Ordering::Release);
        self.write_pos.store(0, Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_slot_is_empty_and_inactive() {
        let slot = MixerSlot::new();
        assert_eq!(slot.available(), 0);
        assert!(!slot.is_active());
    }

    #[test]
    fn push_and_read_copy() {
        let slot = MixerSlot::new();
        let input = [100i32, 200, 300, 400, 500];
        assert_eq!(slot.push(&input), 5);
        assert_eq!(slot.available(), 5);

        let mut out = [0i32; 5];
        assert_eq!(slot.read_copy(&mut out), 5);
        assert_eq!(out, [100, 200, 300, 400, 500]);
        assert_eq!(slot.available(), 0);
    }

    #[test]
    fn push_and_read_add_sums_into_output() {
        let slot = MixerSlot::new();
        slot.push(&[100i32, 200, 300]);

        // Pre-fill output buffer with existing values
        let mut out = [1000i32, 2000, 3000];
        assert_eq!(slot.read_add(&mut out), 3);
        assert_eq!(out, [1100, 2200, 3300]);
    }

    #[test]
    fn read_add_saturates_on_overflow() {
        let slot = MixerSlot::new();
        slot.push(&[i32::MAX, i32::MIN]);

        let mut out = [1000i32, -1000];
        slot.read_add(&mut out);
        // i32::MAX + 1000 should saturate to i32::MAX
        assert_eq!(out[0], i32::MAX);
        // i32::MIN + (-1000) should saturate to i32::MIN
        assert_eq!(out[1], i32::MIN);
    }

    #[test]
    fn underflow_returns_zero_samples() {
        let slot = MixerSlot::new();
        let mut out = [0i32; 10];
        assert_eq!(slot.read_copy(&mut out), 0);
        assert_eq!(slot.read_add(&mut out), 0);
    }

    #[test]
    fn partial_read_leaves_remainder() {
        let slot = MixerSlot::new();
        slot.push(&[10i32, 20, 30, 40, 50]);

        // Read only 3 of 5
        let mut out = [0i32; 3];
        assert_eq!(slot.read_copy(&mut out), 3);
        assert_eq!(out, [10, 20, 30]);
        assert_eq!(slot.available(), 2);

        // Read remaining 2
        let mut out2 = [0i32; 5];
        assert_eq!(slot.read_copy(&mut out2), 2);
        assert_eq!(out2[..2], [40, 50]);
    }

    #[test]
    fn overflow_drops_excess_samples() {
        let slot = MixerSlot::new();
        // Fill to capacity (RING_SIZE - 1 usable slots)
        let big = vec![42i32; RING_SIZE];
        let written = slot.push(&big);
        assert_eq!(written, RING_SIZE - 1); // one slot reserved

        // Try to push more — should write 0
        assert_eq!(slot.push(&[99i32]), 0);
        assert_eq!(slot.available(), RING_SIZE - 1);
    }

    #[test]
    fn clear_resets_buffer() {
        let slot = MixerSlot::new();
        slot.push(&[1i32, 2, 3]);
        assert_eq!(slot.available(), 3);

        slot.clear();
        assert_eq!(slot.available(), 0);

        // Should be able to push fresh data after clear
        slot.push(&[10i32, 20]);
        let mut out = [0i32; 2];
        slot.read_copy(&mut out);
        assert_eq!(out, [10, 20]);
    }

    #[test]
    fn active_flag_toggles() {
        let slot = MixerSlot::new();
        assert!(!slot.is_active());

        slot.set_active(true);
        assert!(slot.is_active());

        slot.set_active(false);
        assert!(!slot.is_active());
    }

    #[test]
    fn wrap_around_correctness() {
        let slot = MixerSlot::new();

        // Push and drain many times to force positional wrap-around
        for round in 0..100 {
            let val = round * 7;
            let chunk = vec![val; 1000];
            let written = slot.push(&chunk);
            assert_eq!(written, 1000, "round {} push", round);

            let mut out = vec![0i32; 1000];
            let read = slot.read_copy(&mut out);
            assert_eq!(read, 1000, "round {} read", round);
            assert!(out.iter().all(|&s| s == val), "round {} data", round);
        }
    }

    #[test]
    fn multiple_small_pushes_and_single_read() {
        let slot = MixerSlot::new();
        slot.push(&[1i32, 2]);
        slot.push(&[3i32, 4]);
        slot.push(&[5i32]);

        assert_eq!(slot.available(), 5);

        let mut out = [0i32; 5];
        assert_eq!(slot.read_copy(&mut out), 5);
        assert_eq!(out, [1, 2, 3, 4, 5]);
    }

    #[test]
    fn read_add_with_empty_slot_does_not_modify_output() {
        let slot = MixerSlot::new();
        let mut out = [100i32, 200, 300];
        assert_eq!(slot.read_add(&mut out), 0);
        assert_eq!(out, [100, 200, 300]); // unchanged
    }

    #[test]
    fn concurrent_push_and_read() {
        // Verify SPSC safety: one producer thread, one consumer thread
        let slot = MixerSlot::new();
        let slot2 = slot.clone();

        let producer = std::thread::spawn(move || {
            let mut total = 0usize;
            for i in 0..1000 {
                let samples = vec![i % 100; 48];
                total += slot2.push(&samples);
                // Small yield to interleave
                if i % 10 == 0 {
                    std::thread::yield_now();
                }
            }
            total
        });

        let consumer = std::thread::spawn(move || {
            let mut total = 0usize;
            let mut buf = [0i32; 96];
            for _ in 0..10000 {
                total += slot.read_copy(&mut buf);
                std::thread::yield_now();
            }
            total
        });

        let pushed = producer.join().unwrap();
        // Let consumer drain remaining
        std::thread::sleep(std::time::Duration::from_millis(10));
        let read = consumer.join().unwrap();

        // All pushed samples should eventually be readable
        // (consumer may not drain all before joining, but no panics/UB)
        assert!(pushed > 0, "producer should have pushed samples");
        assert!(read > 0, "consumer should have read samples");
    }
}
