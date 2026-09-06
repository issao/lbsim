# Scope cut for an eight-hour budget

Written 13:35. Roughly three hours spent on design, so **about three hours of build remain** before
the 16:30 stop. This document ranks every dynamic in `VISION.md` section 3a by what it needs and
what it costs, proposes hard cuts, and names what today's deliverable will and will not be.

Claude starts on the recommended package immediately rather than waiting, so a veto arrives
mid-flight rather than after the deadline.

---

## 1. Every dynamic, what it needs, what it costs

Estimates are cumulative-independent: each row assumes the shared foundation exists and counts only
its own additional work.

| # | Dynamic (VISION 3a) | Needs | Cost |
|---|---|---|---|
| — | **Foundation**: workspace, CI, event queue, seeded streams, metrics, HTML report | — | **1.0 h** |
| 1 | Round-robin rolling hotspot, heterogeneous sizes | replica queue, service time proportional to work, four routing policies | **1.25 h** |
| 2 | Stale-telemetry oscillation, control-theory framing | telemetry period and delay, policies reading only a delayed view, perturbation input | **0.75 h** |
| 3 | Two-phase timing: time-to-first-token separate from inter-token latency | prefill and decode as distinct costs, batch-dependent step time | **0.75 h** |
| 4 | Retry storm, metastable collapse | client timeout, retry with a budget, failure injection | **0.5 h** |
| 5 | Traffic shaping at every layer | admission at gateway, router and replica; token-denominated limits | **0.5 h** |
| 6 | Mixed SLO classes | priority queues, per-class goodput | **0.25 h** |
| 7 | Prefill/decode interference, utilization versus latency | KV accounting, chunked prefill, analytic epochs, the referee | **2.0 h** |
| 8 | KV preemption death spiral | 7, plus preemption with swap or recompute, plus sessions | **1.5 h** |
| 9 | Prefix affinity versus load spreading | prefix segment tree, affinity routing, cache-aware placement | **1.0 h** |
| 10 | Affinity hotspot failover cascade | 9 and 4 | **0.25 h** |
| 11 | Gray failure and detection latency | health vector, outlier detection, ejection | **0.5 h** |
| 12 | Global cascading failure | 4, 11, multi-cluster | **1.0 h** |
| 13 | Multi-geo diurnal autoscaling with cold start | multi-cluster, geo routing, turn-up delay, warm pools | **1.5 h** |
| 14 | KV memory tiering | tier pools, shared bandwidth containers, migration policies | **1.0 h** |
| 15 | Prefill/decode disaggregation | separate pools, KV transfer over the modelled fabric | **2.0 h** |
| 16 | Speculative decoding | 7, plus the N/M model | **0.5 h** |
| 17 | Model weights, multi-model, MoE | weights as tiered objects, cache replacement | **2.0 h** |
| — | React stand-in dashboard | Vite app, panels, mock data | **2.0 h** |
| — | Three-layer Ingress/Leaf split with barriers | shard boundary, determinism-across-shards test | **1.5 h** |

**Total for everything: about 22 hours.** Against three, so roughly seven eighths has to go.

---

## 2. Three packages that fit

**Parallel execution, per Issao:** *"Whenever we can let's fork off side missions to execute in
parallel and report back but make feedback on them for the main agents and myself lower priority."*

Two side missions are file-disjoint from the engine and run in parallel, which is the only kind of
fan-out that is safe: the stand-in dashboard under `web/`, and the test suite under `tests/`. Neither
touches `src/`, so neither can collide with the main line. Their reports come back at lower priority,
so a side mission finishing does not interrupt the critical path.

What is *not* forked: anything touching the cost model or the step loop. Those are coupled through
one function, and parallel agents on coupled work produce plausible-looking integration failures
rather than obvious ones.

### A. Queueing and control — 3.5 h

Foundation, 1, 2, 4, 5, 6. Four of his named dynamics, all the load-balancing fallacies, and the
control-theory story.

**Deprioritised by Issao:** *"Lets make 4 lower priority."* The retry storm moves to the end of the
queue. It is cheap once the rest exists, and it is the least surprising of the four, since retry
amplification is well understood; the rolling hotspot and the oscillation are the results worth
having first.

Delivers: round-robin producing a rolling hotspot below rated capacity; power-of-two-choices
visibly fixing it; oscillation from stale telemetry with a measured frequency; a retry storm that
fails to recover; traffic shaping at three layers; per-class goodput.

Cuts: everything LLM-specific. Today's artefact would be a general serving simulator.

### B. Recognisably LLM — 3.25 h  ← **recommended**


**Selected by Issao:** *"Sounds good, let's start here. It seems like 7 might be cheap here too."*

He is right, and my estimate was wrong in a specific way. Item 7 bundled two things that separate
cleanly:

- **Prefill/decode interference is already done.** The step-time model adds the prefill work done in
  a step to the decode cost, because both contend for one device, so a long prompt already inflates
  everyone else's inter-token latency. Chunked prefill with a token budget is already there too. That
  was never the two hours; it came free with two-phase timing.
- **What is left is the KV capacity constraint**, which limits batch size by resident tokens rather
  than by a request count. That is roughly 20 minutes: a token budget per replica, admission that
  respects it, and a utilization metric. It also sets up item 8, preemption, for very little more.

So item 7 is promoted into today's package at about 0.5 hours rather than 2, and the estimate is
corrected rather than quietly reused.

Foundation, 1, 2, 3, 4. Same as A minus shaping and SLO classes, plus two-phase timing.

Delivers everything in A's first three items, and adds the observable that makes this domain
distinctive: **time-to-first-token and inter-token latency as separate quantities**, with step time
rising as batches grow. That is what makes a chart look like an inference fleet rather than a web
service, and it is 45 minutes rather than the 2 hours full prefill/decode physics costs.

Cuts: KV capacity, preemption, prefix caching, tiering, autoscaling, multi-cluster, disaggregation,
speculative decoding, the shard split, and the React dashboard.

### C. Everything in A and B — 4.25 h

Over budget by an hour. Listed only to be explicit that it does not fit.

---

## 3. Decision: package B, plus item 7

**Chosen by Issao 13:50.** Package B, with item 7 promoted after the estimate was corrected, item 4
demoted, and two side missions running in parallel. Working order:

| Order | Item | Cost | State |
|---|---|---|---|
| 1 | Foundation | 1.0 h | **done** |
| 2 | Rolling hotspot, 1 | 1.25 h | **done**, finding 1 |
| 3 | Two-phase timing, 3 | 0.75 h | **done**, finding 3 |
| 4 | KV capacity, 7 | 0.5 h | **done**, finding 5 |
| 5 | Stale-telemetry oscillation, 2 | 0.75 h | **done**, finding 2 |
| 6 | Retry storm, 4 | 0.5 h | **done**, finding 6 |
| — | Tests | parallel | **done**, 45 pass |
| — | Stand-in dashboard | parallel | **done**, `web/` |

Every row was done by 15:00. Findings are numbered in `docs/findings.md`. The reasoning below is kept
as written, since it is what the decision was made against.

**Take B.** Four dynamics working end to end beats one dynamic half-built, and the two-phase timing
keeps the artefact recognisably about inference.

**What that costs, stated plainly.** The distinctive core of this project is prefill/decode
interference and KV-driven collapse, and B does not include either. Today's deliverable will
demonstrate load-balancing and control dynamics with inference-shaped timing, not inference-specific
capacity behaviour.

**Why that is acceptable rather than a failure.** Three reasons, and the first is his own:

1. `VISION.md` section 3a asks for the round-robin fallacy *"early on"* and says explicitly that it
   *"can be simulated without the detailed prefill/decode LLM dynamics"*. B is what he asked to see
   first.
2. The full cost model is already validated, in `bench/validate_epochs.py`, including the compute
   branch and speculation, proven exactly against a naive per-step oracle. The physics is de-risked;
   porting it to Rust is mechanical rather than exploratory.
3. Item 7 is the one place where a subtle error produces plausible wrong numbers. It deserves the
   differential oracle running against it, which is not a thing to rush in the last ninety minutes
   of a deadline.

**What is cut and why each is affordable:**

| Cut | Why it can wait |
|---|---|
| KV capacity and preemption, 7 and 8 | the highest-value LLM dynamics, and the highest-risk code. Needs the oracle, not a deadline |
| Prefix caching, 9 and 10 | needs 7 first to be meaningful |
| Autoscaling and multi-geo, 12 and 13 | the largest single item; nothing else depends on it |
| Tiering and disaggregation, 14 and 15 | interesting, and additive rather than structural |
| Speculative decoding, 16 | 30 minutes once 7 exists; pointless before |
| React dashboard | **reinstated by Issao**: *"Don't cut the react dashboard. I want that. start executing on that in parallel, it should not have a lot of dependencies."* Built as the stand-in under `web/` |
| Ingress/Leaf shard split | measurement says one core carries the whole fleet, so this buys nothing today and its determinism test deserves care |

The arena was also pulled forward, by Issao: *"Also, to what extent it doesn't slow down everything
else. Start executing on the Arena story, I think that will be fun."* Its mechanical half is built;
`docs/arena-implementation.md` has the measured round.

---

## 4. What "done at 16:30" means

A single command produces a self-contained HTML report showing:

1. Round-robin against power-of-two-choices at identical load and seed, with p99 latency visibly
   worse under round-robin while the fleet sits below rated capacity. The rolling hotspot, on a
   per-replica heatmap over time.
2. Time-to-first-token and inter-token latency as separate distributions, with step time rising as
   batches fill.
3. Oscillation induced by telemetry delay, with the frequency measured and compared against the
   loop's sampling period.
4. A retry storm that does not recover after offered load returns to normal, beside a run with a
   retry budget that does.

Plus: `cargo test` green, including a determinism test asserting identical fingerprints across runs,
and every number in the report reproducible from a committed scenario file.

**If time runs short, items are dropped from the bottom of that list, not the top.**

**Issao:** *"I am ok if we go a little bit over."* Noted. The order stays as written, so going over
buys items from the bottom rather than risking the top.