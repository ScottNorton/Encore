//! Follower jitter buffer and playback — receives audio chunks from
//! the leader, buffers them, and pushes to the network MixerSlot.

use super::clock::ClockSync;
use super::wire::ChannelAssignment;
use std::collections::VecDeque;

/// Duration of one audio chunk in microseconds. Defined once in `wire` so it can
/// never drift from the leader's chunk size (144 frames / 48kHz = 3ms).
// ponytail: a future cross-version-robust follower could derive each gap from the
// chunk's own frame_count instead of this shared constant; same-version peers are
// guaranteed to agree, so the constant is the minimal correct choice today.
const CHUNK_DUR_US: u64 = super::wire::AUDIO_CHUNK_DUR_US;

/// Hard ceiling on buffered chunks, independent of the time-span depth cap. A
/// peer flooding chunks that share (or tightly cluster) one `play_at_us` has a
/// span of ~0, so `depth_us()` never trips the span cap; without a count cap the
/// buffer grows unbounded (OOM) and each insert's O(n) dedup scan becomes O(n²).
/// 4096 chunks is ~12s of audio at `CHUNK_DUR_US`, far above any real buffer
/// depth, so legit playback never reaches it.
const MAX_PENDING_CHUNKS: usize = 4096;

/// Reject a chunk whose PCM is implausibly larger than a real one. A legit chunk
/// is `AUDIO_CHUNK_SAMPLES`; the wire decoder will turn a ~1MB AudioChunk into a
/// 262k-sample Vec, which a flood could use to amplify the buffer toward OOM.
/// 8× the real size is generous headroom for any cross-version drift.
const MAX_CHUNK_SAMPLES: usize = super::wire::AUDIO_CHUNK_SAMPLES * 8;

/// A buffered audio chunk waiting to be played.
struct TimedChunk {
    /// Sequence number assigned by the leader (monotonic per stream).
    seq: u32,
    /// When this chunk should be played (leader's clock, in us).
    play_at_us: u64,
    /// Stereo PCM samples (interleaved L R L R ...).
    pcm: Vec<i32>,
}

/// Jitter buffer for follower audio playback.
pub struct JitterBuffer {
    chunks: VecDeque<TimedChunk>,
    /// Target buffer depth in microseconds (seeded from the controller's lead).
    target_depth_us: u64,
    /// Count of per-frame skip/dup corrections ever applied. The new
    /// prime-and-silence design never compresses or stretches time, so this
    /// stays 0 — it exists only as a regression assertion that no wobble
    /// (clicking drift correction) is ever introduced.
    corrections_applied: u64,
    /// Channel assignment for this speaker.
    channel: ChannelAssignment,
    /// Leader-clock playout watermark: chunks whose `play_at_us` is at or
    /// before this have already been played (or skipped). `None` means nothing
    /// has played yet (so a chunk at `play_at_us == 0` is still valid). Used to
    /// reject stale/late chunks so a lost packet can never slip the beat.
    played_through_us: Option<u64>,
    /// Sequence number of the most recently accepted chunk, used to drop
    /// exact-duplicate retransmissions cheaply.
    last_seq: Option<u32>,
    /// Leader-clock `play_at_us` of the most recently emitted chunk (data or
    /// synthesized silence). Used to detect gaps and fill them with exactly the
    /// right number of silent chunks so the next real chunk still lands on time.
    last_emitted_play_at: Option<u64>,
    /// Stable id (FNV of peer_id) of the leader this follower is currently
    /// streaming. `None` until set. Chunks whose `leader_id` does not match are
    /// dropped by [`accept`], so a brief two-leaders window can never merge two
    /// streams into the buffer.
    current_leader: Option<u32>,
    /// Playout delay past each chunk's `play_at` stamp (µs). This is the
    /// group's alignment element: the leader holds its own DAC back by
    /// lead + backlog-target, so a follower must play `play_at + backlog-target`
    /// to land on the same wall-clock instant. Holding the delay HERE — chunks
    /// wait in this buffer, which is also the burst armor — makes sync
    /// structural from the first chunk. (The old design expected the slot
    /// regulator to accumulate the same delay as FIFO backlog by padding
    /// frames, which could only crawl there at ~2 ms/s: streams started
    /// ~150 ms apart, converged over minutes, and re-opened the gap on every
    /// buffer dump — heard live as slow-heal desyncs plus constant
    /// padding-churn crackle on the follower.)
    playout_delay_us: u64,
}

impl JitterBuffer {
    pub fn new(buffer_ms: u16, channel: ChannelAssignment) -> Self {
        Self {
            chunks: VecDeque::with_capacity(64),
            target_depth_us: buffer_ms as u64 * 1000,
            corrections_applied: 0,
            channel,
            played_through_us: None,
            last_seq: None,
            last_emitted_play_at: None,
            current_leader: None,
            playout_delay_us: 0,
        }
    }

    /// Set the playout delay (µs) applied past every chunk's `play_at`. Must
    /// match the backlog-target half of the leader's local delay.
    pub fn set_playout_delay_us(&mut self, us: u64) {
        self.playout_delay_us = us;
    }

    /// Set the leader this follower is currently streaming (stable id, FNV of
    /// peer_id). Called on every leader change. [`accept`] drops any chunk whose
    /// `leader_id` does not match.
    pub fn set_current_leader(&mut self, leader_id: u32) {
        self.current_leader = Some(leader_id);
    }

    /// Accept a leader-tagged audio chunk: insert it only if `leader_id` matches
    /// the current leader (set via [`set_current_leader`]). A chunk from any
    /// other leader — e.g. a stale stream during a leadership handoff — is
    /// dropped so two streams never merge. Once the leader matches, this is
    /// exactly [`insert`].
    pub fn accept(&mut self, leader_id: u32, seq: u32, play_at_us: u64, pcm: Vec<i32>) {
        if self.current_leader != Some(leader_id) {
            return;
        }
        self.insert(seq, play_at_us, pcm);
    }

    /// Insert a chunk into the buffer (sorted by play_at_us).
    ///
    /// Drops exact-duplicate `seq` values and rejects chunks whose `play_at_us`
    /// has already passed the playout watermark (stale/late arrivals), so a lost
    /// or reordered packet never causes a permanent timing slip.
    pub fn insert(&mut self, seq: u32, play_at_us: u64, pcm: Vec<i32>) {
        // Reject an implausibly large chunk before it can bloat the buffer (the
        // wire decoder does not bound PCM length against frame_count).
        if pcm.len() > MAX_CHUNK_SAMPLES {
            return;
        }

        // Reject stale chunks whose slot has already played out.
        if let Some(watermark) = self.played_through_us {
            if play_at_us <= watermark {
                return;
            }
        }

        // Drop exact-duplicate sequence numbers (retransmits / reflections).
        if self.last_seq == Some(seq) || self.chunks.iter().any(|c| c.seq == seq) {
            return;
        }
        self.last_seq = Some(seq);

        let chunk = TimedChunk {
            seq,
            play_at_us,
            pcm,
        };

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

        self.enforce_depth_cap();
    }

    /// Upper bound on how many silent chunks a single lost-packet gap may be
    /// filled with (see [`emit_chunk`]). Sized at a few target depths so genuine
    /// underruns are covered but an outage re-anchors instead of ballooning. A
    /// fixed floor keeps a small fill working even when the target depth is tiny
    /// or zero (e.g. an unconfigured buffer).
    /// The depth a healthy buffer actually carries: the configured jitter
    /// target plus the playout delay's worth of deliberately-held chunks.
    /// Every cap keys off this, or the hold-back would read as overrun.
    fn effective_depth_target_us(&self) -> u64 {
        self.target_depth_us + self.playout_delay_us
    }

    fn max_gap_fill_chunks(&self) -> u64 {
        const GAP_FILL_TARGET_MULTIPLE: u64 = 4;
        const GAP_FILL_FLOOR_CHUNKS: u64 = 32; // ~320ms at 10ms/chunk
        let from_target = self
            .effective_depth_target_us()
            .saturating_mul(GAP_FILL_TARGET_MULTIPLE)
            / CHUNK_DUR_US;
        from_target.max(GAP_FILL_FLOOR_CHUNKS)
    }

    /// Hard ceiling on the buffered span (oldest..newest play_at). The span cap
    /// holds a legit buffer at ~2× the target depth, so anything wildly beyond
    /// that is a far-future outlier (a crafted or buggy chunk). Kept comfortably
    /// above the span cap's 2× so genuine overrun is never mistaken for an
    /// outlier, with a generous absolute floor for tiny/zero targets.
    fn max_buffer_span_us(&self) -> u64 {
        const FLOOR_US: u64 = 30_000_000; // 30s
        const TARGET_MULTIPLE: u64 = 8; // must exceed the span cap's 2×
        self.effective_depth_target_us()
            .saturating_mul(TARGET_MULTIPLE)
            .max(FLOOR_US)
    }

    /// Cap pending audio at ~2× the target depth. On overrun the leader is
    /// running ahead of us faster than playback can drain (or a burst arrived),
    /// so we drop the *oldest* whole chunks until the buffer is back down to the
    /// target depth. This is a one-time jump forward — never per-frame
    /// time-compression — so it cannot wobble a steady stream. `play_at_us` on
    /// the kept chunks is untouched, so playback still lands on the leader's grid.
    fn enforce_depth_cap(&mut self) {
        // Drop far-future back-outliers FIRST. A chunk (or a small cluster) whose
        // play_at sits absurdly beyond the rest — e.g. a crafted or buggy
        // AudioChunk with play_at = u64::MAX — would otherwise inflate depth_us()
        // so the span cap below front-drops every real chunk (marking each as
        // played), permanently wedging the follower to silence for the session.
        // Drop from the back while the newest chunk is more than max_buffer_span_us
        // beyond the oldest. These never played, so do NOT advance the watermark
        // (that would stale-reject every future real chunk). A legit buffer spans
        // at most ~2× the target depth, far under this bound, so real overrun is
        // never mistaken for an outlier.
        let max_span = self.max_buffer_span_us();
        while self.chunks.len() >= 2 {
            let front = self.chunks.front().unwrap().play_at_us;
            let back = self.chunks.back().unwrap().play_at_us;
            if back.saturating_sub(front) > max_span {
                self.chunks.pop_back();
            } else {
                break;
            }
        }

        let mut trimmed = false;

        // Hard count cap (see MAX_PENDING_CHUNKS): the span cap below keys off
        // depth_us(), which a same-/tight-play_at flood (span ~0) slips past.
        // Drop the oldest past the cap; note_played_through advances the
        // watermark, so the flood's remaining same-slot chunks are then
        // stale-rejected at the top of insert (O(1)) and the buffer stays bounded.
        while self.chunks.len() > MAX_PENDING_CHUNKS {
            match self.chunks.pop_front() {
                Some(dropped) => {
                    self.note_played_through(dropped.play_at_us);
                    trimmed = true;
                }
                None => break,
            }
        }

        // Span-based cap: the leader is running ahead of us faster than playback
        // can drain (or a burst arrived), so drop the *oldest* whole chunks until
        // the buffer is back down to the target depth. A one-time jump forward —
        // never per-frame time-compression — so it cannot wobble a steady stream.
        let effective_target = self.effective_depth_target_us();
        let cap = effective_target.saturating_mul(2);
        if cap != 0 && self.depth_us() > cap {
            while self.depth_us() > effective_target && self.chunks.len() > 1 {
                if let Some(dropped) = self.chunks.pop_front() {
                    // Mark the discarded slot as played so a late retransmit for
                    // it can never be re-inserted behind the new front.
                    self.note_played_through(dropped.play_at_us);
                    trimmed = true;
                }
            }
        }

        // After a forward jump the previously-emitted watermark is meaningless;
        // clearing it suppresses a spurious giant silence-fill before the new
        // front chunk (which is exactly what we just discarded our way past).
        if trimmed {
            self.last_emitted_play_at = None;
        }
    }

    /// Drain all chunks due for playout (in local clock). Converts the local
    /// deadline into leader-clock time, shifts it back by the playout delay
    /// (a chunk is due at `play_at + playout_delay`), and delegates to
    /// [`drain_due`], so both entry points share one playback path.
    ///
    /// Uses the clock's *stable* mapping: the live offset estimate can swing
    /// by whole seconds while sync probes fight radio contention, and release
    /// timing must not follow it (that read as play-then-desync on real
    /// hardware). The last converged lock, skew-extrapolated, drives playout.
    pub fn drain_ready(&mut self, local_now_us: u64, clock: &ClockSync) -> Vec<i32> {
        let leader_now_us = clock.local_to_remote_stable(local_now_us);
        self.drain_due(leader_now_us.saturating_sub(self.playout_delay_us))
    }

    /// Drain all chunks whose `play_at_us` is at or before `leader_now_us`
    /// (leader clock). Applies channel routing and gap-fill silence, but never
    /// time-compresses: a synced stream plays straight through on the leader's
    /// grid. Returns processed stereo PCM samples ready for the MixerSlot.
    pub fn drain_due(&mut self, leader_now_us: u64) -> Vec<i32> {
        let mut output = Vec::new();

        while let Some(front) = self.chunks.front() {
            if front.play_at_us <= leader_now_us {
                let chunk = self.chunks.pop_front().unwrap();
                self.emit_chunk(&chunk, &mut output);
            } else {
                break;
            }
        }

        output
    }

    /// Route a chunk's PCM, fill any preceding lost-packet gap with exactly the
    /// right amount of silence, append it to `output`, and advance the playout
    /// watermarks. No time-compression happens here — playback always lands on
    /// the chunk's own `play_at_us`.
    fn emit_chunk(&mut self, chunk: &TimedChunk, output: &mut Vec<i32>) {
        let routed = self.apply_channel_routing(&chunk.pcm);

        // Gap detection: if this chunk's play_at sits more than one chunk
        // duration past the end of the previously emitted chunk, a packet
        // was lost. Synthesize exactly gap/CHUNK_DUR silent chunks of the
        // same length so this chunk still lands on its own play_at instead
        // of slipping the whole stream earlier.
        //
        // The fill is capped at [`MAX_GAP_FILL_CHUNKS`]. A small gap (a few
        // lost packets) is a true underrun we paper over with silence; a gap
        // larger than the cap is an outage (network partition, leader stall, or
        // a pause/resume where the sample-counted play_at kept advancing) and
        // would otherwise build one i32 per silent sample — minutes of outage
        // scaling toward OOM in a single drain. The overrun depth-cap does not
        // cover this: an outage drains the buffer as fast as it fills, so it
        // never trips. Past the cap we treat the gap as a discontinuity and
        // re-anchor to this chunk's play_at (zero fill) — the chunk still lands
        // on the leader's grid; we simply do not pre-roll a multi-second wall of
        // silence to get there.
        if let Some(prev) = self.last_emitted_play_at {
            let prev_end = prev + CHUNK_DUR_US;
            if chunk.play_at_us > prev_end {
                let gap = chunk.play_at_us - prev_end;
                let missing = gap / CHUNK_DUR_US;
                if missing <= self.max_gap_fill_chunks() {
                    output.extend(std::iter::repeat_n(0, routed.len() * missing as usize));
                }
                // else: discontinuity — re-anchor (no fill). last_emitted_play_at
                // is set to this chunk's play_at below, so the next gap is
                // measured from here.
            }
        }

        output.extend_from_slice(&routed);
        self.last_emitted_play_at = Some(chunk.play_at_us);
        self.played_through_us = Some(chunk.play_at_us);
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

    /// Number of per-frame skip/dup corrections ever applied. The
    /// prime-and-silence design performs none, so this stays 0 forever; it is
    /// retained purely so tests can assert a synced stream never wobbles.
    pub fn corrections_applied(&self) -> u64 {
        self.corrections_applied
    }

    /// Current buffered audio span in microseconds: the leader-clock distance
    /// between the oldest and newest pending chunk.
    pub fn depth_us(&self) -> u64 {
        if self.chunks.len() < 2 {
            return 0;
        }
        let first = self.chunks.front().unwrap().play_at_us;
        let last = self.chunks.back().unwrap().play_at_us;
        last.saturating_sub(first)
    }

    /// Current buffer depth in microseconds (approximate). Alias of
    /// [`depth_us`] kept for existing callers (`health_percent`, telemetry).
    pub fn buffer_depth_us(&self) -> u64 {
        self.depth_us()
    }

    /// Intended buffering lead (leader-clock microseconds).
    pub fn target_depth_us(&self) -> u64 {
        self.target_depth_us
    }

    /// Leader-clock `play_at` of the most recently emitted chunk, if any.
    pub fn last_emitted_play_at_us(&self) -> Option<u64> {
        self.last_emitted_play_at
    }

    /// Number of chunks in the buffer.
    pub fn len(&self) -> usize {
        self.chunks.len()
    }

    /// Number of chunks currently pending (not yet drained). Alias of `len`
    /// used by tests that reason about accepted-vs-dropped inserts.
    pub fn pending_chunks(&self) -> usize {
        self.chunks.len()
    }

    /// Advance the playout watermark to `us` (leader clock). Chunks whose
    /// `play_at_us` is at or before this are considered already played and any
    /// later arrival for those slots is rejected by `insert`.
    pub fn note_played_through(&mut self, us: u64) {
        self.played_through_us = Some(match self.played_through_us {
            Some(w) => w.max(us),
            None => us,
        });
    }

    /// Clear the buffer (e.g. on mode change).
    pub fn clear(&mut self) {
        self.chunks.clear();
        self.last_seq = None;
        self.last_emitted_play_at = None;
        self.played_through_us = None;
    }

    /// Update channel assignment (e.g. from web UI).
    pub fn set_channel(&mut self, channel: ChannelAssignment) {
        self.channel = channel;
    }

    /// Update target buffer depth.
    ///
    // ponytail: retained as the follower-side hook for the leader-broadcast
    // target_lead_us. The host SetBufferMs command that used to call this is gone
    // (the controller owns the lead); the device-side broadcast that will drive it
    // is out of scope here.
    #[allow(dead_code)]
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

/// Fine bidirectional drift controller state for one follower->leader stream.
/// Held in the group event loop; `reset()` on every `jitter_buffer.clear()`.
pub(crate) struct DriftController {
    integ_us: i64,
    correcting: bool, // Schmitt latch
    decim: u32,       // tick decimator: bounds the sustained slew rate
    coarse_decim: u32, // rate-limits coarse fires so a sustained over-buffer can't storm coarse every tick
    smoothed_backlog_us: i64, // EMA of raw network_slot backlog (us); -1 = uninit
    pub skips: u64,   // telemetry: total stereo frames dropped (follower was ahead)
    pub dups: u64,    // telemetry: total stereo frames duplicated (follower was behind)
    pub last_error_us: i64, // telemetry: most recent error fed in
}

impl DriftController {
    pub fn new() -> Self {
        Self { integ_us: 0, correcting: false, decim: 0, coarse_decim: 0, smoothed_backlog_us: -1, skips: 0, dups: 0, last_error_us: 0 }
    }
    /// Reset loop state across a stream change. Cumulative skips/dups are kept.
    pub fn reset(&mut self) {
        self.integ_us = 0;
        self.correcting = false;
        self.decim = 0;
        self.coarse_decim = 0;
        self.last_error_us = 0;
        self.smoothed_backlog_us = -1;
    }

    /// Reset only the fine PI loop after a coarse move. Crucially does NOT touch
    /// smoothed_backlog_us: zeroing it re-primes the EMA to the still-near-full raw
    /// backlog next tick, so coarse would re-fire every 2ms (thrash). The full
    /// reset() is kept for genuine stream changes, where re-priming is correct.
    pub fn reset_fine(&mut self) {
        self.integ_us = 0;
        self.correcting = false;
        self.decim = 0;
        self.last_error_us = 0;
        // smoothed_backlog_us + coarse_decim intentionally preserved.  // keep:
    }

    /// True at most once every COARSE_PERIOD ticks, so a sustained over-buffer
    /// drains via occasional bounded snaps instead of a per-tick coarse storm.
    pub(crate) fn coarse_ready(&mut self) -> bool {
        const COARSE_PERIOD: u32 = 16; // ~32ms at the 2ms drain tick
        self.coarse_decim += 1;
        if self.coarse_decim >= COARSE_PERIOD {
            self.coarse_decim = 0;
            true
        } else {
            false
        }
    }

    /// EMA-smooth the raw network_slot backlog so the controller tracks the
    /// trend, not the per-tick WiFi burst jitter (raw ~+-5ms was thrashing the
    /// coarse path every tick). ~32ms time constant at the 2ms drain tick; the
    /// -1 sentinel primes the filter on the first sample so it never ramps from
    /// zero.
    pub(crate) fn smoothed_backlog(&mut self, raw_us: i64) -> i64 {
        if self.smoothed_backlog_us < 0 {
            self.smoothed_backlog_us = raw_us;
        } else {
            self.smoothed_backlog_us += (raw_us - self.smoothed_backlog_us) / 16;
        }
        self.smoothed_backlog_us
    }
}

/// Per-drain fine drift correction. Returns signed STEREO FRAMES to apply this
/// tick: >0 = drop that many trailing frames (follower AHEAD of leader timeline),
/// <0 = duplicate the last frame that many times (follower BEHIND), 0 = leave
/// untouched. At most +/-MAX_CORRECTION per call, and at most one edit per
/// DECIMATE ticks once engaged, so the timeline slews ~1ms/s max — inaudible and
/// self-limiting even if gains are mistuned. Pure function: host-testable.
///
/// Sign convention: error_us > 0 means the follower is playing AHEAD (buffer too
/// full / DAC draining slow relative to the leader grid) => drop. error_us < 0 =>
/// behind => duplicate.
pub(crate) fn drift_correction(error_us: i64, converged: bool, st: &mut DriftController) -> i32 {
    const DEAD_BAND_US: i64 = 2_000; // locked stream: never touch it
    const ENGAGE_US: i64 = 5_000;    // Schmitt: must exceed this to START correcting
    const MAX_CORRECTION: i32 = 1;   // <=1 stereo frame (~20.8us) per fire
    const DECIMATE: u32 = 5;         // >=5 ticks between fires => sustained slew ~<=2ms/s
    const KP_NUM: i64 = 1;
    const KP_DEN: i64 = 8;
    const KI_NUM: i64 = 1;
    const KI_DEN: i64 = 4096;
    const I_CLAMP: i64 = 2_000_000;

    st.last_error_us = error_us;

    // Never correct against an untrusted clock; bleed state for a clean resume.
    if !converged {
        st.integ_us = 0;
        st.correcting = false;
        st.decim = 0;
        return 0;
    }

    let mag = error_us.abs();

    // Dead-band: locked. Relax the integrator, release the latch, do nothing.
    if mag < DEAD_BAND_US {
        st.correcting = false;
        st.integ_us -= st.integ_us / 8;
        st.decim = 0;
        return 0;
    }

    // Accumulate every tick outside the dead-band (so a small but persistent bias
    // is fully observed even before the latch engages) and clamp against windup.
    st.integ_us = (st.integ_us + error_us).clamp(-I_CLAMP, I_CLAMP);
    let u_us = (KP_NUM * error_us) / KP_DEN + (KI_NUM * st.integ_us) / KI_DEN;

    // Schmitt hysteresis: a fresh stream must exceed ENGAGE_US to START
    // correcting. A sub-ENGAGE error that nonetheless persists long enough for the
    // integral term to demand a full frame also engages (otherwise a steady small
    // drift inside the hysteresis band could never be corrected). Once latched,
    // we keep going until back inside the dead-band (handled above).
    let was_correcting = st.correcting;
    if !was_correcting && mag < ENGAGE_US && (u_us * 48).abs() / 1_000 < 1 {
        return 0;
    }
    st.correcting = true;

    // The engaging tick itself does not advance the decimator, so the first edit
    // lands exactly DECIMATE ticks after engaging. Once engaged, emit an edit only
    // every DECIMATE ticks, capping the sustained slew rate.
    if !was_correcting {
        return 0;
    }
    st.decim += 1;
    if st.decim < DECIMATE {
        return 0;
    }
    st.decim = 0;

    let mut frames = (u_us * 48) / 1_000; // us -> frames @48kHz (48 frames/ms)
    frames = frames.clamp(-(MAX_CORRECTION as i64), MAX_CORRECTION as i64);
    let frames = frames as i32;

    if frames > 0 {
        st.skips += frames as u64;
    } else if frames < 0 {
        st.dups += (-frames) as u64;
    }
    frames
}

/// Coarse one-shot drift correction. When the buffer error exceeds ~one grain,
/// drop (slot too full) or pad (too empty) WHOLE AUDIO_CHUNK_FRAMES grains from
/// `samples` to snap the network_slot backlog toward target in a few fires,
/// instead of the fine loop's ~2ms/s slew (which would take ~75s to drain a
/// 150ms surplus). Updates skip/dup counters; the caller resets the fine
/// integrator after a coarse move. Pure -> host-testable on a plain Vec<i32>.
pub(crate) fn coarse_correction(error_us: i64, samples: &mut Vec<i32>, st: &mut DriftController) {
    const COARSE_FRAMES: usize = super::wire::AUDIO_CHUNK_FRAMES; // one wire audio chunk, ~3ms @48k
    const COARSE_THRESH_US: i64 = (COARSE_FRAMES as i64) * 1_000_000 / 48_000;
    if COARSE_THRESH_US == 0 { return; }
    let grains = (error_us.abs() / COARSE_THRESH_US) as usize;
    const MAX_COARSE_GRAINS: usize = 4; // <= ~12ms edit per fire; bounds the pad allocation hard
    let grains = grains.min(MAX_COARSE_GRAINS);
    let frames = grains * COARSE_FRAMES;
    if error_us > 0 {
        // Too full -> drop whole grains from the tail (skip ahead).
        let drop = (frames * 2).min(samples.len().saturating_sub(2));
        samples.truncate(samples.len() - drop);
        st.skips += (drop / 2) as u64;
    } else if samples.len() >= 2 {
        // Too empty -> pad whole grains of the last frame (dup).
        let last = [samples[samples.len() - 2], samples[samples.len() - 1]];
        for _ in 0..frames { samples.extend_from_slice(&last); }
        st.dups += frames as u64;
    }
}

#[cfg(test)]
mod tests {
    use super::super::clock::ClockSync;
    use super::*;

    fn make_stereo_pcm(frames: usize, left: i32, right: i32) -> Vec<i32> {
        let mut pcm = Vec::with_capacity(frames * 2);
        for _ in 0..frames {
            pcm.push(left);
            pcm.push(right);
        }
        pcm
    }

    /// Build `n` stereo frames (interleaved L R) of constant value `val`.
    fn frames(n: usize, val: i32) -> Vec<i32> {
        make_stereo_pcm(n, val, val)
    }

    /// True when every sample in `pcm` is zero (a synthesized silent chunk).
    fn is_silence(pcm: &[i32]) -> bool {
        pcm.iter().all(|&s| s == 0)
    }

    /// Repeatedly drain everything that is due under an identity clock (no
    /// offset/skew) at a local time far past every chunk's play_at.
    fn drain_all(jb: &mut JitterBuffer) -> Vec<i32> {
        let mut clock = ClockSync::new();
        clock.process_response(0, 0, 0, 0); // identity: remote == local
        jb.drain_ready(u64::MAX, &clock)
    }

    // ── playout delay (the group alignment element) ──

    #[test]
    fn playout_delay_holds_chunks_past_play_at() {
        let mut jb = JitterBuffer::new(35, ChannelAssignment::Stereo);
        jb.set_playout_delay_us(150_000);
        let mut clock = ClockSync::new();
        clock.process_response(0, 0, 0, 0); // identity: remote == local
        jb.insert(1, 1_000, frames(144, 7));

        // At play_at the chunk is NOT yet due — the leader's own DAC plays it
        // backlog-target later, and we must land on the same instant.
        assert!(jb.drain_ready(1_000, &clock).is_empty());
        assert!(jb.drain_ready(140_000, &clock).is_empty());
        // Due exactly one playout delay past the stamp.
        let out = jb.drain_ready(151_100, &clock);
        assert_eq!(out.len(), 144 * 2, "chunk must release at play_at + delay");
    }

    #[test]
    fn playout_delay_does_not_trip_depth_caps() {
        // ~185ms of deliberately-held chunks must read as healthy depth, not
        // overrun: the effective target includes the playout delay.
        let mut jb = JitterBuffer::new(35, ChannelAssignment::Stereo);
        jb.set_playout_delay_us(150_000);
        let n = (185_000 / CHUNK_DUR_US) as u32; // ~61 chunks
        for i in 0..n {
            jb.insert(i, 1_000 + i as u64 * CHUNK_DUR_US, frames(144, 5));
        }
        assert_eq!(
            jb.chunks.len(),
            n as usize,
            "held chunks must not be front-dropped as overrun"
        );
    }

    #[test]
    fn insert_and_drain_in_order() {
        let mut jb = JitterBuffer::new(80, ChannelAssignment::Stereo);
        let mut clock = ClockSync::new();
        // Zero offset clock
        clock.process_response(0, 0, 0, 0);

        jb.insert(0, 1000, make_stereo_pcm(4, 100, 200));
        jb.insert(1, 2000, make_stereo_pcm(4, 300, 400));

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

        jb.insert(0, 10_000, make_stereo_pcm(4, 1, 2));
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
        jb.insert(0, 0, make_stereo_pcm(2, 100, 200));
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

        jb.insert(0, 0, make_stereo_pcm(2, 100, 200));
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

        jb.insert(0, 0, make_stereo_pcm(2, 100, 200));
        let out = jb.drain_ready(1000, &clock);

        assert_eq!(out, vec![100, 200, 100, 200]);
    }

    #[test]
    fn clear_resets_buffer() {
        let mut jb = JitterBuffer::new(80, ChannelAssignment::Stereo);
        jb.insert(0, 0, make_stereo_pcm(4, 1, 2));
        assert_eq!(jb.len(), 1);
        jb.clear();
        assert_eq!(jb.len(), 0);
    }

    #[test]
    fn out_of_order_insertion() {
        let mut jb = JitterBuffer::new(80, ChannelAssignment::Stereo);
        let mut clock = ClockSync::new();
        clock.process_response(0, 0, 0, 0);

        // Insert out of order (seq matches arrival order, play_at is shuffled)
        jb.insert(0, 3000, make_stereo_pcm(1, 30, 30));
        jb.insert(1, 1000, make_stereo_pcm(1, 10, 10));
        jb.insert(2, 2000, make_stereo_pcm(1, 20, 20));

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
        jb.insert(0, 1000, make_stereo_pcm(1, 42, 42));

        // At local time 400, shouldn't drain (play_at_local = 500)
        let out = jb.drain_ready(400, &clock);
        assert!(out.is_empty());

        // At local time 600, should drain
        let out = jb.drain_ready(600, &clock);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0], 42);
    }

    #[test]
    fn single_lost_chunk_inserts_exact_silence_no_slip() {
        let d = CHUNK_DUR_US;
        let mut jb = JitterBuffer::new(60, ChannelAssignment::Stereo);
        jb.insert(0, d, frames(480, 1)); // seq 0 @ play_at d
                                         // seq 1 (play_at 2*d) is LOST
        jb.insert(2, 3 * d, frames(480, 3)); // seq 2 @ play_at 3*d
        let out = drain_all(&mut jb);
        // Output must be 3 chunks long: data, SILENCE, data — chunk 2 still lands at 3*d.
        assert_eq!(out.len(), 480 * 2 * 3);
        assert!(is_silence(&out[480 * 2..480 * 2 * 2]));
        // Sanity: the bookends are the real data, not slipped together.
        assert!(out[..480 * 2].iter().all(|&s| s == 1));
        assert!(out[480 * 2 * 2..].iter().all(|&s| s == 3));
    }

    #[test]
    fn duplicate_seq_is_dropped() {
        let mut jb = JitterBuffer::new(60, ChannelAssignment::Stereo);
        jb.insert(0, 10_000, frames(480, 1));
        jb.insert(0, 10_000, frames(480, 9)); // dup seq -> ignored
        assert_eq!(jb.pending_chunks(), 1);
    }

    #[test]
    fn stale_chunk_already_past_is_rejected() {
        let mut jb = JitterBuffer::new(60, ChannelAssignment::Stereo);
        jb.note_played_through(50_000); // playout index already at 50ms
        jb.insert(0, 10_000, frames(480, 1)); // play_at in the past
        assert_eq!(jb.pending_chunks(), 0);
    }

    #[test]
    fn synced_zero_drift_stream_produces_zero_corrections() {
        let mut jb = JitterBuffer::new(60, ChannelAssignment::Stereo);
        // Feed a perfectly-spaced (exactly one chunk apart), on-time stream.
        let d = CHUNK_DUR_US;
        for seq in 0..200u32 {
            jb.insert(seq, d + seq as u64 * d, frames(480, 1));
            let _ = jb.drain_due(d + seq as u64 * d + 60_000);
        }
        assert_eq!(
            jb.corrections_applied(),
            0,
            "a synced stream must never skip/dup"
        );
    }

    #[test]
    fn follower_drops_foreign_leader_chunks() {
        let mut jb = JitterBuffer::new(60, ChannelAssignment::Stereo);
        jb.set_current_leader(7);
        jb.accept(7, 0, 10_000, frames(480, 1)); // current leader -> accepted
        jb.accept(9, 1, 20_000, frames(480, 1)); // foreign leader -> dropped
        assert_eq!(jb.pending_chunks(), 1);
    }

    #[test]
    fn overrun_discards_to_target_not_timecompress() {
        let mut jb = JitterBuffer::new(60, ChannelAssignment::Stereo);
        for seq in 0..50u32 {
            jb.insert(seq, 10_000 + seq as u64 * 10_000, frames(480, 1));
        }
        // Way overfull: depth >> 2x target -> discard down to target, no frame skipping.
        jb.drain_due(0);
        assert!(jb.depth_us() <= 2 * 60_000);
        assert_eq!(jb.corrections_applied(), 0);
    }

    #[test]
    fn huge_gap_does_not_balloon_silence() {
        // A 60s outage (leader stall / network partition / pause-resume where the
        // sample-counted play_at keeps advancing). The buffer drains as fast as it
        // fills, so the overrun depth-cap never trips — the only thing carrying
        // across the outage is `last_emitted_play_at`. The post-gap chunk arrives
        // 600 chunks in the future; the gap-fill must NOT allocate one silent
        // chunk per missing slot (~23 MB built and pushed in a single drain).
        // Beyond a few target depths it must treat the gap as a discontinuity and
        // re-anchor instead of fill.
        let mut jb = JitterBuffer::new(60, ChannelAssignment::Stereo);
        // Anchor chunk: insert and drain it so the buffer is empty (the depth-cap
        // never sees a deep buffer — this is the outage path, not the burst path).
        jb.insert(0, 10_000, frames(480, 1));
        let first = drain_all(&mut jb);
        assert_eq!(first.len(), 480 * 2, "anchor chunk drains by itself");
        // 60s later the next real chunk arrives. The buffer was empty, so nothing
        // got discarded; `last_emitted_play_at` still points at the anchor.
        jb.insert(1, 10_000 + 60_000_000, frames(480, 3));
        let out = drain_all(&mut jb);
        // The unbounded bug would emit ~6000 silent chunks ahead of the data here.
        // A bounded fill adds at most a handful. Cap well under that.
        let max_chunks = 64;
        assert!(
            out.len() <= 480 * 2 * max_chunks,
            "gap-fill ballooned to {} samples ({} chunks)",
            out.len(),
            out.len() / (480 * 2)
        );
        // The post-gap real chunk must still land (no loss): its data is the tail.
        assert!(out[out.len() - 480 * 2..].iter().all(|&s| s == 3));
    }

    #[test]
    fn same_play_at_flood_stays_bounded() {
        // A peer flooding chunks with distinct seqs but one shared FUTURE
        // play_at (span ~0) slips past the span-based cap. The count cap must
        // still bound the buffer instead of letting it grow toward OOM.
        let mut jb = JitterBuffer::new(50, ChannelAssignment::Stereo);
        jb.set_current_leader(1);
        for seq in 0..(MAX_PENDING_CHUNKS as u32 + 5_000) {
            jb.accept(1, seq, 1_000_000, frames(144, 1));
        }
        assert!(
            jb.pending_chunks() <= MAX_PENDING_CHUNKS,
            "buffer exceeded the count cap: {}",
            jb.pending_chunks()
        );
    }

    #[test]
    fn oversized_chunk_is_rejected() {
        let mut jb = JitterBuffer::new(50, ChannelAssignment::Stereo);
        jb.set_current_leader(1);
        jb.accept(1, 0, 1_000_000, vec![0i32; MAX_CHUNK_SAMPLES + 1]);
        assert_eq!(
            jb.pending_chunks(),
            0,
            "an implausibly large chunk must be dropped at the door"
        );
        // A legit-sized chunk is still accepted.
        jb.accept(1, 1, 1_000_000, frames(144, 1));
        assert_eq!(jb.pending_chunks(), 1);
    }

    #[test]
    fn far_future_poison_chunk_does_not_wedge() {
        // A single crafted chunk at play_at = u64::MAX must not anchor the span
        // and make the span cap evict every real chunk (permanent silence for
        // the session). The far-future back-outlier drop discards it.
        let mut jb = JitterBuffer::new(50, ChannelAssignment::Stereo);
        jb.set_current_leader(1);
        jb.accept(1, 0, u64::MAX, frames(144, 1)); // poison arrives first
        let d = CHUNK_DUR_US;
        let mut delivered = 0;
        for i in 1..200u32 {
            jb.accept(1, i, i as u64 * d, frames(144, 1));
            delivered += jb.drain_due(i as u64 * d + d).len();
        }
        assert!(delivered > 0, "far-future poison wedged the buffer to silence");
    }

    #[test]
    fn dead_band_locks_a_synced_stream() {
        let mut st = DriftController::new();
        for _ in 0..1000 {
            assert_eq!(drift_correction(1_500, true, &mut st), 0);
            assert_eq!(drift_correction(-1_500, true, &mut st), 0);
        }
        assert_eq!(st.skips, 0);
        assert_eq!(st.dups, 0);
    }

    #[test]
    fn unconverged_clock_never_corrects() {
        let mut st = DriftController::new();
        assert_eq!(drift_correction(500_000, false, &mut st), 0);
        assert_eq!(st.integ_us, 0);
    }

    #[test]
    fn ahead_skips_behind_dups_within_bound() {
        let mut a = DriftController::new();
        let mut last = 0;
        for _ in 0..6 { last = drift_correction(100_000, true, &mut a); }
        assert_eq!(last, 1, "ahead must drop exactly one frame per fire");
        let mut b = DriftController::new();
        let mut lastb = 0;
        for _ in 0..6 { lastb = drift_correction(-100_000, true, &mut b); }
        assert_eq!(lastb, -1, "behind must dup exactly one frame per fire");
    }

    #[test]
    fn hysteresis_requires_engage_then_holds_until_deadband() {
        let mut st = DriftController::new();
        assert_eq!(drift_correction(3_000, true, &mut st), 0);
        for _ in 0..5 { drift_correction(6_000, true, &mut st); }
        assert!(st.correcting);
        drift_correction(1_000, true, &mut st);
        assert!(!st.correcting);
        assert_eq!(drift_correction(3_000, true, &mut st), 0);
    }

    #[test]
    fn persistent_ahead_bias_nets_to_skips_bounded_per_tick() {
        let mut st = DriftController::new();
        let mut total = 0i64;
        for _ in 0..10_000 {
            let c = drift_correction(4_000, true, &mut st);
            assert!(c.abs() <= 1, "slew bounded every tick");
            total += c as i64;
        }
        assert!(total > 0, "a persistent ahead bias must net to skips, got {total}");
    }

    #[test]
    fn windup_is_clamped() {
        let mut st = DriftController::new();
        for _ in 0..1_000_000 { let _ = drift_correction(1_000_000, true, &mut st); }
        assert!(st.integ_us.abs() <= 2_000_000);
    }

    #[test]
    fn sustained_slew_is_rate_limited_by_decimator() {
        let mut st = DriftController::new();
        let mut fires = 0;
        for _ in 0..1000 { if drift_correction(200_000, true, &mut st) != 0 { fires += 1; } }
        assert!(fires <= 1000 / 5 + 1, "decimator must cap firing rate, got {fires}");
    }

    #[test]
    fn coarse_drops_when_too_full_and_pads_when_too_empty() {
        let mut st = DriftController::new();
        let mut full = make_stereo_pcm(3000, 1, 1);
        let before = full.len();
        coarse_correction(30_000, &mut full, &mut st); // +30ms -> drop grains
        assert!(full.len() < before, "too-full must drop frames");
        assert!(st.skips > 0);
        let mut empty = make_stereo_pcm(100, 2, 2);
        let before2 = empty.len();
        coarse_correction(-30_000, &mut empty, &mut st); // -30ms -> pad grains
        assert!(empty.len() > before2, "too-empty must pad frames");
        assert!(st.dups > 0);
        // sub-grain error does nothing
        let mut tiny = make_stereo_pcm(500, 3, 3);
        let before3 = tiny.len();
        coarse_correction(1_000, &mut tiny, &mut st);
        assert_eq!(tiny.len(), before3, "sub-grain error must not change length");
    }

    #[test]
    fn reset_fine_preserves_ema_full_reset_clears_it() {
        let mut st = DriftController::new();
        st.smoothed_backlog(200_000);
        st.reset_fine();
        assert_ne!(st.smoothed_backlog_us, -1, "reset_fine must keep the EMA primed");
        st.reset();
        assert_eq!(st.smoothed_backlog_us, -1, "full reset re-primes the EMA");
    }

    #[test]
    fn coarse_ready_rate_limits() {
        let mut st = DriftController::new();
        let fires = (0..160).filter(|_| st.coarse_ready()).count();
        assert!(fires <= 160 / 16 + 1, "coarse must be rate-limited, got {fires}");
    }

    #[test]
    fn coarse_grains_are_capped() {
        let mut st = DriftController::new();
        let mut s = make_stereo_pcm(50, 1, 1);
        let before = s.len();
        coarse_correction(-2_000_000, &mut s, &mut st); // huge underflow would pad huge without the cap
        assert!(s.len() - before <= 4 * 144 * 2, "pad must be capped at MAX_COARSE_GRAINS");
    }
}
