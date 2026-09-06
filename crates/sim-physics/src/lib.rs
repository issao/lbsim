//! The cost model: what a replica step costs, and what a fleet is rated to serve.
//!
//! One place, on purpose. `docs/agent-architecture.md` section 7 names "two agents owning the cost
//! model" as the anti-pattern that guarantees divergence in the one place divergence is invisible, so
//! the step-time formula and the rated-capacity formula live here and nowhere else. The engine calls
//! [`CostModel::step_ns`]; the scenario's `rated_rps` calls [`CostModel::rated_rps`]. Neither
//! reimplements the other.
//!
//! The model is the calibrated two-point roofline from `bench/validate_epochs.py`: a fixed per-step
//! cost that covers the weight read plus launch overhead, a bandwidth term proportional to resident
//! key-value tokens, and prefill as a compute-bound token rate. The closed-form epoch advance from
//! `docs/ARCHITECTURE.md` section 3 lands here when the KV growth term does.

use sim_core::Nanos;

/// The four constants every step cost derives from. Copied out of the scenario so the physics crate
/// does not need to know what a scenario is.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CostModel {
    /// Fixed per-step cost: weight read, launch, sampling, scheduler.
    pub step_base_ms: f64,
    /// Marginal step cost per decoding sequence.
    pub step_per_seq_ms: f64,
    /// Step cost per thousand resident key-value tokens: the memory-bandwidth term.
    pub step_per_kv_ktoken_ms: f64,
    /// Prefill is compute-bound, so it is a token rate rather than a per-sequence cost.
    pub prefill_tokens_per_s: f64,
}

impl CostModel {
    /// Duration of one engine step.
    ///
    /// Fixed overhead, a marginal cost per decoding sequence, the bandwidth term over every resident
    /// token, and the prefill work done this step. Prefill and decode contend for one device, which is
    /// why the terms add and why a big prefill shows up in everyone's inter-token latency. The
    /// bandwidth term is why step time grows as contexts lengthen rather than only as the batch widens:
    /// the engine re-reads every resident key-value token every step, and at batch 256 and 4,000 tokens
    /// of context it is larger than the weight read.
    ///
    /// Integer nanoseconds, never below one, so a degenerate scenario cannot schedule a step at the
    /// current instant and spin.
    #[inline]
    pub fn step_ns(&self, decoding: usize, kv_tokens: u64, prefill_tokens: u32) -> Nanos {
        let step_ns = (self.step_base_ms * 1e6) as Nanos
            + (self.step_per_seq_ms * 1e6) as Nanos * decoding as Nanos
            + ((self.step_per_kv_ktoken_ms * kv_tokens as f64 / 1000.0) * 1e6) as Nanos
            + ((prefill_tokens as f64 / self.prefill_tokens_per_s) * 1e9) as Nanos;
        step_ns.max(1)
    }

    /// Effective batch limit: the sequence cap, or the token budget, whichever binds first.
    pub fn effective_batch(&self, kv_capacity_tokens: f64, max_batch: usize, ctx_mean: f64) -> f64 {
        (kv_capacity_tokens / ctx_mean).min(max_batch as f64).max(1.0)
    }

    /// Rated capacity in requests per second, from the cost model rather than from a guess.
    ///
    /// Prefill and decode contend for the same device, so device-seconds per request are additive.
    /// `p_mean` and `o_mean` are the workload's mean prompt and output lengths over its mixture.
    pub fn rated_rps(
        &self,
        replicas: usize,
        kv_capacity_tokens: f64,
        max_batch: usize,
        p_mean: f64,
        o_mean: f64,
    ) -> f64 {
        // Mean resident context over a request's life: the prompt plus half its output.
        let ctx_mean = p_mean + o_mean / 2.0;
        let batch = self.effective_batch(kv_capacity_tokens, max_batch, ctx_mean);
        let step_s = (self.step_base_ms
            + self.step_per_seq_ms * batch
            + self.step_per_kv_ktoken_ms * batch * ctx_mean / 1000.0)
            / 1000.0;
        let prefill_s = p_mean / self.prefill_tokens_per_s;
        let decode_s = o_mean * step_s / batch;
        replicas as f64 / (prefill_s + decode_s)
    }
}
