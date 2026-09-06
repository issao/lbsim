#!/usr/bin/env python3
"""Verify the closed-form epoch advance against a brute-force per-step simulation.

This checks the load-bearing claim of docs/ARCHITECTURE.md section 3: that a replica can be
advanced by solving a closed form instead of iterating every decode step, with *no* loss of
fidelity. If the two disagree on completion times, the architecture is wrong and the scale
target is unreachable.

Three properties are checked:

  P1  T(n), the duration of n steps, equals the step-by-step sum. Checked in exact rational
      arithmetic, so a match is a proof for those inputs rather than a float coincidence.
  P2  The inverse solve is correct: for a mid-epoch event at offset d, the returned step
      count n satisfies T(n) <= d < T(n+1).
  P3  A whole replica run with sequences completing at different times produces identical
      completion times either way, and does so in far fewer operations.

Deliberately verification, not implementation: this validates the design so it can be
blessed, and shares no code with the eventual Rust engine.

Run: python3 bench/validate_epochs.py
"""

from __future__ import annotations

import math
import random
from dataclasses import dataclass
from fractions import Fraction as F

# ---------------------------------------------------------------------------
# Reference hardware, from docs/llm-serving-primer.md section 10.7
# ---------------------------------------------------------------------------

REF = dict(
    weight_bytes=140_000_000_000,      # Llama-3-70B, bf16
    kv_bytes_per_token=327_680,        # 2 * 80 layers * 8 kv heads * 128 head_dim * 2 bytes
    tp=8,                              # 8x H100
    hbm_bw=3.35e12,
    mbu=0.70,
    fixed_step_s=0.008,                # calibration anchor; see primer 10.4
)


def coeffs(batch: int, kv_tokens_resident: int, hw=REF, exact: bool = False):
    """Return (beta, alpha): first-step duration, and growth per step.

    t_step(k) = beta + alpha * k, because resident KV grows by exactly `batch` tokens per
    step and step time is linear in bytes read.
    """
    num = F if exact else float
    W = num(hw["weight_bytes"])
    kv = num(hw["kv_bytes_per_token"])
    G = num(hw["tp"])
    if exact:
        denom = F(hw["hbm_bw"]) * F(hw["mbu"])
        fix = F(hw["fixed_step_s"]).limit_denominator(10**9)
    else:
        denom = hw["hbm_bw"] * hw["mbu"]
        fix = hw["fixed_step_s"]
    beta = (W + kv * num(kv_tokens_resident)) / (G * denom) + fix
    alpha = kv * num(batch) / (G * denom)
    return beta, alpha


def duration_closed(n: int, beta, alpha):
    """T(n) = n*beta + alpha*n*(n-1)/2. Sum of an arithmetic series."""
    if n <= 0:
        return beta * 0
    return n * beta + alpha * n * (n - 1) / 2


def duration_stepwise(n: int, beta, alpha):
    """Ground truth: add up each step."""
    total = beta * 0
    for k in range(n):
        total += beta + alpha * k
    return total


def steps_within(d: float, beta: float, alpha: float) -> int:
    """Largest n with T(n) <= d. Inverts the quadratic.

    (alpha/2) n^2 + (beta - alpha/2) n - d = 0
    """
    if d <= 0:
        return 0
    if alpha == 0.0:
        return int(math.floor(d / beta))
    b = beta - alpha / 2.0
    disc = b * b + 2.0 * alpha * d
    n = (-b + math.sqrt(disc)) / alpha
    n = int(math.floor(n))
    # Float guard: nudge into the correct bucket rather than trusting the root exactly.
    while n > 0 and duration_closed(n, beta, alpha) > d:
        n -= 1
    while duration_closed(n + 1, beta, alpha) <= d:
        n += 1
    return n


# ---------------------------------------------------------------------------
# P1: the closed form equals the step-by-step sum, exactly
# ---------------------------------------------------------------------------

def check_p1() -> bool:
    print("P1  closed form vs step-by-step sum, exact rational arithmetic")
    ok = True
    for batch, s0, n in [
        (1, 2_000, 1), (1, 2_000, 500), (16, 64_000, 1_000),
        (64, 262_144, 4_000), (256, 1_048_576, 200), (128, 500_000, 30_000),
    ]:
        beta, alpha = coeffs(batch, s0, exact=True)
        a = duration_closed(n, beta, alpha)
        b = duration_stepwise(n, beta, alpha)
        same = a == b
        ok &= same
        print(f"    batch={batch:<4} S0={s0:<9} n={n:<6} exact match: {same}")
    return ok


def check_p1_float_accuracy() -> None:
    """The closed form is not just faster, it is more accurate.

    Naive summation accumulates rounding over n additions; the closed form does not. Worth
    recording because it removes a reason someone might prefer the loop.
    """
    print("P1b float error against exact, closed form vs naive summation")
    beta_e, alpha_e = coeffs(128, 500_000, exact=True)
    beta_f, alpha_f = coeffs(128, 500_000, exact=False)
    for n in (1_000, 100_000, 1_000_000):
        truth = float(duration_closed(n, beta_e, alpha_e))
        closed = duration_closed(n, beta_f, alpha_f)
        stepwise = duration_stepwise(n, beta_f, alpha_f)
        e_closed = abs(closed - truth) / truth
        e_step = abs(stepwise - truth) / truth
        print(f"    n={n:<9} closed rel err {e_closed:.3e}   summed rel err {e_step:.3e}")


# ---------------------------------------------------------------------------
# P2: the inverse solve lands in the right bucket
# ---------------------------------------------------------------------------

def check_p2(trials: int = 20_000, seed: int = 12345) -> bool:
    print(f"P2  inverse solve, {trials} randomised trials")
    rng = random.Random(seed)
    ok = True
    worst = None
    for _ in range(trials):
        batch = rng.randint(0, 512)
        s0 = rng.randint(0, 1_400_000)
        beta, alpha = coeffs(batch, s0)
        n_true = rng.randint(0, 20_000)
        # Pick an offset strictly inside step n_true, so the answer must be exactly n_true.
        lo = duration_closed(n_true, beta, alpha)
        hi = duration_closed(n_true + 1, beta, alpha)
        d = lo + (hi - lo) * rng.random()
        got = steps_within(d, beta, alpha)
        if got != n_true:
            ok = False
            worst = (batch, s0, n_true, got, d)
    print(f"    all trials correct: {ok}" + (f"  worst: {worst}" if worst else ""))
    # Boundaries: exactly at T(n) must return n, not n-1.
    beta, alpha = coeffs(64, 262_144)
    edges = all(steps_within(duration_closed(n, beta, alpha), beta, alpha) == n
                for n in (0, 1, 2, 10, 1_000, 50_000))
    print(f"    exact-boundary offsets correct: {edges}")
    return ok and edges


# ---------------------------------------------------------------------------
# P3: a whole replica run, both ways
# ---------------------------------------------------------------------------

@dataclass
class Seq:
    sid: int
    held: int        # tokens of KV currently resident: prompt + generated
    remaining: int   # output tokens still to generate


def run_stepwise(seqs: list[Seq], hw=REF) -> tuple[dict[int, float], int]:
    """Ground truth: one iteration per decode step."""
    live = [Seq(s.sid, s.held, s.remaining) for s in seqs]
    resident = sum(s.held for s in live)
    t = 0.0
    done: dict[int, float] = {}
    ops = 0
    while live:
        beta, alpha = coeffs(len(live), resident, hw)
        t += beta                      # one step: k=0 relative to current resident
        ops += 1
        for s in live:
            s.held += 1
            s.remaining -= 1
        resident += len(live)
        finished = [s for s in live if s.remaining == 0]
        for s in finished:
            done[s.sid] = t
            resident -= s.held
        if finished:
            live = [s for s in live if s.remaining > 0]
    return done, ops


def run_epochs(seqs: list[Seq], hw=REF) -> tuple[dict[int, float], int]:
    """Closed form: one iteration per composition change."""
    live = [Seq(s.sid, s.held, s.remaining) for s in seqs]
    resident = sum(s.held for s in live)
    t = 0.0
    done: dict[int, float] = {}
    ops = 0
    while live:
        n = min(s.remaining for s in live)
        beta, alpha = coeffs(len(live), resident, hw)
        t += duration_closed(n, beta, alpha)
        ops += 1
        for s in live:
            s.held += n
            s.remaining -= n
        resident += len(live) * n
        finished = [s for s in live if s.remaining == 0]
        for s in finished:
            done[s.sid] = t
            resident -= s.held
        live = [s for s in live if s.remaining > 0]
    return done, ops


def check_p3(seed: int = 999) -> bool:
    print("P3  whole replica run: per-step vs epochs")
    rng = random.Random(seed)
    ok = True
    total_step_ops = total_epoch_ops = 0
    for case in range(8):
        n_seq = rng.randint(2, 64)
        seqs = [
            Seq(i,
                held=rng.randint(200, 8_000),
                remaining=rng.randint(1, 3_000))
            for i in range(n_seq)
        ]
        a, ops_a = run_stepwise(seqs)
        b, ops_b = run_epochs(seqs)
        total_step_ops += ops_a
        total_epoch_ops += ops_b
        assert a.keys() == b.keys()
        # Both accumulate float error differently, so compare on relative tolerance rather
        # than bit equality. Anything above 1e-12 would mean a real algebraic mismatch.
        worst = max(abs(a[k] - b[k]) / max(a[k], 1e-12) for k in a)
        good = worst < 1e-12
        ok &= good
        print(f"    case {case}: {n_seq:>3} seqs  steps={ops_a:<6} epochs={ops_b:<4} "
              f"speedup={ops_a / ops_b:>6.1f}x  worst rel diff={worst:.2e}  ok={good}")
    print(f"    totals: {total_step_ops} step iterations vs {total_epoch_ops} epoch "
          f"iterations = {total_step_ops / total_epoch_ops:.1f}x fewer")
    return ok


# ---------------------------------------------------------------------------
# Sanity: do the primer's published numbers come out?
# ---------------------------------------------------------------------------

def check_primer_numbers() -> None:
    print("Sanity  reproduce docs/llm-serving-primer.md section 10.4 table")
    print("    batch  ctx     roofline step   published   per-replica out tok/s")
    for batch, ctx, published in [
        (1, 2_000, "7.5 ms"), (16, 4_000, "8.5 ms"), (64, 4_000, "11.9 ms"),
        (128, 4_000, "16.3 ms"), (256, 4_000, "25.3 ms"), (64, 32_000, "43.1 ms"),
    ]:
        beta, _ = coeffs(batch, batch * ctx)
        # The table's "roofline step" excludes the fixed overhead, which it lists separately.
        roofline = beta - REF["fixed_step_s"]
        tok_s = batch / beta
        print(f"    {batch:<6} {ctx:<7} {roofline * 1e3:>8.1f} ms      {published:<11} "
              f"{tok_s:>10,.0f}")


def main() -> int:
    print("=" * 78)
    print("Validating docs/ARCHITECTURE.md section 3: analytic epoch advancement")
    print("=" * 78)
    results = []
    results.append(("P1", check_p1()))
    print()
    check_p1_float_accuracy()
    print()
    results.append(("P2", check_p2()))
    print()
    results.append(("P3", check_p3()))
    print()
    check_primer_numbers()
    print()
    print("=" * 78)
    for name, ok in results:
        print(f"{name}: {'PASS' if ok else 'FAIL'}")
    every = all(ok for _, ok in results)
    print("VERDICT:", "closed form is exact; the architecture's core claim holds"
          if every else "MISMATCH — the architecture needs revision")
    print("=" * 78)
    return 0 if every else 1


if __name__ == "__main__":
    raise SystemExit(main())
