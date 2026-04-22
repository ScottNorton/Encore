//! Leader streaming task — reads from the mixer network tap and
//! sends audio chunks to followers via UDP.

use super::clock;
use super::wire::{self, GroupPacket};
use crate::audio::mixer::MixerSlot;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::net::UdpSocket;

/// Frames per audio chunk sent to followers (10ms @ 48kHz).
const CHUNK_FRAMES: usize = 480;
/// Samples per chunk (stereo).
const CHUNK_SAMPLES: usize = CHUNK_FRAMES * 2;

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
    follower_addrs: Vec<std::net::SocketAddr>,
    buffer_ms: u64,
) {
    let mut read_buf = vec![0i32; CHUNK_SAMPLES];
    let mut interval = tokio::time::interval(std::time::Duration::from_millis(10));
    let mut seq: u32 = 0;

    while tap_active.load(Ordering::Relaxed) {
        interval.tick().await;

        if !tap_active.load(Ordering::Relaxed) {
            break;
        }

        // Drain all available audio from the tap in chunks
        while network_tap.available() >= CHUNK_SAMPLES {
            let read = network_tap.read_copy(&mut read_buf);
            if read < CHUNK_SAMPLES {
                break;
            }

            let play_at_us = clock::now_us() + buffer_ms * 1000;

            let packet = GroupPacket::AudioChunk {
                play_at_us,
                frame_count: CHUNK_FRAMES as u16,
                hop_count: 0,
                pcm: read_buf[..CHUNK_SAMPLES].to_vec(),
            };

            seq = seq.wrapping_add(1);
            let data = wire::encode(&packet, seq);

            // Fire-and-forget UDP unicast to each follower
            for addr in &follower_addrs {
                let _ = udp_socket.send_to(&data, addr).await;
            }
        }
    }
}
