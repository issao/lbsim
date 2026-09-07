# TASKS — things that need Issao

Last updated: 2026-09-07 11:17 PDT by Claude.

**Resumed 2026-09-07 11:17 PDT.** The fleet stopped at 21:00 on 2026-09-06 at your request (*"we are approaching
limit... tie up loose ends... give me a doc summarizing the state and most interesting outcomes with deep
links and I will review and wrap up tomorrow"*) and is back this morning: the tech lead (`a0899ca58df6febdc`)
is on U79–U83, the Cloud Run live-server fix and your product-polish pass, per main. This file is the review
list, stack ranked, each item with what Claude does if you say nothing. What is finished and live is in
`STATUS.md`, whose top caveat is the known issue from `docs/wrap-up-2026-09-06.md` §5b: the live server on
Cloud Run degrades after a handful of runs, so open one tab at a time until U79 lands; replay (`?server=off`)
is unaffected.

## 0. Read `docs/wrap-up-2026-09-06.md`

The main agent's summary of the day: state, the most interesting outcomes, deep links into the
findings, the graph, the deployed site and the catalog. On `master` since 5da8a67 (merged cc8a229, 21:01 PDT).

**If you say nothing:** the agents resume tomorrow from their files with the graph's queue as is.

## 1. Review the `TraceSpan` extension in `proto/lbsim/v1/metrics.proto`

On `master` at f5eddf1 (a035514, 17:20), acting on *"We should have a way to sample requests to see
execution traces and what was busy in each resource as it executed."* `TraceSpan` now carries the
resource state the engine knows at the step: `batch_size`, `queued`, `kv_tokens_resident`,
`kv_capacity`, `step_ns`, `bound` (bandwidth or compute), and for routing spans the `candidates`
considered and `stale_view_age_ns` of the view they were scored on; `RequestTrace` gains the
`TraceBucket` the sampler stratifies on (p50, p90, p99, p99.9). Existing field numbers unchanged;
operation names now match the engine's (`queue`, `route`, `prefill`, `decode`, `kv_fetch`,
`preempted`). It matches `sim_metrics::trace::ResourceState` field for field.

**If you say nothing:** it stands as written.

## 2. Review `docs/vision-progress.md`

Snapshot at 2026-09-06 16:19 PDT, on `master` d42546a: every `VISION.md` requirement classified as done,
building in the next two hours, or far away, each with one line of evidence. It ends with five priority
far-away items and eight requirements no plan document mentions: the `disable_decode` knob, load and
latency forecasting, redundancy policies, model-weight locality, per-machine trace spans, the fluid
limit, and the training-versus-serving question.

**If you say nothing:** it is deleted at the next housekeeping round after 24 hours, 2026-09-07 16:19. The
reading is stale by a day of work now; `docs/execution-graph.md` and `STATUS.md` are current.

## 3. Delete the stale `claude/*` branches on `origin`

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

## 4. Routed to the tech lead, for the record

Each of these is a unit in `docs/execution-graph.md`, the tech lead's graph, which carries its state
and ETA; this list is the record of what was routed and why.

**2026-09-07, routed to the tech lead (`a0899ca58df6febdc`):** your instruction this morning, the
product-polish pass now running as part of U79–U83 per main. The tech lead records it verbatim in its
`docs/execution-graph.md` entry; that entry was on no branch on `origin` and in no worktree at 11:17, so the
verbatim text is quoted here the round after it lands rather than paraphrased now, and the tech lead has been
asked for it. **If you say nothing:** the tech lead's reading of it in the graph stands.

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

## 5. Decisions with a default

`docs/execution-graph.md` has a section "Waiting on Issao" listing the units that need a word from
you, each with the default it takes if you say nothing. Those defaults stand until you say otherwise.
The design decisions below are the ones outside that graph.

| Decision | Default | Rework if changed later |
|---|---|---|
| A routing policy that scans the fleet fails the run rather than warning (`docs/ARCHITECTURE.md` §10.4) | fail | small |
| Prefix-affinity index at Ingress is a bounded top-K, not exact (§10.4) | bounded | small |

## 6. Later, when this phase ends

- [ ] Delete the deploy key. `gcloud iam service-accounts keys list --iam-account=lbsim-deployer@lbsim-gcp.iam.gserviceaccount.com`
      shows the id, which starts `94afd556`; then `keys delete KEY_ID --iam-account=...`.
- [ ] Grant `roles/logging.viewer` to the deploy account **only when a container fails on startup**.
      Today it cannot read container or request logs; nothing has needed them yet, and `docs/deploy.md`
      records the gap. Not worth granting pre-emptively.

- [ ] Delete the leftover test image, which the deploy account cannot delete itself:
      `gcloud artifacts docker images delete us-central1-docker.pkg.dev/lbsim-gcp/lbsim/lbsim:wstest --project lbsim-gcp --quiet`.
      If you do nothing it sits in the registry costing cents.
- [ ] Two more grants to hold back until needed, both from `docs/deploy.md`: `storage.buckets.update`
      for the 90-day delete rule on `gs://lbsim-gcp-runs`, which matters once runs write results there;
      and `artifactregistry.repoAdmin`, or running the tear-down as yourself, when this phase ends.

**If you do nothing:** the key stays until you delete it, the first startup crash is undiagnosable
until you grant the role, and the results bucket keeps everything it is ever given.

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
