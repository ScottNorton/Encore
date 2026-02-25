//! Follower jitter buffer and playback — receives audio chunks from
//! the leader, buffers them, and pushes to the network MixerSlot.

use super::clock::ClockSync;
use super::wire::ChannelAssignment;
use std::collections::VecDeque;

/// A buffered audio chunk waiting to be played.
struct TimedChunk {
    /// When this chunk should be played (leader's clock, in us).
    play_at_us: u64,
    /// Stereo PCM samples (interleaved L R L R ...).
    pcm: Vec<i32>,
}

/// Jitter buffer for follower audio playback.
pub struct JitterBuffer {
    chunks: VecDeque<TimedChunk>,
    /// Target buffer depth in microseconds (derived from config buffer_ms).
    target_depth_us: u64,
    /// Drift correction state: accumulated sub-sample error.
    drift_accumulator: i64,
    /// Channel assignment for this speaker.
    channel: ChannelAssignment,
}

impl JitterBuffer {
    pub fn new(buffer_ms: u16, channel: ChannelAssignment) -> Self {
        Self {
            chunks: VecDeque::with_capacity(64),
            target_depth_us: buffer_ms as u64 * 1000,
            drift_accumulator: 0,
            channel,
        }
    }

    /// Insert a chunk into the buffer (sorted by play_at_us).
    pub fn insert(&mut self, play_at_us: u64, pcm: Vec<i32>) {
        let chunk = TimedChunk { play_at_us, pcm };

        // Insert in sorted order (most chunks arrive in order, so check tail first)
        if self.chunks.is_empty() || self.chunks.back().unwrap().play_at_us <= play_at_us {
            self.chunks.push_back(chunk);
        } else {
            // Binary search for insertion point (rare — out-of-order packet)
            let pos = self
                .chunks
                .iter()
                .position(|c| c.play_at_us > play_at_us)
                .unwrap_or(self.chunks.len());
            self.chunks.insert(pos, chunk);
        }
    }

    /// Drain all chunks whose play_at time has arrived (in local clock).
    /// Applies channel routing and drift correction. Returns processed
    /// stereo PCM samples ready for the MixerSlot.
    pub fn drain_ready(
        &mut self,
        local_now_us: u64,
        clock: &ClockSync,
    ) -> Vec<i32> {
        let mut output = Vec::new();

        while let Some(front) = self.chunks.front() {
            // Convert leader's play_at to our local clock
            let local_play_at = clock.remote_to_local(front.play_at_us);

            if local_now_us >= local_play_at {
                let chunk = self.chunks.pop_front().unwrap();
                let routed = self.apply_channel_routing(&chunk.pcm);
                output.extend_from_slice(&routed);
            } else {
                break;
            }
        }

        // Apply drift correction if we have output
        if !output.is_empty() {
            self.apply_drift_correction(&mut output);
        }

        output
    }

    /// Apply channel routing to stereo PCM:
    /// - Stereo: pass through
    /// - Left: extract L samples, duplicate to both channels
    /// - Right: extract R samples, duplicate to both channels
    fn apply_channel_routing(&self, pcm: &[i32]) -> Vec<i32> {
        match self.channel {
            ChannelAssignment::Stereo => pcm.to_vec(),
            ChannelAssignment::Left => {
                let frames = pcm.len() / 2;
                let mut out = Vec::with_capacity(frames * 2);
                for i in 0..frames {
                    let l = pcm[i * 2];
                    out.push(l);
                    out.push(l);
                }
                out
            }
            ChannelAssignment::Right => {
                let frames = pcm.len() / 2;
                let mut out = Vec::with_capacity(frames * 2);
                for i in 0..frames {
                    let r = pcm[i * 2 + 1];
                    out.push(r);
                    out.push(r);
                }
                out
            }
        }
    }

    /// Monitor buffer fill level and apply micro-corrections.
    /// If buffer is overfull: skip 1 sample per 1000 frames (~0.1% speedup)
    /// If buffer is underfull: duplicate 1 sample per 1000 frames (~0.1% slowdown)
    fn apply_drift_correction(&mut self, samples: &mut Vec<i32>) {
        let current_depth_us = self.buffer_depth_us();
        let target_half = self.target_depth_us / 2;

        // Check if we're significantly off target
        let drift_threshold_us = 10_000; // 10ms
        let frames = samples.len() / 2;

        if current_depth_us > target_half + drift_threshold_us && frames > 2 {
            // Overfull: skip one stereo frame every 1000 frames
            self.drift_accumulator += 1;
            if self.drift_accumulator >= 1000 {
                // Remove one stereo frame (2 samples) from the middle
                let mid = (samples.len() / 2) & !1; // align to frame boundary
                if mid + 2 <= samples.len() {
                    samples.drain(mid..mid + 2);
                }
                self.drift_accumulator = 0;
            }
        } else if current_depth_us + drift_threshold_us < target_half && frames > 2 {
            // Underfull: duplicate one stereo frame every 1000 frames
            self.drift_accumulator -= 1;
            if self.drift_accumulator <= -1000 {
                let mid = (samples.len() / 2) & !1;
                if mid + 2 <= samples.len() {
                    let l = samples[mid];
                    let r = samples[mid + 1];
                    samples.insert(mid, r);
                    samples.insert(mid, l);
                }
                self.drift_accumulator = 0;
            }
        }
    }

    /// Current buffer depth in microseconds (approximate).
    pub fn buffer_depth_us(&self) -> u64 {
        if self.chunks.len() < 2 {
            return 0;
        }
        let first = self.chunks.front().unwrap().play_at_us;
        let last = self.chunks.back().unwrap().play_at_us;
        last.saturating_sub(first)
    }

    /// Number of chunks in the buffer.
    pub fn len(&self) -> usize {
        self.chunks.len()
    }

    /// Clear the buffer (e.g. on mode change).
    pub fn clear(&mut self) {
        self.chunks.clear();
        self.drift_accumulator = 0;
    }

    /// Update channel assignment (e.g. from web UI).
    pub fn set_channel(&mut self, channel: ChannelAssignment) {
        self.channel = channel;
    }

    /// Update target buffer depth.
    pub fn set_buffer_ms(&mut self, buffer_ms: u16) {
        self.target_depth_us = buffer_ms as u64 * 1000;
    }

    /// Buffer health as a percentage (0-100). 100 = at target depth.
    pub fn health_percent(&self) -> u8 {
        if self.target_depth_us == 0 {
            return 100;
        }
        let fill = self.buffer_depth_us();
        ((fill as f64 / self.target_depth_us as f64) * 100.0).min(100.0) as u8
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::clock::ClockSync;

    fn make_stereo_pcm(frames: usize, left: i32, right: i32) -> Vec<i32> {
        let mut pcm = Vec::with_capacity(frames * 2);
        for _ in 0..frames {
            pcm.push(left);
            pcm.push(right);
        }
        pcm
    }

    #[test]
    fn insert_and_drain_in_order() {
        let mut jb = JitterBuffer::new(80, ChannelAssignment::Stereo);
        let mut clock = ClockSync::new();
        // Zero offset clock
        clock.process_response(0, 0, 0, 0);

        jb.insert(1000, make_stereo_pcm(4, 100, 200));
        jb.insert(2000, make_stereo_pcm(4, 300, 400));

        assert_eq!(jb.len(), 2);

        // Drain at time 1500: only first chunk should come out
        let out = jb.drain_ready(1500, &clock);
        assert_eq!(out.len(), 8); // 4 frames * 2 channels
        assert_eq!(out[0], 100);
        assert_eq!(out[1], 200);
        assert_eq!(jb.len(), 1);

        // Drain at time 2500: second chunk
        let out = jb.drain_ready(2500, &clock);
        assert_eq!(out.len(), 8);
        assert_eq!(out[0], 300);
        assert_eq!(out[1], 400);
        assert_eq!(jb.len(), 0);
    }

    #[test]
    fn nothing_drained_before_play_time() {
        let mut jb = JitterBuffer::new(80, ChannelAssignment::Stereo);
        let mut clock = ClockSync::new();
        clock.process_response(0, 0, 0, 0);

        jb.insert(10_000, make_stereo_pcm(4, 1, 2));
        let out = jb.drain_ready(5_000, &clock);
        assert!(out.is_empty());
        assert_eq!(jb.len(), 1);
    }

    #[test]
    fn channel_routing_left() {
        let mut jb = JitterBuffer::new(80, ChannelAssignment::Left);
        let mut clock = ClockSync::new();
        clock.process_response(0, 0, 0, 0);

        // Input: L=100, R=200 per frame
        jb.insert(0, make_stereo_pcm(2, 100, 200));
        let out = jb.drain_ready(1000, &clock);

        // Left channel routing: L duplicated to both channels
        assert_eq!(out.len(), 4);
        assert_eq!(out[0], 100); // L
        assert_eq!(out[1], 100); // L duplicated to R
        assert_eq!(out[2], 100); // L
        assert_eq!(out[3], 100); // L duplicated to R
    }

    #[test]
    fn channel_routing_right() {
        let mut jb = JitterBuffer::new(80, ChannelAssignment::Right);
        let mut clock = ClockSync::new();
        clock.process_response(0, 0, 0, 0);

        jb.insert(0, make_stereo_pcm(2, 100, 200));
        let out = jb.drain_ready(1000, &clock);

        // Right channel routing: R duplicated to both channels
        assert_eq!(out.len(), 4);
        assert_eq!(out[0], 200); // R duplicated to L
        assert_eq!(out[1], 200); // R
        assert_eq!(out[2], 200); // R duplicated to L
        assert_eq!(out[3], 200); // R
    }

    #[test]
    fn channel_routing_stereo_passthrough() {
        let mut jb = JitterBuffer::new(80, ChannelAssignment::Stereo);
        let mut clock = ClockSync::new();
        clock.process_response(0, 0, 0, 0);

        jb.insert(0, make_stereo_pcm(2, 100, 200));
        let out = jb.drain_ready(1000, &clock);

        assert_eq!(out, vec![100, 200, 100, 200]);
    }

    #[test]
    fn clear_resets_buffer() {
        let mut jb = JitterBuffer::new(80, ChannelAssignment::Stereo);
        jb.insert(0, make_stereo_pcm(4, 1, 2));
        assert_eq!(jb.len(), 1);
        jb.clear();
        assert_eq!(jb.len(), 0);
    }

    #[test]
    fn out_of_order_insertion() {
        let mut jb = JitterBuffer::new(80, ChannelAssignment::Stereo);
        let mut clock = ClockSync::new();
        clock.process_response(0, 0, 0, 0);

        // Insert out of order
        jb.insert(3000, make_stereo_pcm(1, 30, 30));
        jb.insert(1000, make_stereo_pcm(1, 10, 10));
        jb.insert(2000, make_stereo_pcm(1, 20, 20));

        // Should drain in order
        let out = jb.drain_ready(5000, &clock);
        assert_eq!(out.len(), 6);
        assert_eq!(out[0], 10);
        assert_eq!(out[2], 20);
        assert_eq!(out[4], 30);
    }

    #[test]
    fn clock_offset_adjusts_play_time() {
        let mut jb = JitterBuffer::new(80, ChannelAssignment::Stereo);
        let mut clock = ClockSync::new();
        // Remote clock is 500us ahead of us
        clock.process_response(0, 500, 500, 0); // offset = 500

        // Chunk with remote play_at = 1000
        // Local equivalent = 1000 - 500 = 500
        jb.insert(1000, make_stereo_pcm(1, 42, 42));

        // At local time 400, shouldn't drain (play_at_local = 500)
        let out = jb.drain_ready(400, &clock);
        assert!(out.is_empty());

        // At local time 600, should drain
        let out = jb.drain_ready(600, &clock);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0], 42);
    }
}
