# Agent architecture

How to staff this project with Claude instances, who owns what, and the rules that keep parallel
work from colliding. Reviewed by Issao. Sections 0, 6 and 7 are the proposal as written at design
time, kept because the reasoning is what the decisions were made against; what runs now is below.

Every recommendation here is grounded in something that actually happened in the first session
rather than in general principle, and each says which.

---

## What runs now, since 15:00

Issao overrode the two-agent recommendation once the build stalled on cloud work: *"delegate all
cloud deployment to a separate agent. delgate development progress to a TL ... Delegate to a
separate agent to mointor for my prs and comments to you, as well as do housekeeping/simplify/remove
stale content of the md files meant to interact with me."*

So three long-lived agents run, with the file ownership table in `CLAUDE.md`: a tech lead on
`src/`, `tests/`, `scenarios/` and `web/src/lib/`; a cloud agent on the container and deploy files;
and a monitor and housekeeping agent on `TASKS.md`, `STATUS.md`, `README.md` and `docs/`. The main
agent coordinates and owns `CLAUDE.md` and `proto/`.

Three departures from the proposal, stated so nobody mistakes the document for the practice:

- The tech lead writes code as well as specifications. Section 1.2's five-field work unit is still
  the bar for anything it fans out.
- The inbox is an agent running the scripts on a ninety-second loop, not a script alone. Section 3
  still holds: `tools/inbox.py` and `tools/sync.sh` find every instruction, the agent only acts.
  PR comment ingestion, section 8 item 1, is done by that loop by hand and is still not a script.
- Rule 3 of section 4, one worktree per agent, was only partly kept at first: two agents shared
  `/home/agents/repo/lbsim`, and one agent's in-flight files reached another's commit. Since 15:40
  every agent, subagents included, works in its own worktree with the commands in section 4.3, and
  `CLAUDE.md` records the rule.

---

## 0. The headline: two roles now, seven later

The right staffing today, with the protos under review and no code written, is **two**: an
Architect and a Verifier. Fan-out before the interfaces are blessed produces incompatible
designs, and this project has already demonstrated why in miniature. The full structure below
is what to grow into, not what to start with.

Section 6 gives the staffing per phase.

---

## 1. The roles Issao asked for

### 1.1 Architect

**Owns:** strategy, `VISION.md` alignment, `docs/ARCHITECTURE.md`, everything in `proto/`, the
decision log, and **reading and reacting to Issao's comments** wherever they appear: in chat, as
in-file markers, and as GitHub PR review comments.

**Does not:** write feature code. The moment the Architect starts implementing, its context fills
with implementation detail and the strategic judgement it exists for degrades.

**The hard constraint on this role is context.** Over one session the Architect's context filled
with proto field numbering, and that is precisely the material that crowds out the design thinking
it is there to do. Two mitigations, both already in place and both mandatory rather than optional:

- **Externalise state into files.** `docs/ARCHITECTURE.md`, `TASKS.md`, `STATUS.md` and the
  decision log are the Architect's memory. A fresh Architect reading those should be able to
  continue without the transcript. Test that claim occasionally by actually starting fresh.
- **Delegate reading.** Anything that means opening many files goes to a subagent that returns a
  conclusion, not excerpts.

**Owns the inbox, but not as a judgement call.** See section 3: the watcher must be a script.

### 1.2 Execution TL

**Owns:** decomposing blessed design into work units, farming them out, integrating, keeping
`master` green, and merge order.

**Does not:** write feature code either, and does not make design decisions. A question that needs
a design answer goes back to the Architect rather than being resolved in a work unit, because a
decision made inside an implementation is invisible to everyone else.

**The TL's real job is writing specifications tight enough that fan-out is safe.** A work unit is
only ready when the TL can state, in writing:

| Field | Meaning |
|---|---|
| Contract | which proto messages and services it implements, verbatim |
| Files owned | exclusive write access; no other unit may touch them |
| Invariants | what the referee must accept, what must never happen |
| Tests | the specific tests that must pass, named before the work starts |
| Done | an observable condition, not "it works" |

**If the TL cannot fill all five, the unit is not ready and goes back to the Architect.** This is
the single most important rule in the document. Four agents on a fuzzy spec produce four
incompatible designs, and the cost is not the wasted work but the plausible-looking integration
that follows.

---

## 2. The roles I would add

### 2.1 Verifier, or red team

**The highest-value non-obvious role, and I would hire it before any implementer.**

**Owns:** adversarially checking claims. Not code style, not lint. Numbers, reasoning, and whether
a stated conclusion follows from its evidence.

**Why, concretely.** In one session my own architecture document was wrong twice in ways that
would have propagated into code:

1. I predicted a flat event queue would cost 1-2 microseconds per operation and proposed a
   hand-rolled heap as the mitigation. Measurement showed 441 ns, and the proposed mitigation was
   *slower* than the standard library at every size. A Verifier with a mandate to demand a
   benchmark before a mitigation is adopted would have caught it.
2. I quoted 9,000 output tokens per second per replica as a fleet budget input. It was decode-only
   at full batch. With prefill included the real figure is 4,533 at a chat-like mix and 889 at a
   prompt-heavy one. **Issao caught this, not me**, and it had already propagated through the
   request-rate, in-flight-count and event-budget numbers.

**Standing mandate**, which is what makes the role work rather than making it advisory:

> Every quantitative claim in a document must be reproducible by a script in `bench/`, or be
> explicitly marked as an estimate with a range. A claim that is neither is a defect, and the
> Verifier files it as one.

That rule is cheap to enforce and it converts the failure mode above from "someone notices later"
into "CI notices now". `docs/calibration.md` already applies the same discipline to external
numbers with its provenance tags; this extends it to our own.

**Also owns** the determinism fingerprint tests, including the one that runs a scenario at several
shard counts and asserts identical output. That test is the tripwire for the whole parallel design
and it belongs to someone whose job is suspicion.

### 2.2 Physics owner

**Owns:** the cost model, the referee, the memory-tier and bandwidth-container model, and the
naive-versus-fast differential oracle Issao asked for.

**Why separate from implementers.** A subtle error here produces plausible numbers that no
ordinary test catches, and every conclusion the simulator ever produces depends on it. Two things
from this session make the case. The compute-bound branch was missing entirely until Issao asked
about it, and bandwidth-only would have systematically overstated speculative decoding and
quantization, which are exactly the policies under study. And a true-division bug in the reference
model silently broke the exact-arithmetic proof while every float-based check still passed, because
a float answer is correct to 1e-16 and only exact arithmetic notices.

This role and the Verifier are close cousins. Keep them separate because this one is a builder
with deep persistent domain state, and the Verifier must stay free to disbelieve it.

### 2.3 Calibration owner

**Owns:** `docs/calibration.md`, trace ingestion, and the sensitivity sweeps that establish which
parameters change conclusions.

**Why.** Workload realism is the top remaining technical risk, and it is research-shaped rather
than build-shaped: it needs someone who will download a trace and compute statistics from it. That
already paid for itself once, finding that the achievable prefix-reuse ceiling on production traces
is 0.37 to 0.55 rather than the 0.9 that benchmark numbers imply, and that burstiness is
timescale-dependent in a way a single coefficient of variation cannot express.

Runs episodically, not continuously. A long-lived calibration agent has nothing to do most of the
time.

**Its second job matters as much as the first:** the sensitivity sweep. A conclusion that flips
when the utilization constants move by 30% is not a conclusion, and someone has to own finding out.

### 2.4 Frontend owner

**Owns:** `web/`, against frozen `ingress.proto` and `subscription.proto`.

**Why separate.** The dashboard described in `VISION.md` section 6 is a substantial product on its
own. Sharing a backlog with the engine lets it expand without limit, which is already recorded as
a risk. Starting it before the engine reproduces at least one dynamic means building panels for
data that does not exist yet.

### 2.5 Implementers, ephemeral and disposable

**Owns:** one work unit each, per the TL's five-field specification.

Deliberately short-lived. An implementer that survives several work units accumulates context and
starts making decisions that belong to the Architect. Spawn, deliver, discard.

---

## 3. The watcher is a script, not an agent

Issao's comments arrive in three places: chat, in-file markers, and GitHub PR review comments. The
temptation is to staff that with an agent. **Do not.**

A missed instruction is the worst failure mode this project has, and reliability beats judgement
for it. A script does not forget, does not run out of context, and does not decide something looks
unimportant. `tools/inbox.py` and `tools/sync.sh` do this today, and their history is the argument:

- The first version anchored marker matching to the start of a line. **Four of the seven
  instructions in one proto file were invisible**, because they trailed code as comments. An agent
  reading the same file might have caught them, or might not, and there would be no way to know
  which.
- One instruction carried no name prefix at all. Marker scanning could not find it in principle,
  so `sync.sh` now diffs upstream commits and surfaces every comment line they added.
- The scanner also matched its own documentation, reporting three instructions that did not exist.
  A tool that cries wolf gets ignored, which is the same failure with extra steps.

Each of those was a mechanical fix with a regression test. None of them is a judgement call.

**What the Architect owns is reacting**, and the protocol is act, then delete, then quote the
instruction in the commit message. A marker still in the tree means the work is not done, which
makes the tree itself the queue and removes any possibility of a silently dropped item.

**Add PR review comments to the same pipeline.** `gh pr view --json reviews,comments` plus the
same act-then-resolve discipline. Not built yet; it belongs in the next tooling pass.

---

## 4. Rules that make parallelism safe

Ordered by how much damage ignoring them causes.

1. **Protos are the contract, and only the Architect changes them.** An implementer that needs a
   field change stops and asks. A proto change invalidates in-flight fan-out, so the TL must drain
   dependent units before it lands.
2. **One writer per file, declared up front.** Two of today's git incidents came from concurrent
   writers to the same paths. The TL maintains an ownership map, and a unit that needs a file it
   does not own is not ready.
3. **Every agent gets its own git worktree, with a single writer.** An outside process, most likely
   a VS Code Git extension attached to the working directory, stashed uncommitted work and switched
   branches mid-edit, silently reverting a large document. With several agents in one directory that
   becomes routine rather than exceptional. `sync.sh` now warns on an unexpected stash and on HEAD
   moving between runs, which is a detector, not a cure.

   The cure, in three commands, which every subagent runs before touching a file:

   ```bash
   git fetch origin
   git worktree add /home/agents/repo/lbsim-wt-<name> -b claude/tl-<name> origin/master
   cd /home/agents/repo/lbsim-wt-<name>   # build only through tools/build.sh
   ```

   Each worktree has its own cargo target directory (`.cargo/config.toml`); a shared one handed a
   worktree another branch's compiled crates on 2026-09-06. `tools/build.sh` bounds concurrent
   builds to two machine-wide. When the branch is merged, `git worktree remove` deletes the
   directory and its target with it.
4. **Never leave the tree dirty, and always push to `master`.** Issao pulls `master`; work sitting
   on an unpushed branch does not exist. `sync.sh` refuses to operate on a dirty tree for this
   reason.
5. **Fan out on independent files, stay sequential on coupled work.** The engine core, the replica
   model and the policy set are coupled through the cost model and must not be parallel. Individual
   policies against a frozen trait are ideal fan-out: same contract, different files.
6. **Concurrency is bounded by Issao's review bandwidth, not by agent availability.** Six agents
   producing six PRs an hour is a denial of service on the one human. Two or three concurrent units
   with real review is worth more than six without.
7. **One PR per work unit**, so review comments land on specific lines. That is the interaction
   Issao asked for, and it only works if units are small enough to review in one sitting.

---

## 5. Model selection

| Role | Model | Why |
|---|---|---|
| Architect | strongest available | wrong design decisions are the expensive ones |
| Verifier | strongest available | its job is finding subtle errors; a cheaper model finds fewer |
| Physics owner | strongest available | see the true-division bug |
| TL | strong | specification quality determines whether fan-out is safe |
| Implementers | mid-tier, once the spec is tight | mechanical work against a written contract |
| Calibration | strong, with web access | judgement about source quality is the whole job |
| Frontend | mid-tier | conventional work against a frozen interface |
| Watcher | none, it is a script | reliability beats judgement |

---

## 6. Staffing by phase

| Phase | Agents | Notes |
|---|---|---|
| **Design under review**, the morning | Architect, Verifier | Nothing to fan out. The Verifier's first task is auditing the numbers already in `docs/ARCHITECTURE.md` against `bench/`. |
| Protos blessed, skeleton | Architect, TL, Physics | TL builds the workspace and CI; Physics builds the cost model and the differential oracle. Still no fan-out. |
| Core engine | + 1-2 implementers | Event queue, replica model, metrics. Coupled, so sequential with review between. |
| Workload and policies | + 2-3 implementers, Calibration | First real fan-out: one policy per unit against a frozen trait. |
| Failures, scenarios | + 1-2 implementers | Fan-out by scenario. |
| Dashboard | + Frontend | Only once the engine reproduces a dynamic worth displaying. |

### 2.6 The arena agents

`docs/arena.md` specifies three, and they are agents in this structure rather than a separate
system. Ordered by when they can exist: none of them until the engine reproduces a dynamic.

| Role | Model | Reads | Writes | Lifetime |
|---|---|---|---|---|
| **Policy generator** | strong | the previous round's full results, including traces and referee counters | new policy candidates as typed `PolicySpec` values | one round, then discarded |
| **Load generator** | strong | the previous round's results, and which policies survived | new `LoadShape` candidates, plus the mandatory vanilla shapes | one round |
| **Arena referee, judging half** | strongest | the round's scores, both archives, and the rule-change log | proposed rule changes, for a human to accept | long-lived, but see below |

Three structural points, because getting these wrong makes the arena measure its own noise:

**The mechanical referee is not an agent.** Physics invariants, the realism envelope,
rated-capacity honesty, the SLA gate, and determinism replay are all code, per `docs/arena.md`
section 2.3. Only the residual judgement goes to an agent, and its output is a *proposal* rather
than a score adjustment. A judge that can silently change scores is a judge that can be argued
with.

**Generators are ephemeral, and that is not an optimisation.** A long-lived generator accumulates
context about what it already tried, which sounds useful and is how a search collapses into a
niche. The *archive* is the memory, it is on disk, and a fresh generator reading it is the design.

**The judging referee is long-lived but must externalise its rules.** The rule set lives in a file
with a version, and every score records which version it was earned under. Rules apply forward
only. Without that, "the referee makes new rules" quietly destroys the ability to tell whether
anything improved.

Where the arena sits relative to the rest of this structure: the Architect owns the rule set and
accepts or rejects proposed changes, the TL owns running rounds as batch jobs, and the Verifier owns
the fixed held-out suite from `docs/arena.md` section 5, because the whole point of that suite is
that nobody with an interest in the scores can touch it.

---

## 7. Anti-patterns, each one a mistake available today

- **Spawning implementers before the protos are blessed.** That was where the project stood at
  design time, and it is why the recommendation was two agents rather than seven.
- **Two agents owning the cost model.** Guarantees divergence in the one place divergence is
  invisible.
- **An implementer changing a proto** to unblock itself. Silently breaks every parallel unit.
- **Using an agent for the inbox.** See section 3.
- **A long-lived implementer.** Accumulates context, starts making design decisions, and nobody
  notices until a decision surfaces in a diff.
- **Fan-out measured in agents rather than in reviewed PRs.** The bottleneck is human review.
- **Trusting an agent's report without a reproducible artefact.** Including mine: two of my own
  claims this session were wrong, and both were caught by measurement rather than by reasoning.

---

## 8. What is missing before this can run

1. **PR comment ingestion.** Marker scanning and upstream comment diffing exist; GitHub review
   comments do not yet feed the same pipeline. Needed before PR-per-unit review works.
2. **The ownership map.** A file listing which role owns which paths, machine-checkable in CI.
3. **A work-unit template**, so the TL's five fields are filled by construction rather than by
   memory.
4. **The determinism fingerprint test.** Cannot be written until the engine exists, but it is the
   tripwire for the whole sharded design and should be written the same day the shard boundary is.
