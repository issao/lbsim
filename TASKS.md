# TASKS — things that need Issao

Stack ranked. Highest first. Item 1 is the most valuable thing you can do for the project
right now. Claude keeps this file current; anything Claude can do alone is not here.

Each item says what happens if you do not answer, so nothing stalls indefinitely.

Last updated: 2026-09-06 12:50 by Claude.

---

## 1. Review `docs/agent-architecture.md`

You asked for a proposed agent structure. Written up, with a recommendation you may not like:
**staff two agents now, not seven.** Architect and Verifier. Fan-out before the protos are blessed
produces incompatible designs, and this project has already shown why in miniature.

Beyond the Architect and TL you named, I argue for four more, in priority order. The one I would
hire before any implementer is a **Verifier**, because my own architecture document was wrong twice
in one session in ways that propagated: I proposed a queue mitigation that measurement showed was
slower than the standard library, and I quoted a decode-only throughput figure as a fleet budget
input, which you caught rather than I did. Its standing mandate would be that every quantitative
claim must be reproducible by a script in `bench/` or marked as an estimate.

The other three are a Physics owner for the cost model and the differential oracle, a Calibration
owner for trace work and sensitivity sweeps, and a Frontend owner against frozen interfaces.

One recommendation to push back on if you disagree: **the comment watcher should stay a script, not
an agent.** A missed instruction is the worst failure this project has, and today four of seven
instructions in one proto file were invisible to the scanner until I fixed it. That was a mechanical
bug with a regression test. An agent might have caught them, or might not, and there would be no
way to know which.

**If you say nothing:** Claude proceeds as Architect plus an ad-hoc Verifier pass on each unit of
work, and does not fan out.

## 2. BLOCKING — the interfaces in `proto/`

Thirteen files. Your subscription and telemetry feedback is folded in and the markers are removed;
see `STATUS.md` for what changed. Still unreviewed: `policy.proto`, `leaf.proto`, `scenario.proto`,
`workload.proto`, `ingress.proto`, `metrics.proto`, `common.proto`, `request.proto`,
`serving.proto`, `kv.proto`, `capacity.proto`.

Suggested order by consequence: `policy.proto` first, since it decides whether the agent arena can
be trusted; then `leaf.proto`, which carries your three-layer sketch; then `scenario.proto`, the
largest and the one you will live in; then `workload.proto`, which is the load-shape interface you
asked for.

Two notes on things I changed beyond what you flagged. Absolute epoch time is applied to all
thirteen files rather than only `telemetry.proto`, and it turns out to *remove* an option:
float64 resolves only about 200 ns at 1.79e18, so no simulated timestamp may pass through a float.
And on percentiles at the leaf, I did not do quite what you suggested, because percentiles do not
merge; the engine computes them at the Leaf when one shard owns the target and merges histograms
otherwise, with a flag saying which.

Leave corrections as comments in the files, prefixed or not. Both are caught now.

**If you say nothing:** Claude treats the interfaces as accepted and starts generating code.

## 3. Five smaller decisions, all with a default so none of them blocks

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

## 4. Should Claude build the cargo workspace skeleton while gated?

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

## 5. Real traces for calibration: partly answered already

`docs/calibration.md` now exists: 2,000 lines, every number carrying a provenance tag, and the
strongest ones computed directly from the primary datasets rather than quoted. Four traces are
replayable. Azure LLM 2024 and BurstGPT give multi-day fleet arrivals with no sharing structure,
Mooncake is the only one with prefix structure but covers one hour, TraceLab gives real agentic
session structure with only 43 users.

Still worth asking: **do you have anything internal, even aggregated?** The largest unmeasured lever
is prefix-sharing topology, meaning how many distinct system-prompt roots exist and how skewed their
popularity is. Nothing public reveals it, and it moves achievable cache hit rate a long way.

**If you say nothing:** Claude sweeps that parameter rather than fixing it, and reports conclusions
as ranges.

## 6. Confirm the reference hardware and model for calibration

`docs/llm-serving-primer.md` section 10.7 assumes a 70-billion-parameter model on eight H100s.
Every number in the analysis follows from that pair, and the validated cost model reproduces
the published step-time table for it to within 0.1 ms.

**If you say nothing:** that pair stays the default.

## 7. What should the agent arena optimise?

Goodput alone is gameable: an agent can starve batch traffic to raise interactive goodput.
It likely needs a fairness or SLO-attainment constraint alongside it.

**If you say nothing:** Claude proposes a specific objective when the arena is built, rather
than guessing now.

## 8. Is the dashboard a separate workstream?

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
