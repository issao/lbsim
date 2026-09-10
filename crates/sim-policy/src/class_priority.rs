//! lbsim-policy: scheduling names=class_priority,slo_class_priority_queues
//! The scheduling half of the SLO classes: interactive before agent before batch, at admission and
//! again when the step's prefill budget is handed out, FIFO within a class; and batch is the first
//! thing evicted when the cache overflows. Class 0 (classes off, or a session's parked context) goes
//! with interactive, so a scenario without classes behaves as FIFO with `newest` eviction.
//!
//! What this can and cannot do: it orders the work the replica already holds. When the fleet is inside
//! its capacity every class is served whatever the order, and when it is far past it no order rescues
//! interactive either, because the queue is longer than the head this policy sees. The band where it
//! matters is the one where batch's slack is the only slack there is.
//!
//! The order is a stable sort of at most `max_batch` entries, O(n log n) in the head of the queue;
//! the victim is one pass over the candidates.

use crate::scheduling::{pick_max_by, SchedulingPolicy, SeqView, StepView};
use sim_scenario::Scenario;

pub struct ClassPriority;

pub fn make(_sc: &Scenario) -> Box<dyn SchedulingPolicy> {
    Box::new(ClassPriority)
}

/// Admission rank: lower first. Interactive and unclassed share the front; anything above the class
/// table ranks with batch.
fn rank(class: u8) -> u8 {
    class.clamp(1, 3)
}

impl SchedulingPolicy for ClassPriority {
    fn label(&self) -> String {
        "class_priority".into()
    }

    fn admit_order(&mut self, v: &StepView<'_>) -> Vec<usize> {
        let mut order: Vec<usize> = (0..v.queued.len()).collect();
        order.sort_by_key(|&i| rank(v.queued[i].class));
        order
    }

    fn prefill_budget(&mut self, v: &StepView<'_>) -> u32 {
        v.step_token_budget
    }

    /// Interactive prompts get their chunks first: a batch prompt admitted a step earlier no longer
    /// puts twenty steps between an interactive request and its first token.
    fn prefill_order(&mut self, v: &StepView<'_>) -> Option<Vec<usize>> {
        let mut order: Vec<usize> =
            (0..v.running.len()).filter(|&i| v.running[i].prefill_left > 0).collect();
        order.sort_by_key(|&i| rank(v.running[i].class));
        Some(order)
    }

    /// The newest of the lowest class present: batch before agent before interactive.
    fn victim(&mut self, _v: &StepView<'_>, candidates: &[SeqView]) -> Option<usize> {
        pick_max_by(candidates, |c| (rank(c.class), c.admitted_at))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seq(id: u64, class: u8, admitted_at: u64) -> SeqView {
        SeqView {
            id,
            class,
            deadline: 1_000,
            arrived_at: id,
            admitted_at,
            prompt_tokens: 10,
            prefill_left: 10,
            resident_tokens: 10,
            queued: admitted_at == 0,
        }
    }

    fn view<'a>(queued: &'a [SeqView], running: &'a [SeqView]) -> StepView<'a> {
        StepView {
            now: 0,
            queued,
            running,
            kv_tokens: 0,
            kv_capacity: 1000,
            max_batch: 8,
            step_token_budget: 1024,
            prefill_tokens_per_s: 28_286.0,
            step_ns: &|_, _, _| 0,
        }
    }

    #[test]
    fn interactive_first_then_agent_then_batch_fifo_within_each() {
        // Queue order: batch, agent, interactive, unclassed, batch, interactive.
        let q = [seq(1, 3, 0), seq(2, 2, 0), seq(3, 1, 0), seq(4, 0, 0), seq(5, 3, 0), seq(6, 1, 0)];
        let got = ClassPriority.admit_order(&view(&q, &[]));
        assert_eq!(got, vec![2, 3, 5, 1, 0, 4]);
        assert_eq!(ClassPriority.prefill_budget(&view(&q, &[])), 1024);
    }

    #[test]
    fn prefill_goes_to_interactive_prompts_first_and_skips_finished_ones() {
        let mut r = [seq(1, 3, 10), seq(2, 1, 20), seq(3, 2, 30), seq(4, 1, 40)];
        r[2].prefill_left = 0;
        assert_eq!(ClassPriority.prefill_order(&view(&[], &r)), Some(vec![1, 3, 0]));
    }

    #[test]
    fn victim_is_the_newest_of_the_lowest_class_present() {
        let c = [seq(1, 1, 50), seq(2, 3, 10), seq(3, 2, 40), seq(4, 3, 20)];
        assert_eq!(ClassPriority.victim(&view(&[], &c), &c), Some(3), "newest batch");
        let c = [seq(1, 1, 50), seq(3, 2, 40), seq(5, 2, 40)];
        assert_eq!(ClassPriority.victim(&view(&[], &c), &c), Some(2), "newest agent, ties later");
        let c = [seq(1, 1, 50), seq(6, 0, 60)];
        assert_eq!(ClassPriority.victim(&view(&[], &c), &c), Some(1), "class 0 ranks with interactive");
        assert_eq!(ClassPriority.victim(&view(&[], &c), &[]), None);
    }
}
