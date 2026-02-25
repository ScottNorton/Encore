//! Relay topology — well-connected speakers rebroadcast audio to
//! poorly-connected peers, forming a tree rooted at the leader.

use std::collections::HashMap;

/// Maximum relay hops from the leader (prevents loops and limits latency).
pub const MAX_HOP_COUNT: u8 = 3;

/// A relay assignment: `target` receives audio from `relay` instead of directly from the leader.
#[derive(Debug, Clone)]
pub struct RelayAssignment {
    pub target_peer_id: String,
    pub relay_peer_id: String,
}

/// Compute the optimal relay tree based on RTT measurements.
///
/// `leader_id`: the current leader
/// `rtts`: map from peer_id → (map from other_peer_id → rtt_us)
///
/// Returns a list of relay assignments. Peers not in the result connect directly to the leader.
pub fn compute_relay_tree(
    leader_id: &str,
    rtts: &HashMap<String, HashMap<String, u64>>,
) -> Vec<RelayAssignment> {
    let mut assignments = Vec::new();

    // Get leader's RTT to each follower
    let leader_rtts = match rtts.get(leader_id) {
        Some(r) => r,
        None => return assignments,
    };

    // For each follower, check if another follower has a better RTT to it
    for (target_id, &leader_rtt) in leader_rtts {
        if target_id == leader_id {
            continue;
        }

        let mut best_relay: Option<(&str, u64)> = None;

        for (relay_candidate, candidate_rtts) in rtts {
            if relay_candidate == leader_id || relay_candidate == target_id {
                continue;
            }

            if let Some(&candidate_rtt) = candidate_rtts.get(target_id) {
                // Relay candidate must have better RTT to target than leader does,
                // AND the relay candidate must have a good link to the leader
                if let Some(&relay_to_leader) = leader_rtts.get(relay_candidate.as_str()) {
                    // Total latency via relay: leader→relay + relay→target
                    let relay_total = relay_to_leader.saturating_add(candidate_rtt);

                    // Only use relay if it saves at least 30% latency
                    if relay_total < leader_rtt * 70 / 100 {
                        match best_relay {
                            Some((_, best_total)) if relay_total >= best_total => {}
                            _ => {
                                best_relay = Some((relay_candidate, relay_total));
                            }
                        }
                    }
                }
            }
        }

        if let Some((relay_id, _)) = best_relay {
            assignments.push(RelayAssignment {
                target_peer_id: target_id.clone(),
                relay_peer_id: relay_id.to_string(),
            });
        }
    }

    assignments
}

/// Determine the hop count for a peer based on relay assignments.
pub fn peer_hop_count(peer_id: &str, leader_id: &str, assignments: &[RelayAssignment]) -> u8 {
    if peer_id == leader_id {
        return 0;
    }

    // Find if this peer is relayed
    let mut hops = 1u8; // direct follower
    let mut current = peer_id;

    for _ in 0..MAX_HOP_COUNT as usize {
        if let Some(assignment) = assignments.iter().find(|a| a.target_peer_id == current) {
            hops += 1;
            current = &assignment.relay_peer_id;
            if current == leader_id {
                return hops;
            }
        } else {
            return hops;
        }
    }

    hops
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_relay_when_leader_has_best_rtt() {
        let mut rtts = HashMap::new();
        let mut leader = HashMap::new();
        leader.insert("follower-a".to_string(), 1000u64);
        leader.insert("follower-b".to_string(), 2000);
        rtts.insert("leader".to_string(), leader);

        let mut a = HashMap::new();
        a.insert("follower-b".to_string(), 3000u64); // worse than leader
        rtts.insert("follower-a".to_string(), a);

        let assignments = compute_relay_tree("leader", &rtts);
        assert!(assignments.is_empty());
    }

    #[test]
    fn relay_when_peer_has_better_link() {
        let mut rtts = HashMap::new();
        let mut leader = HashMap::new();
        leader.insert("relay".to_string(), 1000u64);
        leader.insert("far".to_string(), 50000); // leader→far is bad
        rtts.insert("leader".to_string(), leader);

        let mut relay = HashMap::new();
        relay.insert("far".to_string(), 2000u64); // relay→far is great
        rtts.insert("relay".to_string(), relay);

        let assignments = compute_relay_tree("leader", &rtts);
        assert_eq!(assignments.len(), 1);
        assert_eq!(assignments[0].target_peer_id, "far");
        assert_eq!(assignments[0].relay_peer_id, "relay");
    }

    #[test]
    fn hop_count_direct() {
        let assignments = vec![];
        assert_eq!(peer_hop_count("leader", "leader", &assignments), 0);
        assert_eq!(peer_hop_count("follower", "leader", &assignments), 1);
    }

    #[test]
    fn hop_count_relayed() {
        let assignments = vec![RelayAssignment {
            target_peer_id: "far".into(),
            relay_peer_id: "relay".into(),
        }];
        assert_eq!(peer_hop_count("far", "leader", &assignments), 2);
        assert_eq!(peer_hop_count("relay", "leader", &assignments), 1);
    }

    #[test]
    fn max_hop_prevention() {
        // Chain: far → relay2 → relay1 → leader
        let assignments = vec![
            RelayAssignment {
                target_peer_id: "far".into(),
                relay_peer_id: "relay2".into(),
            },
            RelayAssignment {
                target_peer_id: "relay2".into(),
                relay_peer_id: "relay1".into(),
            },
        ];
        let hops = peer_hop_count("far", "leader", &assignments);
        assert!(hops <= MAX_HOP_COUNT);
    }
}
