//! lbsim-policy: routing names=random
//! Uniform random. Better than round robin under size heterogeneity because it does not cycle, and
//! the cheapest O(1) policy there is.

use crate::{RouteContext, RoutingPolicy};
use sim_scenario::Scenario;

pub struct Random;

pub fn make(_sc: &Scenario) -> Box<dyn RoutingPolicy> {
    Box::new(Random)
}

impl RoutingPolicy for Random {
    fn label(&self) -> String {
        "random".into()
    }
    fn inspected(&self, _fleet: usize) -> usize {
        1
    }
    fn choose(&mut self, ctx: &mut RouteContext<'_>) -> Option<usize> {
        let n = ctx.fleet();
        if n == 0 {
            return None;
        }
        // A few draws before falling back to a scan, so an ejected replica costs a retry, not a scan.
        for _ in 0..8 {
            let i = ctx.rng.below(n as u64) as usize;
            if ctx.usable(i) {
                return Some(i);
            }
        }
        ctx.first_usable()
    }
}
