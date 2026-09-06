//! lbsim-policy: routing names=least_queue_tokens
//! Fewest queued tokens, over a full fleet scan of the stale snapshot. The same idea as
//! least_requests measured in the right unit, and just as vulnerable to herding.

use crate::{RouteContext, RoutingPolicy};
use sim_scenario::Scenario;

pub struct LeastQueueTokens;

pub fn make(_sc: &Scenario) -> Box<dyn RoutingPolicy> {
    Box::new(LeastQueueTokens)
}

impl RoutingPolicy for LeastQueueTokens {
    fn label(&self) -> String {
        "least_queue_tokens".into()
    }
    fn inspected(&self, fleet: usize) -> usize {
        fleet
    }
    fn choose(&mut self, ctx: &mut RouteContext<'_>) -> Option<usize> {
        let views = ctx.views;
        (0..views.len())
            .filter(|&i| ctx.usable(i))
            .min_by_key(|&i| (views[i].queued_tokens, i as u64))
    }
}
