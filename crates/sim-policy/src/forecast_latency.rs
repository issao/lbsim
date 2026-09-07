//! lbsim-policy: routing names=forecast_latency
//! Latency forecasting, applied to routing. Sample `d` replicas exactly as `p2c` does — same draw
//! order, so the two policies see byte-identical candidate sets at the same seed — but instead of
//! ranking candidates by `queued_tokens`, predict this request's own time to first token on each and
//! route to the minimum.
//!
//! docs/policy-catalog.md, "latency forecasting": "Expected TTFT from queued prefill tokens ahead of
//! the request divided by the replica's prefill rate, plus the chunk schedule". This is that idea
//! without the chunk schedule: TTFT is forecast as the prefill work already queued ahead of the
//! request, plus the request's own prefill, both divided by the fleet's prefill rate (prefill is
//! compute-bound, so tokens-per-second is the right unit), plus one step latency to join the batch.
//! No state and no randomness beyond the shared sampling draw: the same seed produces the same
//! candidate sets as `p2c`, so any difference in outcome between the two is the ranking, not the dice.

use crate::{RouteContext, RoutingPolicy};
use sim_scenario::Scenario;

pub struct ForecastLatency {
    choices: usize,
    prefill_tokens_per_s: f64,
}

pub fn make(sc: &Scenario) -> Box<dyn RoutingPolicy> {
    Box::new(ForecastLatency {
        choices: sc.p2c_choices,
        prefill_tokens_per_s: sc.cost_model().prefill_tokens_per_s,
    })
}

impl RoutingPolicy for ForecastLatency {
    fn label(&self) -> String {
        format!("forecast_latency(d={})", self.choices)
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
        let mut best_key = f64::INFINITY;
        for _ in 0..self.choices.max(1) {
            let i = ctx.rng.below(n as u64) as usize;
            if !ctx.usable(i) {
                continue;
            }
            let view = &ctx.views[i];
            // Prefill work already ahead of this request, plus this request's own prefill, both
            // compute-bound and so both in the same tokens-per-second rate; then one step to join
            // the batch.
            let prefill_ahead_tokens = view.queued_tokens as f64 + ctx.request.prompt_tokens as f64;
            let predicted_ttft_ns =
                (prefill_ahead_tokens / self.prefill_tokens_per_s) * 1e9 + view.last_step_ns as f64;
            if predicted_ttft_ns < best_key || (predicted_ttft_ns == best_key && Some(i) < best) {
                best_key = predicted_ttft_ns;
                best = Some(i);
            }
        }
        best.or_else(|| ctx.first_usable())
    }
}
