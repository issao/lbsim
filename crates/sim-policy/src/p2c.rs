//! Power of two choices. Sample `d` replicas at random, take the least loaded by queued tokens.
//!
//! Nearly as good as global least-loaded, and far more robust to stale telemetry, because only a
//! fraction of routers consider any one apparently-idle replica at a time. Also O(1) by construction,
//! which is the second independent reason section 10.4 prefers sampling policies.

use crate::{RouteContext, RoutingPolicy};
use sim_scenario::Scenario;

pub struct PowerOfTwoChoices {
    choices: usize,
}

pub fn make(sc: &Scenario) -> Box<dyn RoutingPolicy> {
    Box::new(PowerOfTwoChoices { choices: sc.p2c_choices })
}

impl RoutingPolicy for PowerOfTwoChoices {
    fn label(&self) -> String {
        format!("p2c(d={})", self.choices)
    }
    fn inspected(&self, _fleet: usize) -> usize {
        self.choices
    }
    fn choose(&mut self, ctx: &mut RouteContext<'_>) -> Option<usize> {
        let n = ctx.fleet();
        if n == 0 {
            return None;
        }
        let mut best: Option<usize> = None;
        let mut best_key = u64::MAX;
        for _ in 0..self.choices.max(1) {
            let i = ctx.rng.below(n as u64) as usize;
            if !ctx.usable(i) {
                continue;
            }
            let key = ctx.views[i].queued_tokens;
            if key < best_key || (key == best_key && Some(i) < best) {
                best_key = key;
                best = Some(i);
            }
        }
        best.or_else(|| ctx.first_usable())
    }
}
