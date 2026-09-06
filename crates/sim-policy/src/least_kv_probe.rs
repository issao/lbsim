//! Power of `d` choices on *live* KV occupancy, paying for every look.
//!
//! `p2c` reads the delayed snapshot for free and measures queued tokens. This policy keeps the same
//! sampling, so a head-to-head comparison isolates two changes and nothing else: the signal is
//! resident KV tokens, the unit a replica's memory is actually bounded in, and the state is fetched
//! with [`RouteContext::probe`], which the engine charges one modelled round trip per call. Freshness
//! then shows up in queue wait rather than being free, which is what makes the comparison against
//! scrape-based policies honest (`policy.proto` `Intent.RequestProbe`).
//!
//! Still O(1): `d` probes per decision regardless of fleet size, which is what section 10.4 asks for.

use crate::{RouteContext, RoutingPolicy};
use sim_scenario::Scenario;

pub struct LeastKvProbe {
    choices: usize,
}

pub fn make(sc: &Scenario) -> Box<dyn RoutingPolicy> {
    Box::new(LeastKvProbe { choices: sc.p2c_choices })
}

impl RoutingPolicy for LeastKvProbe {
    fn label(&self) -> String {
        format!("least_kv_probe(d={})", self.choices)
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
        // Same draw sequence as p2c, so the two policies consume the routing stream identically and
        // the only difference between their runs is the decision, not the dice.
        for _ in 0..self.choices.max(1) {
            let i = ctx.rng.below(n as u64) as usize;
            // Ejection is known from the snapshot; probing a replica the fleet has already given up
            // on would pay for information the router already has.
            if !ctx.usable(i) {
                continue;
            }
            let key = ctx.probe(i).kv_tokens;
            if key < best_key || (key == best_key && Some(i) < best) {
                best_key = key;
                best = Some(i);
            }
        }
        best.or_else(|| ctx.first_usable())
    }
}
