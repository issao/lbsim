# VISION.md, requirement by requirement: done, building, far away

**Temporary.** Snapshot taken 2026-09-06 16:19 PDT from `origin/master` f263575 for Issao to review; to be
deleted or folded into `STATUS.md` once read. Every row cites its evidence. "Done" means the requirement is
met as VISION.md states it, on `master`, verified; a partial is Building or Far away with what exists noted.
ETAs are the tech lead's, wall clock, conditional on agents landing at today's rate.

Legend: **D** done, **B** building in the next ~2 hours, **F** far away.

## §1 Pitch and the two goals

| Requirement | | Evidence |
|---|---|---|
| Simulator of a large-scale LLM serving system with fidelity to reproduce insightful dynamics | B | six dynamics reproduce (`docs/findings.md` 1–6) on a 32-replica fleet; "large scale" is unmeasured, see §8 |
| Learn the real-world challenges of LLM serving | D | `docs/llm-serving-primer.md`, `docs/calibration.md` (SOTA numbers you asked for), six findings each stating the mechanism |
| Learn publicly available SOTA serving information | D | `docs/calibration.md`: H100 HBM, prefill tok/s, NIM batch-1 decode 10.25 ms, Azure trace dispersion; cost model matched to it |
| Playground to teach others | B | showcase page exists with mock data (`web/src/pages/Showcase.tsx`); real runs in the dashboard ~19:00 (`docs/dashboard-plan.md`) |
| Evaluate policies across the stack: GPU/host/node scheduling, LB, affinity, shaping, redundancy | B | LB: six routing policies; shaping: admission seam + two policies on branches (`tl-policy-deadline`, `tl-policy-fair-share`); GPU-level scheduling: chunk budget and batch cap only; affinity, redundancy: F (see §3b) |
| Scorecard metrics: throughput, SLO, goodput, service quality | D | `RunResult` scorecard: goodput, throughput, completed rps, SLO attainment, outcomes, TTFT/ITL/E2E/queue-wait percentiles; `summary.csv` per run |

## §2 Why: cheap, fast, hermetic, usable by LLM agents

| Requirement | | Evidence |
|---|---|---|
| Hermetic, cheap, fast | D | zero-dependency Rust workspace; a 600 s, 32-replica run in seconds; `tools/build.sh test --workspace` |
| A playground for LLM agents to evaluate policies | B | arena mechanical half (`crates/sim-arena`, `docs/arena-implementation.md` §3 measured round); `GeneratedPolicy` proto slot (947649b) and one-file-per-policy registry (289cb22) so a generated policy is a file; the generator/judge loop itself is not built, queued, no ETA |

## §3a Dynamics to understand

| Dynamic | | Evidence |
|---|---|---|
| Round-robin rolling hotspot with heterogeneous sizes, below rated capacity, "early on" | D | finding 1, `scenarios/route_round_robin.txt` vs `route_p2c.txt`, report `1-routing.html` live at lbsim.ai |
| Knob to turn off decode so traffic looks stateless | F | **no `disable_decode` key exists** (`crates/sim-scenario`); `output_mean` can be set small but that is not the knob asked for. Small; not in any brief |
| Stale-state LB oscillation, time/frequency-domain, control-theory framing | B (partial D) | finding 2 reproduces herding vs telemetry staleness with the dominant frequency measured (`Series::dominant_frequency`); the perturbation input, Bode plot and robust-control policy (execution plan M7) are **not started** |
| Value of policies with robust control logic | F | depends on M7 above; no controller policy exists |
| Global cascading failure | F | depends on failure injection (scope item 11) and multi-cluster (item 12); neither started |
| GPU utilization vs latency when tuning prefill/decode; large-prefill traffic | D | finding 3 (chunk budget sweep, `3-chunking.html`), finding 5 (long-context mixture, `5-long-context.html`) |
| KV-cache preemption pathology: many sessions, low qps, severe degradation | B | KV is a token budget with queue-on-full today (finding 5); preemption with swap/recompute and sessions are queued behind the engine-core unit (`claude/tl-engine`, frames landed 16:15); tech lead's order puts it first after the physics oracle; no ETA stated |
| Value of traffic shaping at each layer, in every scenario | B | admission seam on `master`; `deadline_aware` and `fair_share` on branches (16:17); a per-layer contrast report is not written; shaping at the router and replica layers is `max_queue` only |
| Model-weight locality, MoE, multi-model | F | scope item 17, "if we have time"; nothing started; depends on tiering (item 14) |
| Multi-geo diurnal autoscaling with on-demand allocation and spike absorption, as robust control | F | scope item 13; depends on replica lifecycle and turn-up delay (§4), multi-cluster, and M7 control analysis |
| Traffic with different latency SLOs | B | queued as "SLO classes" in the tech lead's fan-out; single `ttft_slo_ms`/`itl_slo_ms` today; `docs/arena.md` §5b argues per-class targets are needed for a 0.99 cap; no ETA |
| Speculative decoding with a stochastic small-model agreement | B | the N/M model is exact in Python (`bench/validate_epochs.py`) and being ported in `claude/tl-physics` (16:18); wiring it into the engine as a scenario knob is queued (scope item 16, "30 minutes once 7 exists") |
| Affinity: good case vs failover hotspot cascade | F | scope items 9, 10; prefix model decided (§14 row 9: session model with fork/merge rates) but nothing built; depends on preemption and sessions |

## §3b Policies to evaluate

| Policy family | | Evidence |
|---|---|---|
| Load balancing | D | round_robin, random, least_requests, least_queue_tokens, p2c, least_kv_probe (`crates/sim-policy/src/`), O(1) each per §10.4 |
| Batch scheduling, prefill batch sizing | B (partial D) | chunked prefill `step_token_budget` and `max_batch` are knobs (finding 3); a pluggable batch-scheduling policy seam does not exist; the engine decides |
| Speculative decoding | B | see §3a |
| Load forecasting | F | not in any document beyond VISION.md |
| Latency forecasting | B (partial) | `deadline_aware` sheds on *expected* queue wait, which is a one-step latency forecast; a forecasting policy family is not designed |
| Machine failover | F | depends on failure injection (item 11) |
| Prefill-decode disaggregation | F | scope item 15, 2 h estimate, depends on tiering/fabric model |
| Prefill-decode batch co-scheduling | B (partial D) | co-scheduling is what chunked prefill does today; the policy is fixed, not pluggable |
| Affinity, how sticky | F | items 9, 10 |
| Load shaping with stale cluster status | D | admission and routing read the delayed telemetry view (`telemetry_interval_ms`, `telemetry_delay_ms`); finding 2 |
| Auto scale | F | item 13 |

## §4 What "realistic" means

| Requirement | | Evidence |
|---|---|---|
| Batch-level realism computed analytically; work scales with requests, not tokens | D | analytic epoch advance, exact against a naive oracle in rational arithmetic (`bench/validate_epochs.py`), 56–82× fewer iterations; Rust port with the same oracle on `claude/tl-physics` |
| Request cohorts / fluid limit | F | **parked by decision** (§14 row 2): trigger and knob recorded in `docs/ARCHITECTURE.md` §4; not built |
| KV data locality: HBM/DRAM/SSD, where preempted KV lives | F | design decided (§14 rows 4, 8: pooled per cluster, Ingress-owned, bloom-filter residency hint); scope item 14; nothing built |
| KV prefix modelling, feasibility documented | B (design D) | feasibility documented in `docs/ARCHITECTURE.md` §7.3 and `docs/calibration.md` §9; §14 row 9 chose a session model; code not started (item 9) |
| Model weights locality | F | item 17 |
| Stale status from machines to upper layers, delayed actuator | D | telemetry publish/delay events in the engine; finding 2 |
| Turn-up delay for autoscale | F | with item 13 |
| Failure discovery delay, gray failure vs clean shutdown | F | item 11; the mock dashboard shows gray-failure events, the engine has none |
| Calibrate against SOTA NVIDIA cluster shapes and published workload data, user-configurable | D | `docs/calibration.md`; every cost-model number is a scenario key (`step_base_ms`, `step_per_kv_ktoken_ms`, `prefill_tokens_per_s`, `kv_capacity_tokens`); `check-sensitivity.sh` shows ordering survives ±30% error |

## §5 Non-goals

All honoured: no RPC hook-in, no billing model, no training, no kernel or packet modelling. The only
tension is with "small simulator footprint": unmeasured at target scale, see §8.

## §6 Success criteria and surfaces

| Requirement | | Evidence |
|---|---|---|
| Each key dynamic shown in a dashboard with strong visuals at every relevant layer | B | today: six static HTML reports with charts (live at lbsim.ai/reports/); in the React dashboard, real data ~19:00 pre-baked, live ~22:30 (`docs/dashboard-plan.md`) |
| A/B comparison between policy sets | D (reports) / B (dashboard) | `sim-run compare` and findings 1, 6; `web/src/pages/Compare.tsx` exists on mock data |
| Home page listing surfaces | D | lbsim.ai `/`, with report links since 16:03 (16daf22) |
| Interactive load-test control: load shape, cluster shape, policies, all parameters on the fly | B | control panel built on the mock (`web/src/panels/ControlPanel.tsx`); on-the-fly changes need the live Ingress (`UpdateWorkload`/`UpdatePolicies` in `WIRE.md`), ~21:00 |
| Observability panel: time series at cluster and machine level | B | panels exist on mock; fleet series real ~19:00, per-replica heatmap after `claude/tl-engine` frames merge; prefix/tier/failure panels stay mock until those dynamics exist |
| Showcase: scenario cards with preloaded settings and a step-by-step "scenario play" | B | `Showcase.tsx` + `walkthrough.ts` on mock; one real walkthrough was the M0.5 goal; wiring to real runs follows the pre-baked path |
| Standalone simulator as an agent playground | B | CLI `sim-run run/compare/sweep` is standalone today; the arena loop is the missing half (§2) |

## §8 Constraints and preferences

| Requirement | | Evidence |
|---|---|---|
| Dependency-light core | D | zero external crates in the workspace |
| Deterministic, one global seed, named streams | D | `sim-core/rng.rs` xoshiro256++ streams; `tests/determinism.rs`, `tests/stream_independence.rs`; fingerprints in `bench/golden-fingerprints.txt` |
| Periodic snapshots for scroll-back / replay; fast-forward and rewind in the UI | B | proto `Rewind`/`StepForward` defined; snapshots designed (`ARCHITECTURE.md` §8.2); the resumable `Sim` is the engine-core unit in flight (`claude/tl-engine`); UI scrub on pre-baked runs ~19:00, true rewind with re-simulation ~22:00 |
| Trace single requests, sampled uniformly and by latency bucket, across machines | B (partial D) | per-request records exist (arrival, admit, first token, finish, outcome) and the budgeted telemetry dump is stratified by outcome/latency with a manifest (`sim-report`); **no per-machine span trace** and `GetTraces` is unimplemented; the UI trace view is mock (`web/src/lib/traces.ts`) |
| Proto-defined interfaces, written or reviewed by you | D | twelve files in `proto/lbsim/v1/`, reviewed; every change since goes through the main agent; `WIRE.md` maps them to JSON until codegen |
| A TODO file for you | D | `TASKS.md` |
| Standalone dashboard as the final result | B | see §6 |
| ≥2× realtime for 5 clusters × 10k GPUs, ideally 10× that | F | **not measured** (M9). `ARCHITECTURE.md` §1.4 argues one core carries 6,250 replicas at 20×; today's runs are 32 replicas; the multi-cluster dimension does not exist in the scenario at all |
| Sharded service on GCP, ≤10 backend replicas | B (partial D) | deployed, public, scale-to-zero measured, `--max-instances 10`; **one service, not sharded**: Leaf-as-process is decided (§14 row 7) and the leaf trait is in `claude/tl-engine`, the process boundary is not built |
| Rust backends, Node + React frontend | D | as built |
| Static HTML report with interactive links to replay | B (partial D) | static reports done; "links to replay" need the dashboard to load a run by id, which the pre-baked index (`runs/index.json`, `claude/tl-export`) provides ~19:00 |

## §10 Open questions

| Question | | Evidence |
|---|---|---|
| Request cohorts, pressure-tested | D (decided) | parked with a stated trigger, §14 row 2 |
| Engine/policy separation with a physics referee so policies cannot cheat | B (partial D) | policies are behind traits and see only `ReplicaView`/`RequestView` (delayed); the engine owns physics; the arena's realism envelope checks outputs; a strict-mode referee that audits a policy's *proposed* schedule (execution plan M5) does not exist |
| Battle arena for agents, after realism is validated | B | §2 |

## Counts

- **Done: 19** rows (counting split rows by their D part where the report side is complete).
- **Building in the next ~2 hours: 22** rows, of which the dashboard path (~19:00) covers 8, in-flight
  engine/physics/policy branches cover 7, and 7 are queued in the tech lead's fan-out with no ETA
  (preemption, SLO classes, speculative decoding knob, prefix caching, arena loop, batch-policy seam, trace spans).
- **Far away: 17** rows, all downstream of four missing foundations: failure injection (item 11),
  replica lifecycle with turn-up delay, multi-cluster, and memory tiering.

## The five most valuable far-away items

1. **Failure injection with gray failure and detection delay** (item 11). It unlocks cascading failure,
   failover, affinity-hotspot cascade and the "global cascading failure" showcase; nothing else does.
2. **Multi-cluster in the scenario** (prerequisite of items 12, 13, and of the scale target). Today the
   whole model is one fleet; the 5×10k GPU target cannot even be expressed, let alone measured.
3. **Control analysis M7** (perturbation input, Bode plot, controller policy). It is the thesis of
   VISION.md §3a and the most distinctive result the project can show; finding 2 is only its prologue.
4. **The `disable_decode` knob.** Tiny, explicitly asked for "early on", and it is what makes the
   rolling-hotspot finding legible to a non-LLM audience. It is in no brief.
5. **Scale measurement M9.** Every design claim about realtime factor rests on one extrapolation in
   `ARCHITECTURE.md` §1.4; one measured run at 6,250 replicas would either confirm the whole approach or
   change the plan.

## VISION.md requirements that no plan document mentions

These appear in VISION.md and in none of `docs/ARCHITECTURE.md`, `docs/execution-plan.md`,
`docs/scope-today.md`, `docs/dashboard-plan.md`, `docs/arena.md`, or `TASKS.md`:

- **`disable_decode` knob** (§3a) to make traffic look like stateless serving.
- **Load forecasting** as a policy family (§3b).
- **Latency forecasting** as a policy family (§3b); `deadline_aware` touches it by accident, not by design.
- **Redundancy policies** (§1) as a named family.
- **Model-weight locality / MoE / multi-model** (§3a, §4) is in `scope-today.md` item 17 only as a row
  with an estimate; no design section exists.
- **Sampled request tracing across machines by latency bucket** (§8): the proto has `GetTraces` and
  `record_traces`, and the telemetry dump stratifies records, but no document plans per-machine spans.
- **"Fluid" simulation as cohorts = 1** (§4) is mentioned only as the parked cohorts decision; the fluid
  limit itself is not discussed.
- **The question in §5**, whether production mixes training and serving, was never answered: in
  practice large operators keep them on separate fleets; some share nodes off-peak for fine-tuning, but
  latency-sensitive serving and training are not co-scheduled on the same GPUs.
