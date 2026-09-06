#!/usr/bin/env python3
"""Reference cost model, and proof that the closed-form epoch advance is exact.

This is the naive per-batch implementation Issao asked for: a deliberately obvious
step-by-step simulator, used as a differential-testing oracle against the fast closed-form
advance. The Rust engine will carry the same pair, and the same test.

Checks, all in exact terms where possible:

  P1  T(n), the duration of n steps, equals the step-by-step sum.
  P2  The inverse solve is correct: for a mid-epoch event at offset d, the returned step
      count n satisfies T(n) <= d < T(n+1).
  P3  A whole replica run with sequences completing at different times produces identical
      completion times either way, in far fewer operations.
  P4  The same holds with speculative decoding, and with the compute-bound branch active.

Cost model, per Issao's correction: a step costs the **worse of** memory-bandwidth time and
compute time, not bandwidth alone. Both are linear in the step index within an epoch, so the
step-time function is a maximum of two lines: piecewise linear with at most one crossover.
The closed form therefore survives, integrated per piece.

Run: python3 bench/validate_epochs.py
"""

from __future__ import annotations

import math
import random
from dataclasses import dataclass
from fractions import Fraction as F

# ---------------------------------------------------------------------------
# Reference hardware and model. See docs/llm-serving-primer.md section 10.7 and
# docs/calibration.md section 6 for provenance of every constant.
# ---------------------------------------------------------------------------

REF = dict(
    weight_bytes=140_000_000_000,      # Llama-3-70B, bf16
    n_params=70_000_000_000,
    n_layers=80,
    n_heads=64,
    n_kv_heads=8,
    head_dim=128,
    kv_bytes_per_token=327_680,        # 2 * 80 * 8 * 128 * 2
    tp=8,                              # 8x H100 SXM
    hbm_bw=3.35e12,
    peak_flops=990e12,                 # bf16 dense
    mbu=0.70,                          # large-batch asymptote; see note below
    mfu_decode=0.45,
    mfu_prefill=0.50,                  # dense model; MoE is 0.16-0.36
    # Fixed per-step cost: launch, sampling, scheduler, TP all-reduce latency.
    #
    # 2.75 ms is derived from NVIDIA NIM's measured 10.25 ms batch-1 step for
    # Llama-3.3-70B bf16 TP8 on 8xH100, against a 7.50 ms roofline. An earlier value of 8 ms
    # came from 2024-era vLLM, which spent most of a low-batch step in Python; it is kept
    # below as a second profile because a scenario may want to model that engine.
    #
    # Convention, which the primer previously got muddled: `mbu` is the large-batch asymptote
    # and `fixed_step_s` explains the shortfall at low batch. Together they reproduce an
    # *effective* batch-1 MBU of 0.51, which is the measured figure. Lowering `mbu` as well
    # would double-count the same overhead.
    fixed_step_s=0.00275,
)

LEGACY_ENGINE = dict(REF, fixed_step_s=0.008)   # vLLM <= 0.5.3 era


@dataclass(frozen=True)
class Spec:
    """Speculative decoding, modelled the way Issao specified.

    Propose `draft` tokens per step, accept `accepted` on average. Deterministic within an
    epoch, drawn per request at Ingress, so the closed form is untouched.

    The interesting consequence is emergent rather than coded: verification multiplies the
    compute term by `draft` while leaving the weight-read term alone, so once the compute
    branch of the max() dominates, the speedup erodes on its own. No rule about batch size is
    needed anywhere.
    """
    draft: int = 1        # N
    accepted: float = 1.0  # M, tokens of real progress per step

    @property
    def enabled(self) -> bool:
        return self.draft > 1


NO_SPEC = Spec()


def coeffs(batch, kv_tokens_resident, ctx_mean, hw=REF, spec=NO_SPEC, exact=False):
    """Return the two lines whose maximum is the step time.

    ((beta_bw, alpha_bw), (beta_cp, alpha_cp)), each t(k) = beta + alpha*k.

    Bandwidth: weights plus resident KV, read once per step regardless of how many draft
    tokens are verified. Resident KV grows by `batch * accepted` tokens per step.

    Compute: 2 FLOPs per parameter per verified token, so `batch * draft` tokens per step,
    plus the attention term which grows with context.
    """
    num = F if exact else float
    G = num(hw["tp"])
    if exact:
        bw = F(hw["hbm_bw"]) * F(hw["mbu"])
        fl = F(hw["peak_flops"]) * F(hw["mfu_decode"])
        fix = F(hw["fixed_step_s"]).limit_denominator(10**12)
        acc = F(spec.accepted).limit_denominator(10**6)
    else:
        bw = hw["hbm_bw"] * hw["mbu"]
        fl = hw["peak_flops"] * hw["mfu_decode"]
        fix = hw["fixed_step_s"]
        acc = spec.accepted

    W = num(hw["weight_bytes"])
    kv = num(hw["kv_bytes_per_token"])
    S0 = num(kv_tokens_resident)
    B = num(batch)
    draft = num(spec.draft)

    # -- bandwidth line ---------------------------------------------------
    beta_bw = (W + kv * S0) / (G * bw) + fix
    alpha_bw = kv * B * acc / (G * bw)

    # -- compute line -----------------------------------------------------
    # Dense GEMM: 2 * params * verified_tokens.
    gemm = 2 * num(hw["n_params"]) * B * draft
    # Attention: for each verified token, attend over the sequence. 2 matmuls, so
    # 4 * layers * heads * head_dim FLOPs per key position.
    attn_per_key = 4 * num(hw["n_layers"]) * num(hw["n_heads"]) * num(hw["head_dim"])
    # Total keys attended this step, using mean context. Grows by B*accepted per step.
    keys0 = num(ctx_mean) * B
    beta_cp = (gemm + attn_per_key * keys0 * draft) / (G * fl) + fix
    alpha_cp = (attn_per_key * B * acc * draft) / (G * fl)

    return (beta_bw, alpha_bw), (beta_cp, alpha_cp)


def t_step(k, lines):
    (b1, a1), (b2, a2) = lines
    return max(b1 + a1 * k, b2 + a2 * k)


def duration_closed(n, lines):
    """Exact sum of max(line1, line2) over k in [0, n).

    Two lines cross at most once, so split the range at the crossover and sum each piece as
    an arithmetic series. Still O(1).
    """
    if n <= 0:
        return type(lines[0][0])(0)
    (b1, a1), (b2, a2) = lines

    def series(beta, alpha, lo, hi):
        # Sum of beta + alpha*k for k in [lo, hi).
        #
        # `m * (m - 1) // 2` must be integer division. True division here produced a float,
        # which contaminated the exact-rational path and silently broke the proof: P1 failed
        # while the float-based checks all passed, because a float answer is still correct to
        # 1e-16 and only exact arithmetic notices.
        m = hi - lo
        if m <= 0:
            return beta * 0
        return m * beta + alpha * (lo * m + m * (m - 1) // 2)

    if a1 == a2:
        # Parallel: whichever is higher stays higher for the whole range.
        return series(b1, a1, 0, n) if b1 >= b2 else series(b2, a2, 0, n)

    # Crossover where b1 + a1*k == b2 + a2*k
    k_cross = (b2 - b1) / (a1 - a2)
    # Which line leads at k = 0
    first, second = ((b1, a1), (b2, a2)) if b1 >= b2 else ((b2, a2), (b1, a1))
    if k_cross <= 0 or k_cross >= n:
        # No crossover inside the range: the leader at k=0 leads throughout.
        return series(first[0], first[1], 0, n)
    split = math.floor(k_cross) + 1 if not isinstance(k_cross, F) else int(k_cross) + 1
    split = max(0, min(n, split))
    return series(first[0], first[1], 0, split) + series(second[0], second[1], split, n)


def duration_stepwise(n, lines):
    total = type(lines[0][0])(0)
    for k in range(n):
        total += t_step(k, lines)
    return total


def steps_within(d, lines):
    """Largest n with T(n) <= d. Bisection, because the max() makes T piecewise quadratic."""
    if d <= 0:
        return 0
    lo, hi = 0, 1
    while duration_closed(hi, lines) <= d:
        lo, hi = hi, hi * 2
        if hi > 1 << 40:
            break
    while lo + 1 < hi:
        mid = (lo + hi) // 2
        if duration_closed(mid, lines) <= d:
            lo = mid
        else:
            hi = mid
    return lo


# ---------------------------------------------------------------------------
# P1, P1b, P2
# ---------------------------------------------------------------------------

def check_p1() -> bool:
    print("P1  closed form vs step-by-step sum, exact rational arithmetic")
    ok = True
    cases = [
        (1, 2_000, 2_000, 1, NO_SPEC), (1, 2_000, 2_000, 500, NO_SPEC),
        (16, 64_000, 4_000, 1_000, NO_SPEC), (64, 262_144, 4_000, 4_000, NO_SPEC),
        (256, 1_048_576, 4_000, 200, NO_SPEC), (128, 500_000, 4_000, 30_000, NO_SPEC),
        # Short context and large batch: the compute branch leads.
        (512, 65_536, 128, 2_000, NO_SPEC),
        # Speculative decoding, where verification inflates compute 5x.
        (256, 1_048_576, 4_000, 500, Spec(5, 3.0)),
        (64, 8_192, 128, 1_000, Spec(5, 3.0)),
    ]
    for batch, s0, ctx, n, spec in cases:
        lines = coeffs(batch, s0, ctx, spec=spec, exact=True)
        a = duration_closed(n, lines)
        b = duration_stepwise(n, lines)
        same = a == b
        ok &= same
        lead = "compute" if lines[1][0] > lines[0][0] else "bandwidth"
        print(f"    batch={batch:<4} ctx={ctx:<6} n={n:<6} draft={spec.draft} "
              f"leads@0={lead:<9} exact match: {same}")
    return ok


def check_p1_float_accuracy() -> None:
    print("P1b float error against exact, closed form vs naive summation")
    le = coeffs(128, 500_000, 4_000, exact=True)
    lf = coeffs(128, 500_000, 4_000, exact=False)
    for n in (1_000, 100_000, 1_000_000):
        truth = float(duration_closed(n, le))
        closed = duration_closed(n, lf)
        stepwise = duration_stepwise(n, lf)
        print(f"    n={n:<9} closed rel err {abs(closed-truth)/truth:.3e}   "
              f"summed rel err {abs(stepwise-truth)/truth:.3e}")


def check_p2(trials: int = 20_000, seed: int = 12345) -> bool:
    print(f"P2  inverse solve, {trials} randomised trials including spec decoding")
    rng = random.Random(seed)
    ok = True
    for _ in range(trials):
        batch = rng.randint(0, 512)
        s0 = rng.randint(0, 1_400_000)
        ctx = rng.choice([128, 1_000, 4_000, 32_000])
        spec = rng.choice([NO_SPEC, Spec(3, 2.0), Spec(5, 3.0), Spec(8, 4.5)])
        lines = coeffs(batch, s0, ctx, spec=spec)
        n_true = rng.randint(0, 5_000)
        lo = duration_closed(n_true, lines)
        hi = duration_closed(n_true + 1, lines)
        d = lo + (hi - lo) * rng.random()
        if steps_within(d, lines) != n_true:
            ok = False
    print(f"    all trials correct: {ok}")
    lines = coeffs(64, 262_144, 4_000)
    edges = all(steps_within(duration_closed(n, lines), lines) == n
                for n in (0, 1, 2, 10, 1_000, 50_000))
    print(f"    exact-boundary offsets correct: {edges}")
    return ok and edges


# ---------------------------------------------------------------------------
# P3, P4: whole replica runs
# ---------------------------------------------------------------------------

@dataclass
class Seq:
    sid: int
    held: int
    remaining: int
    spec: Spec = NO_SPEC


def _ctx_mean(live):
    return sum(s.held for s in live) / len(live) if live else 0.0


def _epoch_spec(live):
    """One spec profile per epoch. Real engines run one speculation config per step, so the
    batch's effective profile is the mean over its sequences."""
    if not live:
        return NO_SPEC
    draft = max(s.spec.draft for s in live)
    acc = sum(s.spec.accepted for s in live) / len(live)
    return Spec(draft, acc)


def run_stepwise(seqs, hw=REF):
    """The naive oracle: one iteration per step, no algebra."""
    live = [Seq(s.sid, s.held, s.remaining, s.spec) for s in seqs]
    resident = sum(s.held for s in live)
    t, ops = 0.0, 0
    done = {}
    while live:
        spec = _epoch_spec(live)
        lines = coeffs(len(live), resident, _ctx_mean(live), hw, spec)
        t += t_step(0, lines)
        ops += 1
        step_tokens = spec.accepted
        for s in live:
            grown = min(s.remaining, step_tokens)
            s.held += grown
            s.remaining -= grown
        resident = sum(s.held for s in live)
        finished = [s for s in live if s.remaining <= 1e-9]
        for s in finished:
            done[s.sid] = t
            resident -= s.held
        if finished:
            live = [s for s in live if s.remaining > 1e-9]
    return done, ops


def run_epochs(seqs, hw=REF):
    """The fast path: one iteration per composition change."""
    live = [Seq(s.sid, s.held, s.remaining, s.spec) for s in seqs]
    resident = sum(s.held for s in live)
    t, ops = 0.0, 0
    done = {}
    while live:
        spec = _epoch_spec(live)
        step_tokens = spec.accepted
        # Steps until the first sequence finishes.
        n = min(math.ceil(s.remaining / step_tokens) for s in live)
        lines = coeffs(len(live), resident, _ctx_mean(live), hw, spec)
        t += duration_closed(n, lines)
        ops += 1
        for s in live:
            grown = min(s.remaining, step_tokens * n)
            s.held += grown
            s.remaining -= grown
        resident = sum(s.held for s in live)
        finished = [s for s in live if s.remaining <= 1e-9]
        for s in finished:
            done[s.sid] = t
            resident -= s.held
        live = [s for s in live if s.remaining > 1e-9]
    return done, ops


def _compare(label, make_seqs, cases=8, seed=999):
    rng = random.Random(seed)
    ok = True
    tot_a = tot_b = 0
    for case in range(cases):
        seqs = make_seqs(rng)
        a, ops_a = run_stepwise(seqs)
        b, ops_b = run_epochs(seqs)
        tot_a += ops_a
        tot_b += ops_b
        assert a.keys() == b.keys()
        worst = max(abs(a[k] - b[k]) / max(a[k], 1e-12) for k in a)
        good = worst < 1e-9
        ok &= good
        print(f"    case {case}: {len(seqs):>3} seqs  steps={ops_a:<6} epochs={ops_b:<4} "
              f"speedup={ops_a/ops_b:>6.1f}x  worst rel diff={worst:.2e}  ok={good}")
    print(f"    totals: {tot_a} step iterations vs {tot_b} epoch iterations = "
          f"{tot_a/tot_b:.1f}x fewer")
    return ok


def check_p3():
    print("P3  whole replica run, no speculation")
    return _compare("plain", lambda rng: [
        Seq(i, rng.randint(200, 8_000), rng.randint(1, 3_000))
        for i in range(rng.randint(2, 64))
    ])


def check_p4():
    print("P4  whole replica run with speculative decoding, per-request profiles")
    def make(rng):
        n = rng.randint(2, 48)
        prof = rng.choice([Spec(3, 2.0), Spec(5, 3.0), Spec(8, 4.5)])
        return [Seq(i, rng.randint(200, 8_000), rng.randint(1, 3_000), prof)
                for i in range(n)]
    return _compare("spec", make, seed=4242)


# ---------------------------------------------------------------------------
# Calibration anchors and the throughput question Issao asked
# ---------------------------------------------------------------------------

def check_anchors() -> None:
    print("Anchors  measured values from docs/calibration.md section 6")
    lines = coeffs(1, 2_000, 2_000)
    ms = t_step(0, lines) * 1e3
    print(f"    NIM Llama-3.3-70B bf16 TP8 batch 1: model {ms:.2f} ms / {1e3/ms:.0f} tok/s"
          f"   measured 10.25 ms / 97 tok/s")
    eff = (REF["weight_bytes"] / REF["tp"]) / (ms * 1e-3 * REF["hbm_bw"])
    print(f"    implied effective MBU at batch 1: {eff:.2f}   measured ~0.51")


def prefill_tokens_per_s(hw=REF) -> float:
    return hw["mfu_prefill"] * hw["peak_flops"] * hw["tp"] / (2 * hw["n_params"])


def check_throughput() -> None:
    """Issao's question: is 9,000 output tokens/s decode-only at full batch? Yes, and that
    makes it the wrong number for a fleet budget, because prefill competes for the same GPU.
    """
    print("Throughput  decode-only vs prefill-inclusive, per replica")
    pref = prefill_tokens_per_s()
    print(f"    prefill capacity: {pref:,.0f} tok/s at MFU {REF['mfu_prefill']} (dense)")
    print()
    print(f"    {'prompt':>7} {'output':>7} {'batch':>6} {'decode-only':>12} "
          f"{'with prefill':>13} {'prefill share':>14}")
    for p, o, b in [(2_000, 500, 256), (2_048, 2_048, 256), (1_000, 1_000, 256),
                    (8_000, 300, 256), (32_000, 500, 128), (500, 2_000, 256)]:
        lines = coeffs(b, b * (p + o / 2), p + o / 2)
        step = t_step(0, lines)
        decode_only = b / step
        # GPU-seconds per request: exclusive prefill, plus this request's share of decode
        # steps. Additive because they contend for the same device.
        t_prefill = p / pref
        t_decode = o * step / b
        per_req = t_prefill + t_decode
        with_prefill = o / per_req
        print(f"    {p:>7,} {o:>7,} {b:>6} {decode_only:>12,.0f} {with_prefill:>13,.0f} "
              f"{t_prefill/per_req:>13.0%}")


def main() -> int:
    print("=" * 82)
    print("Reference cost model and closed-form validation")
    print("=" * 82)
    results = [("P1", check_p1())]
    print()
    check_p1_float_accuracy()
    print()
    results.append(("P2", check_p2()))
    print()
    results.append(("P3", check_p3()))
    print()
    results.append(("P4", check_p4()))
    print()
    check_anchors()
    print()
    check_throughput()
    print()
    print("=" * 82)
    for name, ok in results:
        print(f"{name}: {'PASS' if ok else 'FAIL'}")
    every = all(ok for _, ok in results)
    print("VERDICT:", "closed form exact under max(bandwidth, compute) and speculation"
          if every else "MISMATCH - the architecture needs revision")
    print("=" * 82)
    return 0 if every else 1


if __name__ == "__main__":
    raise SystemExit(main())
