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
pub fn election_score(avg_rtt_us: u64, uptime_secs: u64, peer_id: &str) -> u64 {
    let rtt_score = avg_rtt_us;
    let stability_bonus = uptime_secs.min(10_000) * 100; // up to 1M bonus reduction
    let tiebreak = fnv_hash(peer_id) % 1000;
    rtt_score.saturating_sub(stability_bonus) + tiebreak
}

/// Tracks the state of an in-progress election.
struct ActiveElection {
    id: u32,
    votes: HashMap<String, u64>, // peer_id → score
    our_score: u64,
    started_at: Instant,
    peer_count: usize,
}

/// Election state machine.
pub struct ElectionState {
    active: Option<ActiveElection>,
    cooldown_until: Option<Instant>,
    next_election_id: u32,
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
    Winner { election_id: u32, winner_id: String, source: String },
}

impl ElectionState {
    pub fn new() -> Self {
        Self {
            active: None,
            cooldown_until: None,
            next_election_id: 1,
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
        self.cooldown_until = Some(
            Instant::now() + std::time::Duration::from_secs(ELECTION_COOLDOWN_SECS),
        );
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
            return ElectionAction::None;
        }

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
        });

        ElectionAction::StartElection {
            election_id,
            trigger: trigger.to_string(),
        }
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
        if active.votes.len() >= active.peer_count + 1 {
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

        ElectionAction::Winner {
            election_id,
            winner_id: winner_id.to_string(),
            source: source.to_string(),
        }
    }

    /// Resolve the election: lowest score wins.
    fn resolve_election(&mut self, our_id: &str) -> ElectionAction {
        let active = self.active.take().unwrap();
        self.start_cooldown();

        let (winner_id, _) = active
            .votes
            .iter()
            .min_by_key(|(_, &score)| score)
            .unwrap();

        // Only the winner broadcasts ElectionResult (prevents duplicates)
        if winner_id == our_id {
            ElectionAction::Winner {
                election_id: active.id,
                winner_id: winner_id.clone(),
                source: "elected".to_string(),
            }
        } else {
            // We lost, but we know who won
            ElectionAction::Winner {
                election_id: active.id,
                winner_id: winner_id.clone(),
                source: "elected".to_string(),
            }
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn score_deterministic() {
        let s1 = election_score(5000, 100, "peer-a");
        let s2 = election_score(5000, 100, "peer-a");
        assert_eq!(s1, s2);
    }

    #[test]
    fn lower_rtt_wins() {
        let good = election_score(1000, 100, "peer-a");
        let bad = election_score(10000, 100, "peer-b");
        assert!(good < bad, "good={} bad={}", good, bad);
    }

    #[test]
    fn uptime_bonus() {
        let new = election_score(5000, 10, "peer-a");
        let old = election_score(5000, 1000, "peer-a");
        assert!(old < new, "old={} new={}", old, new);
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
        assert!(matches!(action, ElectionAction::StartElection { election_id: 1, .. }));

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
}
