//! Routing policies.
//!
//! Every policy here sees only `ReplicaView`, which is a *delayed* snapshot. That is structural
//! rather than a convention: a policy has no access to live replica state, so the staleness dynamics
//! in `docs/ARCHITECTURE.md` section 6 are a property of the architecture rather than something
//! bolted on. The one exception is an explicit probe, which pays a modelled round trip so the cost of
//! freshness is visible.
//!
//! The measured constraint from section 10.4 applies to all of them: a decision must be O(1) or
//! O(log N), never a scan of the fleet. `LeastRequests` and `LeastQueueTokens` violate that
//! deliberately, because they are the baselines whose cost and behaviour are the point.

use crate::rng::Rng;
use crate::Nanos;

/// What a replica reports about itself. Not everything the simulator knows: a replica cannot report
/// the true output length of its running requests, because it does not know it.
#[derive(Clone, Copy, Default, Debug)]
pub struct ReplicaView {
    pub sampled_at: Nanos,
    pub queued: u32,
    pub running: u32,
    /// Queued work in tokens rather than requests. One long-context request costs what many chat
    /// turns cost, so a policy counting requests is measuring the wrong quantity.
    pub queued_tokens: u64,
    pub last_step_ns: Nanos,
    pub ejected: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Routing {
    RoundRobin,
    Random,
    LeastRequests,
    LeastQueueTokens,
    PowerOfTwoChoices { choices: usize },
}

impl Routing {
    pub fn parse(name: &str, choices: usize) -> Result<Routing, String> {
        Ok(match name {
            "round_robin" => Routing::RoundRobin,
            "random" => Routing::Random,
            "least_requests" => Routing::LeastRequests,
            "least_queue_tokens" => Routing::LeastQueueTokens,
            "p2c" | "power_of_two_choices" => Routing::PowerOfTwoChoices { choices },
            other => return Err(format!("unknown routing policy {:?}", other)),
        })
    }

    pub fn label(&self) -> String {
        match self {
            Routing::RoundRobin => "round_robin".into(),
            Routing::Random => "random".into(),
            Routing::LeastRequests => "least_requests".into(),
            Routing::LeastQueueTokens => "least_queue_tokens".into(),
            Routing::PowerOfTwoChoices { choices } => format!("p2c(d={})", choices),
        }
    }

    /// Cost of one decision, in units of replicas inspected. Recorded so a policy that cannot hold
    /// at target scale is visible rather than merely slow.
    pub fn inspected(&self, fleet: usize) -> usize {
        match self {
            Routing::RoundRobin | Routing::Random => 1,
            Routing::LeastRequests | Routing::LeastQueueTokens => fleet,
            Routing::PowerOfTwoChoices { choices } => *choices,
        }
    }

    pub fn choose(
        &self,
        views: &[ReplicaView],
        rr_cursor: &mut usize,
        rng: &mut Rng,
    ) -> Option<usize> {
        let n = views.len();
        if n == 0 {
            return None;
        }
        let usable = |i: usize| !views[i].ejected;
        match self {
            Routing::RoundRobin => {
                for _ in 0..n {
                    let i = *rr_cursor % n;
                    *rr_cursor += 1;
                    if usable(i) {
                        return Some(i);
                    }
                }
                None
            }
            Routing::Random => {
                for _ in 0..8 {
                    let i = rng.below(n as u64) as usize;
                    if usable(i) {
                        return Some(i);
                    }
                }
                (0..n).find(|&i| usable(i))
            }
            Routing::LeastRequests => (0..n)
                .filter(|&i| usable(i))
                .min_by_key(|&i| (views[i].queued + views[i].running, i)),
            Routing::LeastQueueTokens => (0..n)
                .filter(|&i| usable(i))
                .min_by_key(|&i| (views[i].queued_tokens, i as u64)),
            Routing::PowerOfTwoChoices { choices } => {
                // Sample d at random, take the least loaded of those. Nearly as good as global
                // least-loaded, and far more robust to stale telemetry, because only a fraction of
                // routers consider any one apparently-idle replica at a time.
                let mut best: Option<usize> = None;
                let mut best_key = u64::MAX;
                for _ in 0..(*choices).max(1) {
                    let i = rng.below(n as u64) as usize;
                    if !usable(i) {
                        continue;
                    }
                    let key = views[i].queued_tokens;
                    if key < best_key || (key == best_key && Some(i) < best) {
                        best_key = key;
                        best = Some(i);
                    }
                }
                best.or_else(|| (0..n).find(|&i| usable(i)))
            }
        }
    }
}
