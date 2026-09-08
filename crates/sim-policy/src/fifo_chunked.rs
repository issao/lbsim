//! lbsim-policy: scheduling names=fifo_chunked,fcfs
//! The engine's own default, registered under its name: first come first served over the queue,
//! chunked prefill at `step_token_budget`, victims by the scenario's `preemption_victim`. The
//! implementation is `sim_core::scheduling::FifoChunked`, because a `Replica::step` driven without
//! a policy (the model's own tests) has to be able to reach it below the policy layer.

use crate::scheduling::SchedulingPolicy;
use sim_core::scheduling::FifoChunked;
use sim_scenario::Scenario;

pub fn make(sc: &Scenario) -> Box<dyn SchedulingPolicy> {
    Box::new(FifoChunked::new(&sc.preemption_victim))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scheduling::{SeqView, StepView};

    fn seq(id: u64, admitted_at: u64, resident: u64, deadline: u64) -> SeqView {
        SeqView {
            id,
            class: 0,
            deadline,
            arrived_at: admitted_at,
            admitted_at,
            prompt_tokens: 10,
            prefill_left: 0,
            resident_tokens: resident,
            queued: false,
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
        }
    }

    #[test]
    fn fifo_admits_in_queue_order_and_spends_the_scenario_budget() {
        let q = [seq(1, 0, 0, 5), seq(2, 0, 0, 1), seq(3, 0, 0, 3)];
        let mut p = FifoChunked::new("newest");
        assert_eq!(p.admit_order(&view(&q, &[])), vec![0, 1, 2]);
        assert_eq!(p.prefill_budget(&view(&q, &[])), 1024);
        assert_eq!(p.prefill_order(&view(&q, &[])), None, "fifo spends the budget in batch order");
    }

    #[test]
    fn each_victim_rule_picks_its_maximum_with_ties_to_the_later_entry() {
        let c = [seq(1, 10, 500, 90), seq(2, 30, 200, 50), seq(3, 30, 500, 90)];
        let v = view(&[], &c);
        assert_eq!(FifoChunked::new("newest").victim(&v, &c), Some(2));
        assert_eq!(FifoChunked::new("largest_kv").victim(&v, &c), Some(2));
        assert_eq!(FifoChunked::new("latest_deadline").victim(&v, &c), Some(2));
        let c = [seq(1, 40, 500, 99), seq(2, 30, 200, 50)];
        assert_eq!(FifoChunked::new("newest").victim(&v, &c), Some(0));
        assert_eq!(FifoChunked::new("newest").victim(&v, &[]), None);
    }
}
