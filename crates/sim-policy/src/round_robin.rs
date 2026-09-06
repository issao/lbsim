//! lbsim-policy: routing names=round_robin
//! Round robin. Ignores load entirely, which with heterogeneous request sizes produces the rolling
//! hotspot: dynamic 1 in `docs/ARCHITECTURE.md` section 12.

use crate::{RouteContext, RoutingPolicy};
use sim_scenario::Scenario;

#[derive(Default)]
pub struct RoundRobin {
    cursor: usize,
}

pub fn make(_sc: &Scenario) -> Box<dyn RoutingPolicy> {
    Box::new(RoundRobin::default())
}

impl RoutingPolicy for RoundRobin {
    fn label(&self) -> String {
        "round_robin".into()
    }
    fn inspected(&self, _fleet: usize) -> usize {
        1
    }
    fn choose(&mut self, ctx: &mut RouteContext<'_>) -> Option<usize> {
        let n = ctx.fleet();
        if n == 0 {
            return None;
        }
        for _ in 0..n {
            let i = self.cursor % n;
            self.cursor += 1;
            if ctx.usable(i) {
                return Some(i);
            }
        }
        None
    }
}
