# The policy arena: the mechanical half, built

`docs/arena.md` is the specification and stays the authority. This records what of it exists in code,
the numbers the code actually measured, and what the judging half of the referee still has to do.

Everything is in `crates/sim-arena/src/lib.rs` since the workspace split (163995c), plus the frozen suite in `scenarios/holdout/`. Zero dependencies,
single-threaded, same seed for every policy. Not yet wired into the CLI: `arena::arena_main` is the
entry point a `sim-run arena` arm would call, and wiring it is one line in `report::cli`.

Reproduce with `tools/build.sh test --release -p sim-arena -- --nocapture arena_round` (5 s, 41 runs per round).

---

## 1. What is implemented

| `docs/arena.md` | In code |
|---|---|
| §1 SLA gate, worst-case scoring | `score_run`, `score_policy`, `arena_attainment` |
| §2.2 realism envelope, every load parameter | `envelope` (21 bounds, each citing `docs/calibration.md`), `check_realism` → violations with values |
| §3 rated-capacity honesty | `measure_honest_capacity` → `HonestCapacity`, and the free-shedding exemption in `OfferedLoad::free_shed_fraction` |
| §4 a round, same seeds for every policy | `run_round` → payoff matrix, ranking, load difficulty |
| §5 fixed held-out suite | `scenarios/holdout/h1..h8`, `HOLDOUT` |
| determinism replay on a sample | `replay_is_deterministic`, and one replayed pair per round |

Two decisions worth flagging because they change results rather than only shape code.

**The arena computes its own SLO attainment.** `RunResult::slo_attainment` divides by *successful*
requests, so a rejection or a client timeout leaves the denominator instead of failing it. Under that
definition a policy that shed nine requests in ten would report perfect attainment — exactly the trade
§1's gate exists to forbid. `arena_attainment` counts every measured request. On h8 the two differ by
3.3 points (0.9939 engine, 0.9609 arena) and on h3 by 1 point the other way.

**Free shedding above rated capacity is an integral, not a flag.** §3 allows spilling traffic offered
above the declared capacity without SLO cost. Mechanically that is the time-average of
`max(0, rate(t) − rated)` over the measured window, divided by the mean offered rate, removed from the
attainment denominator. One rule covers a permanently over-rated load (h3: 0.287 of its traffic
exempt) and a bursty one (h5: nothing exempt against the analytic 328 rps, because its 150 rps peak
stays below it — and a share that grows continuously as a policy declares below that peak).

---

## 2. The held-out suite

Eight scenarios, frozen, ordered by filename. Every workload number is cited to a section of
`docs/calibration.md` in the file itself. All eight share one fleet block, so the only thing that
varies is the load. The fleet is 32 replicas, batch cap 256, KV budget 1.37 M tokens per replica.

| File | Regime | Offered | vs analytic rated | Why it is in the suite |
|---|---|---|---|---|
| `h1-light-chat` | light load | 33 rps | 0.10× | The control. Nothing contended; sets the goodput scale. |
| `h2-near-rated-chat` | the knee | 148 rps | 0.45× | The measured SLO capacity of the best policy. Where routing is worth something. |
| `h3-over-rated-chat` | sustained overload | 460 rps | 1.40× | Tests §3: shed early and cheaply, or fail late after burning device time. |
| `h4-long-prompt-mixture` | long-prompt mixture | 10 rps | 0.30× | 22 k mean context, so the KV budget caps the batch at 62. Where counting requests is the wrong unit. |
| `h5-bursty-step` | step change | 50 → 150 → 50 rps | 0.24× mean | Recovery, and §3's "load generator is free to change the load after warm-up": the burst starts 30 s after warm-up ends. |
| `h6-low-rate-heavy-context` | low rate, heavy context | 6.67 rps | 0.22× | Mooncake's measured fleet rate with 23 k mean context. A request-counting policy sees an idle fleet. |
| `h7-code-completion` | prefill-bound extreme | 159 rps | 0.45× | 111:1 in:out, output CV 3.30. The only mixture that can reach a 99.9 % cap. |
| `h8-retry-storm` | retry feedback | 33 rps | 0.10× | Same mixture *and rate* as h1, different client: 10 s deadline, 3 attempts. Isolates client behaviour. |

The rates were chosen by measurement, not by taste: `suite_anchoring::attainment_vs_rate` (a 360-run
`#[ignore]`d sweep) prints attainment against offered rate for all five policies on all eight mixtures,
and the light / knee / overload rates come off those curves. Re-runnable if the engine is recalibrated.

Five of the phase-1 dynamics `docs/arena.md` §5 names **cannot** be in the suite yet, because the
engine has no mechanism for them: the rolling hotspot (no heterogeneity or per-replica degradation),
the preemption cascade (nothing preempts), prefix-affinity tension (no prefix cache), a diurnal
multi-region run (one region, and load shapes are a single step rather than a diurnal curve), and
autoscaling. The retry storm and the long-context head-of-line dynamics are in. When those mechanisms
land, `docs/arena.md` §5's "never added to" means they need a *second* frozen suite, versioned, rather
than edits to this one.

---

## 3. The measured round

Five policies × eight loads, 41 runs, seed 20260906 for every policy, engine at commit `283265c`;
re-run at `907f3a1` under rule set v2 (f6a87a9) with every absolute number identical. Determinism
replay: identical fingerprint. Realism envelope: all eight loads inside every bound.

**Payoff matrix under rule set v2 — goodput as a share of offered output tokens.** This is the objective
since f6a87a9, per Issao at 16:12 on §5b's first flaw (*"You can remove this, I agreed with this."*),
before the gate. The rule set is recorded in every score: *"v2: cap 0.95 default, min over in-scope
loads of gated goodput share"*.

| load | round_robin | random | least_requests | least_queue_tokens | p2c |
|---|---|---|---|---|---|
| h1-light-chat | 0.932 | 0.933 | 0.661 | 0.661 | **0.934** |
| h2-near-rated-chat | 0.874 | 0.855 | 0.148 | 0.173 | **0.886** |
| h3-over-rated-chat | 0.070 | **0.072** | 0.010 | 0.010 | 0.057 |
| h4-long-prompt-mixture | 0.732 | 0.705 | 0.621 | 0.621 | **0.762** |
| h5-bursty-step | 0.923 | 0.911 | 0.265 | 0.273 | **0.930** |
| h6-low-rate-heavy-context | 0.874 | 0.856 | 0.786 | 0.786 | **0.880** |
| h7-code-completion | **0.923** | 0.893 | 0.096 | 0.096 | 0.891 |
| h8-retry-storm | **0.822** | 0.813 | 0.473 | 0.473 | 0.819 |

**Absolute goodput, output tokens/s within SLO.** The v1 objective, kept as the diagnostic beside the
share; the two swapped roles and neither was dropped.

| load | round_robin | random | least_requests | least_queue_tokens | p2c |
|---|---|---|---|---|---|
| h1-light-chat | 3828 | 3834 | 2716 | 2716 | **3837** |
| h2-near-rated-chat | 16099 | 15758 | 2728 | 3191 | **16321** |
| h3-over-rated-chat | 4023 | **4144** | 547 | 549 | 3269 |
| h4-long-prompt-mixture | 2642 | 2546 | 2241 | 2241 | **2752** |
| h5-bursty-step | 9029 | 8913 | 2593 | 2675 | **9100** |
| h6-low-rate-heavy-context | 2632 | 2579 | 2369 | 2369 | **2652** |
| h7-code-completion | **3331** | 3223 | 347 | 347 | 3215 |
| h8-retry-storm | **3377** | 3340 | 1945 | 1945 | 3366 |

**Arena SLO attainment**, all requests, sheds and timeouts included:

| load | round_robin | random | least_requests | least_queue_tokens | p2c |
|---|---|---|---|---|---|
| h1-light-chat | 0.9954 | 0.9957 | 0.7106 | 0.7106 | **0.9960** |
| h2-near-rated-chat | 0.9688 | 0.9650 | 0.1949 | 0.2210 | **0.9857** |
| h3-over-rated-chat | **0.1962** | 0.1934 | 0.0281 | 0.0262 | 0.1502 |
| h4-long-prompt-mixture | 0.9739 | 0.9582 | 0.8606 | 0.8615 | **0.9847** |
| h5-bursty-step | 0.9793 | 0.9783 | 0.3065 | 0.3154 | **0.9904** |
| h6-low-rate-heavy-context | 0.9814 | 0.9725 | 0.9107 | 0.9107 | **0.9886** |
| h7-code-completion | **0.9962** | 0.9705 | 0.1073 | 0.1073 | 0.9637 |
| h8-retry-storm | 0.9609 | 0.9582 | 0.5964 | 0.5964 | **0.9617** |

**Ranking under rule set v2.** The score is the minimum over in-scope loads of the gated goodput share.
h3 is out of scope for every policy (460 rps offered against 328 rated), so seven of eight loads count.

| # | policy | score | mean share | mean tok/s | breaches | worst load |
|---|---|---|---|---|---|---|
| 1 | p2c | **0.762** | 0.872 | 5892 | 1 | h4-long-prompt-mixture |
| 2 | round_robin | **0.732** | 0.868 | 5848 | 1 | h4-long-prompt-mixture |
| 3 | random | **0.705** | 0.852 | 5742 | 1 | h4-long-prompt-mixture |
| 4 | least_queue_tokens | 0 | 0 | 2212 | 8 | h1-light-chat |
| 5 | least_requests | 0 | 0 | 2134 | 8 | h1-light-chat |

The order is unchanged from v1, p2c > round_robin > random > {least_queue_tokens, least_requests}, and
the worst load is now h4, the long-prompt mixture, for all three: the minimum lands on the hardest load
rather than the smallest, which is the inversion §5b of `docs/arena.md` predicted and the reason for
the change.

**Ranking under rule set v1**, absolute gated goodput, kept for the record; never compare a v1 score
with a v2 one:

| cap | 1st | 2nd | 3rd | 4th | 5th |
|---|---|---|---|---|---|
| **0.999** | p2c **0** | round_robin **0** | random **0** | least_queue_tokens 0 | least_requests 0 |
| **0.99** | p2c **0** | round_robin 0 | random 0 | least_queue_tokens 0 | least_requests 0 |
| **0.95** | p2c **2652** | round_robin **2632** | random **2546** | least_queue_tokens 0 | least_requests 0 |

At 0.95 under v1 each of the top three breached on exactly one load (h3, which does not count) and
scored its minimum on h6 or h4, the two lightest loads. At 0.99 every candidate is gated to zero; the
ranking there is broken by mean gated goodput (p2c 1848, round_robin 1023, random 548) and then by mean
ungated goodput, and the report prints both so a degenerate round cannot masquerade as a result.

**Load difficulty** against the best policy at cap 0.95, per §4 step 6: h6 0.838, h4 0.831,
h7 0.803, h3 0.800, h8 0.794, h1 0.765, h5 0.442, h2 0.000. That ordering is an artefact, not a
finding — see §5 below.

**The headline behavioural result** is `least_requests` and `least_queue_tokens`: attainment 0.71 at
0.10× rated capacity and 0.11 at 0.45×, against 0.996 and 0.986 for p2c. With one router and a 1 s
telemetry interval, global least-loaded sends *every* arrival in a scrape window to the same replica,
so it herds itself into a hotspot the moment load is non-trivial. `src/policy.rs` predicts this
("far more robust to stale telemetry"); the arena prices it, at roughly 4× goodput.

---

## 4. Rated capacity, measured

Honest rated capacity = the highest offered rate at which a policy still met the cap, over an 11-point
rate sweep. `Scenario::rated_rps()` is printed beside it for comparison: it is a *saturation* figure
from the cost model, and the gap is the point.

| mixture | analytic | cap 0.999 | cap 0.99 | cap 0.95 |
|---|---|---|---|---|
| chat (h2) round_robin | 328 | **none** | 66 | 177 |
| chat (h2) random | 328 | **none** | 66 | 148 |
| chat (h2) p2c | 328 | **none** | 98 | 197 |
| chat (h2) least_requests / least_queue_tokens | 328 | none | none | none |
| code completion (h7) round_robin | 353 | **71** | 159 | 212 |
| code completion (h7) random | 353 | 35 | 106 | 159 |
| code completion (h7) p2c | 353 | 35 | 71 | 159 |
| code completion (h7) least_* | 353 | none | none | none |

Three things fall out of that table.

1. **The cost model overstates SLO capacity by 2–5×.** 328 rps analytic against 197 rps at a 0.95 cap
   and 98 at 0.99. `rated_rps()` is device-seconds per request against the fleet with no queueing term,
   so it is the rate at which utilisation reaches 1, and service quality is gone well before that. A
   policy that declares the cost model's number breaches everywhere and scores zero — which is §3's
   mechanism working, on the cost model rather than on a lying policy.
2. **99.9 % is reachable, but only on short-output traffic.** round_robin holds 0.9991 at 71 rps on the
   code-completion mixture. No policy reaches 0.999 on any chat or long-context mixture at any rate,
   including 16 rps: the ceiling there is ~0.996. The cause is arithmetic, not policy. Prompt lengths
   are lognormal, so a fixed TTFT SLO is breached by the tail whatever the load does: at
   `prompt_mean` 12,035 with CV 0.94, about 1 % of long-mode prompts exceed 56.5 k tokens, which is
   2 s of pure prefill at 28,286 tok/s against a 2 s TTFT budget, and 8 % of traffic is long-mode.
   0.9992 is the arithmetic ceiling before any queueing. **A 99.9 % cap and a length-independent TTFT
   SLO are not jointly satisfiable**; either the SLO scales with prompt length, or it is per-class, or
   the cap is 0.99.
3. **p2c is not uniformly best.** It wins on the chat mixture at every cap and loses to round_robin on
   code completion at 0.999 (35 vs 71 rps), where requests are near-identical and short, so
   round_robin's exact fairness beats two-random-choices' variance. A worst-case objective over a
   diverse slate is the only thing that would have surfaced that.

---

## 5. What the judging half still needs, and what the spec cannot express as written

Three of these are gaps in `docs/arena.md` itself that showed up only once the numbers existed.

1. **The minimum is over absolute goodput, and absolute goodput is not comparable across loads.**
   §1 scores `min over L of goodput(policy, L)`. h1 offers 33 rps and h2 offers 148, so h1's goodput is
   a quarter of h2's *by construction* and the minimum is decided by the smallest load in the slate,
   never by the hardest. The measured round shows it: every top-three policy takes its minimum on h6 or
   h4, the two lightest loads, and the "load difficulty" ranking puts h6 hardest for the same reason.
   The fix is normalisation — score the minimum of goodput as a *share* of offered work, which
   `RunScore::goodput_share` now reports (h1 0.934, h2 0.886, h4 0.762, h6 0.880, h7 0.891 for p2c) —
   but it is a change to the objective and therefore a rule change for a human to accept, not something
   to slip in. Until then the discriminating signal lives in the gate, not in the score.
2. **A cap that nothing can reach ranks nothing.** At 0.999 all five policies score zero and the
   ranking is decided entirely by tie-breaks. §2.3 already requires that a score record its rule set;
   the cap has to be part of that record, and the arena should refuse to run a round whose cap no
   candidate can meet on the held-out suite rather than silently producing five zeros.
3. **Rated capacity has no home in the interfaces**, exactly as §6.2 predicts. `ScoreConfig` accepts a
   declaration from the caller and defaults to the analytic figure, so honesty can be *measured* but
   not *violated* — no policy can lie yet. This is the one field whose absence blocks the central
   mechanism of the whole design.
4. **Warm-up.** §3 wants several minutes so autoscaling settles. There is no autoscaling and no cold
   start, so `ARENA_MIN_WARMUP_S = 180` is defined and unenforced; rounds enforce
   `WARMUP_FLOOR_TODAY_S = 10`. Revisit when replicas can appear.
5. **Physics referee, strict mode.** Does not exist, so a violation cannot abort a run. `RunScore` has
   a `violations` field and a violation is a hard zero rather than a dropped row, which is the shape the
   strict referee needs; the checks themselves are not written.
6. **Judgement the envelope cannot mechanise**, which is the residual §2.3 leaves to the agent: is a
   mixture whose long mode is 25 % of traffic realistic, when `docs/calibration.md` measures only that
   input length is multi-modal and never publishes mode weights? Is p2c's win an artefact of a single
   simulated router? Both are questions about the model, and neither is a bound.

Bounds in the envelope that `docs/calibration.md` **could not source**, all wide and all labelled in
the code: `telemetry_delay_ms` (no delivery-delay figure exists anywhere in the document),
`load_step_duration_s` (§2.4: no burst-duration distribution is published; §9.1 guesses 30 s),
`long_probability` (the two-mode mixture weight is not a measured quantity; bounded by §4.2's
continuation fraction as the nearest proxy), `replicas` and `max_queue` (§9.2: no public source gives a
fleet size or a queue-depth distribution), and all three SLO thresholds (§9.1: *"Nothing public at all.
Not one trace carries an SLO or priority label."*).
