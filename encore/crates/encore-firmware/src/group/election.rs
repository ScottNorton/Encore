//! Leader election by connection quality.
//!
//! Speakers elect the best-connected peer as leader using a deterministic
//! scoring algorithm. The speaker with the lowest score wins. Primary factor
//! is average RTT to all connected peers (more central = lower RTT = better).

use std::collections::HashMap;
use std::time::Instant;

/// Cooldown between elections to prevent election storms.
const ELECTION_COOLDOWN_SECS: u64 = 5;
/// How long to wait for votes before declaring a winner.
const ELECTION_TIMEOUT_MS: u64 = 1500;

/// Simple FNV-1a hash for deterministic tiebreaking.
fn fnv_hash(s: &str) -> u64 {
    let mut hash: u64 = 0xcbf29ce484222325;
    for b in s.bytes() {
        hash ^= b as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

/// Compute an election score for a peer. Lower is better.
///
/// - `avg_rtt_us`: average round-trip time to all connected peers (microseconds)
/// - `uptime_secs`: how long this speaker has been running
/// - `peer_id`: unique identifier for deterministic tiebreaking
/// - `has_audio`: whether this speaker has active audio sources
pub fn election_score(avg_rtt_us: u64, uptime_secs: u64, peer_id: &str, has_audio: bool) -> u64 {
    let rtt_score = avg_rtt_us;
    let stability_bonus = uptime_secs.min(10_000) * 100; // up to 1M bonus reduction
    let tiebreak = fnv_hash(peer_id) % 1000;
    // Speakers without active audio get an overwhelming penalty so the audio source always wins
    let audio_penalty = if has_audio { 0 } else { u64::MAX / 4 };
    rtt_score.saturating_sub(stability_bonus) + tiebreak + audio_penalty
}

/// Pick the group coordinator deterministically: the lexicographically smallest
/// `peer_id` among this node and all connected peers. Every node computes the same
/// winner from the same membership with no extra messages, and the winner only
/// changes when the min-id node joins or leaves -- so it is sticky by construction.
///
/// The coordinator owns the group's single Spotify Connect identity. It is
/// intentionally independent of the audio-sync leader (which is source-driven).
pub fn choose_coordinator<'a>(
    self_id: &'a str,
    peer_ids: impl IntoIterator<Item = &'a str>,
) -> String {
    let mut min: &str = self_id;
    for id in peer_ids {
        if id < min {
            min = id;
        }
    }
    min.to_string()
}

/// Decide what this node should advertise for Spotify Connect.
/// Returns `(advertise, zone_name)`.
///
/// - Not grouped (disabled, or no peers): advertise under the device's own name.
/// - Grouped coordinator: advertise under the group name (or device name if unnamed).
/// - Grouped non-coordinator (follower): do not advertise -- disappear from the picker.
pub fn decide_spotify_zone(
    enabled: bool,
    has_peers: bool,
    is_coordinator: bool,
    group_name: &str,
    device_name: &str,
) -> (bool, String) {
    let grouped = enabled && has_peers;
    if !grouped {
        (true, device_name.to_string())
    } else if is_coordinator {
        let name = if group_name.is_empty() {
            device_name
        } else {
            group_name
        };
        (true, name.to_string())
    } else {
        (false, String::new())
    }
}

/// Tracks the state of an in-progress election.
struct ActiveElection {
    id: u32,
    votes: HashMap<String, u64>, // peer_id → score
    our_score: u64,
    started_at: Instant,
    peer_count: usize,
    /// Whether we already extended this election once waiting for quorum.
    extended: bool,
}

/// A trigger that arrived while an election was in cooldown (or already active)
/// and therefore could not start immediately. Remembered so it is not dropped.
struct PendingTrigger {
    trigger: String,
    our_id: String,
    our_score: u64,
    peer_count: usize,
}

/// Election state machine.
pub struct ElectionState {
    active: Option<ActiveElection>,
    cooldown_until: Option<Instant>,
    next_election_id: u32,
    /// A real change that was suppressed by cooldown/active election and must
    /// still be honored once we are allowed to run.
    pending: Option<PendingTrigger>,
    /// The current leader and its score, for re-election hysteresis.
    incumbent: Option<(String, u64)>,
}

/// Result of processing an election event.
pub enum ElectionAction {
    /// No action needed.
    None,
    /// Start a new election: broadcast ElectionStart to all peers.
    StartElection { election_id: u32, trigger: String },
    /// Cast our vote: broadcast ElectionVote.
    CastVote { election_id: u32, score: u64 },
    /// Election complete: this peer_id is the winner.
    Winner {
        election_id: u32,
        winner_id: String,
        source: String,
        /// Winning election score, so the caller can record the incumbent for
        /// hysteresis. `0` means "unknown" (e.g. a result learned from a peer):
        /// `should_suppress_reelection` never protects a zero-score incumbent.
        winner_score: u64,
    },
}

impl ElectionState {
    pub fn new() -> Self {
        Self {
            active: None,
            cooldown_until: None,
            next_election_id: 1,
            pending: None,
            incumbent: None,
        }
    }

    /// Check if we're currently in cooldown.
    fn in_cooldown(&self) -> bool {
        self.cooldown_until
            .map(|t| Instant::now() < t)
            .unwrap_or(false)
    }

    /// Start the cooldown timer.
    fn start_cooldown(&mut self) {
        self.cooldown_until =
            Some(Instant::now() + std::time::Duration::from_secs(ELECTION_COOLDOWN_SECS));
    }

    /// Trigger a new election. Returns StartElection action if allowed.
    pub fn trigger(
        &mut self,
        trigger: &str,
        our_id: &str,
        our_score: u64,
        peer_count: usize,
    ) -> ElectionAction {
        if self.in_cooldown() || self.active.is_some() {
            // A real change that we cannot act on right now: remember it so the
            // next election-check tick can fire it instead of losing the change.
            self.pending = Some(PendingTrigger {
                trigger: trigger.to_string(),
                our_id: our_id.to_string(),
                our_score,
                peer_count,
            });
            return ElectionAction::None;
        }

        self.start_election(trigger, our_id, our_score, peer_count)
    }

    /// Start a fresh election (assumes we are allowed to run).
    fn start_election(
        &mut self,
        trigger: &str,
        our_id: &str,
        our_score: u64,
        peer_count: usize,
    ) -> ElectionAction {
        let election_id = self.next_election_id;
        self.next_election_id = self.next_election_id.wrapping_add(1);

        let mut votes = HashMap::new();
        votes.insert(our_id.to_string(), our_score);

        self.active = Some(ActiveElection {
            id: election_id,
            votes,
            our_score,
            started_at: Instant::now(),
            peer_count,
            extended: false,
        });

        ElectionAction::StartElection {
            election_id,
            trigger: trigger.to_string(),
        }
    }

    /// Record the current leader and its score so re-elections can apply
    /// hysteresis (keep the incumbent unless a challenger clearly beats it).
    pub fn set_incumbent(&mut self, incumbent: Option<(String, u64)>) {
        self.incumbent = incumbent;
    }

    /// Whether a suppressed trigger is waiting to be fired.
    pub fn has_pending(&self) -> bool {
        self.pending.is_some()
    }

    /// Fire a deferred trigger once we are clear to run. `cooldown_elapsed` is
    /// the caller's authoritative view of whether the cooldown has passed; the
    /// deferred election starts only if it has elapsed and no election is
    /// already active.
    pub fn poll_pending(&mut self, cooldown_elapsed: bool) -> ElectionAction {
        if self.active.is_some() || !cooldown_elapsed {
            return ElectionAction::None;
        }
        let Some(p) = self.pending.take() else {
            return ElectionAction::None;
        };
        // We are committing to a fresh election; clear any lingering cooldown.
        self.cooldown_until = None;
        self.start_election(&p.trigger, &p.our_id, p.our_score, p.peer_count)
    }

    /// Force the current election to resolve immediately (test/diagnostic entry).
    pub fn resolve(&mut self, our_id: &str) -> ElectionAction {
        if self.active.is_none() {
            return ElectionAction::None;
        }
        self.resolve_election(our_id)
    }

    /// Handle an incoming ElectionStart from a peer.
    /// Returns CastVote action with our score.
    pub fn handle_start(
        &mut self,
        election_id: u32,
        our_id: &str,
        our_score: u64,
        peer_count: usize,
    ) -> ElectionAction {
        // If we have an active election with a lower ID, ignore this one
        if let Some(ref active) = self.active {
            if active.id < election_id {
                return ElectionAction::None;
            }
        }

        let mut votes = HashMap::new();
        votes.insert(our_id.to_string(), our_score);

        self.active = Some(ActiveElection {
            id: election_id,
            votes,
            our_score,
            started_at: Instant::now(),
            peer_count,
            extended: false,
        });

        ElectionAction::CastVote {
            election_id,
            score: our_score,
        }
    }

    /// Handle an incoming ElectionVote. May return Winner if all votes collected.
    pub fn handle_vote(
        &mut self,
        election_id: u32,
        peer_id: &str,
        score: u64,
        our_id: &str,
    ) -> ElectionAction {
        let Some(ref mut active) = self.active else {
            return ElectionAction::None;
        };
        if active.id != election_id {
            return ElectionAction::None;
        }

        active.votes.insert(peer_id.to_string(), score);

        // Check if all votes are in (peer_count + 1 for ourselves)
        if active.votes.len() > active.peer_count {
            return self.resolve_election(our_id);
        }

        ElectionAction::None
    }

    /// Check if the election has timed out. If so, resolve with available votes.
    pub fn check_timeout(&mut self, our_id: &str) -> ElectionAction {
        let Some(ref active) = self.active else {
            return ElectionAction::None;
        };

        let elapsed = active.started_at.elapsed().as_millis() as u64;
        if elapsed >= ELECTION_TIMEOUT_MS {
            return self.resolve_election(our_id);
        }

        ElectionAction::None
    }

    /// Handle an incoming ElectionResult from a peer.
    pub fn handle_result(
        &mut self,
        election_id: u32,
        winner_id: &str,
        source: &str,
    ) -> ElectionAction {
        // Accept the result, clear our active election
        if let Some(ref active) = self.active {
            if active.id == election_id {
                self.active = None;
                self.start_cooldown();
            }
        }

        // The score that produced this winner is not carried over the wire, so we
        // record the incumbent locally with an unknown (0) score: hysteresis on
        // this node's own future re-elections will recompute real scores.
        self.incumbent = Some((winner_id.to_string(), 0));

        ElectionAction::Winner {
            election_id,
            winner_id: winner_id.to_string(),
            source: source.to_string(),
            winner_score: 0,
        }
    }

    /// Resolve the election: lowest score wins, subject to quorum and incumbent
    /// hysteresis.
    ///
    /// Quorum is a strict majority of the known nodes (the other peers plus us).
    /// If quorum has not arrived we extend the election once; if it still has
    /// not arrived we crown with whatever votes we have, by deterministic
    /// lowest-score / lowest-id tiebreak, rather than block forever.
    fn resolve_election(&mut self, our_id: &str) -> ElectionAction {
        let known = self.active.as_ref().unwrap().peer_count + 1;
        let quorum = known / 2 + 1;
        let have = self.active.as_ref().unwrap().votes.len();

        if have < quorum {
            // Not enough votes yet. Extend once to give stragglers a chance.
            let active = self.active.as_mut().unwrap();
            if !active.extended {
                active.extended = true;
                active.started_at = Instant::now();
                return ElectionAction::None;
            }
            // Already extended and still short of quorum: fall through and crown
            // with the votes we have (deterministic), so the group is not stuck.
        }

        let active = self.active.take().unwrap();
        self.start_cooldown();

        // Lowest score wins; ties broken by lowest peer_id for determinism.
        let (best_id, best_score) = active
            .votes
            .iter()
            .min_by(|(id_a, &sa), (id_b, &sb)| sa.cmp(&sb).then_with(|| id_a.cmp(id_b)))
            .map(|(id, &s)| (id.clone(), s))
            .unwrap();

        // Incumbent hysteresis: keep the current leader unless the best
        // challenger beats it by more than the hysteresis margin.
        let (winner_id, winner_score) = match &self.incumbent {
            Some((inc_id, inc_score))
                if Self::should_suppress_reelection(*inc_score, best_score) =>
            {
                (inc_id.clone(), *inc_score)
            }
            _ => (best_id, best_score),
        };

        // Record the standing leader so the NEXT election applies hysteresis in
        // production -- without this, callers never set the incumbent and the
        // within-20% stickiness is inert on-device.
        self.incumbent = Some((winner_id.clone(), winner_score));

        let _ = our_id; // winner broadcast is decided by the caller comparing ids
        ElectionAction::Winner {
            election_id: active.id,
            winner_id,
            source: "elected".to_string(),
            winner_score,
        }
    }

    /// Whether we should suppress a re-election (current leader's score is close enough).
    pub fn should_suppress_reelection(
        current_leader_score: u64,
        best_candidate_score: u64,
    ) -> bool {
        if current_leader_score == 0 {
            return false;
        }
        // Don't re-elect if current leader is within 20% of best
        let threshold = best_candidate_score + best_candidate_score / 5;
        current_leader_score <= threshold
    }

    /// Whether an election is currently active.
    pub fn is_active(&self) -> bool {
        self.active.is_some()
    }

    /// Whether the post-election cooldown has elapsed (so a deferred trigger may
    /// now run). True when no cooldown is set or its deadline has passed.
    pub fn cooldown_elapsed(&self) -> bool {
        !self.in_cooldown()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Thin helper: set the incumbent, run one election with the given votes
    /// (the first entry is treated as "us"), and return the elected winner id.
    fn resolve_with(
        es: &mut ElectionState,
        incumbent: Option<(&str, u64)>,
        votes: &[(&str, u64)],
    ) -> Option<String> {
        es.set_incumbent(incumbent.map(|(id, s)| (id.to_string(), s)));
        let (our_id, our_score) = votes[0];
        let peer_count = votes.len() - 1; // everyone except us
        let _ = es.handle_start(1, our_id, our_score, peer_count);
        let mut resolved: ElectionAction = ElectionAction::None;
        for &(id, score) in &votes[1..] {
            // The last vote may auto-resolve the election; capture that.
            resolved = es.handle_vote(1, id, score, our_id);
        }
        // If all votes are in but the election did not auto-resolve, force it.
        if es.is_active() {
            resolved = es.resolve(our_id);
        }
        match resolved {
            ElectionAction::Winner { winner_id, .. } => Some(winner_id),
            _ => None,
        }
    }

    /// Thin helper: start an election that knows `known` total nodes, inject only
    /// the given votes, and force a timeout resolution. Returns the action.
    fn resolve_timeout_with_votes(
        es: &mut ElectionState,
        known: usize,
        votes: &[(&str, u64)],
    ) -> ElectionAction {
        let (our_id, our_score) = votes[0];
        // peer_count is the number of OTHER nodes; known total = peer_count + 1.
        let peer_count = known.saturating_sub(1);
        let _ = es.handle_start(1, our_id, our_score, peer_count);
        for &(id, score) in &votes[1..] {
            let _ = es.handle_vote(1, id, score, our_id);
        }
        es.resolve(our_id)
    }

    #[test]
    fn incumbent_within_hysteresis_is_not_displaced() {
        // resolve must keep the incumbent if a challenger does not beat it by >20%.
        let mut es = ElectionState::new();
        let winner = resolve_with(&mut es, Some(("A", 1000)), &[("A", 1000), ("B", 900)]);
        assert_eq!(
            winner.as_deref(),
            Some("A"),
            "B is only 10% better -> incumbent stays"
        );
        let mut es2 = ElectionState::new();
        let winner2 = resolve_with(&mut es2, Some(("A", 1000)), &[("A", 1000), ("B", 700)]);
        assert_eq!(winner2.as_deref(), Some("B"), "B beats by 30% -> switch");
    }

    /// Thin helper: drive one full election through the *production* public API
    /// (handle_start + the peers' votes + force-resolve) WITHOUT ever touching
    /// set_incumbent, exactly as mod.rs does. The first entry is "us".
    fn run_election_via_api(es: &mut ElectionState, votes: &[(&str, u64)]) -> Option<String> {
        let (our_id, our_score) = votes[0];
        let peer_count = votes.len() - 1;
        let _ = es.handle_start(1, our_id, our_score, peer_count);
        let mut resolved = ElectionAction::None;
        for &(id, score) in &votes[1..] {
            // The last vote may auto-resolve the election; capture that action.
            resolved = es.handle_vote(1, id, score, our_id);
        }
        // If the votes did not auto-resolve, force a resolution (timeout path).
        if es.is_active() {
            resolved = es.resolve(our_id);
        }
        match resolved {
            ElectionAction::Winner { winner_id, .. } => Some(winner_id),
            _ => None,
        }
    }

    #[test]
    fn resolved_winner_becomes_incumbent_for_next_election() {
        // Production wiring: nobody calls set_incumbent here. Resolving an
        // election must record its own winner as the incumbent so the SECOND
        // election applies hysteresis and a marginal challenger cannot unseat the
        // standing leader on-device. Routes only through the public API mod.rs
        // uses, never the set_incumbent test seam.
        let mut es = ElectionState::new();

        // First election: A (1000) beats B (1100) -> A wins. No incumbent yet, so
        // the auto-resolve on the second vote crowns the lowest score outright.
        let first = run_election_via_api(&mut es, &[("A", 1000), ("B", 1100)]);
        assert_eq!(
            first.as_deref(),
            Some("A"),
            "A is best, A wins the first round"
        );

        // Second election: B improves to 900 (only 10% better than A). If the
        // first resolution did NOT record A as incumbent, self.incumbent is still
        // None and B steals leadership. With the wiring, A stays (within 20%).
        let second = run_election_via_api(&mut es, &[("A", 1000), ("B", 900)]);
        assert_eq!(
            second.as_deref(),
            Some("A"),
            "A is the standing leader and B is only 10% better -> A stays"
        );
    }

    #[test]
    fn trigger_during_cooldown_is_deferred_not_dropped() {
        let mut es = ElectionState::new();
        // A solo group: our single vote is quorum, so the election resolves and
        // a cooldown starts.
        let _ = es.trigger("src", "me", 100, 0);
        let won = es.resolve("me"); // resolves, starts cooldown
        assert!(
            matches!(won, ElectionAction::Winner { .. }),
            "first election should resolve",
        );
        assert!(!es.is_active(), "election cleared after resolve");

        // A new real change arrives while we are in cooldown: it must not be
        // dropped on the floor.
        let a = es.trigger("join", "me", 100, 1);
        assert!(matches!(a, ElectionAction::None), "suppressed by cooldown");
        assert!(
            es.has_pending(),
            "a real change in cooldown must be remembered"
        );

        // Once the cooldown has elapsed, the deferred change fires.
        let fired = es.poll_pending(/*cooldown_elapsed*/ true);
        assert!(matches!(fired, ElectionAction::StartElection { .. }));
        assert!(!es.has_pending(), "pending consumed once fired");
    }

    #[test]
    fn resolve_requires_quorum() {
        let mut es = ElectionState::new();
        // 4 peers known, only 1 vote arrived at timeout -> no crown (defer/extend).
        let action = resolve_timeout_with_votes(&mut es, /*known*/ 4, &[("A", 100)]);
        assert!(
            !matches!(action, ElectionAction::Winner { .. }),
            "1/4 votes is not quorum"
        );
    }

    #[test]
    fn score_deterministic() {
        let s1 = election_score(5000, 100, "peer-a", true);
        let s2 = election_score(5000, 100, "peer-a", true);
        assert_eq!(s1, s2);
    }

    #[test]
    fn lower_rtt_wins() {
        let good = election_score(1000, 100, "peer-a", true);
        let bad = election_score(10000, 100, "peer-b", true);
        assert!(good < bad, "good={} bad={}", good, bad);
    }

    #[test]
    fn uptime_bonus() {
        let new = election_score(5000, 10, "peer-a", true);
        let old = election_score(5000, 1000, "peer-a", true);
        assert!(old < new, "old={} new={}", old, new);
    }

    #[test]
    fn audio_source_always_wins() {
        // Speaker with audio should always beat speaker without, regardless of RTT/uptime
        let with_audio = election_score(50000, 10, "peer-z", true); // worst RTT, low uptime, late alphabet
        let no_audio = election_score(100, 10000, "peer-a", false); // best RTT, high uptime, early alphabet
        assert!(
            with_audio < no_audio,
            "with_audio={} no_audio={}",
            with_audio,
            no_audio
        );
        // The no-audio penalty should be overwhelming
        assert!(
            no_audio > u64::MAX / 8,
            "penalty should be huge, got {}",
            no_audio
        );
    }

    #[test]
    fn cooldown_prevents_rapid_elections() {
        let mut es = ElectionState::new();
        let action = es.trigger("test", "me", 100, 1);
        assert!(matches!(action, ElectionAction::StartElection { .. }));

        // Resolve immediately
        let _ = es.check_timeout("me");

        // Should be in cooldown
        let action2 = es.trigger("test2", "me", 100, 1);
        assert!(matches!(action2, ElectionAction::None));
    }

    #[test]
    fn election_resolves_with_all_votes() {
        let mut es = ElectionState::new();
        let action = es.trigger("audio", "peer-a", 5000, 2);
        assert!(matches!(
            action,
            ElectionAction::StartElection { election_id: 1, .. }
        ));

        let action2 = es.handle_vote(1, "peer-b", 3000, "peer-a");
        assert!(matches!(action2, ElectionAction::None)); // still waiting

        let action3 = es.handle_vote(1, "peer-c", 4000, "peer-a");
        // All 3 votes in → should resolve
        match action3 {
            ElectionAction::Winner { winner_id, .. } => {
                assert_eq!(winner_id, "peer-b"); // lowest score
            }
            _ => panic!("expected Winner"),
        }
    }

    #[test]
    fn suppress_reelection_when_close() {
        assert!(ElectionState::should_suppress_reelection(1100, 1000));
        assert!(!ElectionState::should_suppress_reelection(2000, 1000));
    }

    #[test]
    fn handle_start_casts_vote() {
        let mut es = ElectionState::new();
        let action = es.handle_start(42, "me", 5000, 2);
        match action {
            ElectionAction::CastVote { election_id, score } => {
                assert_eq!(election_id, 42);
                assert_eq!(score, 5000);
            }
            _ => panic!("expected CastVote"),
        }
    }

    #[test]
    fn coordinator_single_node_is_self() {
        let empty: [&str; 0] = [];
        assert_eq!(choose_coordinator("speaker-b", empty), "speaker-b");
    }

    #[test]
    fn coordinator_picks_min_id_and_converges() {
        assert_eq!(
            choose_coordinator("speaker-b", ["speaker-a", "speaker-c"]),
            "speaker-a"
        );
        assert_eq!(
            choose_coordinator("speaker-a", ["speaker-b", "speaker-c"]),
            "speaker-a"
        );
        assert_eq!(
            choose_coordinator("speaker-c", ["speaker-a", "speaker-b"]),
            "speaker-a"
        );
    }

    #[test]
    fn coordinator_sticky_on_higher_id_join() {
        assert_eq!(choose_coordinator("speaker-a", ["speaker-c"]), "speaker-a");
        assert_eq!(
            choose_coordinator("speaker-a", ["speaker-c", "speaker-z"]),
            "speaker-a"
        );
    }

    #[test]
    fn coordinator_reelects_when_min_leaves() {
        assert_eq!(
            choose_coordinator("speaker-b", ["speaker-a", "speaker-c"]),
            "speaker-a"
        );
        assert_eq!(choose_coordinator("speaker-b", ["speaker-c"]), "speaker-b");
    }

    #[test]
    fn zone_standalone_advertises_device_name() {
        assert_eq!(
            decide_spotify_zone(false, false, true, "Home", "Kitchen"),
            (true, "Kitchen".to_string())
        );
        assert_eq!(
            decide_spotify_zone(true, false, true, "Home", "Kitchen"),
            (true, "Kitchen".to_string())
        );
    }

    #[test]
    fn zone_coordinator_advertises_group_name() {
        assert_eq!(
            decide_spotify_zone(true, true, true, "Home", "Kitchen"),
            (true, "Home".to_string())
        );
    }

    #[test]
    fn zone_coordinator_falls_back_to_device_name_when_group_unnamed() {
        assert_eq!(
            decide_spotify_zone(true, true, true, "", "Kitchen"),
            (true, "Kitchen".to_string())
        );
    }

    #[test]
    fn zone_follower_is_suppressed() {
        assert_eq!(
            decide_spotify_zone(true, true, false, "Home", "Kitchen"),
            (false, String::new())
        );
    }
}
