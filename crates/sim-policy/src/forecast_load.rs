//! lbsim-policy: routing names=forecast_load
//! Power of two choices over a *predicted* queue depth instead of the stale scrape.
//!
//! `queue_depth_extrapolation` (`docs/policy-catalog.md`, load forecasting): "extrapolate queued
//! tokens over the telemetry delay from the last two snapshots and the known drain rate, so routing
//! scores the predicted state at arrival, not the stale one." This is that policy: per replica, keep
//! the last two *distinct* views seen, fit a slope in tokens/s between them, and predict
//! `queued_tokens + slope * (now - sampled_at)`, clamped at zero. Fewer than two views falls back to
//! the stale value, same as `p2c` would use.
//!
//! The candidate sampling is drawn in exactly `p2c`'s order (`d` draws of `ctx.rng.below(n)`, same
//! `usable` skip, same tie-break to the lower index) so the two policies see byte-identical candidate
//! sets at the same seed. That is deliberate: any difference in outcome between the two is then a
//! difference in the *scoring*, not in which replicas got sampled, which is what makes the comparison
//! of "stale snapshot" against "forecast" honest.

use crate::{RouteContext, RoutingPolicy};
use sim_scenario::Scenario;

#[derive(Clone, Copy)]
struct Snapshot {
    sampled_at: u64,
    queued_tokens: u64,
}

#[derive(Clone, Copy, Default)]
struct History {
    prev: Option<Snapshot>,
    cur: Option<Snapshot>,
}

impl History {
    /// Record a view, but only if it is a genuinely new report (a different `sampled_at`). A repeat
    /// read of the same stale scrape must not be mistaken for a second data point.
    fn observe(&mut self, sampled_at: u64, queued_tokens: u64) {
        if let Some(cur) = self.cur {
            if cur.sampled_at == sampled_at {
                return;
            }
            self.prev = Some(cur);
        }
        self.cur = Some(Snapshot { sampled_at, queued_tokens });
    }

    /// Predicted queued tokens at `now`, or `None` with fewer than two distinct views, in which case
    /// the caller falls back to the stale value itself.
    fn predict(&self, now: u64) -> Option<u64> {
        let cur = self.cur?;
        let prev = self.prev?;
        let dt_ns = cur.sampled_at.saturating_sub(prev.sampled_at);
        if dt_ns == 0 {
            return None;
        }
        let dq = cur.queued_tokens as f64 - prev.queued_tokens as f64;
        let slope_per_ns = dq / dt_ns as f64;
        let elapsed_ns = now.saturating_sub(cur.sampled_at) as f64;
        let predicted = cur.queued_tokens as f64 + slope_per_ns * elapsed_ns;
        Some(predicted.max(0.0).round() as u64)
    }
}

pub struct ForecastLoad {
    choices: usize,
    history: Vec<History>,
}

pub fn make(sc: &Scenario) -> Box<dyn RoutingPolicy> {
    Box::new(ForecastLoad { choices: sc.p2c_choices, history: Vec::new() })
}

impl RoutingPolicy for ForecastLoad {
    fn label(&self) -> String {
        format!("forecast_load(d={})", self.choices)
    }
    fn inspected(&self, _fleet: usize) -> usize {
        self.choices
    }
    fn choose(&mut self, ctx: &mut RouteContext<'_>) -> Option<usize> {
        let n = ctx.fleet();
        if n == 0 {
            return None;
        }
        if self.history.len() != n {
            self.history = vec![History::default(); n];
        }
        let mut best: Option<usize> = None;
        let mut best_key = u64::MAX;
        for _ in 0..self.choices.max(1) {
            let i = ctx.rng.below(n as u64) as usize;
            if !ctx.usable(i) {
                continue;
            }
            let view = ctx.views[i];
            self.history[i].observe(view.sampled_at, view.queued_tokens);
            let key = self.history[i].predict(ctx.now).unwrap_or(view.queued_tokens);
            if key < best_key || (key == best_key && Some(i) < best) {
                best_key = key;
                best = Some(i);
            }
        }
        best.or_else(|| ctx.first_usable())
    }
}
