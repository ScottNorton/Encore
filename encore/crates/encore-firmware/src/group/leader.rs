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
}

impl StreamTimeline {
    fn new(epoch_us: u64, lead_us: u64) -> Self {
        Self {
            epoch_us,
            lead_us,
            chunk_index: 0,
        }
    }

    /// Return the `play_at` for the next chunk and advance by exactly one chunk.
    fn next_play_at(&mut self) -> u64 {
        let play_at = self.epoch_us + self.chunk_index * CHUNK_DUR_US + self.lead_us;
        self.chunk_index += 1;
        play_at
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
