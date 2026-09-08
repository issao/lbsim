//! lbsim-policy: scheduling names=deadline_first,earliest_deadline_first,edf
//! Least slack first, at admission and when the step's prefill budget is handed out; latest deadline
//! evicted first. Slack is the deadline minus now minus the time this replica's prefill rate needs
//! for the tokens still to prefill, so a long prompt with a later deadline can go ahead of a short
//! one whose deadline is sooner: what matters is who can still make it. Ties keep queue order.
//!
//! The rate comes from the view (`prefill_tokens_per_s`), the scenario's per-replica figure, not a
//! measurement of this replica's current speed; a slowed replica's slack is therefore optimistic.
//! The victim rule is the scenario's `latest_deadline` rule, whatever `preemption_victim` says:
//! the request with the most slack is the one that can best afford to be re-admitted.
//!
//! O(n log n) in the head of the queue for the order, one pass for the victim.

use crate::scheduling::{pick_max_by, SchedulingPolicy, SeqView, StepView};
use sim_scenario::Scenario;

pub struct DeadlineFirst;

pub fn make(_sc: &Scenario) -> Box<dyn SchedulingPolicy> {
    Box::new(DeadlineFirst)
}

/// Nanoseconds of slack, negative once missed. Integer arithmetic after one float division, so the
/// order is a pure function of the view.
fn slack(v: &StepView<'_>, s: &SeqView) -> i128 {
    let prefill_ns = if v.prefill_tokens_per_s > 0.0 {
        (s.prefill_left as f64 / v.prefill_tokens_per_s * 1e9) as i128
    } else {
        0
    };
    s.deadline as i128 - v.now as i128 - prefill_ns
}

impl SchedulingPolicy for DeadlineFirst {
    fn label(&self) -> String {
        "deadline_first".into()
    }

    fn admit_order(&mut self, v: &StepView<'_>) -> Vec<usize> {
        let mut order: Vec<usize> = (0..v.queued.len()).collect();
        order.sort_by_key(|&i| slack(v, &v.queued[i]));
        order
    }

    fn prefill_budget(&mut self, v: &StepView<'_>) -> u32 {
        v.step_token_budget
    }

    fn prefill_order(&mut self, v: &StepView<'_>) -> Option<Vec<usize>> {
        let mut order: Vec<usize> =
            (0..v.running.len()).filter(|&i| v.running[i].prefill_left > 0).collect();
        order.sort_by_key(|&i| slack(v, &v.running[i]));
        Some(order)
    }

    fn victim(&mut self, _v: &StepView<'_>, candidates: &[SeqView]) -> Option<usize> {
        pick_max_by(candidates, |c| c.deadline)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seq(id: u64, deadline: u64, prefill_left: u32) -> SeqView {
        SeqView {
            id,
            class: 0,
            deadline,
            arrived_at: 0,
            admitted_at: 0,
            prompt_tokens: prefill_left,
            prefill_left,
            resident_tokens: 0,
            queued: true,
        }
    }

    fn view<'a>(now: u64, queued: &'a [SeqView], running: &'a [SeqView]) -> StepView<'a> {
        StepView {
            now,
            queued,
            running,
            kv_tokens: 0,
            kv_capacity: 1000,
            max_batch: 8,
            step_token_budget: 1024,
            // 1,000 tokens a second: a token of prefill is a millisecond of slack.
            prefill_tokens_per_s: 1_000.0,
        }
    }

    #[test]
    fn least_slack_first_and_prefill_left_counts_against_slack() {
        let ms = 1_000_000u64;
        // Deadlines 100, 50, 60 ms out; the third owes 30 ms of prefill, so its slack (30) is the
        // least, then the second (50), then the first (99).
        let q = [seq(1, 100 * ms, 1), seq(2, 50 * ms, 0), seq(3, 60 * ms, 30)];
        assert_eq!(DeadlineFirst.admit_order(&view(0, &q, &[])), vec![2, 1, 0]);
        // Ties keep queue order.
        let q = [seq(1, 50 * ms, 0), seq(2, 50 * ms, 0), seq(3, 40 * ms, 0)];
        assert_eq!(DeadlineFirst.admit_order(&view(0, &q, &[])), vec![2, 0, 1]);
        // A missed deadline sorts first: it is the most urgent thing there is.
        let q = [seq(1, 50 * ms, 0), seq(2, 10 * ms, 0)];
        assert_eq!(DeadlineFirst.admit_order(&view(20 * ms, &q, &[])), vec![1, 0]);
    }

    #[test]
    fn prefill_goes_to_the_least_slack_and_skips_finished_prompts() {
        let ms = 1_000_000u64;
        let r = [seq(1, 100 * ms, 1), seq(2, 50 * ms, 0), seq(3, 60 * ms, 30)];
        // The second has nothing left to prefill, so only the other two are ordered: least slack
        // first, and the third's 30 ms of prefill puts it ahead of the first.
        assert_eq!(DeadlineFirst.prefill_order(&view(0, &[], &r)), Some(vec![2, 0]));
    }

    #[test]
    fn victim_is_the_latest_deadline_ties_to_the_later_entry() {
        let c = [seq(1, 90, 0), seq(2, 50, 0), seq(3, 90, 0)];
        assert_eq!(DeadlineFirst.victim(&view(0, &[], &c), &c), Some(2));
        assert_eq!(DeadlineFirst.victim(&view(0, &[], &c), &[]), None);
    }
}
