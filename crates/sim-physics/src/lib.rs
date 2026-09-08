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
//! `docs/ARCHITECTURE.md` section 3 lives in [`epoch`], with its naive oracle in [`oracle`]; the
//! engine still steps through [`CostModel`] until the preemption unit wires the epoch advance in.

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
    /// Per Issao: "we could get disable decode basically by setting HBM to infinity." True zeroes the
    /// bandwidth term, so a decode step costs only the fixed and per-sequence parts and traffic looks
    /// like stateless serving. KV accounting is untouched: capacity still binds in tokens.
    pub disable_decode: bool,
    /// Host link bandwidth for swapping key-value context between HBM and DRAM, in GB/s. PCIe 5.0 x16
    /// is 64 GB/s on paper and about 50 in practice, which is the default.
    pub swap_gbps: f64,
    /// Draft tokens per sequence per step from a small speculating model, verified in the same step.
    /// Zero is off. Verification is compute over N extra tokens per sequence, prefill-class work, and
    /// that is the cost side of speculation: it grows with the batch while the gain does not.
    pub spec_draft_tokens: u32,
    /// Probability the big model accepts each draft token. With `spec_draft_tokens` off it is unused.
    pub spec_accept_rate: f64,
}

/// Bytes of key-value cache per resident token: 80 layers, keys and values, 8 grouped-query heads of
/// 128 dimensions, two bytes each. That is a 70B-class model in bf16 with GQA, 320 KiB a token, so
/// a 3,000-token context is close to a gigabyte and crosses a 50 GB/s link in about 20 ms.
pub const KV_BYTES_PER_TOKEN: u64 = 80 * 2 * 8 * 128 * 2;

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
        self.step_split(decoding, kv_tokens, prefill_tokens).1
    }

    /// The step's duration split by roofline: `(compute_ns, total_ns)`, the second being exactly
    /// [`CostModel::step_ns`], which is a thin wrapper so the two cannot drift.
    ///
    /// The compute-bound part is the work priced at `prefill_tokens_per_s`: the prefill chunk and
    /// speculative verification. Everything else, the fixed weight read, the per-sequence cost and the
    /// key-value re-read, is priced as bandwidth. At large batch the weight read is amortised and a
    /// real decode step goes compute-bound, but this model prices decode by bandwidth regardless, and
    /// the split reports what the model priced, not what a device would measure. That is exactly what
    /// `METRIC_WASTED_GPU_FRACTION` means by "bandwidth-bound decode at small batch": busy time the
    /// model charged to memory traffic rather than to the arithmetic units.
    ///
    /// The sum is computed term by term in the same order as before the split existed, so every
    /// fingerprint of every run stays byte-identical.
    #[inline]
    pub fn step_split(&self, decoding: usize, kv_tokens: u64, prefill_tokens: u32) -> (Nanos, Nanos) {
        let bandwidth = if self.disable_decode {
            0
        } else {
            ((self.step_per_kv_ktoken_ms * kv_tokens as f64 / 1000.0) * 1e6) as Nanos
        };
        // Speculation verifies N drafts per decoding sequence in this step: compute work at the
        // prefill rate. The bandwidth term is untouched, the weights are read once a step either way.
        let verify = decoding as f64 * self.spec_draft_tokens as f64;
        let compute_ns = ((prefill_tokens as f64 / self.prefill_tokens_per_s) * 1e9) as Nanos
            + ((verify / self.prefill_tokens_per_s) * 1e9) as Nanos;
        let step_ns = (self.step_base_ms * 1e6) as Nanos
            + (self.step_per_seq_ms * 1e6) as Nanos * decoding as Nanos
            + bandwidth
            + compute_ns;
        (compute_ns, step_ns.max(1))
    }

    /// Expected tokens a decoding sequence advances per step: one, or with N drafts each accepted with
    /// probability a, the truncated geometric sum (1 - a^(N+1)) / (1 - a). The engine applies this as
    /// a deterministic per-sequence fractional accumulator rather than a random draw, so a run with
    /// speculation off is byte-identical to one that never had the feature and the golden fingerprints
    /// prove it. A per-step geometric draw from a named stream is the stochastic refinement for later.
    #[inline]
    pub fn spec_tokens_per_step(&self) -> f64 {
        let n = self.spec_draft_tokens;
        let a = self.spec_accept_rate;
        if n == 0 || a <= 0.0 {
            1.0
        } else if a >= 1.0 {
            (n + 1) as f64
        } else {
            (1.0 - a.powi(n as i32 + 1)) / (1.0 - a)
        }
    }

    /// Time to move `tokens` of key-value context across the host link, one direction. A swap out
    /// and a swap back in are two of these. Never below one nanosecond for a non-empty transfer, so
    /// a swap is never free.
    #[inline]
    pub fn swap_ns(&self, tokens: u64) -> Nanos {
        if tokens == 0 {
            return 0;
        }
        let bytes = tokens as f64 * KV_BYTES_PER_TOKEN as f64;
        ((bytes / (self.swap_gbps * 1e9)) * 1e9).max(1.0) as Nanos
    }

    /// Effective batch limit: the sequence cap, or the token budget, whichever binds first.
    pub fn effective_batch(&self, kv_capacity_tokens: f64, max_batch: usize, ctx_mean: f64) -> f64 {
        (kv_capacity_tokens / ctx_mean).min(max_batch as f64).max(1.0)
    }

    /// Rated capacity in requests per second, from the cost model rather than from a guess.
    ///
    /// Prefill and decode contend for the same device, so device-seconds per request are additive.
    /// `p_mean` and `o_mean` are the workload's mean prompt and output lengths over its mixture.
    /// Speculation enters on both sides: the verify compute lengthens the step, and the expected
    /// tokens per step divides the number of steps an output needs.
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
        let per_kv = if self.disable_decode { 0.0 } else { self.step_per_kv_ktoken_ms };
        let step_s = (self.step_base_ms + self.step_per_seq_ms * batch + per_kv * batch * ctx_mean / 1000.0)
            / 1000.0
            + batch * self.spec_draft_tokens as f64 / self.prefill_tokens_per_s;
        let prefill_s = p_mean / self.prefill_tokens_per_s;
        let decode_s = o_mean * step_s / (batch * self.spec_tokens_per_step());
        replicas as f64 / (prefill_s + decode_s)
    }
}

pub mod epoch;
pub mod oracle;
pub mod rational;

pub use epoch::{
    Bound, Epoch, EpochModel, EpochParams, EpochState, Spec, LEGACY_ENGINE, NO_SPEC, REF,
    SPEC_PROFILES,
};
pub use oracle::{run_epochs, run_stepwise, RunResult, Seq};
pub use rational::Rational;

#[cfg(test)]
mod tests {
    use super::*;

    fn model(disable_decode: bool, spec_draft_tokens: u32) -> CostModel {
        CostModel {
            step_base_ms: 24.0,
            step_per_seq_ms: 0.07,
            step_per_kv_ktoken_ms: 0.011,
            prefill_tokens_per_s: 28_286.0,
            disable_decode,
            swap_gbps: 50.0,
            spec_draft_tokens,
            spec_accept_rate: 0.7,
        }
    }

    /// The split's total is the step time, and its compute part is the prefill-priced work alone.
    #[test]
    fn step_split_total_is_step_ns_and_compute_is_the_prefill_priced_part() {
        for m in [model(false, 0), model(true, 0), model(false, 4), model(true, 3)] {
            for (decoding, kv, prefill) in [(0, 0, 0), (1, 900, 0), (16, 64_000, 512), (256, 1_024_000, 0), (7, 3_000, 28_286)] {
                let (compute, total) = m.step_split(decoding, kv, prefill);
                assert_eq!(total, m.step_ns(decoding, kv, prefill), "{decoding} {kv} {prefill}");
                assert!(compute <= total);
                let verify = decoding as f64 * m.spec_draft_tokens as f64;
                let expected = ((prefill as f64 / m.prefill_tokens_per_s) * 1e9) as Nanos
                    + ((verify / m.prefill_tokens_per_s) * 1e9) as Nanos;
                assert_eq!(compute, expected);
            }
        }
        // A whole second of prefill at the rated token rate, exactly, with decode priced at nothing
        // beyond the fixed cost.
        let m = model(true, 0);
        assert_eq!(m.step_split(0, 0, 28_286).0, 1_000_000_000);
        assert_eq!(m.step_split(0, 0, 28_286).1, 24_000_000 + 1_000_000_000);
        assert_eq!(m.step_split(3, 5_000, 0).0, 0, "no prefill and no speculation is no compute");
    }
}
