//! lbsim-policy: routing names=prefix_affinity,affinity
//! Prefix affinity with a load ceiling. Prefer a replica already holding the request's prefix, unless
//! it is more than `affinity_max_load_ratio` times the fleet-mean load; then fall back to power of
//! `affinity_fallback_choices`.
//!
//! A hit skips the shared prefix's prefill, which is real capacity, and the only way to earn one is
//! to send a root's traffic to the replicas that already hold it. That concentrates load by
//! construction. The ratio is the single knob that decides how much imbalance the router tolerates
//! for the hits: 1.05 is p2c with a tie-break, 2.2 is affinity that almost never yields.
//!
//! The "fleet mean" is never computed over the fleet. Section 10.4 of docs/ARCHITECTURE.md bans an
//! O(N) scan per decision, so the policy keeps a running estimate fed only by the views it touches
//! anyway: the `d` sampled fallback candidates of every decision. That is a bounded-cost compromise,
//! not the exact mean, and it is what a real router with a sampled view of the fleet would have too.

use crate::{RouteContext, RoutingPolicy};
use sim_scenario::Scenario;

/// Weight of one decision's sample mean in the running estimate. Sixteen decisions of memory at 560
/// rps is about 30 ms of fleet history: fresh enough to track a ramp, long enough that one unlucky
/// sample does not flip the ceiling.
const ESTIMATE_ALPHA: f64 = 1.0 / 16.0;

/// At most this many holders come back from the index (section 10.4's bounded top-K), so a decision
/// inspects at most `choices + MAX_HOLDERS` views.
const MAX_HOLDERS: usize = 4;

pub struct PrefixAffinity {
    max_load_ratio: f64,
    choices: usize,
    /// Running estimate of the fleet-mean `queued + running`; `None` until the first decision.
    mean_load: Option<f64>,
}

pub fn make(sc: &Scenario) -> Box<dyn RoutingPolicy> {
    Box::new(PrefixAffinity {
        max_load_ratio: sc.affinity_max_load_ratio,
        choices: sc.affinity_fallback_choices as usize,
        mean_load: None,
    })
}

#[inline]
fn load(ctx: &RouteContext<'_>, i: usize) -> u64 {
    let v = &ctx.views[i];
    v.queued as u64 + v.running as u64
}

impl PrefixAffinity {
    /// Fold this decision's samples into the estimate and return the ceiling a holder must be under.
    fn ceiling(&mut self, sampled: &[usize], ctx: &RouteContext<'_>) -> f64 {
        if !sampled.is_empty() {
            let mean = sampled.iter().map(|&i| load(ctx, i) as f64).sum::<f64>() / sampled.len() as f64;
            self.mean_load = Some(match self.mean_load {
                None => mean,
                Some(est) => est + ESTIMATE_ALPHA * (mean - est),
            });
        }
        // An idle fleet has a mean of zero, and a zero ceiling would refuse a holder with one request
        // in it while the fallback happily picks a replica with one request in it. One request of
        // slack is the floor below which the ratio has nothing to multiply.
        self.max_load_ratio * self.mean_load.unwrap_or(0.0).max(1.0)
    }
}

impl RoutingPolicy for PrefixAffinity {
    fn label(&self) -> String {
        format!("prefix_affinity(ratio={},d={})", self.max_load_ratio, self.choices)
    }
    fn inspected(&self, _fleet: usize) -> usize {
        self.choices + MAX_HOLDERS
    }
    fn choose(&mut self, ctx: &mut RouteContext<'_>) -> Option<usize> {
        let n = ctx.fleet();
        if n == 0 {
            return None;
        }
        // Sample the fallback first, holder or not, so the estimate is fed on every decision and the
        // random stream advances the same way whichever branch wins.
        let mut sampled = [0usize; 8];
        let mut count = 0;
        let mut best: Option<usize> = None;
        let mut best_key = u64::MAX;
        for _ in 0..self.choices.max(1).min(sampled.len()) {
            let i = ctx.rng.below(n as u64) as usize;
            if !ctx.usable(i) {
                continue;
            }
            sampled[count] = i;
            count += 1;
            // The same key p2c uses, so a holder that fails the ceiling is placed exactly as p2c
            // would place it and the control run differs only where affinity acted.
            let key = ctx.views[i].queued_tokens;
            if key < best_key || (key == best_key && Some(i) < best) {
                best_key = key;
                best = Some(i);
            }
        }
        let ceiling = self.ceiling(&sampled[..count], ctx);

        let node = ctx.request.prefix_node;
        if node != 0 {
            // Holders arrive sorted by hit tokens descending, so the first one under the ceiling is
            // the most valuable one the ceiling allows.
            for (i, _hit) in ctx.prefix_holders(node).into_iter().take(MAX_HOLDERS) {
                if i < n && ctx.usable(i) && (load(ctx, i) as f64) <= ceiling {
                    return Some(i);
                }
            }
        }
        best.or_else(|| ctx.first_usable())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::routing::{PrefixIndex, ReplicaView, RequestView};
    use sim_core::rng::Rng;

    struct Holders(Vec<(usize, u32)>);
    impl PrefixIndex for Holders {
        fn holders(&self, _node: u64) -> Vec<(usize, u32)> {
            self.0.clone()
        }
    }

    fn view(queued: u32, running: u32) -> ReplicaView {
        ReplicaView { queued, running, queued_tokens: (queued as u64) * 1000, ..Default::default() }
    }

    fn request(prefix_node: u64) -> RequestView {
        RequestView {
            id: 1,
            prompt_tokens: 1200,
            arrived_at: 0,
            deadline: 2_000_000_000,
            tenant: 0,
            attempts: 1,
            prefix_node,
            prefix_tokens: if prefix_node == 0 { 0 } else { 800 },
        }
    }

    fn policy(ratio: f64) -> PrefixAffinity {
        PrefixAffinity { max_load_ratio: ratio, choices: 2, mean_load: None }
    }

    /// Run one decision over `views` with the holders given; `seed` picks the fallback samples.
    fn decide(p: &mut PrefixAffinity, views: &[ReplicaView], holders: Vec<(usize, u32)>, node: u64, seed: u64) -> Option<usize> {
        let req = request(node);
        let mut rng = Rng::from_seed(seed);
        let live = |i: usize| views[i];
        let index = Holders(holders);
        let mut ctx = RouteContext::new(0, views, &req, &mut rng, &live, &index);
        p.choose(&mut ctx)
    }

    /// Every replica but the holder is evenly loaded; the holder is a little above the mean but under
    /// the ceiling, so affinity wins over the least-loaded sample.
    #[test]
    fn holder_under_the_ratio_wins() {
        let mut views = vec![view(4, 4); 16];
        views[3] = view(5, 4);
        for seed in 1..=8 {
            let mut p = policy(1.3);
            assert_eq!(decide(&mut p, &views, vec![(3, 800)], 42, seed), Some(3), "seed {seed}");
        }
    }

    /// The holder is far above the mean, so the ceiling refuses it and the pick is p2c's: one of the
    /// sampled replicas, never the holder.
    #[test]
    fn holder_over_the_ratio_loses_to_the_fallback() {
        let mut views = vec![view(4, 4); 16];
        views[3] = view(30, 4);
        for seed in 1..=8 {
            let mut p = policy(1.3);
            let pick = decide(&mut p, &views, vec![(3, 800)], 42, seed);
            assert!(pick.is_some() && pick != Some(3), "seed {seed}: {pick:?}");
        }
    }

    /// With no holders the decision is exactly p2c over the same random draws: least queued tokens
    /// among the two samples, lower index on a tie.
    #[test]
    fn no_holders_is_power_of_d_choices() {
        let views: Vec<ReplicaView> = (0..16).map(|i| view(i as u32, 0)).collect();
        for seed in 1..=8 {
            let mut p = policy(1.3);
            let mut rng = Rng::from_seed(seed);
            let a = rng.below(16) as usize;
            let b = rng.below(16) as usize;
            let expected = if views[b].queued_tokens < views[a].queued_tokens { b } else if views[a].queued_tokens < views[b].queued_tokens { a } else { a.min(b) };
            assert_eq!(decide(&mut p, &views, vec![], 42, seed), Some(expected), "seed {seed}");
        }
        // And a request with no prefix at all never consults the index, whatever it would say.
        let mut p = policy(1.3);
        assert_ne!(decide(&mut p, &views, vec![(15, 800)], 0, 3), Some(15));
    }

    /// An ejected holder is skipped even when its snapshot looks idle; the next holder under the
    /// ceiling takes its place.
    #[test]
    fn ejected_holder_is_skipped() {
        let mut views = vec![view(4, 4); 16];
        views[3] = ReplicaView { ejected: true, ..view(0, 0) };
        views[5] = view(4, 4);
        for seed in 1..=8 {
            let mut p = policy(1.3);
            assert_eq!(decide(&mut p, &views, vec![(3, 800), (5, 400)], 42, seed), Some(5), "seed {seed}");
        }
        // With the only holder ejected, the fallback still places the request somewhere usable.
        let mut p = policy(1.3);
        let pick = decide(&mut p, &views, vec![(3, 800)], 42, 1);
        assert!(matches!(pick, Some(i) if i != 3));
    }

    #[test]
    fn label_names_both_parameters() {
        assert_eq!(policy(1.3).label(), "prefix_affinity(ratio=1.3,d=2)");
    }
}
