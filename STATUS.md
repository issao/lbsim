# STATUS

What is live on `origin/master`, what each agent is doing now, and the assumptions being acted on.
For things that need *you*, see `TASKS.md`.

**Last updated:** 2026-09-06 16:29 PDT by Claude.

---

## Phase

**Building, delegated.** Three long-lived agents with disjoint file ownership, recorded in `CLAUDE.md`.
Scope is `docs/scope-today.md` package B plus item 7, and both side missions Issao asked for.

## Live on `origin/master`

| What | Where | Verified by |
|---|---|---|
| Simulator, six dynamics reproducing | `src/`, `scenarios/` | `./run-demos.sh`, six HTML reports in `out/` |
| Test suite: 73 pass, 3 ignored as known defects; `tests/layering.rs` enforces the downward crate dependency direction | `tests/`, `crates/*/` | `tools/build.sh test --workspace` |
| Golden fingerprints: every demo, the held-out suite and a live probe, byte-identical to the baseline | `bench/golden-fingerprints.txt` | `./check-fingerprints.sh` |
| Frontend-to-Ingress wire: JSON over HTTP/1.1, SSE subscriptions, field names held to the proto by a test | `crates/sim-ingress/WIRE.md` | `cargo test -p sim-ingress` |
| Policy ordering survives ±30% cost-model error | `check-sensitivity.sh` | run it; exits non-zero on a flip |
| Arena, mechanical half, eight-scenario held-out suite | `crates/sim-arena/`, `scenarios/holdout/` | `tools/build.sh test --release -p sim-arena -- --nocapture arena_round` |
| Stand-in dashboard, three surfaces, mock data | `web/` | `cd web && npm run build` |
| **Deployed, public at <https://lbsim.ai>** since 16:16, also <https://lbsim-irpwc2yaoa-uc.a.run.app>; dashboard at `/` with links to the reports at `/reports/1-routing.html` to `6-retry.html`; revision `lbsim-00004-t5q` from 6ebfd4e | `Dockerfile`, `cloudbuild.yaml`, `deploy.sh` | `curl -sI` on the URL returns 200; `./deploy.sh --check-idle` reads the instance count from Cloud Monitoring |
| Reference cost model, exact against a naive oracle | `bench/validate_epochs.py` | `tools/sync.sh` runs it |
| Interfaces, twelve files, reviewed; `GeneratedPolicy` slot (947649b), lease expiry renamed `lease_expires_at_wall_ns` because it is wall clock (db390a4) | `proto/lbsim/v1/` | `tools/sync.sh` compiles them |
| Findings, one section per dynamic | `docs/findings.md` | every table from `./run-demos.sh` |


The six dynamics, one line each, all measured at 30% of rated capacity unless the sweep is over load:
reading the whole fleet routes worse than sampling two of it; herding has a staleness threshold, not a
gradient; prefill and decode contend and no chunk size wins both; past the knee more offered load
delivers less; a load-balancer metric improves while service collapses; a retry budget is the
difference between a bad minute and an outage. Numbers in `docs/findings.md`.

## What each agent is doing now

| Agent | Owns | Now |
|---|---|---|
| Tech lead | `src/`, `tests/`, `scenarios/`, `web/src/lib/` | Landed, all byte-identical against `./check-fingerprints.sh`: the eleven-crate workspace (163995c); golden fingerprints and `WIRE.md` (0efb8bd); the policy trait and registry, plus `admission`, `fair_share_burst`, `tenants`, `tenant_weights`, `tenant_demand` in scenarios (289cb22, cfc8f03); leases and the idle guard (d688f8f); Issao's homepage links and `tools/build.sh`, which bounds concurrent cargo builds to two (16daf22); simplification pass 1 on `sim-report`, 884 to 851 lines (4b29810); the browser transport client against `WIRE.md`, mock still the default (389f41e, 16:14); the `least_kv_probe` routing policy, power-of-d choices on live KV occupancy with the probe paid for (120e62d); `deadline_aware` admission, shedding on expected queue wait before a request costs anything (210b657); `fair_share` admission, weighted fair share over tenants in tokens (072a932); the M2 analytic epoch advance with its exact oracle (dbfc553); the wire encoder and `sim-run export --demos` (41f6435). In flight per the last report: engine (`claude/tl-engine`); the tech lead's next message updates this. Queued: preemption and KV eviction, SLO classes, the live ingress server, the dashboard replay source, speculative decoding, prefix caching. From Issao at 15:50: *"continue making more progress on the scope-today.md dynamics we talked about when it is possible to do so in parallel"*, which is that queue. From Issao at 16:22, routed: the `disable_decode` knob, *"basically by setting HBM to infinity"*, first in priority; load and latency forecasting as policies to evaluate, with `docs/policy-catalog.md` fed by the arena generator; sampled request traces *"to see execution traces and what was busy in each resource"*, through `GetTraces` and the export |
| Cloud | `Dockerfile`, `deploy.sh`, `cloudbuild.yaml`, `docs/deploy.md` | **finished.** Redeploys are now run by the main agent on request: `./deploy.sh`, about 2.5 minutes end to end. First deploy at 15:22 (72dfb16); scale-to-zero verified twice (993c03a); `docs/deploy.md` is its handover. No further grant was needed, the pending `legacyBucketReader` request is withdrawn: the blocker was two bucket permissions, worked around in `cloudbuild.yaml` |
| Monitor and housekeeping | `TASKS.md`, `STATUS.md`, `README.md`, `docs/*.md` | inbox and PR loop every 90 s; documents tidied; keeping them aligned to each merge. The homepage-link instruction routed at 15:57 was done by the tech lead at 15:59 |

The main agent coordinates and owns `CLAUDE.md` and `proto/`.

## Progress against the execution plan

One row per milestone in `docs/execution-plan.md` §1, read from `master` at 604bb95 and updated as
units land. `docs/vision-progress.md` is the same reading by `VISION.md` requirement, taken at 16:19,
temporary, for Issao to review.

| Milestone | State | What exists, what does not |
|---|---|---|
| M0 workspace and CI | done, one gap | the section 10.8 workspace, eleven crates (163995c), `tests/layering.rs` proves `sim-ingress` cannot reach `sim-model` or `sim-physics`; no `prost`/`tonic` codegen |
| M0.5 stand-in dashboard | done | `web/`, mock data, marked as such |
| M1 walking skeleton | done and exceeded | six dynamics, `docs/findings.md` |
| M2 physics | load-bearing half done | the analytic epoch advance in Rust, integer arithmetic, with the naive per-step oracle beside it and equality rather than tolerance: 10,000 random epochs agree exactly, 77x fewer evaluations on the test seed (dbfc553); preemption with swap or recompute and KV eviction queued |
| M3 three-layer split | partial, in flight | crate boundary exists; the leaf trait shaped by `leaf.proto` and a resumable `Sim` are on `claude/tl-engine`, toward Leaf as a process per Issao; no cross-shard determinism test |
| M4 workload and telemetry | partial | arrival heterogeneity and telemetry delay built; no trace replay, no session or prefix model |
| M5 policies | partial, in flight | six routing policies and two admission controllers, `deadline_aware` and `fair_share`, behind a one-file-per-policy registry (289cb22, 120e62d, 210b657, 072a932); no prefix affinity, no per-decision cost measurement |
| M6 failures | done | retry contrast, finding 6 |
| M7 control analysis | not started | |
| M8 dashboard and first deploy | in progress | static deploy live, scale-to-zero measured (993c03a); `sim-ingress` has leases and the idle guard (d688f8f) but no run or subscription endpoint; `sim-run export` writes runs as the `WIRE.md` JSON documents the stream will carry (41f6435); browser transport merged (389f41e) with mock still the default; the replay source that joins the two is next; dashboard still mock |
| M9 scale validation | not measured | |

## When the dashboard shows real demos

Today the six demos are live as real-data HTML reports at
<https://lbsim.ai/reports/1-routing.html> through `6-retry.html`. The React
dashboard at `/` shows mock data and says so on every panel. The homepage links to the six reports
since cf12e33, live since 16:03 in revision `lbsim-00004-t5q`, built from `master` 6ebfd4e.

`docs/dashboard-plan.md` (16c28ac) is the breakdown of why the first dashboard with real runs is
estimated at 19:00 and what can be pulled in; its estimate table moves only on word from the tech lead
or the main agent. Three things stand between it and real data: an Ingress endpoint that runs a scenario and streams
metrics over the JSON/SSE wire in `crates/sim-ingress/WIRE.md`, or until then the pre-baked runs that
`sim-run export --demos` now writes (41f6435), loaded by a replay source; the browser transport client, merged at 16:14 (389f41e) but not yet the default, replacing the
mock engine; and a rebuild and redeploy, measured at
1m22s. The shortest path, asked of the tech lead by the main agent: pre-baked run output in wire format
served statically, so every panel shows real data before the live path exists.

**None of this lands before your 16:30 stop.** You will find the state here when you are back. The tech
lead's ETA, wall clock from 15:55 PDT and conditional on agents landing at today's rate:

| Step | ETA |
|---|---|
| Pre-baked path: six demo runs exported by `sim-run` as `WIRE.md` JSON documents, served statically, dashboard loads them when no Ingress answers | about 3 h, roughly 19:00 |
| Live Ingress endpoint streaming a real run over JSON/SSE; needs the engine to become resumable first | about 5 h, roughly 21:00 |
| Web app on that transport, mock markers removed only on wired panels | roughly 22:00 |
| Both on `master`, rebuilt and redeployed | roughly 22:30 |

## Assumptions being acted on

- Package B of `docs/scope-today.md` plus item 7 is today's scope; the dashboard and the arena were
  reinstated by Issao and are built.
- Arena: SLA cap 0.95 is Issao's rule for now, and the objective is goodput as a share of offered
  work, agreed at 16:12; the code still scores the raw objective until the tech lead switches it. The
  policy generator may write code, not only parameters.
- Prefix-sharing topology comes from a session model with fork-off and merge-back rates, swept.
- Leaf shards are separate processes; memory tiers are Ingress-owned with a seeded bloom-filter
  residency hint. Both are design of record, `docs/ARCHITECTURE.md` §14, not yet built.
- `lbsim.ai` is dark: its DNS zone went with `lbsim-prod`. `TASKS.md` item 1.
- Deployment scales to zero within a replica budget of ten, public at `lbsim.ai` since 16:16. Scale-to-zero is measured, not assumed: two revisions
  reached zero instances within minutes of losing traffic, per `docs/deploy.md` (993c03a), and
  `deploy.sh --check-idle` re-measures it.
- Reference hardware is a 70-billion-parameter model on eight H100s.
- Everything in `docs/ARCHITECTURE.md` section 14 stands as recorded there.

## Tree state

Every agent works in its own worktree beside the repo (`/home/agents/repo/lbsim-<name>`) on a
`claude/<topic>` branch. Subagents push their branch and report; they never merge or push `master`.
The tech lead, the main agent and the housekeeping agent rebase onto `master`, merge `--no-ff` and
push. The rules in full are in `CLAUDE.md`; where this file and `CLAUDE.md` differ, `CLAUDE.md` wins.
