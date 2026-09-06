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

// Issao: meta note, can we execute in parallel? Whenever we can let's fork off side missions to execute in parallel and report back but make feedback on them for the main agents and myself lower priority

### A. Queueing and control — 3.5 h

Foundation, 1, 2, 4, 5, 6. Four of his named dynamics, all the load-balancing fallacies, and the
control-theory story.

// Issao: Lets make 4 lower priority.

Delivers: round-robin producing a rolling hotspot below rated capacity; power-of-two-choices
visibly fixing it; oscillation from stale telemetry with a measured frequency; a retry storm that
fails to recover; traffic shaping at three layers; per-class goodput.

Cuts: everything LLM-specific. Today's artefact would be a general serving simulator.

### B. Recognisably LLM — 3.25 h  ← **recommended**


// Issao: Sounds good, let's start here.  It seems like 7 might be cheap here too.

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

## 3. Recommendation, and the honest cost of it

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
| React dashboard | the static HTML report serves the same purpose today at a tenth of the cost |
| Ingress/Leaf shard split | measurement says one core carries the whole fleet, so this buys nothing today and its determinism test deserves care |

// Issao Don't cut the react dashboard. I want that. start executing on that in parallel, it should not have a lot of dependencies.

// Issao Also, to what extent it doesn't slow down everything else. Start executing on the Arena story, I think that will be fun.

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

// Issao: I am ok if we go a little bit over.