//! The analytic epoch advance: `docs/ARCHITECTURE.md` section 3, in exact arithmetic.
//!
//! Within an epoch the batch composition is fixed, so the step time is the maximum of two lines in
//! the step index `k`, one for memory bandwidth and one for compute. Two lines cross at most once,
//! so the duration of `n` steps is at most two arithmetic series split at the crossover, and the
//! inverse question, how many steps fit in an offset, is the floor of a quadratic root. Both are
//! O(1) whatever `n` is, which is what makes engine cost scale with composition changes rather than
//! with tokens.
//!
//! Everything here is integer or rational. `bench/validate_epochs.py` proved the closed form equal
//! to the per-step sum in Python's `Fraction`; this is the same proof in Rust, and it runs in the
//! test suite on every change because a plausible-looking edit to this file produces
//! plausible-looking wrong numbers that no float tolerance catches.
//!
//! # Representation
//!
//! Every coefficient of both lines is an integer numerator over one denominator shared by the
//! epoch, `den = lcm(G*bw*u_n, G*fl*m_n, 10^9) * lcm(M_d, C_d)`, where `u_n`, `m_n` are the
//! numerators of the two utilizations in lowest terms, `10^9` carries `t_fix` in nanoseconds, and
//! `M_d`, `C_d` are the denominators of the accepted-token rate and the mean context. The hardware
//! part is computed once per [`EpochModel`]; an [`Epoch`] then costs a dozen multiplications and
//! no gcd. Summing, comparing and splitting the lines are then plain integer operations on
//! numerators, and the naive per-step sum lands on the same denominator, so the differential test
//! compares integers for equality.
//!
//! # Overflow bound
//!
//! For the reference hardware `den` is `2^16 * 3^4 * 5^13 * 7 * 11 * 67 * lcm(M_d, C_d)`, below
//! 2^66 times a context denominator of at most the batch size, so below 2^76 at batch 512 and
//! `M = 9/2`. The largest intermediate in `duration` is `alpha * n * (n - 1) / 2`; with `alpha`
//! under a millisecond per step, that is under `2^66 * n^2`, so `n < 2^30` steps overflows nothing,
//! and `n * beta` is under 2^128 for durations up to 2^52 seconds. No epoch at the section 1.1 fleet
//! scale comes within a factor of a thousand of either: the longest epoch is one sequence's output
//! length in steps, and the largest batch is 512. A test asserts the reference denominator bound so
//! a change to the constants that breaks it is caught rather than assumed away. All arithmetic is
//! checked, so an input outside the bound panics instead of wrapping.

use crate::rational::{add, lcm, mul, Rational};
use sim_core::rng::Rng;

/// Hardware and model constants, named as in `docs/ARCHITECTURE.md` section 3.1. Provenance for
/// every value is `docs/calibration.md` section 6 and `bench/validate_epochs.py`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EpochParams {
    /// `G`: tensor-parallel degree, GPUs per replica.
    pub tp: u64,
    /// `bw`: HBM bandwidth per GPU, bytes per second.
    pub hbm_bytes_per_s: u64,
    /// `u`: memory-bandwidth utilization at large batch.
    pub mbu: Rational,
    /// `fl`: peak dense FLOPS per GPU.
    pub peak_flops: u64,
    /// `m`: model-FLOPS utilization during decode.
    pub mfu_decode: Rational,
    /// Model-FLOPS utilization during prefill, for the throughput anchor.
    pub mfu_prefill: Rational,
    /// `kv`: bytes of key-value cache per token, over all layers.
    pub kv_bytes_per_token: u64,
    /// `W`: total weight bytes.
    pub weight_bytes: u64,
    /// `P`: parameter count.
    pub n_params: u64,
    pub n_layers: u64,
    pub n_heads: u64,
    pub head_dim: u64,
    /// `t_fix`: fixed per-step overhead in nanoseconds: launch, sampling, scheduler, all-reduce.
    pub fixed_step_ns: u64,
}

/// Llama-3-70B bf16 on 8x H100 SXM, `bench/validate_epochs.py` `REF`.
pub const REF: EpochParams = EpochParams {
    tp: 8,
    hbm_bytes_per_s: 3_350_000_000_000,
    mbu: Rational::raw(70, 100),
    peak_flops: 990_000_000_000_000,
    mfu_decode: Rational::raw(45, 100),
    mfu_prefill: Rational::raw(50, 100),
    kv_bytes_per_token: 327_680,
    weight_bytes: 140_000_000_000,
    n_params: 70_000_000_000,
    n_layers: 80,
    n_heads: 64,
    head_dim: 128,
    fixed_step_ns: 2_750_000,
};

/// The same hardware under a 2024-era Python-scheduler engine, which spent most of a low-batch
/// step off the GPU.
pub const LEGACY_ENGINE: EpochParams = EpochParams { fixed_step_ns: 8_000_000, ..REF };

/// Speculative decoding as section 3.4 specifies it: propose `N` per step, accept `M` on average,
/// fixed for the epoch. Not speculating is `N = 1, M = 1`: one token proposed and one accepted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Spec {
    /// `N`: draft tokens verified per step.
    pub draft: u64,
    /// `M`: tokens of real progress per step.
    pub accepted: Rational,
}

pub const NO_SPEC: Spec = Spec { draft: 1, accepted: Rational::raw(1, 1) };

/// The profiles `bench/validate_epochs.py` draws from, including a fractional acceptance rate.
pub const SPEC_PROFILES: [Spec; 4] = [
    NO_SPEC,
    Spec { draft: 3, accepted: Rational::raw(2, 1) },
    Spec { draft: 5, accepted: Rational::raw(3, 1) },
    Spec { draft: 8, accepted: Rational::raw(9, 2) },
];

/// What is fixed for the life of one epoch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EpochState {
    /// `B`: sequences decoding.
    pub batch: u64,
    /// `S0`: key-value tokens resident at the first step.
    pub resident_tokens: u64,
    /// `C`: mean context attended per sequence. Usually `S0 / B`, but kept separate because a
    /// caller may count resident KV and attended context differently, as the Python does.
    pub ctx_mean: Rational,
    pub spec: Spec,
}

impl EpochState {
    /// A batch whose attended context is exactly its resident KV: the common case.
    pub fn uniform(batch: u64, resident_tokens: u64, spec: Spec) -> Self {
        let ctx_mean = if batch == 0 {
            Rational::ZERO
        } else {
            Rational::new(resident_tokens as u128, batch as u128)
        };
        EpochState { batch, resident_tokens, ctx_mean, spec }
    }

    /// The distribution property P2 of the Python draws from, so the Rust suite covers the same
    /// regimes: empty through oversize batches, resident KV up to a full 8x H100, contexts from
    /// chat to long-document, and every speculation profile.
    pub fn random(rng: &mut Rng) -> Self {
        const CONTEXTS: [u64; 4] = [128, 1_000, 4_000, 32_000];
        let batch = rng.below(513);
        let resident_tokens = rng.below(1_400_001);
        let ctx_mean = Rational::int(CONTEXTS[rng.below(4) as usize] as u128);
        let spec = SPEC_PROFILES[rng.below(4) as usize];
        EpochState { batch, resident_tokens, ctx_mean, spec }
    }
}

impl EpochParams {
    pub fn compile(self) -> EpochModel {
        EpochModel::new(self)
    }

    /// Hardware nobody sells, to exercise the arithmetic off the reference constants: any
    /// tensor-parallel degree, bandwidth in whole GB/s, FLOPS in whole TFLOPS, utilizations in
    /// percent. The granularity is deliberate: it keeps the shared denominator within the
    /// documented bound, since bandwidth and FLOPS then share the 10^9 factor `t_fix` needs.
    pub fn random(rng: &mut Rng) -> Self {
        let percent = |rng: &mut Rng| Rational::new(1 + rng.below(100) as u128, 100);
        let weight_bytes = 1_000_000_000 * (1 + rng.below(400));
        EpochParams {
            tp: 1 << rng.below(4),
            hbm_bytes_per_s: 1_000_000_000 * (100 + rng.below(4_900)),
            mbu: percent(rng),
            peak_flops: 1_000_000_000_000 * (10 + rng.below(1_990)),
            mfu_decode: percent(rng),
            mfu_prefill: percent(rng),
            kv_bytes_per_token: 1_024 * (1 + rng.below(512)),
            weight_bytes,
            n_params: weight_bytes / (1 + rng.below(4)),
            n_layers: 1 + rng.below(128),
            n_heads: 1 + rng.below(128),
            head_dim: 64 << rng.below(2),
            fixed_step_ns: rng.below(10_000_000),
        }
    }
}

/// [`EpochParams`] with the hardware part of the shared denominator precomputed, so opening an
/// epoch is multiplications only. Build one per hardware profile, not per epoch.
#[derive(Clone, Copy, Debug)]
pub struct EpochModel {
    params: EpochParams,
    /// `lcm(G*bw*u_n, G*fl*m_n, 10^9)`.
    d_hw: u128,
    /// `d_hw / (G*bw*u_n)`: scales a byte count into `1/d_hw` seconds after multiplying by `u_d`.
    s_bw: u128,
    /// `d_hw / (G*fl*m_n)`: the same for a FLOP count and `m_d`.
    s_cp: u128,
    /// `d_hw / 10^9`: the same for nanoseconds.
    s_fix: u128,
    u: Rational,
    m: Rational,
}

impl EpochModel {
    pub fn new(params: EpochParams) -> Self {
        let u = params.mbu.reduced();
        let m = params.mfu_decode.reduced();
        let bw_den = mul(mul(params.tp as u128, params.hbm_bytes_per_s as u128), u.num());
        let cp_den = mul(mul(params.tp as u128, params.peak_flops as u128), m.num());
        let d_hw = lcm(lcm(bw_den, cp_den), 1_000_000_000);
        EpochModel {
            params,
            d_hw,
            s_bw: d_hw / bw_den,
            s_cp: d_hw / cp_den,
            s_fix: d_hw / 1_000_000_000,
            u,
            m,
        }
    }

    pub fn params(&self) -> &EpochParams {
        &self.params
    }

    /// The hardware part of every epoch's denominator; the whole denominator is this times
    /// `lcm(M_d, C_d)`.
    pub fn hardware_denominator(&self) -> u128 {
        self.d_hw
    }

    /// Prefill is compute-bound: `mfu_prefill * fl * G / (2 * P)` tokens per second.
    pub fn prefill_tokens_per_s(&self) -> Rational {
        let p = &self.params;
        let flops = Rational::int(mul(p.peak_flops as u128, p.tp as u128));
        p.mfu_prefill.mul(flops).div(Rational::int(2 * p.n_params as u128))
    }

    /// Open an epoch: the two lines of section 3.1 over the shared denominator.
    pub fn epoch(&self, s: &EpochState) -> Epoch {
        let p = &self.params;
        let acc = s.spec.accepted.reduced();
        let ctx = s.ctx_mean.reduced();
        let l = lcm(acc.den(), ctx.den());
        let l_acc = l / acc.den();
        let l_ctx = l / ctx.den();
        let den = mul(self.d_hw, l);

        let b = s.batch as u128;
        let n = s.spec.draft as u128;
        let fix = mul(p.fixed_step_ns as u128, self.s_fix);
        let kv = p.kv_bytes_per_token as u128;

        // Bandwidth: weights plus resident KV, read once per step whatever N is; grows by B*M
        // tokens of KV per step.
        let bytes0 = add(p.weight_bytes as u128, mul(kv, s.resident_tokens as u128));
        let beta_bw = mul(add(mul(mul(bytes0, self.u.den()), self.s_bw), fix), l);
        let alpha_bw = mul(mul(mul(mul(mul(kv, b), acc.num()), self.u.den()), self.s_bw), l_acc);

        // Compute: 2 FLOPs per parameter per verified token, plus attention over every key, two
        // matmuls per layer per head, for each of the N verified tokens.
        let gemm = mul(mul(2 * p.n_params as u128, b), n);
        let attn = mul(mul(4 * p.n_layers as u128, p.n_heads as u128), p.head_dim as u128);
        let attn0 = mul(mul(mul(mul(mul(mul(attn, ctx.num()), b), n), self.m.den()), self.s_cp), l_ctx);
        let beta_cp = add(mul(add(mul(mul(gemm, self.m.den()), self.s_cp), fix), l), attn0);
        let alpha_cp =
            mul(mul(mul(mul(mul(mul(attn, b), acc.num()), n), self.m.den()), self.s_cp), l_acc);

        Epoch::from_lines(den, beta_bw, alpha_bw, beta_cp, alpha_cp)
    }
}

/// Which of the two lines leads at a step.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Bound {
    Bandwidth,
    Compute,
}

/// One epoch's step-time function, `t(k) = max(beta_bw + alpha_bw*k, beta_cp + alpha_cp*k)`, with
/// every coefficient an integer over `den` seconds.
#[derive(Clone, Copy, Debug)]
pub struct Epoch {
    den: u128,
    bw: (u128, u128),
    cp: (u128, u128),
    /// The line that leads at `k = 0`, then the other. Bandwidth wins a tie, arbitrarily.
    lead: (u128, u128),
    trail: (u128, u128),
    /// The first step index at which the trailing line strictly leads, if that ever happens.
    split: Option<u128>,
    leads_at_start: Bound,
}

impl Epoch {
    /// Lines given directly as `(beta, alpha)` numerators over `den`. This is what `EpochModel`
    /// produces and what a test wanting a crossover at a chosen step constructs by hand.
    pub fn from_lines(den: u128, beta_bw: u128, alpha_bw: u128, beta_cp: u128, alpha_cp: u128) -> Self {
        assert!(den > 0, "sim-physics: zero epoch denominator");
        let bw = (beta_bw, alpha_bw);
        let cp = (beta_cp, alpha_cp);
        let (lead, trail, leads_at_start) = if beta_bw >= beta_cp {
            (bw, cp, Bound::Bandwidth)
        } else {
            (cp, bw, Bound::Compute)
        };
        // The trailing line overtakes only if it is steeper: beta_t + alpha_t*k > beta_l +
        // alpha_l*k  <=>  k > (beta_l - beta_t) / (alpha_t - alpha_l), so the first such integer
        // step is the floor of that plus one. Parallel or shallower never overtakes.
        // A crossover past 2^62 steps is outside any epoch the fleet can run and outside the
        // documented arithmetic bound, so it counts as never.
        let split = if trail.1 > lead.1 {
            Some((lead.0 - trail.0) / (trail.1 - lead.1) + 1).filter(|s| *s < 1 << 62)
        } else {
            None
        };
        Epoch { den, bw, cp, lead, trail, split, leads_at_start }
    }

    pub fn denominator(&self) -> u128 {
        self.den
    }

    pub fn leads_at_start(&self) -> Bound {
        self.leads_at_start
    }

    /// The step index from which the line that trailed at `k = 0` leads, when the crossover falls
    /// at a non-negative step.
    pub fn crossover_step(&self) -> Option<u64> {
        self.split.map(|s| u64::try_from(s).unwrap_or(u64::MAX))
    }

    /// `(beta, alpha)` numerators of the bandwidth line, over [`Self::denominator`].
    pub fn bandwidth_line(&self) -> (u128, u128) {
        self.bw
    }

    pub fn compute_line(&self) -> (u128, u128) {
        self.cp
    }

    fn step_num(&self, k: u128) -> u128 {
        let bw = add(self.bw.0, mul(self.bw.1, k));
        let cp = add(self.cp.0, mul(self.cp.1, k));
        bw.max(cp)
    }

    /// Duration of step `k`, counted from zero.
    pub fn step_time(&self, k: u64) -> Rational {
        Rational::raw(self.step_num(k as u128), self.den)
    }

    /// Sum of `beta + alpha*k` for `k` in `[lo, hi)`. `m*(m-1)` is even, so the halving is exact:
    /// this is the line where the Python reference once used true division and broke the proof.
    fn series((beta, alpha): (u128, u128), lo: u128, hi: u128) -> u128 {
        if hi <= lo {
            return 0;
        }
        let m = hi - lo;
        let index_sum = add(mul(lo, m), mul(m, m - 1) / 2);
        add(mul(m, beta), mul(alpha, index_sum))
    }

    /// Duration of the first `n` steps, in closed form. O(1).
    pub fn duration(&self, n: u64) -> Rational {
        let n = n as u128;
        let num = match self.split {
            Some(s) if s < n => {
                add(Self::series(self.lead, 0, s), Self::series(self.trail, s, n))
            }
            _ => Self::series(self.lead, 0, n),
        };
        Rational::raw(num, self.den)
    }

    /// The same by adding up every step. This is the oracle, never the fast path.
    pub fn naive_duration(&self, n: u64) -> Rational {
        let mut total = 0u128;
        for k in 0..n as u128 {
            total = add(total, self.step_num(k));
        }
        Rational::raw(total, self.den)
    }

    /// How many whole steps have elapsed `offset` seconds into the epoch: the largest `n` with
    /// `duration(n) <= offset`. Exact.
    ///
    /// A float solves the quadratic to land within a step or two of the answer, then integer
    /// comparison walks to the exact one, so the float never determines the result. If the walk
    /// does not settle in a few moves, which only a pathological line can cause, an exact
    /// bisection takes over. Saturates at 2^40 steps if the steps are free.
    pub fn steps_elapsed(&self, offset: Rational) -> u64 {
        if self.duration(1) > offset {
            return 0;
        }
        let d = offset.to_f64() * self.den as f64;
        // Which piece the offset lands in decides which line's quadratic to solve.
        let (base, line) = match self.split {
            Some(s) if self.duration_u128(s) <= offset => (s, self.trail),
            _ => (0, self.lead),
        };
        let t_base = self.duration_u128(base).to_f64() * self.den as f64;
        let (beta, alpha) = (line.0 as f64, line.1 as f64);
        // Steps m past `base` satisfy  (alpha/2) m^2 + (beta + alpha*base - alpha/2) m <= d - t_base.
        let c = t_base - d;
        let b = beta + alpha * base as f64 - alpha / 2.0;
        let m = if alpha == 0.0 {
            if beta == 0.0 {
                f64::NAN
            } else {
                -c / beta
            }
        } else {
            // The root of a x^2 + b x + c with c <= 0, in the form that does not cancel.
            let disc = (b * b - 2.0 * alpha * c).sqrt();
            if b + disc == 0.0 {
                0.0
            } else {
                -2.0 * c / (b + disc)
            }
        };
        let guess = if m.is_finite() && m >= 0.0 && m < (1u64 << 40) as f64 {
            Some(base.saturating_add(m.floor() as u128))
        } else {
            None
        };
        if let Some(mut n) = guess {
            for _ in 0..8 {
                if self.duration_u128(n + 1) <= offset {
                    n += 1;
                } else if n > 0 && self.duration_u128(n) > offset {
                    n -= 1;
                } else {
                    return u64::try_from(n).unwrap_or(u64::MAX);
                }
            }
        }
        self.steps_elapsed_bisect(offset)
    }

    /// The same for an offset on the engine's clock.
    pub fn steps_elapsed_ns(&self, offset_ns: u64) -> u64 {
        self.steps_elapsed(Rational::from_nanos(offset_ns))
    }

    fn duration_u128(&self, n: u128) -> Rational {
        self.duration(u64::try_from(n).unwrap_or(u64::MAX))
    }

    fn steps_elapsed_bisect(&self, offset: Rational) -> u64 {
        const CAP: u64 = 1 << 40;
        let (mut lo, mut hi) = (0u64, 1u64);
        while self.duration(hi) <= offset {
            lo = hi;
            if hi >= CAP {
                return CAP;
            }
            hi *= 2;
        }
        while lo + 1 < hi {
            let mid = lo + (hi - lo) / 2;
            if self.duration(mid) <= offset {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        lo
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(batch: u64, resident: u64, ctx: u64, spec: Spec) -> EpochState {
        EpochState { batch, resident_tokens: resident, ctx_mean: Rational::int(ctx as u128), spec }
    }

    /// One case: closed form against the loop, for equality on the rational.
    fn check(epoch: &Epoch, n: u64) -> (bool, bool) {
        let inside = matches!(epoch.crossover_step(), Some(s) if s > 0 && s < n);
        (epoch.duration(n) == epoch.naive_duration(n), inside)
    }

    #[test]
    fn closed_form_equals_naive_on_random_epochs_exactly() {
        let mut rng = Rng::from_seed(0x5EED_0001);
        let profiles = [REF.compile(), LEGACY_ENGINE.compile()];
        let (mut cases, mut inside, mut compute_led, mut naive_steps) = (0u64, 0u64, 0u64, 0u64);
        for i in 0..10_000u64 {
            // Half on the reference hardware, half on hardware nobody sells.
            let model = if i % 2 == 0 {
                profiles[rng.below(2) as usize]
            } else {
                EpochParams::random(&mut rng).compile()
            };
            let s = EpochState::random(&mut rng);
            let epoch = model.epoch(&s);
            // Steer a share of cases onto the crossover, which random `n` alone rarely reaches:
            // with the reference constants the bandwidth line is always the steeper one, so a
            // crossover needs compute to lead at k = 0, a large batch and a short context.
            let n = match epoch.crossover_step() {
                Some(split) if split < 20_000 && rng.below(2) == 0 => {
                    let lo = split.saturating_sub(50);
                    lo + rng.below(split - lo + 100)
                }
                _ => rng.below(3_000),
            };
            let (same, crossed) = check(&epoch, n);
            assert!(
                same,
                "closed form differs from naive sum: case {i} state {s:?} n {n} lines bw {:?} cp {:?} den {}",
                epoch.bandwidth_line(),
                epoch.compute_line(),
                epoch.denominator()
            );
            cases += 1;
            inside += crossed as u64;
            compute_led += (epoch.leads_at_start() == Bound::Compute) as u64;
            naive_steps += n;
        }
        println!(
            "{cases} random epochs, {naive_steps} naive steps summed, {inside} with the crossover \
             inside the epoch, {compute_led} compute-led at k=0"
        );
        assert_eq!(cases, 10_000);
        assert!(inside >= 500, "only {inside} cases exercised the crossover");
        assert!(compute_led >= 500, "only {compute_led} cases were compute-led at k=0");
    }

    #[test]
    fn crossover_between_bandwidth_and_compute_is_exact() {
        // Hand-built lines over a small denominator, so the crossover lands where the case says.
        // Bandwidth leads at k=0 with a shallow slope; compute starts lower and is steeper.
        // (beta_bw=1000, alpha_bw=3) vs (beta_cp=400, alpha_cp=7): equal at k=150 exactly.
        let on_step = Epoch::from_lines(1_000, 1_000, 3, 400, 7);
        assert_eq!(on_step.crossover_step(), Some(151));
        for n in [0, 1, 150, 151, 152, 300, 5_000] {
            assert_eq!(on_step.duration(n), on_step.naive_duration(n), "n={n}");
        }
        // The split matters: a single series of the leader is wrong past the crossover.
        let one_series = Rational::raw(Epoch::series((1_000, 3), 0, 300), 1_000);
        assert_ne!(on_step.duration(300), one_series);
        assert_eq!(on_step.duration(150), Rational::raw(Epoch::series((1_000, 3), 0, 150), 1_000));

        // Crossover between steps: equal at k = 600/7 = 85.71, so compute leads from 86.
        let between = Epoch::from_lines(1_000, 1_000, 3, 400, 10);
        assert_eq!(between.crossover_step(), Some(86));
        for n in [85, 86, 87, 1_000] {
            assert_eq!(between.duration(n), between.naive_duration(n), "n={n}");
        }

        // Before the epoch: compute already leads at k=0 and is steeper, so no crossover at all.
        let before = Epoch::from_lines(1_000, 400, 3, 1_000, 7);
        assert_eq!(before.crossover_step(), None);
        assert_eq!(before.leads_at_start(), Bound::Compute);
        assert_eq!(before.duration(777), before.naive_duration(777));

        // After the epoch: crossover at 151 with an epoch of 100 steps stays a single series.
        assert_eq!(on_step.duration(100), Rational::raw(Epoch::series((1_000, 3), 0, 100), 1_000));

        // Parallel lines never cross, whichever is higher.
        let parallel = Epoch::from_lines(1_000, 400, 5, 1_000, 5);
        assert_eq!(parallel.crossover_step(), None);
        assert_eq!(parallel.duration(999), parallel.naive_duration(999));

        // And on the real constants: compute-led at k=0, bandwidth overtakes a few thousand steps in.
        let model = REF.compile();
        let s = state(256, 32_768, 128, SPEC_PROFILES[2]);
        let epoch = model.epoch(&s);
        assert_eq!(epoch.leads_at_start(), Bound::Compute);
        let split = epoch.crossover_step().expect("bandwidth must overtake compute here");
        assert!(split > 1_000 && split < 10_000, "split {split}");
        for n in [split - 1, split, split + 1, split + 2_000] {
            assert_eq!(epoch.duration(n), epoch.naive_duration(n), "n={n}");
        }
    }

    #[test]
    fn inversion_recovers_the_step_count() {
        let mut rng = Rng::from_seed(0x5EED_0002);
        let profiles = [REF.compile(), LEGACY_ENGINE.compile()];
        for i in 0..20_000u64 {
            let model = if i % 4 == 3 {
                EpochParams::random(&mut rng).compile()
            } else {
                profiles[rng.below(2) as usize]
            };
            let epoch = model.epoch(&EpochState::random(&mut rng));
            let n = rng.below(5_001);
            let lo = epoch.duration(n);
            let hi = epoch.duration(n + 1);
            // A random fraction of the next step, in 64ths so the offset stays exact.
            let frac = Rational::new(rng.below(64) as u128, 64);
            let offset = lo.add(hi.sub(lo).mul(frac));
            assert_eq!(epoch.steps_elapsed(offset), n, "case {i} n {n}");
        }
        // Offsets exactly on a boundary, and the engine's integer-nanosecond clock.
        let epoch = REF.compile().epoch(&state(64, 262_144, 4_000, NO_SPEC));
        for n in [0, 1, 2, 10, 1_000, 50_000] {
            assert_eq!(epoch.steps_elapsed(epoch.duration(n)), n);
            let ns = epoch.duration(n).to_nanos_ceil();
            let k = epoch.steps_elapsed_ns(ns);
            let at = Rational::from_nanos(ns);
            assert!(epoch.duration(k) <= at && at < epoch.duration(k + 1), "n {n} ns {ns} k {k}");
        }
        assert_eq!(epoch.steps_elapsed(Rational::ZERO), 0);
        // Free steps saturate rather than loop.
        let free = Epoch::from_lines(1, 0, 0, 0, 0);
        assert_eq!(free.steps_elapsed(Rational::int(1)), 1 << 40);
    }

    #[test]
    fn batch_one_decode_within_5pct_of_10_25_ms() {
        let epoch = REF.compile().epoch(&state(1, 2_000, 2_000, NO_SPEC));
        let ms = epoch.step_time(0).to_f64() * 1e3;
        println!("batch-1 decode step {ms:.3} ms against 10.25 ms measured");
        assert!((ms - 10.25).abs() / 10.25 < 0.05, "{ms} ms");
    }

    #[test]
    fn batch_256_throughput_within_15pct_of_8773_tok_s() {
        // TensorRT-LLM's 8,773 output tok/s is at 2048 in / 2048 out and includes prefill, which
        // contends for the same device: device-seconds per request are an exclusive prefill plus
        // the request's share of the decode steps it lives through.
        let model = REF.compile();
        let (p, o, b) = (2_048u64, 2_048u64, 256u64);
        let ctx = p + o / 2;
        let epoch = model.epoch(&state(b, b * ctx, ctx, NO_SPEC));
        let step = epoch.step_time(0).to_f64();
        let prefill = model.prefill_tokens_per_s().to_f64();
        let per_request = p as f64 / prefill + o as f64 * step / b as f64;
        let tok_s = o as f64 / per_request;
        println!(
            "batch-256 2048/2048: {tok_s:.0} tok/s with prefill ({:.0} decode-only) against 8,773",
            b as f64 / step
        );
        assert!((tok_s - 8_773.0).abs() / 8_773.0 < 0.15, "{tok_s} tok/s");
    }

    #[test]
    fn speculation_erodes_at_large_batch() {
        // Speedup is real tokens per second with speculation over without. Verification multiplies
        // the compute line by N and leaves the bandwidth line alone, so once compute leads the
        // gain shrinks, with no rule about batch size anywhere.
        let model = REF.compile();
        let spec = SPEC_PROFILES[2];
        let speedup = |batch: u64| {
            let plain = model.epoch(&EpochState::uniform(batch, batch * 2_000, NO_SPEC));
            let fast = model.epoch(&EpochState::uniform(batch, batch * 2_000, spec));
            spec.accepted.mul(plain.step_time(0)).div(fast.step_time(0))
        };
        let (small, large) = (speedup(8), speedup(256));
        println!("N=5 M=3 speedup: {:.2}x at batch 8, {:.2}x at batch 256", small.to_f64(), large.to_f64());
        assert!(small > Rational::int(2), "{}", small.to_f64());
        assert!(large < small);
        assert!(large < Rational::new(3, 2), "{}", large.to_f64());
    }

    #[test]
    fn reference_denominator_stays_within_documented_bound() {
        let model = REF.compile();
        assert!(model.hardware_denominator() < 1u128 << 66, "{}", model.hardware_denominator());
        let mut rng = Rng::from_seed(7);
        for _ in 0..1_000 {
            let s = EpochState::random(&mut rng);
            let uniform = EpochState::uniform(s.batch, s.resident_tokens, s.spec);
            for e in [model.epoch(&s), model.epoch(&uniform)] {
                assert!(e.denominator() < 1u128 << 76, "{}", e.denominator());
            }
        }
    }
}
