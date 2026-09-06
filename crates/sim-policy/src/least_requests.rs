//! Fewest queued-plus-running requests, over a full fleet scan of the stale snapshot.
//!
//! Included as a baseline that is *wrong* for this domain twice over: it counts requests where
//! capacity is denominated in tokens, and it reads every replica's stale state, so every router sees
//! the same apparently idle replica and herds onto it. Its `inspected` is the whole fleet, which is
//! the O(N) cost section 10.4 bans at scale.

use crate::{RouteContext, RoutingPolicy};
use sim_scenario::Scenario;

pub struct LeastRequests;

pub fn make(_sc: &Scenario) -> Box<dyn RoutingPolicy> {
    Box::new(LeastRequests)
}

impl RoutingPolicy for LeastRequests {
    fn label(&self) -> String {
        "least_requests".into()
    }
    fn inspected(&self, fleet: usize) -> usize {
        fleet
    }
    fn choose(&mut self, ctx: &mut RouteContext<'_>) -> Option<usize> {
        let views = ctx.views;
        (0..views.len())
            .filter(|&i| ctx.usable(i))
            .min_by_key(|&i| (views[i].queued + views[i].running, i))
    }
}
