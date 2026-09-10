# Policy catalog

Every policy idea this project has had so far, one table per family, so the arena has a menu to evaluate
and a place to append what it authors. Created 2026-09-06 16:30 PDT from a sweep of `VISION.md`,
`docs/ARCHITECTURE.md`, `docs/arena.md`, `docs/arena-implementation.md`, `docs/llm-serving-primer.md`,
`docs/findings.md`, `docs/scope-today.md`, `proto/lbsim/v1/scenario.proto` and `crates/sim-policy/src/`.

Issao, 2026-09-06 16:22: *"Load and latency forecasting should be added as potential policies to evaluate
(populate an md with all policy ideas we have had so far and instruct the arena policy generator to
populate that as well with any that it authors)."*

## Format

Every table in this file has exactly this header, and code depends on it:

```
| Name | Family | Idea | Status | Source | Score | Rule set | Added |
```

- **Status** is one of `shipped` (on `master`, a scenario exercises it), `in-flight` (an agent is
  building it on a `claude/tl-*` branch), `idea` (named in a document or a proto, no code), or
  `arena-authored` (written by the arena's policy generator).
- **Source** is the file that defines it, or the document section that proposed it.
- **Score** and **Rule set** are the arena's: the score under the rule set named beside it, because a
  score is only comparable within one rule set (`docs/arena.md` §2.3). Under the current rule set,
  `RULE_SET` in `crates/sim-arena/src/lib.rs`, *"v2: cap 0.95 default, min over in-scope loads of gated
  goodput share"*, the score is the minimum over the in-scope held-out loads of goodput as a share of
  offered output tokens, gated by the SLA cap; under v1 it was absolute gated goodput in tokens/s, and
  v1 rows are kept only where no v2 score exists. Empty for ideas. Where no arena score exists but a
  finding measured the policy, the finding's number is quoted in the Idea column instead.
- **Added** is the date the row was written.

**The arena policy generator appends one row per policy it authors**, with Status `arena-authored`,
Source the path of the generated file, and Score and Rule set from the round that scored it. The append
is `sim_arena::catalog::append` (f6a87a9), and `tests/arena_catalog.rs` checks every table header in
this file against `catalog::HEADER`, so the catalog and the code cannot silently diverge; an identical
row is not appended twice, and pipes in a cell are escaped. Rows are appended to the table of the policy's family;
a family that does not exist yet gets a new table with the same header.

Every policy is a pure function from a **stale observation** to intents, checked by the referee
(`docs/ARCHITECTURE.md` §5). Routing policies must be O(1) or O(log N) per decision; a fleet scan fails
the run (§10.4). Both constraints apply to arena-authored policies too.

## Routing

Where a request goes once admitted. Finding 1: at 30% of rated capacity, reading the whole fleet routes
worse than sampling two of it; finding 2: herding has a staleness threshold, not a gradient.

| Name | Family | Idea | Status | Source | Score | Rule set | Added |
|---|---|---|---|---|---|---|---|
| `round_robin` | routing | Ignore load; with heterogeneous request sizes produces the rolling hotspot (dynamic 1). Finding 1: 17,361 tok/s goodput, first-token p99 8.3 s | shipped | `crates/sim-policy/src/round_robin.rs` | 0.732 | v2: cap 0.95 default, min over in-scope loads of gated goodput share | 2026-09-06 |
| `random` | routing | Uniform random; better than round robin under size heterogeneity because it does not cycle. O(1). Finding 1: 16,961 tok/s | shipped | `crates/sim-policy/src/random.rs` | 0.705 | v2: cap 0.95 default, min over in-scope loads of gated goodput share | 2026-09-06 |
| `least_requests` | routing | Fewest queued-plus-running requests over a full stale-snapshot scan. Wrong unit (requests, not tokens) and herds. Finding 1: 6,943 tok/s, attainment 39% | shipped | `crates/sim-policy/src/least_requests.rs` | 0 | v2: cap 0.95 default, min over in-scope loads of gated goodput share | 2026-09-06 |
| `least_queue_tokens` | routing | Fewest queued tokens over a full stale scan: the right unit, still herds. Finding 2: goodput collapses twelvefold across scrape intervals 100 ms to 4 s | shipped | `crates/sim-policy/src/least_queue_tokens.rs` | 0 | v2: cap 0.95 default, min over in-scope loads of gated goodput share | 2026-09-06 |
| `p2c` | routing | Power of two choices on queued tokens from the stale snapshot; only a fraction of routers see any one idle replica, which bounds herding. O(1). Finding 1: 18,127 tok/s, attainment 97% | shipped | `crates/sim-policy/src/p2c.rs` | 0.762 | v2: cap 0.95 default, min over in-scope loads of gated goodput share | 2026-09-06 |
| `least_kv_probe` | routing | Power of `d` choices on *live* KV occupancy, paying a modelled probe per look, so the cost of freshness is visible rather than free. Head-to-head with `p2c` isolates signal and freshness | shipped | `crates/sim-policy/src/least_kv_probe.rs` | | | 2026-09-06 |
| `least_kv_tokens` | routing | Fewest resident KV tokens; the capacity unit that finding 5 says matters, over the stale view | idea | `proto/lbsim/v1/scenario.proto` `RoutingPolicy.LeastKvTokens` | | | 2026-09-06 |
| `prefix_affinity` | routing | Prefer the replica already holding the request's prefix until its load exceeds the fleet mean by `affinity_max_load_ratio`, then fall back to power of `affinity_fallback_choices`; a bounded-cost load estimate, no fleet scan. The knob between hit rate and hotspots, and finding 11 measures it as monotone: hit rate 17 → 22 %, load CV 0.32 → 0.75, goodput 147,043 → 141,314 tok/s from p2c to ratio 2.2, on fresh telemetry; at the baseline's staleness every ratio herds to CV ≈ 1.2 | shipped | `crates/sim-policy/src/prefix_affinity.rs` (U27b, d2c117e, merged 485f368; the prefix tree and replica cache U27a 820e752; hit rate on the wire U27c 1ee2f80); `scenarios/affinity_{off,spread,sticky}.txt`; findings 11 and 14 | | | 2026-09-06 |
| `estimated_work` | routing | Score candidates by prompt length as a proxy for prefill cost plus queued tokens, so a 100k-token context is not treated as one chat turn | idea | `docs/llm-serving-primer.md` §6 | | | 2026-09-06 |
| `geo_routing` | routing | Route across clusters by geography and cluster headroom, with a cost for crossing regions; needed for the multi-geo dynamics | idea | `VISION.md` §3a; `docs/scope-today.md` item 13 | | | 2026-09-06 |
| `bounded_top_k_affinity` | routing | Ingress keeps a bounded top-K prefix index rather than an exact one, so affinity is O(log N) at 50k replicas | idea | `docs/ARCHITECTURE.md` §10.4; `TASKS.md` §3 default | | | 2026-09-06 |
| `forecast_load` | routing | The shipped form of `queue_depth_extrapolation`: power of two choices scored on a predicted queue depth, the slope fitted between the last two distinct views per replica and extrapolated over the telemetry delay; candidates drawn in exactly `p2c`'s order so the two see byte-identical candidate sets. Staleness test at 4000 ms: load CV 0.1449 for p2c, 0.1414 for forecast_load (the tech lead, U40a) | shipped | `crates/sim-policy/src/forecast_load.rs` (9b4fc5c, merged 9ee9841); `scenarios/route_forecast_load.txt` | — | v2 | 2026-09-06 |
| `forecast_latency` | routing | The shipped form of `queue_model_ttft`: sample `d` replicas exactly as `p2c` does, then route to the minimum predicted time to first token for this request (queued prefill ahead of it plus its own prefill over the prefill rate, plus one step), no state and no randomness beyond the shared draw. At 0.8 of rated load with `long_probability = 0.16` it beats p2c on TTFT p99 (3,768 → 3,291 ms) and attainment (0.6235 → 0.6349); at 0.3 the two rank identically in the eight-replica fixture (U40b's tests) | shipped | `crates/sim-policy/src/forecast_latency.rs` (9cae0fa, merged 052d40e); `scenarios/route_forecast_latency.txt` | — | v2 | 2026-09-06 |
| weighted_random | routing | Weighted random over the stale view, weight = max(0, c1 + c2·[queued decode] + c3·[queued prefill] + c4·decode beyond the open buffer + c5·prefill beyond it); Fenwick tree with lazy refresh, O(log N) a draw, 5 views a decision; c1 alone is random to the byte. Demo 21 with (1, −0.3, −0.5, −0.05, −0.1): herds on the 1 s view (CV 0.43 vs 0.20), sheds 15 %, attainment 9 % | shipped | crates/sim-policy/src/weighted_random.rs (263d62f); scenarios/buffer_weighted.txt; proto RoutingPolicy.WeightedRandom | | | 2026-09-10 |

## Scheduling

What a replica does each step: which queued requests to admit into the batch and in what order, the
prefill chunk budget, the order the batch's unfinished prompts are prefilled in, and the preemption
victim. A pluggable seam since U108 (`SchedulingPolicy`, `crates/sim-core/src/scheduling.rs`, key
`scheduling`), bounded to `max_batch` queue entries per decision. Finding 3: prefill and decode
contend for one device and no chunk size wins both; finding 13: a replica scheduler orders what it
holds and creates no capacity, and only separates from FIFO once the prefill budget is contended.

| Name | Family | Idea | Status | Source | Score | Rule set | Added |
|---|---|---|---|---|---|---|---|
| `fifo_chunked` (was `fcfs`) | scheduling | First come first served over the replica queue, a fixed `step_token_budget` prefill chunk, batch-order prefill and U22's victim rule: the engine's behaviour before the seam, now the default `SchedulingPolicy`, proven byte-identical on every golden row. Finding 13: 94.3 % attainment, interactive 0.926 at 1,120 rps | shipped | `crates/sim-core/src/scheduling.rs` (`FifoChunked`, the seam's home, re-exported by `sim-policy`; U108, 96a6456, merged ce4a0bb); `scenarios/sched_fifo.txt`; `proto/lbsim/v1/scenario.proto` `SchedulingPolicy.Fcfs` | | | 2026-09-06 |
| `prefill_priority` | scheduling | Favour prefill up to `max_prefill_tokens_per_step`: better TTFT, worse ITL for everyone decoding, faster KV growth | idea | `scenario.proto` `SchedulingPolicy.PrefillPriority`; primer §4 | | | 2026-09-06 |
| `decode_priority` | scheduling | Reserve `min_decode_tokens_per_step` for decode: protects ITL, can starve arrivals | idea | `scenario.proto` `SchedulingPolicy.DecodePriority` | | | 2026-09-06 |
| `deadline_first` (was `earliest_deadline_first`) | scheduling | Admit by least slack, deadline minus now minus the prefill still owed at the view's prefill rate; evict the latest deadline; a stable sort of at most `max_batch` entries. Finding 13: interactive attainment falls 0.926 → 0.883, because charging the prefill owed against slack makes the longest prompts look most urgent, EDF over chunked prefill is longest-job-first in disguise | shipped | `crates/sim-policy/src/deadline_first.rs` (U108, c43b56e, merged ce4a0bb); `scenarios/sched_deadline.txt`; `scenario.proto` `SchedulingPolicy.EarliestDeadlineFirst`; primer §9 | | | 2026-09-06 |
| `class_priority` (was `slo_class_priority_queues`) | scheduling | Strict priority by SLO class, interactive before agent before batch, FIFO within a class; evicts the newest of the lowest class present. Finding 13: interactive attainment 0.926 → 0.943 and interactive goodput 156,563 → 159,615 tok/s at 1,120 rps; agent pays 0.972 → 0.960, batch stays 0.997; throughput within a quarter of a percent, since a replica scheduler orders what it holds and creates no capacity | shipped | `crates/sim-policy/src/class_priority.rs` (U108, c43b56e, merged ce4a0bb); `scenarios/sched_class.txt`; `scenario.proto` `SchedulingPolicy.ClassPriority` (14, d9a2757); `VISION.md` §3a "different latency SLO requirements"; `docs/scope-today.md` item 6 | | | 2026-09-06 |
| `n_requests_m_batches` | scheduling | A policy sees N queued requests and plans the next M batches across a subset of machines, the batch-level realism VISION.md asks for, computed analytically | idea | `VISION.md` §4 | | | 2026-09-06 |
| `prefill_decode_co_scheduling` | scheduling | Co-schedule prefill chunks and decode steps across replicas so one replica's prefill burst does not stall another's decode; the non-disaggregated answer to interference | idea | `VISION.md` §3b "prefill-decode batch co-scheduling" | | | 2026-09-06 |
| `speculative_decoding` | scheduling | Draft-model speculation with a stochastic acceptance model: fewer steps per token when the small model agrees, wasted compute when it does not | idea | `VISION.md` §3a, §3b; `docs/scope-today.md` item 16 | | | 2026-09-06 |
| `step_budget_controller` | scheduling | Adjust `step_token_budget` per step from the current ITL against its SLO, via the `SetStepBudget` intent, rather than fixing it per scenario | idea | `proto/lbsim/v1/policy.proto` `Intent.SetStepBudget`; finding 3 | | | 2026-09-06 |
| buffered_batch | scheduling | A batch buffer at the GPU scheduler: a step admits only what its chunk (buffer_max_prefill_tokens), seats (buffer_max_batch), decode seats (buffer_max_decode_seqs) and bandwidth line (step priced without its chunk against the ITL target) can serve; an idle replica holds its queue up to buffer_max_hold_ms. Demo 21 at 1.2x: throughput +24 %, ITL p99 68 → 57 ms, timeouts 38,466 → 6,562, goodput 80.5k → 253 tok/s (strict FIFO vs fifo's newest-first seat inheritance) | shipped | crates/sim-policy/src/buffered_batch.rs (263d62f); scenarios/buffer_batch.txt; proto SchedulingPolicy.BufferedBatch | | | 2026-09-10 |

## Admission and shaping

Whether to accept a request at all, before it costs anything. Rate limits are denominated in tokens,
never requests. Finding 4: past the knee, offering more load delivers less; finding 6: a retry budget is
the difference between a bad minute and an outage.

| Name | Family | Idea | Status | Source | Score | Rule set | Added |
|---|---|---|---|---|---|---|---|
| `accept_all` | admission | No admission control; the baseline every admission policy is compared against | shipped | `crates/sim-policy/src/accept_all.rs` | | | 2026-09-06 |
| `deadline_aware` | admission | Shed what cannot make its deadline *before* it consumes GPU time, with `queue_wait_headroom` reserved for serving; systems that check afterwards spend overload capacity on tokens nobody reads | shipped | `crates/sim-policy/src/deadline_aware.rs`; `scenarios/admit_deadline_aware.txt` | | | 2026-09-06 |
| `fair_share` | admission | Weighted fair share over tenants in tokens of *serviceable* throughput, with `burst_multiplier` slack; only binds once the fleet is contended | shipped | `crates/sim-policy/src/fair_share.rs`; `scenarios/admit_fair_share.txt` | | | 2026-09-06 |
| `token_bucket` | admission | Token-denominated rate limit with burst; `charge_max_tokens` charges at admission against the maximum output and refunds on completion, and the gap is where preemption is born | idea | `scenario.proto` `AdmissionPolicy.TokenBucket`; primer §9 | | | 2026-09-06 |
| `layered_shaping` | admission | Shaping at every layer: gateway, router and replica each shed with their own view, so no single stale view can admit a storm | idea | `VISION.md` §3a "value of traffic shaping at each layer"; `docs/scope-today.md` item 5 | | | 2026-09-06 |
| `rated_capacity_spill` | admission | Declare a rated capacity and shed above it for free; the arena's anti-gaming mechanism, also a real policy: an honest declaration maximizes score | idea | `docs/arena.md` §3; `policy.proto` `Intent.DeclareRatedCapacity` | | | 2026-09-06 |
| `retry_budget` | admission | Client-side: retries allowed as a fraction of successes, so a retry storm cannot exceed the budget. Finding 6: 10% budget recovers 86% of no-retry goodput; no budget turns a bad minute into an outage | shipped as a scenario knob | `scenarios/retry_budget.txt`; `policy.proto` `Intent.ScheduleRetry` | | | 2026-09-06 |
| `stale_view_shaping` | admission | Shaping that accounts for its own staleness: admit against the *predicted* fleet state at the delay horizon rather than the last scrape; see load forecasting | idea | `VISION.md` §3b "load shaping decisions with stale cluster status information" | | | 2026-09-06 |

## Autoscaling

When to change fleet size. The actuator delay is a cold start of 30 to 300 seconds against a burst that
develops in tens of seconds, which is why reactive scaling cannot catch one. A pluggable seam since U32
(`AutoscalingPolicy`, `crates/sim-policy/src/autoscaling.rs`, key `autoscaling`): the policy returns a
number, the engine owns the WARMING / DRAINING / ABSENT lifecycle. Finding 17 measures the cold start.

| Name | Family | Idea | Status | Source | Score | Rule set | Added |
|---|---|---|---|---|---|---|---|
| `none` (`fixed`) | autoscaling | Fleet size never changes and no tick is scheduled; the default `AutoscalingPolicy`, and every result before finding 17 is at fixed size. `min_replicas` and `max_replicas` default to `replicas`, so a scenario written before the seam has no room to move and is byte-identical | shipped | `crates/sim-policy/src/no_autoscaling.rs`, seam `crates/sim-policy/src/autoscaling.rs` (U32, 65a407e, merged e703f6a); `scenarios/autoscale_none.txt` | | | 2026-09-06 |
| `target_utilization` | autoscaling | Hold the fleet's mean `running / max_batch` at `autoscale_target` with a dead band (up above the target, down only below half of it), at most `autoscale_step` replicas per decision in either direction, no scale-down within `autoscale_cooldown_s` of a scale-up; a decision every `autoscale_interval_s` from the delayed views. The engine owns the lifecycle: WARMING for `warmup_delay_s`, DRAINING until `drain_timeout_s`. Finding 17: 29 % fewer replica-seconds than a fixed fleet, attainment 91.6 % at a 5 s cold start and 51.3 % at 30 s against 96.7 % fixed, because the saturating signal caps growth at 1/target per cycle and the cold start is the cycle | shipped | `crates/sim-policy/src/target_utilization.rs` (U32, 65a407e, merged e703f6a); `scenarios/autoscale_cold{5,30}.txt` | | | 2026-09-06 |
| `reactive_threshold` | autoscaling | Scale on queue depth or KV utilization (never GPU utilization, which reads high across a wide load range on a decode-bound replica), with cooldown and step size; `target_utilization` above is the utilization form, and finding 17 is the case for a queue-depth signal that does not saturate | idea | `scenario.proto` `AutoscalingPolicy.ReactiveThreshold` | | | 2026-09-06 |
| `predictive_diurnal` | autoscaling | Follow a configured or learned diurnal curve, acting ahead of demand by the cold-start time with `headroom_fraction` | idea | `scenario.proto` `AutoscalingPolicy.PredictiveDiurnal`; `VISION.md` §3a multi-geo | | | 2026-09-06 |
| `warm_pool` | autoscaling | Hold loaded-but-idle replicas; the only thing that makes scaling fast enough to matter, and it costs money to hold | idea | `scenario.proto` `AutoscalingPolicy.WarmPool`; `docs/scope-today.md` item 13 | | | 2026-09-06 |
| `robust_controller` | autoscaling | Treat scaling as a control problem with the turn-up delay as a known actuator lag: gain and phase margin from the M7 Bode sweep, so the oscillation onset is predicted rather than discovered | idea | `VISION.md` §3a "robust control theory problem"; `docs/execution-plan.md` M7 | | | 2026-09-06 |
| `spike_absorption` | autoscaling | Scale to absorb unexpected spikes while optimizing utilization across geographies with different diurnal patterns; the global-serving variant of the above | idea | `VISION.md` §3a | | | 2026-09-06 |

## Failover and health

How to notice a replica that is failing without saying so. Detection latency is the property under
test: a replica reporting healthy while serving at a tenth of its speed keeps receiving work. A
pluggable seam since U31b (`HealthPolicy`, `crates/sim-policy/src/health.rs`, key `ejection`): assessed
once per telemetry delivery from the delayed view, the verdict written back into it. Finding 12 measures
the detection latency; finding 14 the failover cascade.

| Name | Family | Idea | Status | Source | Score | Rule set | Added |
|---|---|---|---|---|---|---|---|
| `none` (`trust_announced`) | failover | Only announced crashes count, through `ReplicaView.ejected`; gray failure is invisible. The default `HealthPolicy` and the baseline; every run without `ejection` is byte-identical. Finding 12: 49 requests reach a replica at 0.3× after onset, 48 of them miss their SLO, and no fleet percentile moves | shipped | `crates/sim-policy/src/no_ejection.rs`, seam `crates/sim-policy/src/health.rs` (U31b, 206cb85, merged 02039ea); `scenarios/gray_failure_none.txt`; `scenario.proto` `FailureDetectionPolicy.TrustAnnounced` | | | 2026-09-06 |
| `outlier` (`latency_outlier`) | failover | Eject a replica whose `last_step_ns` in the delayed view exceeds `ejection_ratio` × the fleet median on `ejection_views` consecutive views, for `ejection_cooldown_s`, then judge afresh; assessed once per telemetry delivery, never per request, the median over a fixed-stride sample above 256 replicas. Finding 12: detection latency 4.3 s on a 1 s scrape, 20 requests reach the gray replica instead of 49; the demo needs ratio 8 because a healthy fleet's step time is bimodal (11 ms decode, 48 ms with a prefill chunk) and ratio 3 ejected 146 healthy replicas | shipped | `crates/sim-policy/src/outlier_ejection.rs` (U31b, 206cb85, merged 02039ea); `scenarios/gray_failure_eject.txt`; `scenario.proto` `FailureDetectionPolicy.LatencyOutlier`; `docs/scope-today.md` item 11 | | | 2026-09-06 |
| `silence_timeout` | failover | Eject after `silent_for_ns` without telemetry; catches partitions, misses slow-but-talking | idea | `scenario.proto` `FailureDetectionPolicy.SilenceTimeout` | | | 2026-09-06 |
| `failover_spread` | failover | On ejection, spread the failed replica's sessions across many replicas rather than the affinity-nearest one, to avoid the hotspot failover cascade | idea | `VISION.md` §3a "hot spot of machine fail over can lead to a cascading overload"; `docs/scope-today.md` item 10 | | | 2026-09-06 |
| `cascade_breaker` | failover | Cross-cluster shedding when a cluster's ejection rate rises, so a local failure does not become a global cascading failure through retries and failover | idea | `VISION.md` §3a "global cascading failure"; `docs/scope-today.md` item 12 | | | 2026-09-06 |

## KV and memory tiers

What to evict when KV fills, where its state goes, and where cached prefixes live. Swap costs roughly
20 ms each way to cluster DRAM; recompute roughly 270 ms of prefill for 4k tokens, so swap wins until the
shared bandwidth saturates and the advantage inverts. Finding 5: capacity is a token budget.

| Name | Family | Idea | Status | Source | Score | Rule set | Added |
|---|---|---|---|---|---|---|---|
| `preempt_never` | kv | No preemption: a request that does not fit waits in the queue. What the engine does today | shipped | `crates/sim-leaf/src/lib.rs` KV budget | | | 2026-09-06 |
| `preempt_recompute` | kv | Evict a victim and re-prefill it later; the death spiral of `docs/scope-today.md` item 8 when preempted requests demand prefill again. Also where a tiered context goes when every pool is full (finding 16) | shipped | `crates/sim-leaf/` (U22, d48191f, merged dcf77c8); `scenario.proto` `PreemptionPolicy.Recompute` | | | 2026-09-06 |
| `preempt_swap_to_dram` | kv | Evict to cluster DRAM over the modelled PCIe/fabric and restore later; since U30 the destination is the cluster pool when `dram_pool_tokens` is set, otherwise the per-replica cap. Finding 8: 2 % → 95 % attainment on the session spiral | shipped | `crates/sim-leaf/` (U22, d48191f, merged dcf77c8); `scenarios/kv_spiral_swap.txt`; `scenario.proto` `PreemptionPolicy.SwapToDram` | | | 2026-09-06 |
| `preempt_swap_else_recompute` | kv | Swap while the shared bandwidth has headroom, recompute once it saturates | idea | `scenario.proto` `PreemptionPolicy.SwapToDramElseRecompute` | | | 2026-09-06 |
| victim selection: `newest`, `largest_kv`, `lowest_slo_class`, `latest_deadline` | kv | Four victim rules, orthogonal to the mechanism above: least work lost, most freed, batch traffic exists to be preempted, or deadline order | idea | `scenario.proto` `PreemptionPolicy.Victim` | | | 2026-09-06 |
| `tier_discard_only` | kv | Preempted KV and prefixes are discarded; nothing below HBM | idea | `scenario.proto` `KvTieringPolicy.DiscardOnly` | | | 2026-09-06 |
| `tier_dram_then_discard` | kv | Spill to a cluster DRAM pool of `dram_pool_tokens`, then drop the overflow for recompute. Finding 16: a 28k-token pool, a quarter of what swap-everything parks at peak, serves the spiral at 100 % attainment (goodput 1,421 tok/s, ITL p99 70 ms) against 97.6 % for swapping everything | shipped as `dram_pool_tokens` with `ssd_pool_tokens = 0` | `crates/sim-model/src/lib.rs` `Tiers` (U30, 3eb8495, merged 192c6e0); `scenarios/tier_dram.txt`; `scenario.proto` `KvTieringPolicy.DramThenDiscard` | | | 2026-09-06 |
| `tier_dram_then_ssd` | kv | DRAM pool, then an SSD pool of `ssd_pool_tokens` at `ssd_gbps`, every migration through one `fabric_gbps` container at the slower of tier and fabric. Finding 16: a single drive's 10 GB/s drops nothing and loses (goodput 893, 70.3 %, ITL p99 361 ms, a tier slower than the ITL budget is worse than none); behind a 10 GB/s fabric the swap advantage inverts (666 tok/s, 59.6 %, barely above never-swap's 608). No `ssd_stop_below` yet | shipped as `ssd_pool_tokens`, `ssd_gbps`, `fabric_gbps` | `crates/sim-model/src/lib.rs` `Tiers` (U30, 3eb8495, merged 192c6e0); `scenarios/tier_dram_ssd.txt`, `tier_contended.txt`; `scenario.proto` `KvTieringPolicy.DramThenSsd`; `docs/scope-today.md` item 14 | | | 2026-09-06 |
| `bloom_residency_hint` | kv | Ingress owns the tiers and tells the leaf whether a prefix is in DRAM/SSD through a seeded bloom filter with a tuned, deterministic false-negative rate | design of record | `docs/ARCHITECTURE.md` §10.6, §14 (Issao 15:26) | | | 2026-09-06 |
| `rdma_kv_migration` | kv | Move KV between hosts over RDMA (`MigrateKv` intent) rather than swap or recompute, when the destination has HBM headroom | idea | `VISION.md` §2 "RDMA within host, outside"; `policy.proto` `Intent.MigrateKv` | | | 2026-09-06 |
| `weight_locality` | kv | Model weights as tiered objects with cache replacement for multi-model and MoE serving; route to where the weights are | idea | `VISION.md` §3a, §3b "model loading"; `docs/scope-today.md` item 17 | | | 2026-09-06 |

## Prefill/decode disaggregation

Run prefill on one pool and decode on another, shipping KV over the modelled fabric (node-to-node
RDMA ~400 GB/s, ~3.3 ms for a 4k context).

| Name | Family | Idea | Status | Source | Score | Rule set | Added |
|---|---|---|---|---|---|---|---|
| `colocated` | disaggregation | Prefill and decode on the same replica with chunked prefill; today's engine | shipped | `crates/sim-leaf/src/lib.rs` | | | 2026-09-06 |
| `static_split` | disaggregation | Fixed prefill and decode pools; KV transfer cost per request; the pool ratio is the knob | idea | `VISION.md` §3b; `docs/scope-today.md` item 15; primer §4 | | | 2026-09-06 |
| `dynamic_split` | disaggregation | Move replicas between pools on the prefill backlog versus decode ITL; a scaling policy inside one fleet | idea | `docs/ARCHITECTURE.md` §7 "model the split as a policy knob" | | | 2026-09-06 |

## Affinity and prefix

Prefix caching turns routing into a locality problem: least-loaded destroys hit rate, naive affinity
creates hot spots. The prefix topology is derived from a session model with fork-off and merge-back
rates (`docs/ARCHITECTURE.md` §14, Issao 15:26); the tree, the per-replica LRU prefix cache and the
router's `PrefixIndex` seam are U27a (820e752, merged 361d59a), merge-back is not modelled yet.
Findings 11 and 14 measure it.

| Name | Family | Idea | Status | Source | Score | Rule set | Added |
|---|---|---|---|---|---|---|---|
| `sticky_session` | affinity | Route every turn of a session to the replica that served the last one; stickiness is the knob VISION asks about ("how sticky do we need to be") | idea | `VISION.md` §3b | | | 2026-09-06 |
| `prefix_affinity_load_capped` | affinity | The `prefix_affinity` routing policy above, viewed as an affinity strategy: yield to load past a ratio. Finding 14 is its failover cascade: the hottest holder dies with 36 running / 145k KV tokens against p2c's 8 / 39k, and returns to 90 % load in 0.5 s serving misses | shipped | `crates/sim-policy/src/prefix_affinity.rs` (U27b); `scenarios/cascade_affinity.txt` (U27d, e942298, merged 8b2348f); `scenario.proto` `RoutingPolicy.PrefixAffinity`; primer §6 | | | 2026-09-06 |
| `cache_aware_placement` | affinity | Place sessions so that shared prefixes (system prompts, few-shot preambles) concentrate on few replicas without concentrating load | idea | `docs/scope-today.md` item 9; primer §5 | | | 2026-09-06 |
| `affinity_with_failover_spread` | affinity | Affinity in the good case, spread on failover, so a hot spot of failover does not cascade | idea | `VISION.md` §3a; `docs/scope-today.md` item 10 | | | 2026-09-06 |

## Redundancy

VISION.md §1 names redundancy policies as a family; no document proposes a concrete one yet, so these
are the first candidates.

| Name | Family | Idea | Status | Source | Score | Rule set | Added |
|---|---|---|---|---|---|---|---|
| `hedged_request` | redundancy | Send the request to a second replica if the first has not produced a first token by a tail deadline; cancel the loser. Trades capacity for tail latency, and is a retry storm in disguise under overload | idea | `VISION.md` §1; primer §7 tail latency | | | 2026-09-06 |
| `prefix_replication` | redundancy | Keep hot prefixes resident on k replicas so affinity has a fallback that is still warm | idea | `VISION.md` §1; primer §5 | | | 2026-09-06 |
| `n_plus_k_capacity` | redundancy | Hold k replicas of headroom per cluster against failure, sized from the detection latency and turn-up delay | idea | `VISION.md` §1, §3a "Turn up delay" | | | 2026-09-06 |
| `cross_cluster_spill` | redundancy | Spill above rated capacity to another cluster before shedding, with the geo cost | idea | `VISION.md` §1; `docs/arena.md` §3 | | | 2026-09-06 |

## Load forecasting

New family, Issao 2026-09-06. Every upper-layer policy acts on a view that is `telemetry_interval_ms`
plus delay old (finding 2 measures what that costs). A load forecast turns the stale view into an
estimate of the state *at the moment the decision takes effect*. What each needs from the engine is
listed, because most need only what `ReplicaView` and `Workload::rate_at` already expose.

| Name | Family | Idea | Status | Source | Score | Rule set | Added |
|---|---|---|---|---|---|---|---|
| `ewma_offered_load` | load forecasting | Exponentially weighted moving average of offered tokens/s at the gateway, feeding admission thresholds and `reactive_threshold` scaling. Needs: the arrival counter per sample, already in `Series offered_rps` | idea | this file | | | 2026-09-06 |
| `holt_winters_diurnal` | load forecasting | Holt-Winters with a daily seasonal term over offered load, giving `predictive_diurnal` its curve from data rather than configuration. Needs: multi-hour runs, a seasonal workload shape | idea | this file; `VISION.md` §3a diurnal | | | 2026-09-06 |
| `queue_depth_extrapolation` | load forecasting | Per replica, extrapolate queued tokens over the telemetry delay from the last two snapshots and the known drain rate, so routing scores the *predicted* state at arrival, not the stale one. The direct answer to finding 2's herding threshold. Needs: two consecutive `ReplicaView`s and the replica's rated drain rate; O(1). **Shipped as `forecast_load`** (U40a): p2c over a predicted queue depth, slope fitted between the last two distinct views, fewer than two falls back to the stale value; candidates drawn in exactly `p2c`'s order so the two policies see byte-identical candidate sets and any difference is in the scoring; tests hold that, and that it beats p2c under stale telemetry | shipped | `crates/sim-policy/src/forecast_load.rs` (9b4fc5c, merged 9ee9841); `check-fingerprints.sh` 12-forecast-load; finding 2 | | | 2026-09-06 |
| `in_flight_correction` | load forecasting | Each router adds its own routed-but-unreported tokens to the stale view of each replica (what it sent since the last scrape), which removes the self-inflicted part of herding at zero telemetry cost. Needs: a per-router send counter per replica | idea | this file; finding 2 | | | 2026-09-06 |
| `session_continuation_forecast` | load forecasting | Predict near-future load from live sessions: a session in decode will return with a longer prompt, at a rate given by the session model's turn interval. Needs: the session model with fork-off and merge-back rates (§14) | idea | this file; `docs/ARCHITECTURE.md` §14 | | | 2026-09-06 |
| `burst_detector` | load forecasting | Detect a step change in arrival rate within a few samples (CUSUM or a two-rate likelihood ratio) and pre-shed or pre-scale before the queue shows it. Needs: nothing beyond `offered_rps` | idea | this file; `scenarios/holdout/h5-bursty-step.txt` | | | 2026-09-06 |
| `learned_load_model` | load forecasting | A table-driven or fitted model over (hour, tenant mix, recent rate) that the arena's generator can author and tune against the load archive | idea | this file; `docs/arena.md` §2.1 | | | 2026-09-06 |

## Latency forecasting

New family, Issao 2026-09-06. Admission and scheduling decisions that depend on a deadline need an
estimate of the latency a request *will* see, from state that is stale and from a cost that is unknown
at admission (output length). These give `deadline_aware`, `earliest_deadline_first` and SLO-class
scheduling something better than a fixed headroom.

| Name | Family | Idea | Status | Source | Score | Rule set | Added |
|---|---|---|---|---|---|---|---|
| `queue_model_ttft` | latency forecasting | Expected TTFT from queued prefill tokens ahead of the request divided by the replica's prefill rate, plus the chunk schedule; feeds `deadline_aware` in place of `queue_wait_headroom`. Needs: queued tokens per replica (have) and the prefill rate from the cost model (have). **Shipped as `forecast_latency`** (U40b) without the chunk schedule: TTFT forecast as the prefill work queued ahead of the request plus its own prefill, over the fleet's prefill rate, plus one step to join the batch; candidates drawn in exactly `p2c`'s order | shipped | `crates/sim-policy/src/forecast_latency.rs` (9cae0fa, merged 052d40e); `check-fingerprints.sh` 13-forecast-latency | | | 2026-09-06 |
| `kingman_wait_estimate` | latency forecasting | Kingman's G/G/1 approximation for queue wait from utilization and the arrival and service coefficients of variation, per replica, from telemetry only. Needs: arrival and service-time variance per replica in `ReplicaView` | idea | this file | | | 2026-09-06 |
| `batch_state_itl` | latency forecasting | Predict inter-token latency from the current batch size and resident KV through the step-time model (`step_base_ms + step_per_kv_ktoken_ms × KV`), so a request is admitted only where its ITL SLO will hold. Needs: batch size and KV resident per replica (have) | idea | this file; `docs/calibration.md` | | | 2026-09-06 |
| `output_length_predictor` | latency forecasting | Predict output length from prompt features and tenant history, since cost is unknown at admission and `charge_max_tokens` overcharges; the gap is where preemption is born. Needs: per-request prompt features and a per-tenant history | idea | this file; `scenario.proto` `AdmissionPolicy.TokenBucket` comment | | | 2026-09-06 |
| `e2e_deadline_feasibility` | latency forecasting | TTFT forecast plus predicted output length times forecast ITL, compared with the deadline; the input to shedding and to `earliest_deadline_first` | idea | this file | | | 2026-09-06 |
| `tail_forecast_from_windows` | latency forecasting | Forecast p99 of the next window from the last k windowed histograms (the export already carries windowed distributions), for autoscaling on tail rather than mean | idea | this file; `crates/sim-ingress/WIRE.md` | | | 2026-09-06 |
| `learned_latency_model` | latency forecasting | A fitted model over (queue, batch, KV, prompt length) → TTFT and ITL, authored and tuned by the arena generator against the load archive | idea | this file; `docs/arena.md` §2.1 | | | 2026-09-06 |

## Not yet a policy, but a knob

Scenario parameters that get swept, not policies that decide. Filing these as policies would let the
arena "win" by tuning the simulator.

- `step_token_budget` (chunk size), finding 3
- `telemetry_interval_ms` and telemetry delay, finding 2
- `arrival_rps`, `long_probability`, `max_batch`, `kv_tokens`, `sample_interval_ms`
- `retry_budget`, `retry_attempts`, client timeout (client behaviour, finding 6; a policy only once it reads fleet state)
- `disable_decode` (infinite HBM bandwidth; VISION.md §3a), being added
- `probe_live` on `p2c` (a cost switch, not a decision)
- `trace_sample_rate`, being added
