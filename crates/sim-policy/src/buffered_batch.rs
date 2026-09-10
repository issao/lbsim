//! lbsim-policy: scheduling names=buffered_batch
//! A batch buffer at the GPU scheduler.
//!
//! Issao: *"one piece of policy missing is at the GPU scheduler. We need a buffer for collecting
//! multiple prefill/decode requests in a single batch within available hbm bandwidth. That buffer
//! should be sized at up to the batch size (and max prefill and decode capacity per buffer tunable)
//! as well as have a time parameter that keeps a buffer around for at most n milliseconds."*
//!
//! The buffer is the next step's batch, and the policy decides two things `fifo_chunked` never did.
//! Admission is bounded by what the step can actually serve: seats (`buffer_max_batch`, never above
//! the replica's limit), prefill tokens (`buffer_max_prefill_tokens`, the chunk), decode seats
//! (`buffer_max_decode_seqs`) and the bandwidth line, the bandwidth-bound part of the step priced by
//! the cost model against the inter-token target. `fifo_chunked` admits everything the cache can
//! take and lets the admitted wait in the batch for their chunk, paying the key-value re-read for
//! them every step; this admits only what the chunk covers and leaves the rest queued, which is
//! `sim_core::scheduling::fill_buffer`. And an idle replica holds its queue for up to
//! `buffer_max_hold_ms` so one step serves several arrivals, stepping early when the buffer fills;
//! the engine keeps the clock, the policy gives the verdict (`SchedulingPolicy::hold`).
//!
//! Prefill spends `buffer_max_prefill_tokens` in batch order, the chunk the buffer was sized to, and
//! the victim rule is the scenario's `preemption_victim`, exactly as `fifo_chunked` applies it. What
//! the buffer leaves out is reported to the router as work beyond the open buffer
//! (`ReplicaView::prefill_beyond_buffer`), which is what `weighted_random` steers by.

use crate::scheduling::{fill_buffer, BufferLimits, SchedulingPolicy, SeqView, StepView};
use sim_core::scheduling::FifoChunked;
use sim_core::Nanos;
use sim_scenario::Scenario;

pub struct BufferedBatch {
    limits: BufferLimits,
    hold_ns: Nanos,
    fifo: FifoChunked,
}

pub fn make(sc: &Scenario) -> Box<dyn SchedulingPolicy> {
    Box::new(BufferedBatch::new(sc))
}

/// The bandwidth line's budget: the tightest inter-token target in force, the class table's when
/// classes are on and the scenario's key otherwise. A step whose fixed cost and key-value re-read
/// already exceed the target cannot meet it whatever else happens, so the buffer stops there. An
/// infinite target (a batch-only fleet) is no line at all.
fn step_budget_ns(sc: &Scenario) -> Nanos {
    let shares = sc.slo_class_shares();
    let itl_ms = if shares.is_empty() {
        sc.itl_slo_ms
    } else {
        shares.iter().map(|(class, _)| sc.slo_for(*class).1).fold(f64::INFINITY, f64::min)
    };
    if itl_ms.is_finite() && itl_ms > 0.0 {
        (itl_ms * 1e6) as Nanos
    } else {
        Nanos::MAX
    }
}

impl BufferedBatch {
    /// Zero in any size key means the replica's own limit: `max_batch`, `step_token_budget`, and
    /// the batch limit again for decode seats. The seat count is never above `max_batch`, because
    /// the engine's admission loop stops there whatever a policy says.
    pub fn new(sc: &Scenario) -> BufferedBatch {
        let max_batch = if sc.buffer_max_batch == 0 {
            sc.max_batch
        } else {
            sc.buffer_max_batch.min(sc.max_batch)
        };
        let limits = BufferLimits {
            max_batch,
            max_prefill_tokens: if sc.buffer_max_prefill_tokens == 0 {
                sc.step_token_budget
            } else {
                sc.buffer_max_prefill_tokens
            },
            max_decode_seqs: if sc.buffer_max_decode_seqs == 0 {
                max_batch
            } else {
                sc.buffer_max_decode_seqs
            },
            step_budget_ns: step_budget_ns(sc),
        };
        BufferedBatch {
            limits,
            hold_ns: (sc.buffer_max_hold_ms.max(0.0) * 1e6) as Nanos,
            fifo: FifoChunked::new(&sc.preemption_victim),
        }
    }

    #[cfg(test)]
    fn limits(&self) -> BufferLimits {
        self.limits
    }
}

impl SchedulingPolicy for BufferedBatch {
    fn label(&self) -> String {
        format!("buffered_batch(hold={}ms)", self.hold_ns as f64 / 1e6)
    }

    fn admit_order(&mut self, v: &StepView<'_>) -> Vec<usize> {
        fill_buffer(v, &self.limits).admit
    }

    fn prefill_budget(&mut self, _v: &StepView<'_>) -> u32 {
        self.limits.max_prefill_tokens
    }

    fn victim(&mut self, v: &StepView<'_>, candidates: &[SeqView]) -> Option<usize> {
        self.fifo.victim(v, candidates)
    }

    /// Step now if the queue head already fills the buffer; otherwise let the oldest entry wait the
    /// hold for company. The engine only asks with nothing running, so this never delays a token.
    fn hold(&mut self, v: &StepView<'_>) -> Option<Nanos> {
        if v.queued.is_empty() || fill_buffer(v, &self.limits).full {
            None
        } else {
            Some(self.hold_ns)
        }
    }

    fn buffer(&self) -> Option<BufferLimits> {
        Some(self.limits)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn queued(id: u64, prompt: u32) -> SeqView {
        SeqView {
            id,
            class: 0,
            deadline: 0,
            arrived_at: 0,
            admitted_at: 0,
            prompt_tokens: prompt,
            prefill_left: prompt,
            resident_tokens: 0,
            queued: true,
        }
    }

    fn running(id: u64, prefill_left: u32, resident: u64) -> SeqView {
        SeqView { prefill_left, resident_tokens: resident, queued: false, admitted_at: 1, ..queued(id, 1000) }
    }

    /// A step priced like the default cost model without its chunk: 10.2 ms fixed and 0.0175 ms per
    /// thousand resident tokens.
    fn price(_decoding: usize, kv: u64, _prefill: u32) -> Nanos {
        10_200_000 + (17_500.0 * kv as f64 / 1000.0) as Nanos
    }

    fn view<'a>(q: &'a [SeqView], r: &'a [SeqView], kv_tokens: u64) -> StepView<'a> {
        StepView {
            now: 0,
            queued: q,
            running: r,
            kv_tokens,
            kv_capacity: 1_370_000,
            max_batch: 8,
            step_token_budget: 1024,
            prefill_tokens_per_s: 28_286.0,
            step_ns: &price,
        }
    }

    fn scenario() -> Scenario {
        let mut sc = Scenario::default();
        sc.max_batch = 8;
        sc.step_token_budget = 1024;
        sc
    }

    #[test]
    fn zero_keys_mean_the_replica_limits_and_the_seat_count_never_exceeds_them() {
        let sc = scenario();
        let p = BufferedBatch::new(&sc);
        assert_eq!(
            p.limits(),
            BufferLimits { max_batch: 8, max_prefill_tokens: 1024, max_decode_seqs: 8, step_budget_ns: 80_000_000 }
        );
        let mut sc = scenario();
        sc.buffer_max_batch = 200;
        sc.buffer_max_prefill_tokens = 512;
        sc.buffer_max_decode_seqs = 3;
        sc.buffer_max_hold_ms = 2.0;
        let p = BufferedBatch::new(&sc);
        assert_eq!(p.limits().max_batch, 8, "never above the replica's batch limit");
        assert_eq!(p.limits().max_prefill_tokens, 512);
        assert_eq!(p.limits().max_decode_seqs, 3);
        assert_eq!(p.hold_ns, 2_000_000);
        assert_eq!(p.label(), "buffered_batch(hold=2ms)");
    }

    #[test]
    fn the_budget_is_the_tightest_class_target_and_infinite_for_batch_only() {
        let mut sc = scenario();
        sc.itl_slo_ms = 50.0;
        assert_eq!(step_budget_ns(&sc), 50_000_000);
        sc.slo_classes = "interactive:0.5,agent:0.5".into();
        assert_eq!(step_budget_ns(&sc), 80_000_000, "interactive's 80 ms binds over agent's 150");
        sc.slo_classes = "batch:1".into();
        assert_eq!(step_budget_ns(&sc), Nanos::MAX);
    }

    /// The chunk line: 1024 tokens admit one 1200-token prompt and nothing behind it, so the queue
    /// behind it is beyond the buffer and the buffer is full.
    #[test]
    fn admission_stops_where_the_prefill_tokens_run_out() {
        let sc = scenario();
        let mut p = BufferedBatch::new(&sc);
        let q: Vec<SeqView> = (0..5).map(|i| queued(i, 1200)).collect();
        let v = view(&q, &[], 0);
        assert_eq!(p.admit_order(&v), vec![0]);
        assert_eq!(p.prefill_budget(&v), 1024);
        assert_eq!(p.hold(&v), None, "a full buffer steps now");
        // Short prompts pack until the tokens are gone: 300 + 300 + 300 fit, the fourth takes the
        // last 124 and closes the line.
        let q: Vec<SeqView> = (0..6).map(|i| queued(i, 300)).collect();
        assert_eq!(p.admit_order(&view(&q, &[], 0)), vec![0, 1, 2, 3]);
    }

    /// The seat and decode lines, and the hold verdict on a buffer with room.
    #[test]
    fn seats_and_decode_seats_bound_the_batch_and_an_unfilled_buffer_holds() {
        let mut sc = scenario();
        sc.buffer_max_batch = 3;
        sc.buffer_max_prefill_tokens = 100_000;
        let mut p = BufferedBatch::new(&sc);
        let q: Vec<SeqView> = (0..5).map(|i| queued(i, 100)).collect();
        assert_eq!(p.admit_order(&view(&q, &[], 0)), vec![0, 1, 2]);
        let one = [queued(9, 100)];
        assert_eq!(p.hold(&view(&one, &[], 0)), Some(5_000_000), "room left: wait the hold for company");
        assert_eq!(p.hold(&view(&[], &[], 0)), None, "nothing queued, nothing to hold");
        // Two sequences already decoding against a decode line of two: no seat for a third. With a
        // line of three, one queued entry whose whole prompt fits the chunk takes the last seat, and
        // a running sequence that finishes its prefill this step counts as decoding too.
        sc.buffer_max_batch = 8;
        sc.buffer_max_decode_seqs = 2;
        let mut p = BufferedBatch::new(&sc);
        let r = [running(1, 0, 500), running(2, 0, 500)];
        assert_eq!(p.admit_order(&view(&q, &r, 1000)), Vec::<usize>::new());
        sc.buffer_max_decode_seqs = 3;
        let mut p = BufferedBatch::new(&sc);
        assert_eq!(p.admit_order(&view(&q, &r, 1000)), vec![0]);
        let r = [running(1, 0, 500), running(2, 40, 500), running(3, 0, 500)];
        assert_eq!(p.admit_order(&view(&q, &r, 1500)), Vec::<usize>::new(), "the finishing prefill takes the third seat");
    }

    /// The bandwidth line: at an 11 ms target the fixed 10.2 ms leaves 0.8 ms of re-read, about
    /// 45,700 tokens, so 4,000-token prompts stop at eleven; the empty-batch rule still admits the
    /// first however large it is.
    #[test]
    fn the_bandwidth_line_stops_admission_where_the_re_read_would_miss_the_target() {
        let mut sc = scenario();
        sc.max_batch = 64;
        sc.itl_slo_ms = 11.0;
        sc.buffer_max_prefill_tokens = 1_000_000;
        let mut p = BufferedBatch::new(&sc);
        let q: Vec<SeqView> = (0..40).map(|i| queued(i, 4000)).collect();
        let mut v = view(&q, &[], 0);
        v.max_batch = 64;
        let admitted = p.admit_order(&v);
        assert_eq!(admitted.len(), 11, "{admitted:?}");
        assert!(price(11, 44_000, 0) <= 11_000_000 && price(12, 48_000, 0) > 11_000_000);
        let huge = [queued(1, 900_000)];
        assert_eq!(p.admit_order(&view(&huge, &[], 0)), vec![0], "an empty batch always takes its first entry");
    }
}
