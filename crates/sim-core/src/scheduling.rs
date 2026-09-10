//! The scheduling seam: what a replica does with the work it already holds.
//!
//! Issao: *"I assumed policies would cover the routing decision, the host and gpu local scheduling
//! decision and the global admission/rejection decisions."* Routing and admission had seams; the
//! replica's own scheduler was hard-wired into the engine's step. This is the third seam, at replica
//! scope, and it decides exactly four things per step:
//!
//! 1. **Batch admission and priority**: which queued sequences join the batch, and in what order.
//! 2. **The prefill chunk budget** for this step.
//! 3. **Who gets that budget**: the order in which the batch's unfinished prompts are prefilled.
//!    In a fleet whose replicas never queue, this is the only scheduling decision left: a long
//!    prompt ahead of you in the batch is many steps before your first chunk.
//! 4. **The preemption victim** when the key-value budget is exceeded.
//!
//! A fifth, defaulted, exists for a scheduler that batches by waiting: [`SchedulingPolicy::hold`]
//! lets an idle replica keep its queue for a bounded time so one step serves several arrivals, and
//! [`SchedulingPolicy::buffer`] tells the telemetry how much of the queue the next step can hold.
//! Issao: *"a buffer for collecting multiple prefill/decode requests in a single batch within
//! available hbm bandwidth."* The engine owns the timer; the policy owns the verdict.
//!
//! Everything else stays in the engine because it is accounting rather than policy: parked context
//! is evicted before a running sequence, the queue head's own parked context is exempt, an evicted
//! sequence re-enters ahead of the queue, and the step's cost comes from the cost model.
//!
//! **A policy never sees the whole queue.** The engine builds [`StepView::queued`] from at most
//! `max_batch` entries at the head of the queue, because that is the most a step could admit and
//! because a queue under overload is thousands deep: a decision that scanned it would cost O(queue)
//! per step, which is the scan section 10.4 of `docs/ARCHITECTURE.md` forbids. The view is therefore
//! O(batch + queue head) to build, and a policy's decision is bounded by the same. A sequence beyond
//! the head waits its turn at the head, whatever its class or deadline.
//!
//! The seam lives here, below both sides, for the reason `sim-leaf-api` does: the engine
//! (`sim-model`) calls it and the policies (`sim-policy`) implement it, and section 10.8 of
//! `docs/ARCHITECTURE.md` lets neither crate reach the other, so the interface has to sit where both
//! can see it. `sim_policy::scheduling` re-exports it, and the registry there resolves the
//! scenario's `scheduling` name. [`FifoChunked`] is here too: it is the engine's own default, what
//! every step did before the seam, and a `Replica::step` driven without a policy uses it.

use crate::Nanos;

/// One sequence as a scheduler sees it: queued or running, identified by request id.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SeqView {
    pub id: u64,
    /// SLO class, `sim_scenario::SLO_CLASSES` index plus one; zero when classes are off, and zero
    /// for parked context, which has no request behind it until its next turn arrives.
    pub class: u8,
    pub deadline: Nanos,
    /// When the request first arrived at the fleet.
    pub arrived_at: Nanos,
    /// When this replica admitted it to the batch; for parked context, when it was parked. Zero for
    /// a queued sequence, which has not been admitted yet.
    pub admitted_at: Nanos,
    pub prompt_tokens: u32,
    /// Prompt tokens still to prefill. For a queued sequence this is its whole prompt: what parked
    /// context or a prefix hit will spare is only known at admission, and looking it up for every
    /// queued entry would cost a scan per step.
    pub prefill_left: u32,
    /// Tokens this sequence holds in the cache right now. Zero for a queued sequence.
    pub resident_tokens: u64,
    pub queued: bool,
}

/// The replica at the moment of a decision. `queued` is the head of the queue only, see the module
/// doc; `running` is the whole batch.
#[derive(Clone, Copy)]
pub struct StepView<'a> {
    pub now: Nanos,
    pub queued: &'a [SeqView],
    pub running: &'a [SeqView],
    pub kv_tokens: u64,
    pub kv_capacity: u64,
    pub max_batch: usize,
    pub step_token_budget: u32,
    /// The replica's prefill rate, so a policy can turn tokens left into time left.
    pub prefill_tokens_per_s: f64,
    /// The cost model's price of a step, `(decoding sequences, resident key-value tokens, prefill
    /// tokens this step)` to nanoseconds, so a scheduler can price the batch it is assembling. A
    /// borrowed function rather than the model itself, because the layering keeps `sim-physics` out
    /// of `sim-policy` and the formula must stay in the one place it lives.
    pub step_ns: &'a dyn Fn(usize, u64, u32) -> Nanos,
}

impl std::fmt::Debug for StepView<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StepView")
            .field("now", &self.now)
            .field("queued", &self.queued)
            .field("running", &self.running)
            .field("kv_tokens", &self.kv_tokens)
            .field("kv_capacity", &self.kv_capacity)
            .field("max_batch", &self.max_batch)
            .field("step_token_budget", &self.step_token_budget)
            .field("prefill_tokens_per_s", &self.prefill_tokens_per_s)
            .finish_non_exhaustive()
    }
}

/// The open buffer's capacity: what one step may hold, for a scheduler that collects work into a
/// batch rather than admitting whatever is there (`SchedulingPolicy.BufferedBatch` in the proto).
/// Three counts and a bandwidth line: `step_budget_ns` is the longest the bandwidth-bound part of
/// the step may take, the fixed cost, the per-sequence cost and the key-value re-read, priced
/// through [`StepView::step_ns`] with no prefill chunk, so the tokens a step re-reads stay inside
/// what the device's memory bandwidth covers in that time. The chunk has its own line,
/// `max_prefill_tokens`, and is compute-bound, which is why it is not in this one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BufferLimits {
    pub max_batch: usize,
    pub max_prefill_tokens: u32,
    pub max_decode_seqs: usize,
    pub step_budget_ns: Nanos,
}

/// What filling a buffer from a view admits, and what it leaves waiting.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Fill {
    /// Indices into [`StepView::queued`] that fit, in queue order: the admission order.
    pub admit: Vec<usize>,
    /// Sequences that decode this step: the batch's finished prefills, plus every admitted entry
    /// whose whole prompt fits the chunk.
    pub decoding: usize,
    /// Running sequences still owed prefill that get no tokens this step: prefill work the buffer
    /// cannot hold.
    pub prefill_starved: usize,
    /// Prefill tokens the step spends.
    pub prefill_tokens: u32,
    /// A line bound the fill: no further queued entry can join this step.
    pub full: bool,
}

/// Fill the buffer from `v` under `l`: the batch first, in its own order, then the queue head entry
/// by entry until a line binds. The lines are the seat count, the prefill tokens, the decode
/// seats, the key-value capacity and the priced step; every one of them is monotone in what is
/// already in, so the first entry that does not fit ends the fill, exactly as the engine's own
/// admission loop stops at the first entry the cache cannot take. An empty batch always admits its
/// first entry, as the engine does, because refusing it would idle the replica with work queued.
/// A queued prompt is priced whole, as [`SeqView::prefill_left`] reports it. O(batch + queue head).
pub fn fill_buffer(v: &StepView<'_>, l: &BufferLimits) -> Fill {
    let mut f = Fill::default();
    let seats = l.max_batch.min(v.max_batch);
    let mut tokens = l.max_prefill_tokens;
    let mut kv = v.kv_tokens;
    let mut in_batch = v.running.len();
    for s in v.running {
        if s.prefill_left == 0 {
            f.decoding += 1;
        } else if tokens == 0 {
            f.prefill_starved += 1;
        } else {
            let take = s.prefill_left.min(tokens);
            tokens -= take;
            f.prefill_tokens += take;
            if take == s.prefill_left {
                f.decoding += 1;
            }
        }
    }
    for (i, s) in v.queued.iter().enumerate() {
        let take = s.prefill_left.min(tokens);
        let whole = take == s.prefill_left;
        let fits = in_batch == 0
            || (in_batch < seats
                && tokens > 0
                && f.decoding < l.max_decode_seqs
                && kv + s.prefill_left as u64 <= v.kv_capacity
                && (v.step_ns)(f.decoding + whole as usize, kv + s.prefill_left as u64, 0)
                    <= l.step_budget_ns);
        if !fits {
            f.full = true;
            break;
        }
        tokens -= take;
        f.prefill_tokens += take;
        kv += s.prefill_left as u64;
        in_batch += 1;
        if whole {
            f.decoding += 1;
        }
        f.admit.push(i);
    }
    f.full |= in_batch >= seats || tokens == 0 || f.decoding >= l.max_decode_seqs;
    f
}

/// What a replica has waiting, as its telemetry reports it: decode items and prefill items, and of
/// each the part the open buffer cannot hold, which is the work that waits a whole step cycle. The
/// engine computes it (`Replica::queued_work`) from [`SchedulingPolicy::buffer`]; a scheduler with
/// no buffer reports nothing beyond it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct QueuedWork {
    pub decode: u32,
    pub prefill: u32,
    pub decode_beyond: u32,
    pub prefill_beyond: u32,
}

pub trait SchedulingPolicy {
    fn label(&self) -> String;

    /// Indices into `v.queued`, in the order admission should try them. An index left out stays
    /// queued this step; an index out of range or repeated is ignored. FIFO is `0..n`.
    fn admit_order(&mut self, v: &StepView<'_>) -> Vec<usize>;

    /// Prefill tokens this step may spend.
    fn prefill_budget(&mut self, v: &StepView<'_>) -> u32;

    /// The order in which that budget is spent, as indices into `v.running`. `None` means batch
    /// order, which is what chunked prefill always did and what the default returns; an index with
    /// no prefill left, out of range or repeated is skipped.
    fn prefill_order(&mut self, _v: &StepView<'_>) -> Option<Vec<usize>> {
        None
    }

    /// Which of `candidates` to evict, as an index into it, or `None` to evict nothing. The engine
    /// calls this first over the eligible parked contexts and only then over the running batch, so a
    /// policy never has to choose between the two.
    fn victim(&mut self, v: &StepView<'_>, candidates: &[SeqView]) -> Option<usize>;

    /// On a replica with nothing running and something queued: how long the oldest queued entry may
    /// wait for company before the step runs, or `None` to step now. The engine defers the step to
    /// that entry's enqueue time plus this and asks again on every arrival, so a buffer that fills
    /// early steps early. Never asked while anything decodes: holding a running sequence's next
    /// token is a stall, not a buffer. The default steps at once, which is what every step did.
    fn hold(&mut self, _v: &StepView<'_>) -> Option<Nanos> {
        None
    }

    /// The open buffer's limits, for the telemetry's "beyond the buffer" counts ([`QueuedWork`]);
    /// `None` for a scheduler that admits whatever is there.
    fn buffer(&self) -> Option<BufferLimits> {
        None
    }
}

/// The index whose `key` is largest, ties to the later entry, so the choice is a pure function of
/// the state. Shared by every victim rule here.
pub fn pick_max_by<K: Ord>(candidates: &[SeqView], key: impl Fn(&SeqView) -> K) -> Option<usize> {
    let mut best: Option<(K, usize)> = None;
    for (i, c) in candidates.iter().enumerate() {
        let k = key(c);
        if best.as_ref().map_or(true, |(b, _)| k >= *b) {
            best = Some((k, i));
        }
    }
    best.map(|(_, i)| i)
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Victim {
    /// Last admitted, what vLLM does.
    Newest,
    LargestKv,
    LatestDeadline,
}

impl Victim {
    pub fn parse(name: &str) -> Victim {
        match name {
            "largest_kv" => Victim::LargestKv,
            "latest_deadline" => Victim::LatestDeadline,
            _ => Victim::Newest,
        }
    }
}

/// What the engine did before it had a scheduling seam: first come first served over the queue,
/// chunked prefill at the scenario's `step_token_budget`, and the scenario's `preemption_victim`
/// rule when the cache overflows. Every golden fingerprint from before the seam is a run of this
/// policy, and `check-fingerprints.sh` is what proves that the seam changed nothing.
pub struct FifoChunked {
    victim: Victim,
}

impl FifoChunked {
    /// `victim_rule` is the scenario's `preemption_victim` name.
    pub fn new(victim_rule: &str) -> FifoChunked {
        FifoChunked { victim: Victim::parse(victim_rule) }
    }
}

impl SchedulingPolicy for FifoChunked {
    fn label(&self) -> String {
        "fifo_chunked".into()
    }

    fn admit_order(&mut self, v: &StepView<'_>) -> Vec<usize> {
        (0..v.queued.len()).collect()
    }

    fn prefill_budget(&mut self, v: &StepView<'_>) -> u32 {
        v.step_token_budget
    }

    fn victim(&mut self, _v: &StepView<'_>, candidates: &[SeqView]) -> Option<usize> {
        match self.victim {
            Victim::Newest => pick_max_by(candidates, |c| c.admitted_at),
            Victim::LargestKv => pick_max_by(candidates, |c| c.resident_tokens),
            Victim::LatestDeadline => pick_max_by(candidates, |c| c.deadline),
        }
    }
}
