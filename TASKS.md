# TASKS — things that need Issao

Stack ranked. Highest first. Item 1 is the most valuable thing you can do for the project
right now. Claude keeps this file current; anything Claude can do alone is not here.

Each item says what happens if you do not answer, so nothing stalls indefinitely.

Last updated: 2026-09-06 12:12 by Claude.

---

## 1. BLOCKING — review the interfaces in `proto/`

Now the only blocking gate. The architecture decisions are answered; see below.

Twelve files, all compiling. Two are new since your feedback, and they are the ones worth your
attention first because they encode your three-layer sketch:

1. `leaf.proto` — Ingress to Leaf. `Leaf.Advance` is the synchronisation barrier and carries
   work, tier grants, control actions, completions, control telemetry, metrics and tier requests
   in one round trip. One message per shard per window is what makes it affordable.
2. `subscription.proto` — the O(1) observability contract, shared by both hops. **One deliberate
   departure from your sketch, please check it:** you specified that the frontend requests a time
   sampling factor based on intended simulation speed. That alone does not hold the bound, because
   raising the speed then raises the wire rate proportionally. So a subscription declares a
   wall-clock budget, updates per second and rows per update, and the server derives the simulated
   interval to fit and re-derives it whenever speed changes. Your preference is kept as
   `desired_sim_interval_ns`, honoured where it fits.
3. `ingress.proto` — Frontend to Ingress. Was `control.proto`. `StreamRun` is gone, replaced by
   the budgeted subscription; `SetSpeed`, `StepForward` and `Rewind` added.
4. `policy.proto` — the engine/policy seam and the referee. Decides whether the agent arena can
   be trusted.
5. `telemetry.proto` — the staleness boundary. Decides whether the control-theory dynamics are
   reachable.
6. `scenario.proto` — the whole configuration surface, and the largest file. The one you will
   live in.
7. `common.proto`, `request.proto` — vocabulary. Note `Truth`: it holds what the simulator knows
   and no policy may ever see.
8. `serving.proto`, `kv.proto`, `capacity.proto`, `metrics.proto` — the modelled data plane and
   results.

Leave corrections as marker lines directly in the files. That worked well; the watcher caught
your last batch within a minute of the push.

**If you say nothing:** Claude treats the interfaces as accepted and starts generating code from
them.

## 2. Five smaller decisions, all with a default so none of them blocks

From `docs/ARCHITECTURE.md` section 14. Each has a stated default being taken.

1. **Phase 1 cut.** Section 12 proposes phase 1 in full plus prefix affinity as v1.
2. **Leaf shards as threads, not processes.** Measured: a process boundary costs 50 to 100
   microseconds per barrier against a 0.5 ms lookahead, which breaks the 20x target. Threads cost
   1 to 5 microseconds. The proto boundary is preserved either way, so cross-process remains
   possible later at a lower realtime factor. Default: threads.
3. **Ban O(N) routing policies.** A full fleet scan per request costs 14 cores at target scale, so
   routing must be O(1) or O(log N) and "least loaded" must be an incrementally maintained index
   rather than a scan. Default: the harness *fails* a run whose policy would not hold at target
   scale, rather than warning.
4. **Cap the prefix-affinity index at Ingress** to a bounded top-K, which is what real routers
   use. Default: capped.
5. **Cluster tiers owned by Ingress**, with a tier operation as a modelled network round trip.
   Faithful, since reaching a pooled tier genuinely is a network operation, and it keeps shards
   free of shared mutable state. Default: yes.

## 3. Should Claude build the cargo workspace skeleton while gated?

Two of the three groundwork items are **done**, and both were worth doing:

- **Closed-form epoch math validated.** Exactly equivalent to per-step iteration. See item 1.
- **Event queue benchmarked.** 54 ns per event at fleet scale, roughly 2x inside budget, so
  open risk 1 is closed. It also proved one of Claude's own recommendations wrong: the
  hand-rolled heap proposed as a mitigation is slower than the standard library. Corrected in
  `docs/ARCHITECTURE.md` section 1.4.

Remaining: a **cargo workspace skeleton** with crate boundaries and proto codegen wired up, no
logic, so that blessing unblocks work immediately instead of after setup.

**Waiting on you**, because it presumes the crate structure in `docs/ARCHITECTURE.md`
section 10.5, which you have not blessed. Say the word and it is thirty minutes.

## 4. Do you have real traces for calibration, even aggregated?

**This is now the top technical risk**, since the queue risk is closed. Workload realism is the
weakest link in the design and the least glamorous thing to get right. Arrival burstiness,
prompt and output length distributions, and session continuation rates are what determine
whether any conclusion transfers to reality. Published numbers exist but are thin.

**If you say nothing:** Claude calibrates against published vLLM and SGLang benchmark figures
and documents the resulting uncertainty as a named risk.

## 5. Confirm the reference hardware and model for calibration

`docs/llm-serving-primer.md` section 10.7 assumes a 70-billion-parameter model on eight H100s.
Every number in the analysis follows from that pair, and the validated cost model reproduces
the published step-time table for it to within 0.1 ms.

**If you say nothing:** that pair stays the default.

## 6. What should the agent arena optimise?

Goodput alone is gameable: an agent can starve batch traffic to raise interactive goodput.
It likely needs a fairness or SLO-attainment constraint alongside it.

**If you say nothing:** Claude proposes a specific objective when the arena is built, rather
than guessing now.

## 7. Is the dashboard a separate workstream?

`VISION.md` section 6 describes a substantial product. Sharing a backlog with the engine will
let it expand without limit. See `docs/ARCHITECTURE.md` open risk 7.

**If you say nothing:** Claude scopes it separately against a frozen metrics and control
interface, and does not start it until the engine reproduces at least one dynamic.

---

## Heads up: something outside this session is moving the working tree

A VS Code Git extension appears to be attached to `/home/agents/repo/lbsim`. At 11:59 it stashed
uncommitted work and switched branches, which silently reverted a large edit to
`docs/ARCHITECTURE.md`. The work was recovered from the stash, nothing is lost, and everything
described above is committed.

Nothing needed from you unless you are pointing an editor at this sandbox, in which case that is
the cause and it will recur. `tools/sync.sh` now warns on an unexpected stash and on HEAD moving
between runs, so a repeat is visible immediately rather than discovered later.

## Done

- [x] **Architecture reviewed.** Findings 1, 3 and 4 accepted; finding 2 parked with a trigger
      and a knob design; the three-layer deployment supplied and now written up as
      `docs/ARCHITECTURE.md` section 10, with `leaf.proto` and `subscription.proto` to match.
      Your earlier recommendation against intra-cluster parallelism is withdrawn.

- [x] Write `VISION.md` sections 1-8.
- [x] Publish the repository to GitHub.
- [x] Install and authenticate `gh` so Claude can push.
- [x] Wire diagram: you opted out of drawing, Claude drew `docs/diagrams/system.drawio`.
- [x] `VISION.md` section 9 now points at `docs/ARCHITECTURE.md` section 10.
