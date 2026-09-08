# VISION.md, requirement by requirement: done, building, far away

**Temporary.** Snapshot taken 2026-09-07 21:31 PDT from `origin/master` 2eb5d4f, corrected 21:38 PDT against f89c666 (six merges landed
meanwhile: U94b, U92b, U97, the replica-state proto, Issao's sharding and mock decisions), refreshing the one of 2026-09-06 16:19
(f263575) for Issao, who asked: *"what is missing in the vision progress, is that up to date? What else can we make
progress there?"* Every row cites its evidence: a commit on `origin/master`, a finding number in `docs/findings.md`, a
scenario, a test, a page on <https://lbsim.ai>, or a unit id in `docs/execution-graph.md` (the tech lead's memory:
87 done, 8 in flight, 17 queued at its 21:35 line). "Done" means the
requirement is met as VISION.md states it, on `master`, verified; a partial is Building or Far away with what exists
noted. Queue positions are the graph's "Queue at the stop" order: U31b, U25b, S3, U69, then the engine roadmap U27,
U30, U32, U33, U29/U36/U37, U38, U39, with U98 and U99 beside them; U29 was taken out of every lane and queue by Issao at 21:32.

Legend: **D** done, **B** building (an agent is on it, or it has a unit id in the graph's queue; the evidence says
which and where in the queue), **F** far away (no unit in any queue, or behind a foundation that has none).

## §1 Pitch and the two goals

| Requirement | | Evidence |
|---|---|---|
| Simulator of a large-scale LLM serving system with fidelity to reproduce insightful dynamics | B | twelve dynamics reproduce on real runs: findings 1–9 in `docs/findings.md`, reports 1–12 at lbsim.ai/reports/ (7 no-decode, 8 admission, 9 fair-share, 10 probes, 11 preemption, 12 spec-decode); default fleet 256 replicas (U92, f95d0ce). Scale measured (`docs/wrap-up-2026-09-06.md` §5d): 1,000 replicas 57× realtime, 10,000 replicas (80k GPUs) 1.5–1.9× on one core. "Large scale" is one cluster; the multi-cluster dimension is U32, queued |
| Learn the real-world challenges of LLM serving | D | `docs/llm-serving-primer.md`, `docs/calibration.md`, nine findings each stating the mechanism, "What these nine have in common" |
| Learn publicly available SOTA serving information | D | `docs/calibration.md`: H100 HBM, prefill tok/s, NIM batch-1 decode, Azure trace dispersion; `check-sensitivity.sh` shows the ordering survives ±30% |
| Playground to teach others | D | lbsim.ai/#/showcase: 22 cards, 14 scripted walkthroughs (`web/public/walkthroughs/`) that open a live run and narrate it step by step (U48 c16675b, U49, U58 9bfe832, U75); 8 phase-2/3 cards are placeholders marked "coming" until their dynamics exist (U27, U30, U31b, U32, U39) |
| Evaluate policies across the stack: GPU/host/node scheduling, LB, affinity, shaping, redundancy | B | LB: eight routing policies (`crates/sim-policy/src/`: round_robin, random, least_requests, least_queue_tokens, p2c, least_kv_probe, forecast_load, forecast_latency); shaping: three admission policies (accept_all, deadline_aware, fair_share); GPU-level scheduling: knobs only (`step_token_budget`, `max_batch`, `preemption`, `preemption_victim`, spec N/M), no scheduling policy trait; affinity: U27 queued (roadmap first); redundancy: four catalog ideas, no unit |
| Scorecard metrics: throughput, SLO, goodput, service quality | D | `RunResult` scorecard plus per-class goodput and attainment since U25a (5a4f185); `summary.csv` carries `gpu_utilization_mean` since U94b (a8cc5ec); U25b (queued 2nd) puts the per-class rows in the report table |

## §2 Why: cheap, fast, hermetic, usable by LLM agents

| Requirement | | Evidence |
|---|---|---|
| Hermetic, cheap, fast | D | zero-dependency Rust workspace; the 256-replica base runs at 180–235× realtime on one core (U92 report); `tools/build.sh test --workspace` |
| A playground for LLM agents to evaluate policies | D | the loop is closed: `sim-run generate --family routing --variant …` writes a policy file, rebuilds, scores a round, appends the catalog row (U34 fe0126f; `tests/arena_generator.rs`); the generator today is a template (p2c variants), so no LLM has yet been given the "free reins" of §10, which is a session to run, not a unit to build |

## §3a Dynamics to understand

| Dynamic | | Evidence |
|---|---|---|
| Round-robin rolling hotspot with heterogeneous sizes, below rated capacity, "early on" | D | finding 1, `scenarios/route_round_robin.txt` vs `route_p2c.txt`, report 1, walkthrough `rolling-hotspot` live |
| Knob to turn off decode so traffic looks stateless | D | `disable_decode` scenario key (U21 c7f8c6a), `scenarios/route_*_no_decode.txt`, report 7, finding 7: the routing ordering holds with the decode physics off, so the effect is the queue's |
| Stale-state LB oscillation, time/frequency-domain, control-theory framing | B (partial D) | finding 2 measures herding against staleness with the dominant frequency (`Series::dominant_frequency`), demo 2 re-baselined at 32 replicas (U92b bff97f2) because at 256 the herd is past the cliff at 100 ms; that cliff moving with fleet size is U98 (queued, demo 13); the perturbation input, Bode plot and controller policy are U33 (queued, after U32) |
| Value of policies with robust control logic | B | U33 queued; first steps landed as the two stale-view-compensating routing policies `forecast_load` (U40a 9b4fc5c) and `forecast_latency` (U40b 9cae0fa); `robust_controller` and `stale_view_shaping` are catalog ideas |
| Global cascading failure | B | failure injection is in the engine (U31a e755f67: `failures = t=60,replica=2,kind=slow=0.3`, silent slow, hang, announced crash; `scenarios/crash.txt`, `gray_failure.txt`; `tests/failures.rs`); U31b ejection is first in the queue; the global half needs multi-cluster, which is inside U32 (queued 7th) |
| GPU utilization vs latency when tuning prefill/decode; large-prefill traffic | D | findings 3 and 5, reports 3 and 5; GPU utilization now a metric with p50/p90/p99 bands across replicas on the Utilization page (U94a 5d9732c, U94b a8cc5ec, U94c 599c41c) |
| KV-cache preemption pathology: many sessions, low qps, severe degradation | D | U22 (d48191f): preemption with swap-to-DRAM or recompute, victim rules, multi-turn sessions (`session_turns_mean`, `session_think_s`, `dram_capacity_tokens`, `swap_gbps`); U62 (51968a7) session turns through admission; finding 8 the KV spiral, report 11, walkthrough `kv-spiral` live; `tests/preemption.rs`. The "connect with cascading failure" clause is not planned anywhere |
| Value of traffic shaping at each layer, in every scenario | B (partial D) | `deadline_aware` and `fair_share` on master with reports 8 and 9 and live walkthroughs; the per-layer contrast report is U38 (queued 10th); router and replica layers shape by `max_queue` only; `layered_shaping` is a catalog idea |
| Model-weight locality, MoE, multi-model | B | U39 is the last name in the roadmap queue, "none started; becomes a section when specified"; catalog `weight_locality`; needs U30 tiering |
| Multi-geo diurnal autoscaling with on-demand allocation and spike absorption, as robust control | B | U32 queued (7th): replica lifecycle with turn-up delay, warm pools, diurnal load per cluster; the proto has `ClusterSpec` with a diurnal phase offset (`scenario.proto:121`) but the scenario parser has no cluster key, so a second cluster cannot be expressed today |
| Traffic with different latency SLOs | B (partial D) | U25a (5a4f185): `slo_classes = interactive:0.7,agent:0.2,batch:0.1`, targets per U42, per-class goodput and attainment on `RunResult`, `tests/slo_classes.rs`; U25b (queued 2nd) surfaces them in the report and export; class-aware scheduling (`slo_class_priority_queues`, `earliest_deadline_first`) is catalog-only; the batch class's longer-window throughput is U44, recorded and unscheduled at your word |
| Speculative decoding with a stochastic small-model agreement | D | U26 (6fd648e): `spec_draft_tokens`, `spec_accept_rate` through `CostModel`; finding 9 (2.61× at batch 4, 0.92× at 144, crossover B≈72); report 12, demo 12 walkthrough `spec-decode` (U26b b827d84); `tests/spec_decoding.rs` |
| Affinity: good case vs failover hotspot cascade | B | agents on it since 21:32: U27a (`claude/tl-prefix-engine`: prefix tree, per-replica prefix cache, session forks, the router seam) and U27b (`claude/tl-prefix-policy`: `prefix_affinity`, scenarios `affinity_off/spread/sticky`, demo 14); the failover-cascade half still needs U31b; the `affinity-vs-spread` walkthrough script exists against today's engine, `affinity-cascade` is a placeholder card |

## §3b Policies to evaluate

| Policy family | | Evidence |
|---|---|---|
| Load balancing | D | eight routing policies, each a file behind `RoutingPolicy` (`crates/sim-policy/src/routing.rs:88`), registry generated from the files (5083b7b) |
| Batch scheduling, prefill batch sizing | F (partial D) | `step_token_budget` and `max_batch` are knobs (finding 3); there is no `SchedulingPolicy` trait (only `AdmissionPolicy` and `RoutingPolicy` exist in `crates/sim-policy`), the engine's step loop decides; `n_requests_m_batches` and `step_budget_controller` are catalog ideas with no unit |
| Speculative decoding | D | see §3a |
| Load forecasting | D | `forecast_load` (U40a 9b4fc5c; `scenarios/route_forecast_load.txt`, `tests/policy_forecast_load.rs`); a seven-idea family in `docs/policy-catalog.md` |
| Latency forecasting | D | `forecast_latency` (U40b 9cae0fa; `scenarios/route_forecast_latency.txt`, `tests/policy_forecast_latency.rs`); a seven-idea family in the catalog |
| Machine failover | B | injection done (U31a); the ejection policy on the health vector (`ReplicaView.ejected`, `last_step_ns` vs fleet median) is U31b, first in the queue, with demo 13 and its finding |
| Prefill-decode disaggregation | B | U30 queued (6th): prefill and decode pools with KV transfer over the modelled fabric; `static_split`, `dynamic_split` in the catalog |
| Prefill-decode batch co-scheduling | F (partial D) | chunked prefill is the co-scheduling and it is fixed, not pluggable; same missing seam as batch scheduling; `prefill_decode_co_scheduling` is a catalog idea with no unit |
| Affinity, how sticky | B | U27a/U27b in flight (`affinity_max_load_ratio`, `affinity_fallback_choices` are the stickiness knobs); `sticky_session`, `prefix_affinity_load_capped` in the catalog |
| Load shaping with stale cluster status | D | admission and routing read the delayed view (`telemetry_interval_ms`, `telemetry_delay_ms`); finding 2; `forecast_load` compensates for it |
| Auto scale | B | U32 queued; `reactive_threshold`, `predictive_diurnal`, `warm_pool`, `robust_controller` in the catalog; every run so far is at fixed fleet size |

## §4 What "realistic" means

| Requirement | | Evidence |
|---|---|---|
| Batch-level realism computed analytically; work scales with requests, not tokens | D | analytic epoch advance exact against a rational-arithmetic oracle (`bench/validate_epochs.py`, Rust port U14 dbfc553); ~30 events per request, 1.1 M events/s at 10,000 replicas (§5d) |
| Request cohorts / fluid limit | F | parked by decision (`docs/ARCHITECTURE.md` §4) with the knob table (`cohort_size` 1 … ∞ = fluid) and three triggers; trigger 1, "cannot reach 2× on one core at target scale", is now measurably close (1.5–1.9× at 10k replicas), and the chosen lever is Leaf sharding (U29/U36), not cohorts |
| KV data locality: HBM/DRAM/SSD, where preempted KV lives | B (partial D) | one DRAM tier exists as the swap target since U22 (`dram_capacity_tokens`, `swap_gbps`); cluster-pooled DRAM/SSD owned by Ingress with the bloom-filter residency hint (§14 rows 4, 8) is U30, queued |
| KV prefix modelling, feasibility documented | B (design D) | feasibility in `docs/ARCHITECTURE.md` §7.3 and `docs/calibration.md` §9; the session model chosen (§14 row 9); code is U27a/U27b, in flight since 21:32 |
| Model weights locality | B | U39, last in the queue, unspecified |
| Stale status from machines to upper layers, delayed actuator | D | telemetry publish/delay events; finding 2; the crash in U31a is announced only through the delayed view |
| Turn-up delay for autoscale | B | inside U32 |
| Failure discovery delay, gray failure vs clean shutdown | D (engine) | U31a: `slow` is silent (gray), `hang` is speed 0, `crash` is announced, `until` restores; the detector that turns discovery delay into a number is U31b, first in the queue |
| Calibrate against SOTA NVIDIA cluster shapes and published workload data, user-configurable | D | `docs/calibration.md`; every cost-model number is a scenario key; `check-sensitivity.sh` is a regression test on the ordering; a real trace can replace the synthetic mixture (`workload = trace`, `trace_file`, U35 ad4ff74) |

## §5 Non-goals

All honoured: no RPC hook-in, no billing model, no training, no kernel or packet modelling. The "small simulator
footprint" tension is now measured rather than unmeasured: 580 MB peak RSS at 10,000 replicas, and the cloud backend
was resized to 2 vCPU / 2 GiB with a memory budget so that run fits (9dd03e9).

## §6 Success criteria and surfaces

| Requirement | | Evidence |
|---|---|---|
| Each key dynamic shown in a dashboard with strong visuals at every relevant layer | D | twelve static reports; the live dashboard streams fleet series, per-replica rows on the Machines page (U90 258c706), GPU and KV utilization with percentile bands (U94), service quality; a panel for a dynamic the engine does not simulate yet says so in plain words under its mode tag (U95); U95b (in flight) and U100 (queued) remove every invented number and the mock mode itself. Exception: the Traces tab, see §8 |
| A/B comparison between policy sets | D | `sim-run compare` and findings 1, 6; the A/B page runs two live runs at one seed differing in policy, or a walkthrough's recorded pair (U96 b9e8b10); lockstep through `StepForward` is U99, queued |
| Home page listing surfaces | D | lbsim.ai in three sections, Live / Replay / Mock (U80 023672d), reports 1–12; the Mock section goes with U100 (queued, Issao 21:35: *"remove all invented numbers everywhere"*) |
| Interactive load-test control: load shape, cluster shape, policies, all parameters on the fly | D | `UpdateWorkload`/`UpdatePolicies` apply to a running simulation, forward-only (U59a 7470b54, U59b 11165ba); engine-only keys ride on `ScenarioConfig.extra` (U72); the replica slider reaches 10,000 and the banner reports the pace achieved (U91); a physics change is refused with the reason and needs a new run |
| Observability panel: time series at cluster and machine level | D | fleet and per-replica subscriptions over SSE (U18 c0e9ea8, U90); heatmap and replica table live and on replay (U23 5e14b23) |
| Showcase: scenario cards with preloaded settings and a step-by-step "scenario play" | D | 14 scripted walkthroughs over 22 cards; a walkthrough opens a live run, its `set` steps change the run mid-flight, the open walkthrough lives in the URL (U48, U49, U58, U59b, U75); eight cards wait on their dynamics |
| Standalone simulator as an agent playground | D | `sim-run run / compare / sweep / export / generate / serve`; the arena round and catalog (`crates/sim-arena`, U20, U34) |

## §8 Constraints and preferences

| Requirement | | Evidence |
|---|---|---|
| Dependency-light core | D | zero external crates in the workspace |
| Deterministic, one global seed, named streams | D | xoshiro256++ streams; `tests/determinism.rs`, `tests/stream_independence.rs`; `bench/golden-fingerprints.txt` gates every integration (`tools/integrate.sh` stage 4), union-mergeable since U66 |
| Periodic snapshots for scroll-back / replay; fast-forward and rewind in the UI | F (partial D) | fast-forward: `StepForward` is implemented (`crates/sim-ingress/src/run.rs:197`), pause and speed apply live, replay scrubs any exported run; the resumable `Sim` and the idle guard's checkpoints exist (U15, `run.rs:79`). Rewind: **`Rewind` answers HTTP 501** (`crates/sim-ingress/src/server.rs:315`) and the UI hides it off-mock; no unit plans re-simulation from a checkpoint |
| Trace single requests, sampled uniformly and by latency bucket, across machines | F (partial D) | the engine records per-request spans with resource state at each step for a seeded sample stratified by p50/p90/p99/p99.9 and outcome (U24 a3b5141, `TraceSpan` extension a035514 awaiting your review, `tests/trace_engine.rs`), and the export carries a budgeted trace set (U19 c766e2f, `export_run_with_traces`). What remains: **`GetTraces` answers 501 on the server** (`server.rs:315`; `WIRE.md:127` still says "until U24 lands"), and the dashboard's Traces tab reads `web/src/lib/traces.ts`, which invents traces from the seed rather than decoding the wire (`decodeTraceSpan` exists in `api.ts:604`, unused by the tab). No unit for either; U69 (queued 4th) adds the eviction span |
| Proto-defined interfaces, written or reviewed by you | D | twelve files in `proto/lbsim/v1/`; additions since the last snapshot: `TraceSpan` resource state (a035514, `TASKS.md` item 1), GPU metrics 67/68 (b92ab52). Gap reported by U96: `RunStatus` carries no seed |
| A TODO file for you | D | `TASKS.md` |
| Standalone dashboard as the final result | D | lbsim.ai, revision `lbsim-00016-nbc` at the last deploy; the U90–U96 wave awaits its READY |
| ≥2× realtime for 5 clusters × 10k GPUs, ideally 10× that | B (partial D) | measured for the first time (§5d, U37's first numbers): 8k GPUs at 57×, 80k GPUs at 1.5–1.9× on one core, at ~0.3 offered/capacity; the event ceiling now scales with the run (U97 a13faf2). Not met as stated: one cluster, not five. Issao at 21:32: *"let's not do sharding then, leave it recorded for future work"*, because the target is 6,250 replicas and one core already does 10,000 at 1.5–1.9×; what remains is U37's measurement at 6,250 and the multi-cluster dimension (U32) |
| Sharded service on GCP, ≤10 backend replicas | B (partial D) | Cloud Run, `--max-instances 10`, `--min-instances 0`, session affinity because a run lives in one instance (be30de6), 2 vCPU / 2 GiB (9dd03e9), the request-scoped-CPU idle rule in `WIRE.md`; one service, not sharded, and sharding is recorded as future work per Issao (U29, U36 out of every queue at 21:32) |
| Rust backends, Node + React frontend | D | as built |
| Static HTML report with interactive links to replay | F (partial D) | twelve reports; **no report carries a link** (`crates/sim-report/src/lib.rs` emits no anchor); the target exists, since the dashboard opens an exported run by id and a walkthrough by URL (U75). No unit |

## §10 Open questions

| Question | | Evidence |
|---|---|---|
| Request cohorts, pressure-tested | D (decided) | parked with a trigger and a knob, `ARCHITECTURE.md` §4; see §4 above for the trigger's status |
| Engine/policy separation with a physics referee so policies cannot cheat | F (partial D) | policies see only delayed `ReplicaView`/`RequestView`; the engine owns physics; the arena's realism envelope checks outputs (`crates/sim-arena`); the strict-mode referee auditing a policy's *proposed* schedule is named in `docs/execution-plan.md` M5 and has never had a unit; with no scheduling seam there is nothing for it to audit yet |
| Battle arena for agents, after realism is validated | D (mechanics) | rule set v2 (U20, U43 default stands), the generator loop (U34), the catalog; the round where Claude is given free rein on a metric has not been run |

## Counts

62 rows.

- **Done: 35** (was 19). Since the last snapshot: `disable_decode`, preemption and sessions, speculative decoding, both
  forecasting families, failure injection in the engine, SLO classes in the engine, the live Ingress server, on-the-fly
  controls, the live showcase and A/B, machine-level and GPU-utilization panels, the arena generator loop, trace replay
  workloads, the home page.
- **Building: 20** (was 22), every one a unit id in the graph's queue, in this order: U31b (ejection, failover, gray-failure
  finding), U25b (per-class rows), U69 (eviction span), U27a/U27b (prefix, sessions, affinity; in flight since 21:32), U30 (tiers,
  disaggregation), U32 (autoscaling, multi-geo, turn-up delay, global cascade), U33 (Bode, robust control), U37 (scale
  validation at 6,250 replicas; U29/U36 sharding is future work per Issao), U38 (shaping contrast), U39 (model weights,
  MoE); plus U98, U95b, U100 and U101 on the surface side. Agents are on U27a, U27b, U98, U97b, U99, U95b and U101 tonight.
- **Far away: 7** (was 17): the batch-scheduling policy seam and co-scheduling behind it, cohorts/fluid (parked), rewind on
  the server, traces end to end (server RPC and the Traces tab), the strict-mode referee, report links to replay. None is
  behind a missing foundation any more; each is missing a unit.

## VISION.md requirements that no plan document mentions

Compared against the last snapshot's eight (`docs/ARCHITECTURE.md`, `docs/execution-plan.md`, `docs/execution-graph.md`,
`docs/scope-today.md`, `docs/dashboard-plan.md`, `docs/arena.md`, `docs/policy-catalog.md`, `TASKS.md`):

Resolved since 2026-09-06: **`disable_decode`** (built, U21), **load forecasting** and **latency forecasting** (built, U40a/b,
and each a catalog family), **model-weight locality / MoE** (now U39 in the queue, a name without a specification, and
`weight_locality` in the catalog), **the fluid limit** (now the ∞ end of the `cohort_size` table in `ARCHITECTURE.md` §4.2,
with a trigger).

Still in no plan document:

- **Sampled request traces, the remaining half** (§8): the engine samples by latency bucket and records per-machine
  spans, but `GetTraces` is 501 on the server and the Traces tab invents its data. No unit names either.
- **Rewind** (§8 "fast forward, rewind"): 501 on the server, hidden in the UI, no unit. The idle guard already checkpoints a
  run, so the mechanism is re-simulation from the nearest checkpoint.
- **Static report links to replay** (§8): no unit; a one-line anchor per report row to the dashboard's run-by-id URL.
- **Redundancy policies** (§1): four ideas in the catalog's Redundancy section (`hedged_request`, `prefix_replication`,
  `n_plus_k_capacity`, `cross_cluster_spill`), no design, no unit.
- **A batch-scheduling policy seam** (§3b, §4 "N requests, M batches"): catalog ideas only; every scheduling row above is
  gated on it, and the M5 referee has nothing to audit without it.
- **Connecting the KV-preemption pathology with a cascading failure** (§3a): both halves exist or are queued; the
  combined scenario is nowhere.
- **The §5 question**, whether production mixes training and serving: still answered only here. In practice large
  operators keep them on separate fleets; some share nodes off-peak for fine-tuning, but latency-sensitive serving and
  training are not co-scheduled on the same GPUs.

Named in a plan document but never given a unit: the strict-mode physics referee (`execution-plan.md` M5); U44, the
batch class's long-window throughput, recorded at your word as future work.

## What we can make progress on next: the ten most valuable items

In the order the execution graph's queue would take them, each with the dependency it needs, so you can re-rank.
Items 9 and 10 are not in the queue; a suggested slot is given.

1. **U31b, outlier ejection and machine failover** (queue 1st). Needs nothing: U31a's health vector and `ejected` flag
   are on master. Unlocks failover, the gray-failure finding with numbers, and demo 13; the cascade and affinity items
   below all cite it.
2. **U27a/U27b, prefix caching, sessions, affinity** (in flight since 21:32, two agents). Needs U22 (done); the
   failover-cascade half needs U31b and is in neither brief. Two VISION dynamics (affinity good case, hotspot cascade), two policy families (affinity, how sticky),
   and it turns two placeholder showcase cards into live ones.
3. **U30, memory tiering and prefill/decode disaggregation** (roadmap 2nd). Needs the swap-to-DRAM tier from U22 (done)
   and the Ingress-owned pooled tiers already designed (§14 rows 4, 8). Three VISION rows: KV locality, disaggregation,
   and the tiering showcase card.
4. **U32, replica lifecycle, turn-up delay, autoscaling, multi-geo** (roadmap 3rd). Needs U31b and a `clusters` scenario
   key, which does not exist (the proto's `ClusterSpec` does). The largest single item, and the only route to "global"
   in global cascading failure and to the five-cluster scale target.
5. **U33, the Bode plot and a robust-control policy** (roadmap 4th). The graph puts it after U32, but the single-fleet
   version needs only U15 (done): a perturbation input on `arrival_rps`, a frequency sweep, the empirical transfer
   function. This is the thesis of VISION §3a and could be pulled ahead of U30 and U32 without conflict.
6. **U37, scale validation at the target, 6,250 replicas** (roadmap 5th; U29/U36 sharding is future work per Issao at
   21:32). Needs U97 (landed) and U97b (in flight, the memory budget from the environment). One measured run at 6,250
   replicates the §8 claim on one core; the five-cluster shape then depends only on U32.
7. **U38, the shaping contrast report** (roadmap 6th). Needs U31b for the failure layer; the admission policies are
   done. It is the row "value of traffic shaping at each layer, in every scenario", which every dynamic cites.
8. **U39, model weights and MoE** (roadmap 7th, unspecified). Needs U30. Lowest in VISION's own words ("if we have
   time"), and last here.
9. **Traces end to end** (not queued; suggested between U25b and U69, since U69 edits the same recorder). Needs
   nothing: implement `GetTraces` over the recorder the engine already fills, decode it in the Traces tab in place of
   `traces.ts`. Small, and the single VISION §8 requirement whose engine half is done while its visible half is
   invented.
10. **Rewind** (not queued; suggested after U99, which builds the stepped-run switch it shares). Needs the checkpoint
    the idle guard already writes and the resumable `Sim`: re-simulate from the nearest checkpoint to the requested
    time. With it, the §8 "fast forward, rewind" row and the "links to replay" row both close.

Also queued and cheap, outside the ten: U25b (per-class rows, 2nd), U98 (the staleness cliff against fleet size, demo
13), U99 (lockstep A/B), U69 (eviction span). Not queued and cheap: the report anchors (one line per row in
`sim-report`), the §5 answer into `docs/llm-serving-primer.md`.
