# TASKS — things that need Issao

Last updated: 2026-09-10 15:50 PDT by Claude.

**2026-09-10 15:50 PDT, from `git log 771021c..origin/master` and `docs/wrap-up-2026-09-06.md` §§5j–5l (housekeeping is not running;
this round is `claude/docs-round42`; no tech lead or productivity agent is running either, and the execution graph has been STOPPED
since 2026-09-07 — everything below landed through the main agent directly).** Nothing here needs you urgently: fifteen more commits
landed clean since round 41, all of it either your own instruction verbatim in the commit (the prompt-size sliders, the Policies tab's
`weighted_random`/`buffered_batch` knobs, the 8 GiB instance) or a bug fix with no design choice attached (the trace-gap `prefill_wait`
span, the `Preempted` span, the memory-release rule that stopped completed runs from piling up against `LBSIM_MEMORY_BUDGET_MB`, two
harness-flake fixes for the kv-spiral trace check, the last (`claude/qa-traces-api`) landing mid-round as `bc7c0d6`. Current site:
`lbsim-00029-7lz`, deployed 15:05 PDT from master 3430476; harness went 143 passed / 1 failed, then, after the third fix landed,
144 passed / 0 failed (0aa636f, 15:54). Full detail, by hash and deploy, is in `STATUS.md`. Every item below stands exactly
as round 41 left it; nothing new is added, and nothing in the last fifteen commits opened a review item.

**2026-09-10 11:48 PDT, from `git log 3de8220..origin/master` (housekeeping is not running; this round is `claude/docs-round41`).**
Nothing here needed you urgently: two days of engine and dashboard work landed clean, and every design decision in it was either
Issao's own instruction verbatim in the commit (the arrival-rate ceiling, the smoothing window, the badput chart, the restart button,
the GPU-utilization split, the batch buffer and the weighted router) or a default the unit already declared for itself ("stands as
written" in the commit body). Five proto/engine additions since 2026-09-08 are recorded below in §6 as review items, each with the
default it already stands on if you say nothing; one new item is queued for the next tech lead, also in §6. `git log` shows nothing
landing on `master` between 2026-09-08 15:34 and 2026-09-10 10:27, about 43 hours; no file this round has access to explains the gap.
Full detail, by hash, is in `STATUS.md`.

**2026-09-08 00:35 PDT, from git and gcloud, by main's docs agent (housekeeping is not running tonight).** The tech lead ran two dynamics
lanes and the dashboard units from 21:00 to 23:54 on 2026-09-07: 38 units landed, among them eight new demos (13–20) and the third, fourth and fifth
pluggable seams (scheduling U108, health/ejection U31b, autoscaling U32); the mock dashboard is gone (U100, live and replay only); findings 10–17
are in `docs/findings.md` as of this round. The graph at 818b34a reads RUNNING, 115 done, waiting for U30 and U32, both of which have since
landed (192c6e0, e703f6a), then STOPPED. Deployed: `lbsim-00017-2wq` at 23:14 (interim), and `lbsim-00018-2hv` created 00:25 and rolling out after
two failed image builds (23:53, 00:05) and a diagnostic build that succeeded at 00:16; `STATUS.md` has the hashes. Two things need a word from
you: item 1, one IAM grant so build failures can be read, and item 3, whose default is now keep. Decisions the units left with main, not
you, are listed under section 6.

**STOPPED 2026-09-07 20:25 PDT, per main: nothing else needed tonight.** The tech lead stopped at 20:24 with 77 done, 0 in flight, 13 queued; ten units
and three deploys tonight; your whole morning instruction, follow-ups included, is on <https://lbsim.ai> as of `lbsim-00016-nbc` (20:23). Resume by
a message from main; agents re-spawned from `docs/agents/`. One thing still needed a word from you, item 2 then (item 3 now). Main's three harness runs against
`lbsim-00016-nbc` were in progress at the stop; main holds the counts.

**RESUMED 2026-09-07 19:13 PDT** at your "resume", relayed by main. The pause (11:22–19:13, quota) changed nothing on `master` but the tech lead's graph checkpoint (1a58186). The tech lead is `a56ff1d3950e7375f` now, resuming U79–U83 from the four WIP branches; U80 (Home in Live / Replay / Mock sections) landed at 023672d, 19:13, not yet deployed; `STATUS.md` has their state. Everything below stands as it was; one thing needs a word from you tonight, item 2 (item 3 since the renumbering of 2026-09-08).

**Resumed 2026-09-07 11:17 PDT.** The fleet stopped at 21:00 on 2026-09-06 at your request (*"we are approaching
limit... tie up loose ends... give me a doc summarizing the state and most interesting outcomes with deep
links and I will review and wrap up tomorrow"*) and is back this morning: the tech lead (`a0899ca58df6febdc`)
is on U79–U83, the Cloud Run live-server fix and your product-polish pass, per main. This file is the review
list, stack ranked, each item with what Claude does if you say nothing. What is finished and live is in
`STATUS.md`. The known issue from `docs/wrap-up-2026-09-06.md` §5b, the live server on Cloud Run degrading after a
handful of runs, is fixed: U79 on `master` at 60327b7 (19:33), deployed 19:39 as `lbsim-00014-gvp`, and main's three
harness runs against lbsim.ai at 19:40–19:55 went 54/2, 55/1, 56/0 with clean stream ends in `/requests.log`.

## 0. Read `docs/wrap-up-2026-09-06.md`

The main agent's summary of the day: state, the most interesting outcomes, deep links into the
findings, the graph, the deployed site and the catalog. On `master` since 5da8a67 (merged cc8a229, 21:01 PDT).

**If you say nothing:** the agents resume tomorrow from their files with the graph's queue as is.

## 1. Grant the deploy identity `roles/logging.viewer`, as yourself

The moment named in the "later" list (item 7) has come. Three image builds failed tonight on the report step with a bare exit code:
23:53 exit 127 (python3 missing from the image), 00:05 exit 2 (`bench/bode.py` not in the image), 00:30 exit 1 (069513de, cause pending). The deploy
identity cannot read Cloud Build's log, so each was diagnosed by reading the Dockerfile and the script instead of the error, and the
workaround now on `master` (682f35a) is a build whose report step writes its own log into the image at `/reports/build.log` and does
not fail the build. Reading the log is one grant, and only you can make it:

```bash
gcloud projects add-iam-policy-binding lbsim-gcp \
  --member=serviceAccount:lbsim-deployer@lbsim-gcp.iam.gserviceaccount.com \
  --role=roles/logging.viewer --quiet
```

**If you say nothing:** builds stay non-fatal on the report step and the log is read from <https://lbsim.ai/reports/build.log>
after each deploy; a report missing from the site is found there rather than in Cloud Build.

## 2. Review the `TraceSpan` extension in `proto/lbsim/v1/metrics.proto`

On `master` at f5eddf1 (a035514, 17:20), acting on *"We should have a way to sample requests to see
execution traces and what was busy in each resource as it executed."* `TraceSpan` now carries the
resource state the engine knows at the step: `batch_size`, `queued`, `kv_tokens_resident`,
`kv_capacity`, `step_ns`, `bound` (bandwidth or compute), and for routing spans the `candidates`
considered and `stale_view_age_ns` of the view they were scored on; `RequestTrace` gains the
`TraceBucket` the sampler stratifies on (p50, p90, p99, p99.9). Existing field numbers unchanged;
operation names now match the engine's (`queue`, `route`, `prefill`, `decode`, `kv_fetch`,
`preempted`). It matches `sim_metrics::trace::ResourceState` field for field.

**If you say nothing:** it stands as written.

## 3. Review `docs/vision-progress.md`

**Refreshed:** the file is now a snapshot as of 2026-09-07 21:38 PDT (b0de727, corrected ac371f4 for the six merges that landed
during the refresh), taken at your question *"what is missing in the vision progress, is that up to date?"*: every `VISION.md`
requirement classified as done, building, or far away, with one line of evidence each; it produced U104 (real traces) and U105
(rewind) in the graph. The first snapshot (2026-09-06 16:19, d42546a) is superseded by it. Since then the evening landed the
prefix model, the scheduling, health and autoscaling seams, tiering and the Bode plot, so its "building" rows are behind again;
`docs/execution-graph.md` and `STATUS.md` are current.

**If you say nothing: it stays**, refreshed on request rather than deleted; the earlier default to delete it is withdrawn now that
you asked for it to be refreshed. Say *"delete vision-progress"* if you want it gone.

## 4. Delete the stale `claude/*` branches on `origin`

The merged branches from before the 16:43 restart (`tl-idle`, `tl-web-transport`, `tl-web`,
`tl-engine`, `tl-export`, `tl-physics`, `tl-policy-*`, `tl-replay`, `tl-trace-wire`, `tl-arena-rules`,
`simplify-1`, `simplify-2`, `docs-round*`) are content-merged, verified by the tech lead with
`git cherry`; their only effect is that `tools/sync.sh` and the inbox list six stale quote markers
from an old `docs/execution-graph.md` on three of them. The tech lead deleted `tl-trace-engine` and
`tl-trace-engine-rebased` from its session at 20:47, so deletion from here works after all.

**If you say nothing:** they stay; Claude deletes nothing it did not create in its own session without
your word. Say *"delete the stale branches"* and housekeeping runs `git push origin --delete` on exactly
that list, name by name, never by prefix. Or do it yourself:
`git fetch --prune && git branch -r --merged origin/master | grep 'origin/claude/' | sed 's|origin/||' | xargs git push origin --delete`.

## 5. Routed to the tech lead, for the record

Each of these is a unit in `docs/execution-graph.md`, the tech lead's graph, which carries its state
and ETA; this list is the record of what was routed and why.

**2026-09-07, routed to the tech lead (`a0899ca58df6febdc`) as U80–U83, beside U79 (the Cloud Run fix),
relayed verbatim by the main agent at 11:18:** *"is the load test dashboard link ready to point to the live sim engine? can you make sure that all links in the homepage are separated by a section for 'live' one for 'replay' and one for 'mock' and that the message in the top right corner for each page is accurate. also, clean up all text from any reference of the build process to make this look like a finished product, ensuring accuracy and succinctness. also take a pass over all the ui to keep things clean and simple and intuitive, with no knobs that are not doing anything."*
Main answered the first question: the Load test link already opens a live run when the server answers, with
the `docs/wrap-up-2026-09-06.md` §5b caveat (one tab at a time until U79 lands). The rest is the tech lead's:
`docs/execution-graph.md` at 5080f22 (merged 37893ca 11:19) carries the same words and the five briefs, U79–U82 spawned
11:19, U83 queued behind U81; `STATUS.md` records each unit as it lands.
**Done, all five, by 19:50 today:** U80 Home sections 023672d, U81 badge bd5ba3f, U82 finished-product text 837910b, U79 the
live-server fix 60327b7 (deployed 19:39 as `lbsim-00014-gvp`), U83 the UI pass b5f5af7 and U77 the live-run cap f4a119f, deployed 19:51 as
`lbsim-00015-79v`. **The whole instruction, follow-ups included, is on <https://lbsim.ai> as of `lbsim-00016-nbc` (20:23).** Four follow-ups spawned at main's instruction at 19:55; by 20:10 all four are on `master`: U86 9c22131, the one-in-fourteen
showcase card stuck at 0 samples (a SetSpeed race, proven and fixed), and U84+U85 90e6b93, the control-panel header tag per mode
and replay's tabs disabled; U89 a9252da at 20:10, Showcase copy and empty states; and U87+U88 03bd74e at 20:19, internal names out of the UI and view-only
sliders as readouts. All four are on `master` as of 20:19 and **deployed 20:23 as `lbsim-00016-nbc`** from 3de8220; main's three harness runs
against it are recorded in `STATUS.md` when they finish. One READY when all four land, one last deploy, then
the tech lead stops.
**If you say nothing:** the tech lead's reading of it in the graph stands.

Nothing else routed since the checkpoint; the tree scans clean (`python3 tools/inbox.py`, 11:17 today). Nothing
from the 17:31–17:35 checkpoints needs you: R1's findings became U54 and U55 in the graph, and its item 8
is a crate-boundary decision for the main agent.

Your feedback inside the graph at 16:45 (0b53c59), on the SLO class targets: *"That looks good. ideally
we would have an average throughput for batch averaged at a longer time window, but don't worry about it
for now, record it for future work."* Acted on by the tech lead before the checkpoint: U42 accepted, the
future work recorded as U44 in `docs/execution-graph.md` (a2eef18).

Three from Issao at 16:22, routed by the main agent:

7. **Done, c7f8c6a.** *"we could get disable decode basically by setting HBM to infinity, so that should
   be straight forward."* Scenario key `disable_decode` zeroes the bandwidth term; demo 7; finding 7.
8. **Catalog append done, f6a87a9; the generator loop that calls it done, fe71bcf**; the forecasting
   families themselves are U40, queued. *"Load and latency forecasting should be added as potential policies to evaluate (populate an md with
   all policy ideas we have had so far and instruct the arena policy generator to populate that as well
   with any that it authors)."* `docs/policy-catalog.md` is being written by a separate agent; the tech
   lead wired the arena generator's append, `sim_arena::catalog::append`, with a test holding the
   catalog's header to the code.
9. **Done: wire side 3a00f92, engine side U24 3a350b1 (20:40)**, spans recorded in the step for a seeded
   sample, byte-identical runs with tracing on or off. *"We should have a way to sample requests to see execution traces and what was busy in each resource
   as it executed."* Request trace sampling: seeded, stratified by latency bucket, spans with
   per-resource state, exposed through `GetTraces` and the export. The struct, sampler, encoder,
   `GetTraces` filters and `traces.jsonl` in the export exist against fixtures; the proto owner was told
   `TraceSpan` lacks queued, batch size, step time, KV resident/capacity, the roofline side and the
   routing candidates (graph U19).

6. **Closed, f6a87a9.** At 16:12, on the arena objective, *"You can remove this, I agreed with this."*
   The score is the minimum over in-scope loads of goodput **as a share of offered work**; absolute
   goodput is the diagnostic beside it; every score records rule set v2. Order unchanged (p2c 0.762,
   round robin 0.732, random 0.705), worst load now the hardest one, `docs/arena-implementation.md` §3.
   U43 in the graph records that v2 stands unless you say otherwise.

Five design decisions from Issao at 15:26, recorded in the design of record (`docs/ARCHITECTURE.md`
section 14, `docs/arena.md`, `docs/execution-plan.md`) by the housekeeping agent. Each has code
consequences that belong to the tech lead; verbatim, from `TASKS.md` before the markers were removed:

1. *"SLA 0.95 ok for now, but we need to figure out how to do better."* Open design task: per-class
   or length-scaled first-token targets so a 0.99 cap becomes reachable; `docs/arena.md` §5b.
2. *"Arena policy generator should actually have full power to write code to write new policies, as
   well as tuning parameters on existing policies."* A policy candidate is code against the policy
   trait, not only a `PolicySpec`; `docs/arena.md` §6. The proto side is done: `GeneratedPolicy`
   variant in every policy slot, 947649b. The registry that resolves a policy by name from one file
   per policy is done and now generated by `build.rs` (289cb22, 5083b7b); the generator loop itself is
   U34, queued.
3. *"Re prefix sharing topology. I don't, assume some reasonable distributions of lengths of session
   and how often they fork off and merge back new agents and create a distribution based on that."*
   The workload model derives the prefix tree from a session process: session length, fork-off rate,
   merge-back rate, each swept; `docs/calibration.md` §9 row and `docs/ARCHITECTURE.md` §14.
4. *"Leaf shards should become separate processes in a sharded server."* Overrides the threads
   recommendation in `docs/ARCHITECTURE.md` §10.5 and `docs/execution-plan.md` §0. The barrier cost
   measurement stands; the realtime target is to be re-measured against processes.
5. *"If cluster memory tiers are owned by ingress, it can just tell the leaf if it is in DRAM/SSD or
   not. We can save a lot of resources if we need to by turning this into a bloom filter tuned
   appropriately using the global seed, preserving an acceptible false negative cache miss rate that
   is deterministic and also realistic from a machine loss perspective."* Ingress-owned tiers
   confirmed, with the residency hint as a seeded bloom filter; `docs/ARCHITECTURE.md` §10.6.

---

## 6. Decisions with a default

`docs/execution-graph.md` has a section "Waiting on Issao" listing the units that need a word from
you, each with the default it takes if you say nothing. Those defaults stand until you say otherwise.
The design decisions below are the ones outside that graph.

| Decision | Default | Rework if changed later |
|---|---|---|
| A routing policy that scans the fleet fails the run rather than warning (`docs/ARCHITECTURE.md` §10.4) | fail | small |
| Prefix-affinity index at Ingress is a bounded top-K, not exact (§10.4) | bounded | small |
| `ejection_ratio` default: the `Scenario` default is 3.0 as briefed, demo 15 runs 8 because a healthy fleet's step time is bimodal (11 ms decode, 48 ms with a prefill chunk) and 3 ejected 146 healthy replicas by t = 20 s (U31b, with main) | 3.0 stays, demo at 8 | one constant |
| Trace budget: an encoded trace is ~570 KB (one decode span per output token with the full resource state), so the 5 MiB ring holds 9–25 journeys and the demos export kept 9 of 943; coalescing decode spans or raising the budget is a proto/wire decision (U104, with main) | 5 MiB, 2,000 traces | wire and proto |
| `SchedulingPolicy` lives in `sim-core`, below both `sim-model` and `sim-policy`, and has a fourth decision, `prefill_order` (U108; ratified by main 22:56, `docs/ARCHITECTURE.md` §10.8) | as landed | crate move |
| `smoothing_window_ns` on `OpenSubscriptionRequest`: a live run is rebuilt server-side over the trailing window (never fewer than one sample, fewer at the start of a run), a replay smoothed identically client-side (1ed9b73/ab7817e, feb1ee0, c4a134f; main, 2026-09-08) | stands as written | wire and proto |
| GPU utilization keeps its original busy/time-in-step meaning (metric 67); a new metric 71, `METRIC_GPU_USEFUL_FRACTION`, is useful work over the maximum possible, so a small mean batch shows up as a gap between the two (Issao reversed his own first cut of this the same morning; 65758e3/4a5d6c0, 8755f2e/5e92cac; main, 2026-09-10) | stands as written | proto, wire, engine, web |
| `SchedulingPolicy.BufferedBatch` (a step admits only what its chunk, seats, decode seats and bandwidth line can serve, holding an idle queue up to `buffer_max_hold_ms`) and `RoutingPolicy.WeightedRandom` (weight linear in queued decode/prefill and each beyond the open buffer) (a5dfd38/e876d44; main, 2026-09-10); demo 21 measures both, finding 18 | stands as written | proto, `sim-policy`, `sim-core` |
| Four replica-scope metrics the buffered-batch unit proposed and main landed on the wire: `METRIC_QUEUED_DECODE_SEQS`/`_PREFILL_SEQS` (72/73), `METRIC_DECODE_BEYOND_BUFFER`/`_PREFILL_BEYOND_BUFFER` (74/75), the terms `weighted_random` weighs, replica scope only, zero under a scheduler with no buffer (d934d1c/75d9a1a; main, 2026-09-10) | stands as written | proto, wire |
| **Queued for the next tech lead**, not yet a decision: `fifo_chunked`'s prefill order is newest-first under churn, because retirement's `running.swap_remove` moves the newest sequence into the freed slot, so a newcomer inherits both the seat and the departed sequence's place in the prefill order (finding 18's mechanism for fifo's own 25% attainment at 1.2x rated load; baseline golden behaviour since U108, 96a6456) | stands (golden baseline) | re-baselines demos 3–5 and 16 |

## 7. Later, when this phase ends

- [ ] Delete the deploy key. `gcloud iam service-accounts keys list --iam-account=lbsim-deployer@lbsim-gcp.iam.gserviceaccount.com`
      shows the id, which starts `94afd556`; then `keys delete KEY_ID --iam-account=...`.
- [x] Grant `roles/logging.viewer` to the deploy account **only when a container fails on startup**. **That moment came on
      2026-09-07 night, three failed image builds; it is item 1 now.** `docs/deploy.md` records the gap.

- [ ] Delete the leftover test image, which the deploy account cannot delete itself:
      `gcloud artifacts docker images delete us-central1-docker.pkg.dev/lbsim-gcp/lbsim/lbsim:wstest --project lbsim-gcp --quiet`.
      If you do nothing it sits in the registry costing cents.
- [ ] Two more grants to hold back until needed, both from `docs/deploy.md`: `storage.buckets.update`
      for the 90-day delete rule on `gs://lbsim-gcp-runs`, which matters once runs write results there;
      and `artifactregistry.repoAdmin`, or running the tear-down as yourself, when this phase ends.

**If you do nothing:** the key stays until you delete it, build and startup failures stay diagnosable only through
`/reports/build.log` until you grant the role (item 1), and the results bucket keeps everything it is ever given.

---

## Answered, kept for the record

- At the wrap-up, 21:09: *"I am clicking on some of the showcase links and I am stuck in 'initializing the
  mock run'. can you look at it and fix?"* Fixed by the main agent (3e78b9a, merged 7d174d0): a refused
  metric in the subscription wish list left the page on its placeholder forever. Deployed as `lbsim-00010-spk` at 21:11 (revision time from gcloud), on
  <https://lbsim.ai> since.

- 17:31, *"can you ask everyone to pause until usage limit resets in 2h 50min"*: every agent paused and
  checkpointed to files, resume 20:21 PDT. Nothing new here needs you.

- 16:22, three decisions, all routed to the tech lead: *"we could get disable decode basically by
  setting HBM to infinity, so that should be straight forward."*; *"Load and latency forecasting should
  be added as potential policies to evaluate (populate an md with all policy ideas we have had so far
  and instruct the arena policy generator to populate that as well with any that it authors)."*, which
  produced `docs/policy-catalog.md` (fc06095); *"We
  should have a way to sample requests to see execution traces and what was busy in each resource as
  it executed."*

- 16:16, **<https://lbsim.ai> is live**: certificate issued, serving the dashboard over IPv4 and IPv6.
  Every domain step was yours and every one is done.

- 16:12, arena objective normalised to a share of offered work: *"You can remove this, I agreed with
  this."* Routed to the tech lead to implement.
- 16:11, the four `AAAA` records for `lbsim.ai`, added by you.

- 16:03, domain: registrar repointed to Porkbun, Search Console verified, mapping created, apex `A`
  records added, all by you. Certificate pending at 16:04, item 1.

- 15:56, *"for the dashboard, add alink to the reports from the homepage."* Done by the tech lead,
  cf12e33, on `master` at 15:59; live after the next `./deploy.sh`.

- 15:26, five design decisions: SLA cap 0.95 for now; the arena policy generator writes code for new
  policies as well as tuning parameters; prefix-sharing topology is derived from a session model with
  fork-off and merge-back rates; Leaf shards are separate processes in a sharded server; memory tiers
  are Ingress-owned with a seeded bloom-filter residency hint. Recorded in `docs/ARCHITECTURE.md` §14.
- `lbsim-prod` removed by you at 15:26. It hosted the `lbsim.ai` DNS zone, see item 1.

- Budgets: `lbsim monthly` $100 and `lbsim monthly cap` $50 on billing account `015B1A-AA7EAB-107FD2`,
  confirmed by you in the console at 15:15. Claude can no longer see billing; re-check as yourself with
  `gcloud billing budgets list --billing-account=015B1A-AA7EAB-107FD2`.
- Deploy credential: `lbsim-deployer@lbsim-gcp` only, no billing, cannot read or grant IAM, verified by
  a real attempt. Service Usage Consumer granted by you at 15:15. The deploy went through at 15:22 and
  needed no other grant: the real blocker was two bucket permissions, worked around in `cloudbuild.yaml`.
- The service is **public**: mock data and published findings, nothing sensitive.
- Scope today: package B of `docs/scope-today.md` plus item 7, retry storm last, side missions in
  parallel. *"Don't cut the react dashboard"* and *"Start executing on the Arena story"*: both built.
- Staffing: three long-lived agents with disjoint file ownership, recorded in `CLAUDE.md`.
- Reviews ticked: execution plan, arena, ui-spec, agent architecture, ARCHITECTURE, calibration.
- Design: analytic epoch advancement, request cohorts parked, prefix caching in phase 2, tiers pooled
  per cluster, three-layer deployment, absolute epoch time in `uint64`, a 70-billion-parameter model
  on eight H100s as reference hardware, the arena objective as specified, scale to zero within ten
  replicas, no always-on. All in `docs/ARCHITECTURE.md` section 14.
