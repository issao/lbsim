# STATUS

What is live on `origin/master`, what each agent is doing now, and the assumptions being acted on.
For things that need *you*, see `TASKS.md`.

**Last updated:** 2026-09-06 17:26 PDT by Claude.

---

## Session restart

**Resumed 2026-09-06 17:04 PDT.** Housekeeping restarted from this file and `TASKS.md` on
`claude/docs-round36` from `origin/master` c1373d0, then 907f3a1. Agent ids for this session, from the
main agent at 17:15: tech lead `a86e5fccbc930bd58` (fresh, on Fable), productivity `a09d10732748a5738`,
cloud `a4c101059bf0331c5` (temporary, deploying the replay dashboard), main `main`. Everything that
landed between the checkpoint and the resume was recorded from `git log 35693bc..907f3a1` and the
main agent's message, not from memory. The
respawn briefs are `docs/agents/` (ad16d4d, 39ec6d7, 4b7c5d3): `README.md` gives the spawn order,
`housekeeping.md`, `tech-lead.md`, `productivity.md`, `cloud.md`, and `brief-template.md`, the preamble
every subagent spawn pastes in.

Checkpoint at 2026-09-06 16:43 PDT. Issao: *"lets commit any state that is critical for any running agent including you
and then start with fresh context from there."* Every agent was killed and restarted from files at this
point. Each agent's memory is a file, and a restarted agent resumes from it alone:

| Agent | Memory | Respawn brief |
|---|---|---|
| Tech lead | `docs/execution-graph.md` | `docs/agents/` |
| Housekeeping, monitor and inbox | `TASKS.md`, `STATUS.md`, this section | `docs/agents/` |
| Main agent | `CLAUDE.md`, `docs/agents/` | `docs/agents/` |

**Housekeeping watcher setup, to re-arm identically** (the brief in `docs/agents/housekeeping.md` is
authoritative). Work in the worktree `/home/agents/repo/lbsim-docs` on branches `claude/docs-<n>`, never
in the shared checkout; commit with `git commit -- <paths>`; merge `--no-ff` into `master` in the main
checkout after `merge --ff-only origin/master`, push `master` and the branch, verify with
`merge-base --is-ancestor`. Poll every 90 seconds: `git fetch origin`, `python3 tools/inbox.py --no-fetch`,
`gh pr list --state open`, and diff new upstream commits for lines naming Issao without a colon, which
the scanner cannot see. A background waiter exits on the first new `origin/master` commit, pending
marker or open PR, and is re-armed after each. Markers inside `docs/execution-graph.md` are the tech lead's quotes of acted-on instructions, not
new ones; act on none of them; the tech lead rephrased them at cf85752 and the tree scans clean.
Every stamp comes from `date '+%Y-%m-%d %H:%M %Z'` or the commit that carried the event. A marker in
`TASKS.md` or `STATUS.md` that asks for code is routed: verbatim under "Routed to the tech lead" in
`TASKS.md`, marker left for the actor. `docs/vision-progress.md` is deleted at the first housekeeping
round after 2026-09-07 16:19 unless Issao has said otherwise. The estimate table in
`docs/dashboard-plan.md` and every line of `docs/execution-graph.md` change only on word from the tech
lead or the main agent, never by inference. Open at checkpoint: nothing routed is unresolved except the
tech lead's units in the graph; `TASKS.md` item 0 awaits Issao's review.

## Phase

**Building, delegated.** Three long-lived agents with disjoint file ownership, recorded in `CLAUDE.md`.
Scope is `docs/scope-today.md` package B plus item 7, and both side missions Issao asked for.

## Live on `origin/master`

| What | Where | Verified by |
|---|---|---|
| Simulator, six dynamics reproducing | `src/`, `scenarios/` | `./run-demos.sh`, six HTML reports in `out/` |
| Test suite: 143 pass, 5 ignored (3 known defects, 2 slow arena reproductions run only by `tools/integrate.sh`), measured at 907f3a1 on 17:20; `tests/layering.rs` enforces the downward crate dependency direction | `tests/`, `crates/*/` | `tools/build.sh test --workspace` |
| Golden fingerprints: every demo, the held-out suite and a live probe, byte-identical to the baseline | `bench/golden-fingerprints.txt` | `./check-fingerprints.sh` |
| Frontend-to-Ingress wire: JSON over HTTP/1.1, SSE subscriptions, field names held to the proto by a test | `crates/sim-ingress/WIRE.md` | `cargo test -p sim-ingress` |
| Policy ordering survives ±30% cost-model error | `check-sensitivity.sh` | run it; exits non-zero on a flip |
| Arena, mechanical half, eight-scenario held-out suite | `crates/sim-arena/`, `scenarios/holdout/` | `tools/build.sh test --release -p sim-arena -- --nocapture arena_round` |
| Stand-in dashboard, three surfaces, mock data | `web/` | `cd web && npm run build` |
| **Deployed, public at <https://lbsim.ai>** since 16:16, also <https://lbsim-irpwc2yaoa-uc.a.run.app>; dashboard at `/` with links to the reports at `/reports/1-routing.html` to `6-retry.html`; revision `lbsim-00004-t5q` from 6ebfd4e | `Dockerfile`, `cloudbuild.yaml`, `deploy.sh` | `curl -sI` on the URL returns 200; `./deploy.sh --check-idle` reads the instance count from Cloud Monitoring |
| Reference cost model, exact against a naive oracle | `bench/validate_epochs.py` | `tools/sync.sh` runs it |
| Interfaces, twelve files, reviewed; `GeneratedPolicy` slot (947649b), lease expiry renamed `lease_expires_at_wall_ns` because it is wall clock (db390a4) | `proto/lbsim/v1/` | `tools/sync.sh` compiles them |
| Findings, one section per dynamic | `docs/findings.md` | every table from `./run-demos.sh` |
| `disable_decode` scenario key, *"basically by setting HBM to infinity"*: zeroes the bandwidth term, KV still binds in tokens; demo 7 (`route_round_robin_no_decode`, `route_p2c_no_decode`), demos 8-10 for admission, fair share and live probes; 28 golden rows added, no existing number moved | `crates/sim-physics/src/lib.rs`, `scenarios/`, `run-demos.sh` (f6b7283, merged c7f8c6a) | `./check-fingerprints.sh` |
| Policy registry generated from the policy files by `build.rs`, so parallel policy branches cannot conflict on one table | `crates/sim-policy/build.rs` (5083b7b) | `tools/build.sh test -p sim-policy` |
| `tools/integrate.sh <branch>`: the merge queue as a script any agent runs at the end of its unit (rebase, workspace test, fingerprints, `--no-ff` merge, push) | `tools/integrate.sh` (dbe5e8e) | `tools/integrate.sh --dry-run` |
| Restart briefs, one per agent, plus the brief template every subagent spawn pastes in; the goal from Issao at 16:53 recorded verbatim in the tech lead's | `docs/agents/` (ad16d4d, 39ec6d7, 5c8686c, 4b7c5d3) | read them |
| Iteration profile: a unit is 68% model time; the one 79 s arena test dominates tool time; ranked fixes | `docs/iteration-profile.md` (8280a48) | read it |
| Execution graph: 22 done, 1 in flight (U18, the live ingress server), 22 queued, 1 waiting on Issao | `docs/execution-graph.md` (edbd04a) | read it |
| **Dashboard replay source**: with `runs/index.json` served beside the app the load-test dashboard plays exported runs, load, throughput, latency, imbalance and KV real, the rest NaN or still mock and tagged; play/pause/speed/step/scrub local, rewind and policy changes refused with a reason; 15+33 self-test cases; headless Chromium showed the 30 demo runs replaying | `web/src/lib/{replay,adapter,mode}.ts`, `web/README.md` "Replay mode" (4fb1105, merged 5077a34) | `cd web && node --experimental-strip-types src/lib/replay.selftest.ts` |
| Request traces on the wire: `RequestTrace`/`TraceSpan` per `WIRE.md`, a seeded stratified sampler, `GetTraces` filters, `runs/<id>/traces.jsonl` in the export inside a 5 MiB budget with every failure kept and the slowest request anchored; against a fixture struct until U24 records spans in the step | `crates/sim-metrics/src/trace.rs`, `crates/sim-ingress/src/trace_wire.rs`, `tests/trace_wire.rs` (c766e2f, merged 3a00f92) | `tools/build.sh test --test trace_wire` |
| Arena rule set v2: the score is the minimum over in-scope loads of goodput as a share of offered output tokens, cap 0.95, recorded in every score; absolute goodput the diagnostic; `catalog::append` writes a row per arena-authored policy into `docs/policy-catalog.md`; the two 80 s arena reproductions `#[ignore]`d behind a smoke round, 81.7 s → under 2 s | `crates/sim-arena/src/{lib,catalog}.rs`, `tests/arena_{rules,catalog}.rs` (14aa4e5, merged f6a87a9) | `tools/build.sh run --release --bin sim-run -- arena --cap 0.95` prints the rule set and p2c 0.762 > round_robin 0.732 > random 0.705 |
| Policy catalog, every policy idea so far, one table per family, including the load and latency forecasting families Issao asked for at 16:22; the arena generator appends a row per policy it authors | `docs/policy-catalog.md` (fc06095; v2 scores since this round) | `tools/build.sh test --test arena_catalog` holds its headers to the code |


Seven dynamics now: the six below plus, since c7f8c6a, that the routing ordering holds with decode
disabled, p2c 97.0% attainment against round robin 93.7% at identical throughput, which is what
`VISION.md` §3a predicted: the rolling hotspot does not need LLM physics. Finding 7 in `docs/findings.md`.

The six dynamics, one line each, all measured at 30% of rated capacity unless the sweep is over load:
reading the whole fleet routes worse than sampling two of it; herding has a staleness threshold, not a
gradient; prefill and decode contend and no chunk size wins both; past the knee more offered load
delivers less; a load-balancer metric improves while service collapses; a retry budget is the
difference between a bad minute and an outage. Numbers in `docs/findings.md`.

## What each agent is doing now

**The file to monitor for execution progress is `docs/execution-graph.md`** (cd9af2a): the tech lead's
dependency graph of every unit, done, in flight, queued or waiting on you, with a section per unit,
updated on every spawn, merge and ETA change. This file summarises; that one is the source.

`docs/iteration-profile.md` (2037a11) is the measured profile of how the tech lead and its subagents
spend a unit of work, with ranked fixes; its numbers live there, not here.

| Agent | Owns | Now |
|---|---|---|
| Tech lead `a86e5fccbc930bd58` | `src/`, `tests/`, `scenarios/`, `web/src/lib/` | **Fresh at 17:15 from `docs/agents/tech-lead.md`, on Fable**; nothing reported to housekeeping yet. The old tech lead's last landings, all in `docs/execution-graph.md` at edbd04a: U17 dashboard replay source (5077a34), U20 arena rule set v2 with catalog append and the fast arena tests (f6a87a9), U19 trace wire and export (3a00f92), U21 `disable_decode` with demos 7-10 (c7f8c6a), the generated policy registry (5083b7b). 22 units done, one in flight: U18 live ingress server on `claude/tl-ingress-server`, design fixed, no code yet. First to spawn per the graph: U22 preemption and KV eviction, the head of the dynamics fan-out. Restart instruction from Issao, verbatim in the brief: *"instructing TL to focus on aggressive delegation and parallelism"*: at least eight units in flight, no code of its own, integration by `tools/integrate.sh` at the end of every unit |
| Productivity `a09d10732748a5738` | `docs/iteration-profile.md`, `docs/agents/brief-template.md` | resumed 17:05 from `docs/agents/productivity.md`, per Issao: *"farming out a separate agent to focus on instrospecting on overall productivity improvements for the tl and coding agents."* Works in `/home/agents/repo/lbsim-prod` on `claude/prod-<n>`, integrates with `tools/integrate.sh`. Every 20 minutes it parses the subagent transcripts and appends a dated section to the profile; first section, the 17:12 baseline, integrating from `claude/prod-1` at 17:15: three subagents of the old tech lead each lost 10 minutes at 16:53 to a hung `tests/ingress_http.rs` that held a build slot; a `tools/build.sh` proposal went to the tech lead. Next section about 17:40. Nothing needs Issao |
| Cloud `a4c101059bf0331c5`, temporary | `Dockerfile`, `deploy.sh`, `cloudbuild.yaml`, `docs/deploy.md` | **deploying the replay dashboard** since 17:15: the `Dockerfile` now builds reports 7-10 and runs `sim-run export --demos` into the image so `/runs/index.json` sits beside the app (efea414, merged 6897543, `docs/deploy.md` updated with it); the revision is recorded here when the main agent relays it. Before that the cloud agent had **finished**; redeploys are run on request: `./deploy.sh`, about 2.5 minutes end to end. First deploy at 15:22 (72dfb16); scale-to-zero verified twice (993c03a); `docs/deploy.md` is its handover. No further grant was needed, the pending `legacyBucketReader` request is withdrawn: the blocker was two bucket permissions, worked around in `cloudbuild.yaml` |
| Monitor and housekeeping | `TASKS.md`, `STATUS.md`, `README.md`, `docs/*.md` except the tech lead's and cloud's | resumed 17:04 on `claude/docs-round36`; inbox and PR loop every 90 s; this round wrote finding 7, the catalog's v2 scores, `docs/arena-implementation.md` §3 under v2, `docs/arena.md` §5b closed, `web/public/runs/` ignored; no open PR on `issao/lbsim`; no pending marker in the tree |

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
| M2 physics | load-bearing half done | the analytic epoch advance in Rust, integer arithmetic, with the naive per-step oracle beside it and equality rather than tolerance: 10,000 random epochs agree exactly, 77x fewer evaluations on the test seed (dbfc553); `disable_decode` (c7f8c6a); preemption with swap or recompute and KV eviction is U22, the head of the dynamics fan-out, not yet spawned |
| M3 three-layer split | seam done, split not | `sim-leaf-api` defines `Leaf` message for message after `leaf.proto`, `LocalLeaf` implements it in-process over a resumable `Sim`, and a Leaf binary exists in the manifest (30e6ef1); the process split Issao decided on and the cross-shard determinism test are still to come |
| M4 workload and telemetry | partial | arrival heterogeneity and telemetry delay built; no trace replay, no session or prefix model |
| M5 policies | partial | six routing policies and two admission controllers, `deadline_aware` and `fair_share`, behind a one-file-per-policy registry that `build.rs` now generates from the files (289cb22, 120e62d, 210b657, 072a932, 5083b7b); no prefix affinity, no per-decision cost measurement |
| M6 failures | done | retry contrast, finding 6 |
| M7 control analysis | not started | |
| M8 dashboard and first deploy | in progress | static deploy live, scale-to-zero measured (993c03a); `sim-ingress` has leases and the idle guard (d688f8f) but no run or subscription endpoint, though `Sim` is now resumable (fc910f6) and per-sample frames exist (2c4a7eb), which the endpoint needs; `sim-run export` writes runs as the `WIRE.md` JSON documents the stream will carry (41f6435); browser transport merged (389f41e) with mock still the default; the replay source that joins the two is on `master` (U17, 5077a34): **the pre-baked dashboard is deployable and the cloud agent is deploying it**, revision to follow; the live server is U18, in flight, not yet written; until the deploy lands the public dashboard is still mock |
| M9 scale validation | not measured | |

22 of the graph's 46 units are done (edbd04a); the arena's judging half scores under rule set v2 with
the catalog append wired (f6a87a9), and request traces exist on the wire against a fixture struct (3a00f92).

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

From `docs/execution-graph.md` at edbd04a: **U17 is on `master` (5077a34), so the pre-baked dashboard is
deployable now.** Deploy is `sim-run export --demos --dir <served dir>` then `./deploy.sh`, per
`web/README.md` "Replay mode"; the temporary cloud agent folded both into the `Dockerfile` (6897543) and its
revision is recorded here when relayed. After the deploy the load, throughput, latency, imbalance and KV
panels show the engine's numbers for the 30 demo runs; the per-replica table and heatmap wait for U23,
the live path is U18 then U28, and the A/B view and the showcase stay mock until then. The 15:55 ETA
table (19:00 pre-baked, 21:00 live, 22:30 deployed) is superseded: the pre-baked step beat it by two hours.

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
