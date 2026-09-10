//! lbsim-policy: routing names=weighted_random
//! Weighted random over the fleet's stale view.
//!
//! Issao: *"in the routing policy, we should create a weighed random policy where the weight for any
//! given node is a linear function of C_1 + C_2*has_queue_decode + C_3*has_queued_prefill +
//! C_4*queued_decode_beyond_current_open_buffer + C_5*queued_prefill_beyond_current_open_buffer."*
//!
//! Each replica weighs `max(0, c1 + c2·[queued decode > 0] + c3·[queued prefill > 0] + c4·decode
//! beyond the open buffer + c5·prefill beyond the open buffer)` over its delayed view (see
//! `ReplicaView`; "beyond the open buffer" is what `buffered_batch` cannot fit into the step it is
//! collecting), and the router samples in proportion. Negative `c2..c5` steer away from queues;
//! with `c2..c5` all zero every usable replica weighs the same and the draw is [`Random`]'s, taken
//! verbatim, so the default is `random` to the byte and `tests/policy_weighted_random.rs` holds the
//! fingerprint to it.
//!
//! **Cost.** Section 10.4 forbids a scan of the fleet per decision, and a fresh weight for every
//! replica on every decision would be exactly that. The weights live in a Fenwick tree, so a draw is
//! one O(log N) descent and a weight change one O(log N) update, and the policy never learns that a
//! view changed: nothing tells a router when telemetry lands. So it refreshes lazily, a fixed
//! [`REFRESH_PER_DECISION`] replicas per decision round the fleet, plus the replica it drew before it
//! trusts the draw. A weight therefore lags its view by at most `N / REFRESH_PER_DECISION` decisions:
//! for 256 replicas at a thousand requests a second that is 64 ms, well inside the telemetry
//! interval that made the view stale in the first place. Per decision: `REFRESH_PER_DECISION + 1`
//! views read and as many O(log N) updates, whatever N; the unit test below measures it.

use crate::{RouteContext, RoutingPolicy};
use sim_scenario::Scenario;

/// Views re-read per decision, besides the drawn replica's.
pub const REFRESH_PER_DECISION: usize = 4;
/// Draws before giving up on the tree: a draw can land on a replica whose refreshed weight is zero
/// or which the view now marks ejected, and each such miss zeroes it so the next draw cannot repeat.
const RETRIES: usize = 8;

pub struct WeightedRandom {
    c: [f64; 5],
    /// `c2..c5` all zero: every usable replica weighs `c1`, and the draw is `Random`'s.
    uniform: bool,
    weights: Vec<f64>,
    /// Fenwick tree over `weights`, one-based.
    tree: Vec<f64>,
    cursor: usize,
    /// Views read so far, so a test can hold the cost per decision.
    views_read: u64,
}

pub fn make(sc: &Scenario) -> Box<dyn RoutingPolicy> {
    Box::new(WeightedRandom::new([sc.wr_c1, sc.wr_c2, sc.wr_c3, sc.wr_c4, sc.wr_c5]))
}

impl WeightedRandom {
    pub fn new(c: [f64; 5]) -> WeightedRandom {
        WeightedRandom {
            c,
            uniform: c[1..].iter().all(|&x| x == 0.0),
            weights: Vec::new(),
            tree: Vec::new(),
            cursor: 0,
            views_read: 0,
        }
    }

    #[cfg(test)]
    fn views_read(&self) -> u64 {
        self.views_read
    }

    fn weight(&self, v: &crate::ReplicaView) -> f64 {
        if v.ejected {
            return 0.0;
        }
        let w = self.c[0]
            + self.c[1] * (v.queued_decode > 0) as u8 as f64
            + self.c[2] * (v.queued_prefill > 0) as u8 as f64
            + self.c[3] * v.decode_beyond_buffer as f64
            + self.c[4] * v.prefill_beyond_buffer as f64;
        if w > 0.0 { w } else { 0.0 }
    }

    fn set(&mut self, i: usize, w: f64) {
        let delta = w - self.weights[i];
        if delta == 0.0 {
            return;
        }
        self.weights[i] = w;
        let n = self.weights.len();
        let mut j = i + 1;
        while j <= n {
            self.tree[j] += delta;
            j += j & j.wrapping_neg();
        }
    }

    fn total(&self) -> f64 {
        let mut j = self.weights.len();
        let mut sum = 0.0;
        while j > 0 {
            sum += self.tree[j];
            j -= j & j.wrapping_neg();
        }
        sum
    }

    /// The index whose cumulative weight first exceeds `u`: the standard descent, one step per bit.
    /// Zero-weight prefixes are skipped by the `<=`, and a rounding overshoot clamps to the last.
    fn find(&self, mut u: f64) -> usize {
        let n = self.weights.len();
        let mut pos = 0;
        let mut step = n.next_power_of_two();
        while step > 0 {
            let next = pos + step;
            if next <= n && self.tree[next] <= u {
                pos = next;
                u -= self.tree[next];
            }
            step >>= 1;
        }
        pos.min(n - 1)
    }

    /// Re-read one replica's view into the tree.
    fn observe(&mut self, ctx: &RouteContext<'_>, i: usize) {
        self.views_read += 1;
        let w = self.weight(&ctx.views[i]);
        self.set(i, w);
    }
}

impl RoutingPolicy for WeightedRandom {
    fn label(&self) -> String {
        format!(
            "weighted_random(c1={},c2={},c3={},c4={},c5={})",
            self.c[0], self.c[1], self.c[2], self.c[3], self.c[4]
        )
    }

    fn inspected(&self, _fleet: usize) -> usize {
        if self.uniform { 1 } else { REFRESH_PER_DECISION + 1 }
    }

    fn choose(&mut self, ctx: &mut RouteContext<'_>) -> Option<usize> {
        let n = ctx.fleet();
        if n == 0 {
            return None;
        }
        if self.uniform {
            return crate::random::Random.choose(ctx);
        }
        // The fleet changed shape (an autoscaler moved it): the one O(N) rebuild, once per change.
        if self.weights.len() != n {
            self.weights = vec![0.0; n];
            self.tree = vec![0.0; n + 1];
            self.cursor = 0;
            for i in 0..n {
                self.observe(ctx, i);
            }
        }
        for _ in 0..REFRESH_PER_DECISION {
            let i = self.cursor;
            self.cursor = (i + 1) % n;
            self.observe(ctx, i);
        }
        for _ in 0..RETRIES {
            let total = self.total();
            if total <= 0.0 {
                break;
            }
            let i = self.find(ctx.rng.f64() * total);
            self.observe(ctx, i);
            if self.weights[i] > 0.0 && ctx.usable(i) {
                return Some(i);
            }
        }
        // Every weight is zero, or the draws kept landing on replicas the fresh view rules out:
        // uniform over whatever is usable, as `random` would.
        crate::random::Random.choose(ctx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{NoPrefixIndex, ReplicaView, RequestView};
    use sim_core::rng::Rng;

    fn request() -> RequestView {
        RequestView {
            id: 1,
            prompt_tokens: 1200,
            arrived_at: 0,
            deadline: 2_000_000_000,
            tenant: 0,
            attempts: 1,
            prefix_node: 0,
            prefix_tokens: 0,
        }
    }

    fn view(decode: u32, prefill: u32, decode_beyond: u32, prefill_beyond: u32) -> ReplicaView {
        ReplicaView {
            queued_decode: decode,
            queued_prefill: prefill,
            decode_beyond_buffer: decode_beyond,
            prefill_beyond_buffer: prefill_beyond,
            ..ReplicaView::default()
        }
    }

    /// `draws` decisions over fixed views, counted per replica.
    fn tally(p: &mut WeightedRandom, views: &[ReplicaView], draws: usize, seed: u64) -> Vec<usize> {
        let req = request();
        let mut rng = Rng::from_seed(seed);
        let live = |i: usize| views[i];
        let mut counts = vec![0; views.len()];
        for _ in 0..draws {
            let mut ctx = RouteContext::new(0, views, &req, &mut rng, &live, &NoPrefixIndex);
            counts[p.choose(&mut ctx).expect("a usable replica")] += 1;
        }
        counts
    }

    #[test]
    fn the_weight_is_the_linear_function_clamped_at_zero_and_zero_when_ejected() {
        let p = WeightedRandom::new([1.0, -0.3, -0.5, -0.05, -0.1]);
        assert_eq!(p.weight(&view(0, 0, 0, 0)), 1.0);
        assert!((p.weight(&view(3, 0, 0, 0)) - 0.7).abs() < 1e-12, "has queued decode, however many");
        assert!((p.weight(&view(1, 1, 0, 0)) - 0.2).abs() < 1e-12);
        assert!((p.weight(&view(1, 1, 2, 1)) - 0.0).abs() < 1e-12, "0.2 - 0.1 - 0.1");
        assert_eq!(p.weight(&view(1, 1, 0, 9)), 0.0, "clamped, never negative");
        assert_eq!(p.weight(&ReplicaView { ejected: true, ..view(0, 0, 0, 0) }), 0.0);
        assert_eq!(p.label(), "weighted_random(c1=1,c2=-0.3,c3=-0.5,c4=-0.05,c5=-0.1)");
    }

    /// Weights 1, 0.7, 0.2 and 0: the shares come out in proportion and the zero is never drawn.
    #[test]
    fn draws_are_proportional_to_weight_and_a_zero_weight_is_never_drawn() {
        let views = [view(0, 0, 0, 0), view(2, 0, 0, 0), view(1, 1, 0, 0), view(1, 1, 0, 3)];
        let mut p = WeightedRandom::new([1.0, -0.3, -0.5, -0.05, -0.1]);
        let draws = 40_000;
        let counts = tally(&mut p, &views, draws, 7);
        let share = |i: usize| counts[i] as f64 / draws as f64;
        assert!((share(0) - 1.0 / 1.9).abs() < 0.02, "{counts:?}");
        assert!((share(1) - 0.7 / 1.9).abs() < 0.02, "{counts:?}");
        assert!((share(2) - 0.2 / 1.9).abs() < 0.02, "{counts:?}");
        assert_eq!(counts[3], 0, "a zero weight is never drawn: {counts:?}");
        assert_eq!(p.inspected(4), REFRESH_PER_DECISION + 1);
    }

    /// The cost per decision is the same at 64 replicas and at 4,096: a fixed number of views read
    /// and no scan, once the fleet's shape is known.
    #[test]
    fn a_decision_reads_a_fixed_number_of_views_whatever_the_fleet_size() {
        let mut per_decision = Vec::new();
        for n in [64usize, 4096] {
            let views: Vec<ReplicaView> = (0..n).map(|i| view((i % 3) as u32, (i % 2) as u32, 0, (i % 5) as u32)).collect();
            let mut p = WeightedRandom::new([1.0, -0.3, -0.5, -0.05, -0.1]);
            tally(&mut p, &views, 1, 1);
            let before = p.views_read();
            let decisions = 2000;
            tally(&mut p, &views, decisions, 2);
            let reads = (p.views_read() - before) as f64 / decisions as f64;
            assert!(reads <= (REFRESH_PER_DECISION + RETRIES) as f64, "{n} replicas: {reads} views per decision");
            per_decision.push(reads);
        }
        assert!(
            (per_decision[0] - per_decision[1]).abs() <= 1.0,
            "cost grew with the fleet: {per_decision:?} views per decision at 64 and 4096"
        );
    }

    /// A replica that the fresh view marks ejected is not chosen even while its weight in the tree
    /// is stale: the draw re-reads before it trusts.
    #[test]
    fn an_ejected_replica_is_refused_on_the_draw_that_finds_it() {
        let mut views = vec![view(0, 0, 0, 0); 8];
        let mut p = WeightedRandom::new([1.0, -0.3, 0.0, 0.0, 0.0]);
        tally(&mut p, &views, 50, 3);
        views[5].ejected = true;
        let counts = tally(&mut p, &views, 2000, 4);
        assert_eq!(counts[5], 0, "{counts:?}");
        assert!(counts.iter().filter(|&&c| c > 0).count() == 7);
    }

    /// With `c2..c5` zero the draw sequence is `random`'s exactly, ejected replicas included.
    #[test]
    fn c1_alone_draws_exactly_as_random_does() {
        let mut views = vec![view(3, 2, 1, 1); 16];
        views[2].ejected = true;
        let req = request();
        let live = |i: usize| views[i];
        let mut a = WeightedRandom::new([4.0, 0.0, 0.0, 0.0, 0.0]);
        let mut b = crate::random::Random;
        let (mut ra, mut rb) = (Rng::from_seed(11), Rng::from_seed(11));
        for _ in 0..500 {
            let mut ca = RouteContext::new(0, &views, &req, &mut ra, &live, &NoPrefixIndex);
            let mut cb = RouteContext::new(0, &views, &req, &mut rb, &live, &NoPrefixIndex);
            assert_eq!(a.choose(&mut ca), b.choose(&mut cb));
        }
        assert_eq!(a.inspected(16), 1);
    }
}
