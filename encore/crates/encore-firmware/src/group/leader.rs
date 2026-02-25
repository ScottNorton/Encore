//! Leader streaming task — reads from the mixer network tap and
//! distributes audio chunks to all follower peers.

use super::clock;
use super::wire::GroupPacket;
use crate::audio::mixer::MixerSlot;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// Frames per audio chunk sent to followers (10ms @ 48kHz).
const CHUNK_FRAMES: usize = 480;
/// Samples per chunk (stereo).
const CHUNK_SAMPLES: usize = CHUNK_FRAMES * 2;

/// Read from the mixer tap and send audio chunks to followers.
///
/// Runs on a dedicated tokio task. Reads from `network_tap` whenever
/// enough samples are available, wraps in AudioChunk with a play_at
/// timestamp, and broadcasts to all connected followers via PeerManager.
///
/// The `buffer_ms` parameter is added to the current time to create
/// the play_at timestamp, giving followers time to buffer and sync.
pub async fn leader_stream_task(
    network_tap: Arc<MixerSlot>,
    tap_active: Arc<AtomicBool>,
    peer_tx: tokio::sync::mpsc::Sender<LeaderAction>,
    buffer_ms: u64,
) {
    let mut read_buf = vec![0i32; CHUNK_SAMPLES];
    let mut interval = tokio::time::interval(std::time::Duration::from_millis(10));

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

            if peer_tx
                .send(LeaderAction::BroadcastAudio(packet))
                .await
                .is_err()
            {
                break;
            }
        }
    }
}

/// Actions from the leader stream task to the group subsystem.
#[derive(Debug)]
pub enum LeaderAction {
    BroadcastAudio(GroupPacket),
}
