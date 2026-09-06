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

```bash
./run-demos.sh          # five experiments, five self-contained HTML reports in out/
./check-sensitivity.sh  # asserts the policy ordering survives 30% cost-model error
```

**Five dynamics reproduce.** Full write-up with tables in `docs/findings.md`. In one line each, all at
30% of rated capacity so none is an overload artefact:

1. **Reading the whole fleet is 2.6x worse than sampling two of it.** Least-requests routing on a
   1.2-second-stale snapshot reaches 39% attainment against 97% for power-of-two-choices, and is worse
   than round robin, which ignores load entirely. Throughput barely moves across all four.
2. **Herding is a steep function of staleness.** Scrape interval from 100 ms to 4 s takes goodput from
   14,912 to 1,264. Almost nothing between 100 and 250 ms, then a cliff: staleness has a threshold set
   by how fast queues change.
3. **Prefill and decode contend, and no setting wins both.** Chunk budget from 512 to 16,384 tokens
   takes the worst gap between tokens from 33 ms to 587 while first-token latency improves. Throughput
   is flat at ~18,700 while goodput falls eightfold, because the gap crosses an 80 ms target. The fleet
   does identical work and delivers a tenth of the value.
4. **Past the knee, offering more load delivers less.** Goodput peaks at 150 requests/s, throughput at
   190. Offering 53% more than the goodput optimum yields 45% less goodput.
5. **A load-balancer metric improves while service collapses.** Raising the long-context share to 32%
   halves goodput and makes tail latency 34x worse *while load spread falls*, because a few enormous
   requests saturate every replica uniformly. The clearest argument for denominating everything in
   tokens rather than requests.

**The orderings are checked, not asserted.** `check-sensitivity.sh` perturbs the bandwidth and prefill
constants by 30% in each direction, seven cases; the ranking is identical in all of them. Magnitudes
move, orderings do not, so the orderings are the conclusions.

## Three efforts running in parallel

Each on its own branch, file-disjoint, so none can collide with the engine:

- **`claude/web-standin`** — the stand-in dashboard against mock data, per `docs/ui-spec.md`.
- **`claude/tests`** — determinism, stream independence, monotonicity, histogram and queue unit tests.
- **`claude/arena`** — the mechanical scoring function, realism envelope, and the fixed held-out suite
  from `docs/arena.md`.

Nothing touching the cost model or step loop is forked, because that is coupled through one function
and parallel work on it produces plausible-looking integration failures rather than obvious ones.

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
