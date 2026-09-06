//! lbsim-policy: admission names=deadline_aware
//! Deadline-aware admission: shed what cannot make its deadline, before it costs anything.
//!
//! `scenario.proto` `AdmissionPolicy.DeadlineAware`: systems that check the deadline *after* running
//! spend most of their capacity under overload producing tokens nobody will read. A request whose
//! expected queue wait would already eat the time it needs to be served is rejected here, at the
//! router, and is recorded against `NO_REPLICA` without ever touching a replica.
//!
//! The rule: with `h = admission_headroom`, a request is admitted only while
//!
//! ```text
//! expected_queue_wait <= (deadline - now) * (1 - h)
//! ```
//!
//! so the fraction `h` of the remaining deadline stays reserved for prefill and decode. `h = 0`
//! sheds only what would time out in the queue alone; `h = 1` sheds anything that would wait at all.
//!
//! # The estimate, and where it is wrong
//!
//! This is a router-side decision, so it sees only the stale telemetry snapshot, never live state.
//! A replica admits from its queue only when a batch slot is free, so a newcomer's queue wait is the
//! time for enough slots to turn over, and the estimate is that turnover, averaged over the usable
//! fleet. Per replica, from the view:
//!
//! ```text
//! slots_needed = max(0, queued + 1 - (max_batch - running))
//! service_s    = mean_output_tokens * last_step_s
//! wait_s       = slots_needed * service_s / running
//! ```
//!
//! `last_step_s` is the step time the replica itself reported, so prefill chunks and the bandwidth
//! term are already in it; `mean_output_tokens` is the workload's mixture mean, standing in for the
//! service-time estimate a real router would keep from its own history. The biases are these:
//!
//! - **It overestimates under overload, and more so the shorter the deadline.** A slot frees when
//!   its request *ends*, and under overload requests end by timing out as well as by finishing, and
//!   the short tail of the output distribution finishes far sooner than the mean. Real turnover is
//!   faster than `service_s` says, so the policy sheds earlier than the deadline alone requires. That
//!   is the safe direction for a shedder, and headroom tunes it, but it is a bias and not a margin.
//! - It ignores the key-value budget. A queue of long prompts blocks admission that the sequence cap
//!   would have allowed, so where `kv_capacity_tokens` binds instead of `max_batch` the estimate is
//!   optimistic.
//! - It ignores queued prefill as a gate, because this engine prefills after admission; an engine
//!   that prefilled before admitting would add `queued_tokens / prefill_tokens_per_s`.
//! - It is a fleet mean, because the router does not yet know which replica the request will land
//!   on; with a balancing router that is close to the truth, with a herding one it is optimistic.
//! - It is as stale as the snapshot. Under a ramp it lags, under a spike it undershoots.
//!
//! A mean rather than a per-replica figure keeps the decision one pass over a slice already in
//! cache, which is what the seam offers; a policy that wanted O(1) would sample.

use crate::{Admission, AdmissionContext, AdmissionPolicy, ReplicaView};
use sim_core::Nanos;
use sim_scenario::Scenario;

pub struct DeadlineAware {
    headroom: f64,
    max_batch: usize,
    /// Decode steps a request is expected to hold its slot for.
    mean_output_tokens: f64,
    /// Step time to assume for a replica that has not reported one yet.
    idle_step_ns: Nanos,
}

pub fn make(sc: &Scenario) -> Box<dyn AdmissionPolicy> {
    let (_, o_mean) = sc.mixture_means();
    Box::new(DeadlineAware {
        headroom: sc.admission_headroom,
        max_batch: sc.max_batch,
        mean_output_tokens: o_mean,
        idle_step_ns: (sc.step_base_ms * 1e6) as Nanos,
    })
}

impl DeadlineAware {
    /// Time for enough of `v`'s batch slots to turn over that a newcomer would be admitted, in ns.
    fn slot_wait_ns(&self, v: &ReplicaView) -> f64 {
        let free = self.max_batch.saturating_sub(v.running as usize);
        let needed = (v.queued as usize + 1).saturating_sub(free);
        if needed == 0 {
            return 0.0;
        }
        let step_ns = if v.last_step_ns > 0 { v.last_step_ns } else { self.idle_step_ns };
        let service_ns = self.mean_output_tokens * step_ns as f64;
        needed as f64 * service_ns / v.running.max(1) as f64
    }

    /// Fleet-mean queue wait ahead of a newcomer, in nanoseconds, from the stale view.
    fn expected_queue_wait(&self, ctx: &AdmissionContext<'_>) -> Nanos {
        let (mut total, mut usable) = (0.0, 0u32);
        for v in ctx.views.iter().filter(|v| !v.ejected) {
            total += self.slot_wait_ns(v);
            usable += 1;
        }
        if usable == 0 {
            // Nothing to wait behind; the router will report that nothing is usable, and that is a
            // different failure from "would miss its deadline".
            return 0;
        }
        (total / usable as f64) as Nanos
    }
}

impl AdmissionPolicy for DeadlineAware {
    fn label(&self) -> String {
        format!("deadline_aware(headroom={})", self.headroom)
    }

    fn admit(&mut self, ctx: &AdmissionContext<'_>) -> Admission {
        let remaining = ctx.request.deadline.saturating_sub(ctx.now);
        let allowance = (remaining as f64 * (1.0 - self.headroom).clamp(0.0, 1.0)) as Nanos;
        if self.expected_queue_wait(ctx) > allowance {
            Admission::Reject
        } else {
            Admission::Admit
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::RequestView;

    const SECOND: Nanos = 1_000_000_000;

    fn ctx<'a>(views: &'a [ReplicaView], req: &'a RequestView) -> AdmissionContext<'a> {
        AdmissionContext { now: 0, views, request: req, tenant_weights: &[] }
    }

    fn request(deadline: Nanos) -> RequestView {
        RequestView { id: 1, prompt_tokens: 100, arrived_at: 0, deadline, tenant: 0, attempts: 1 }
    }

    /// A full batch with a queue behind it, stepping at 50 ms. With the default 300-token mean output
    /// a slot turns over every 15 s / 32 running, so eight queued requests are about 4 s of wait.
    fn saturated(queued: u32) -> ReplicaView {
        ReplicaView {
            queued,
            running: 32,
            last_step_ns: 50_000_000,
            ..Default::default()
        }
    }

    #[test]
    fn idle_fleet_admits_and_a_full_fleet_with_a_deep_queue_rejects() {
        let sc = Scenario::default();
        let mut p = make(&sc);
        let req = request(SECOND);
        assert_eq!(p.admit(&ctx(&[ReplicaView::default(); 4], &req)), Admission::Admit);
        assert_eq!(p.admit(&ctx(&[saturated(40); 4], &req)), Admission::Reject);
    }

    #[test]
    fn a_free_slot_means_no_wait_whatever_the_step_time() {
        let sc = Scenario::default();
        let mut p = make(&sc);
        let mut v = saturated(0);
        v.running = 31;
        assert_eq!(p.admit(&ctx(&[v], &request(1))), Admission::Admit);
    }

    #[test]
    fn ejected_replicas_do_not_count_and_an_empty_fleet_admits() {
        let sc = Scenario::default();
        let mut p = make(&sc);
        let req = request(SECOND);
        let mut views = [saturated(40); 2];
        views.iter_mut().for_each(|v| v.ejected = true);
        assert_eq!(p.admit(&ctx(&views, &req)), Admission::Admit);
        views[0].ejected = false;
        assert_eq!(p.admit(&ctx(&views, &req)), Admission::Reject);
    }

    #[test]
    fn headroom_is_the_only_difference_between_admit_and_reject_at_the_margin() {
        let mut sc = Scenario::default();
        // About 4 s of turnover against a 10 s deadline: admitted with no headroom, shed once more
        // than 60% of the deadline is reserved for serving.
        let views = [saturated(8)];
        let req = request(10 * SECOND);
        sc.admission_headroom = 0.0;
        assert_eq!(make(&sc).admit(&ctx(&views, &req)), Admission::Admit);
        sc.admission_headroom = 0.9;
        assert_eq!(make(&sc).admit(&ctx(&views, &req)), Admission::Reject);
        assert_eq!(make(&sc).label(), "deadline_aware(headroom=0.9)");
    }
}
