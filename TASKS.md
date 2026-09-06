# TASKS — things that need Issao

Stack ranked. Highest first. Item 1 is the most valuable thing you can do for the project
right now. Claude keeps this file current; anything Claude can do alone is not here.

Each item says what happens if you do not answer, so nothing stalls indefinitely.

Last updated: 2026-09-06 13:15 by Claude.

---

## 1. Review `docs/execution-plan.md`

You asked for an execution plan with a GCP deployment goal and a fastest-local-iteration dev plan.
Ten milestones, each with an observable definition of done. Three things in it worth your
attention:

**M1 is a walking skeleton, not a layer.** It targets the round-robin rolling hotspot end to end,
with decode disabled so the workload behaves like stateless serving. That produces a genuinely
useful result in two or three days and exercises every layer, at the cost of the first working
thing not being LLM-specific yet.

**Deployment is reworked for your budget constraint, and one part of the ask does not survive
contact with the measurements.** Section 3 now has two shapes rather than one: a Cloud Run service
that scales to zero for interactive use, capped at three instances, and Cloud Run **Jobs** for
sweeps, which cannot idle by construction and are where most compute will actually go. Approximate
cost: one always-warm instance is about $274 a month, while moderate real use is about $18. So
scale-to-zero is the entire cost story.

The part that does not survive: **autoscaling on the scale of a single simulation is not worth
building.** One core carries the whole 6,250-replica fleet at 20x realtime, so a bigger scenario
needs a bigger instance, not more of them, and spreading one run across processes costs more in
synchronisation than it gains. The rule is to scale on *queued runs*, and the replica budget of ten
becomes `--parallelism 10` on Jobs. If a single scenario ever exceeds one instance the first move is
more vCPU, not a second process.

**The cost trap worth your attention.** A live streaming subscription is a request in flight, and
Cloud Run keeps an instance alive while one exists. A forgotten browser tab would therefore pin an
instance and cost more than all deliberate use. The fix is application-level idle shutdown plus
bounded streams, which the time-leased subscriptions in the protos already set up. Section 3.5
item 2, and it is the most important line in the deployment plan.

**One reversal of an earlier decision, please sanity-check it.** Scenarios in protobuf text format
rather than TOML. `Scenario` is already a proto message, so TOML would mean a converter and two
places for a default to drift. If hand-authoring prototxt turns out to be annoying we add a TOML
front end then.

Section 3.3 lists four things that must be true in the code for the Cloud Run deployment to work,
all of which are unpleasant to retrofit. Worth a look even if the rest can wait.

**If you say nothing:** Claude starts at M0, the workspace and CI, which is half a day and commits
to nothing that later milestones cannot change.

## 2. Review `docs/agent-architecture.md`

Proposed structure for staffing this with agents. Headline recommendation you may not like: **two
agents now, not seven.** Architect and Verifier. Fan-out before interfaces are blessed produces
incompatible designs.

Beyond the Architect and TL you named I argue for four more. The one I would hire before any
implementer is a **Verifier**, because my own architecture document was wrong twice in one session
in ways that propagated: a queue mitigation that measurement showed was slower than the standard
library, and a decode-only throughput figure quoted as a fleet budget input, which you caught
rather than I did. Its standing mandate would be that every quantitative claim must be reproducible
by a script in `bench/` or marked as an estimate.

One recommendation to push back on if you disagree: **the comment watcher stays a script, not an
agent.** A missed instruction is the worst failure this project has, and four of seven instructions
in one proto file were invisible until a mechanical fix with a regression test.

**If you say nothing:** Claude works as Architect with an ad-hoc Verifier pass per unit, and does
not fan out.

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

   Issao: all of these sound good to me.

## 4. Superseded: the workspace skeleton is now M0 of the execution plan

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

Issao: sounds good to me. We can tune later.

## 7. What should the agent arena optimise?

Goodput alone is gameable: an agent can starve batch traffic to raise interactive goodput.
It likely needs a fairness or SLO-attainment constraint alongside it.

**If you say nothing:** Claude proposes a specific objective when the arena is built, rather
than guessing now.

Issao: Here is what it should optimize. It should achieve maximum goodput while keeping out
of SLO sessions under an SLA cap (say 99.9%). That should remain the case as long as the
load is under a rated load capacity, even for an adversarial load generator. We should structure
the arena this way. There is a referee that defines the rule of the game tallies the results and
checks if the load generator or policy generator are doing anything that violates the spirit of
the game (identify difficult but realistic work loads and building policies that served a good
service quality at maximum goodput). The referee reviews the work and makes new rules if it
looks like they are gaming the system. The policy generator reviews the outcome of the prior run
(including all the metrics) and proposes changes to policy. The load generator reviews the prior
run and tries to stress test policies, but also always include some more vanilla/average load
shapes to give the policy good calibration. Each generator keep a set of top 10 candidates.
The policy is allowed to spill traffic above its rated capacity without SLO cost. The policy
can dynamically update its rated capacity at warm up for the initial phase of simulation, the
referee is measuring the metrics after that initial phase.

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

- [x] **All proto feedback folded in and the interfaces simplified.** Twenty-one instructions across
      two rounds. The pass removed one file, one duplicated workload model, thirty hand-named
      scorecard fields, seven identifier wrapper messages, two redundant services and the per-token
      stream.

- [x] **Architecture reviewed.** Findings 1, 3 and 4 accepted; finding 2 parked with a trigger
      and a knob design; the three-layer deployment supplied and now written up as
      `docs/ARCHITECTURE.md` section 10, with `leaf.proto` and `subscription.proto` to match.
      Your earlier recommendation against intra-cluster parallelism is withdrawn.

- [x] Write `VISION.md` sections 1-8.
- [x] Publish the repository to GitHub.
- [x] Install and authenticate `gh` so Claude can push.
- [x] Wire diagram: you opted out of drawing, Claude drew `docs/diagrams/system.drawio`.
- [x] `VISION.md` section 9 now points at `docs/ARCHITECTURE.md` section 10.
