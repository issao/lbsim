# STATUS

What is finished, what is live, what Claude is doing now. For things that need *you*, see `TASKS.md`.

**Last updated:** 2026-09-06 13:50 by Claude.

---

## Phase

**Building.** The simulator runs and produces reports. Scope is `docs/scope-today.md` package B.

## Nothing blocks you leaving

One thing worth ten minutes before you go: **`docs/scope-today.md`**. It ranks all seventeen dynamics
by cost, shows everything totals about 22 hours against three remaining, and recommends package B. If
you disagree with the package, that is the one decision that changes what happens next. Everything
else has a default.

While you are out, Claude will work down section "Queued work" below, in that order, committing and
pushing each step so you can read the diff on your return.

## What runs right now

```
cargo build --release                                     # about one second, zero dependencies
./target/release/sim-run compare scenarios/route_round_robin.txt \
    scenarios/route_p2c.txt scenarios/route_least_requests.txt --out out/routing.html
```

First result, three policies at identical load and seed, 32 replicas, 40 requests/s offered against
a rated 143:

| policy | time-to-first-token p99 | per-replica load CV |
|---|---|---|
| round robin | 4,094 ms | 0.86 |
| power of two choices | 2,617 ms | 0.76 |
| least requests | 9,261 ms | 1.90 |

Round-robin against power-of-two-choices is the expected direction. The interesting one is
**least-requests, more than twice as bad as round-robin**: it routes on a snapshot up to a second
stale, so every router sees the same apparently-idle replica and stampedes it. That is the herding
failure the primer predicts, and it appeared without being engineered.

**Known gap, stated honestly:** service-level attainment is 60 to 80% in every run, so all three sit
above the knee and the goodput ordering is not yet clean. That is scenario tuning, not a modelling
problem, and it is first in the queue below.

## Queued work, in order

1. **Tune the reference scenario** so a balanced policy meets its SLOs and round-robin does not. That
   is what makes the rolling hotspot a *demonstration* rather than a table of numbers.
2. **Tests**: determinism, so the same scenario and seed give identical fingerprints; and a
   monotonicity check that latency rises with offered load.
3. **Stale-telemetry oscillation**, by sweeping the telemetry interval and measuring the ringing
   frequency the report already computes.
4. **Two-phase timing panel**: time-to-first-token and inter-token latency as separate distributions,
   already recorded and needing the chart.
5. **Retry storm and recovery**, a pair of runs, one collapsing and one with a retry budget that does
   not.
6. **A written summary of what each run shows**, so the report reads as findings rather than output.

## Live on `origin/master`

| What | Where | State |
|---|---|---|
| Simulator | `src/`, `scenarios/` | **runs**; routing comparison working |
| Reference cost model | `bench/validate_epochs.py` | four properties pass, including the compute branch |
| Queue benchmark | `bench/queue/` | 54 ns/event, open risk 1 closed |
| Interfaces | `proto/lbsim/v1/*.proto` | twelve files, reviewed and simplified |
| Architecture | `docs/ARCHITECTURE.md` | reviewed; decisions in section 14 |
| Today's scope | `docs/scope-today.md` | **the one thing to read before the gym** |
| Execution plan | `docs/execution-plan.md` | reviewed by you |
| Arena design | `docs/arena.md` | awaiting review |
| Dashboard spec | `docs/ui-spec.md` | awaiting review; playback and trace views added |
| Agent architecture | `docs/agent-architecture.md` | awaiting review; arena agents added |
| Calibration | `docs/calibration.md` | complete |

## Assumptions Claude is running on

- Package B of `docs/scope-today.md` is the scope for today.
- Analytic epoch advance, prefix caching in phase 2, cluster-pooled tiers, three-layer deployment,
  absolute epoch time: all decided, see `docs/ARCHITECTURE.md` section 14.
- Deployment scales to zero within a replica budget of ten.
- Reference hardware is a 70-billion-parameter model on eight H100s.

## Branch state

Working branch `claude/today`, merged to `master` and pushed. Tree clean.
