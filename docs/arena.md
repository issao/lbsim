# The policy arena

Design, from Issao's specification, and the authority for the rules. The mechanical half is built, at
his instruction, in `src/arena.rs`; `docs/arena-implementation.md` records what exists and what it
measured. Section 5b records two rule changes the measurements demand, which need him.

---

## 1. The objective, as specified

> Achieve maximum goodput while keeping out-of-SLO sessions under an SLA cap (say 99.9%). That
> should remain the case as long as the load is under a rated load capacity, even for an
> adversarial load generator.

Unpacked into something scoreable:

```
score(policy) = min over all load shapes L in the load archive of:
                  goodput(policy, L)   if slo_attainment(policy, L) >= SLA_CAP
                  0                    otherwise
                where offered_load(L) <= rated_capacity(policy)
```

Three properties of that definition are worth stating, because they are what make the game work
rather than incidental:

**It is a minimum, not a mean.** The requirement is that the guarantee hold *even for an
adversarial load generator*, so a policy is scored by its worst case across the load archive. A
policy that is excellent on average and collapses on one realistic shape has not met the
specification.

**The SLA cap is a gate, not a term.** Breaching it scores zero rather than costing points. That
prevents the obvious trade of a little quality for a lot of throughput, which is the whole failure
mode the objective exists to rule out.

**Rated capacity is declared by the policy, and that is the anti-gaming mechanism.** See section 3.

---

## 2. The three agents

### 2.1 Policy generator

Reviews the previous round's outcome, including every metric, and proposes changes to policy.
Maintains an archive of its **top ten candidates**.

What it may see: everything in the run result. Scorecards, time series, sampled traces, referee
counters. It is reasoning about its own past behaviour, which is legitimate and is the point.

What it may not do: anything the physics referee in `docs/ARCHITECTURE.md` section 5.2 forbids.
That is enforced structurally rather than judged, because an `Observation` contains no fresh state
and no `Truth`, and an `Intent` cannot express a physics violation. In the arena the referee runs
in **strict** mode, so a violation aborts the run and the candidate scores nothing.

### 2.2 Load generator

Reviews the previous round and tries to stress the policies. Maintains its own archive of **top
ten candidates**.

Per Issao's instruction it must **always include vanilla shapes**: every batch contains some
average, unremarkable load so the policy gets calibration signal rather than only adversarial
signal. Without that, co-evolution drifts into a corner where both sides are excellent at a game
nobody plays. Section 5 argues this is necessary but not sufficient.

Its adversarial freedom is bounded by realism, and bounded **mechanically**: every load it
proposes is a `LoadShape` from `workload.proto`, and every parameter must fall inside the envelope
measured in `docs/calibration.md`. A load of a million-token prompts at ten thousand requests per
second is not a clever attack, it is an invalid submission, and the referee should reject it
without needing judgement.

### 2.3 Referee

Defines the rules of the game, tallies results, and checks whether either generator is violating
the spirit of it: *identify difficult but realistic workloads, and build policies that serve good
service quality at maximum goodput.* When it looks like someone is gaming, the referee makes new
rules.

**Split the referee in two, and keep the mechanical half much larger than the judging half.** This
is the one place I would extend Issao's design, because a judge that is itself an agent is a judge
that can be argued with, drift, or be gamed in turn.

| Mechanical referee, in code | Judging referee, an agent |
|---|---|
| physics invariants, strict mode | is this load realistic in a way the envelope misses? |
| realism envelope on every load parameter | is this policy exploiting a simulator artefact? |
| rated-capacity honesty, section 3 | has the game degenerated into a narrow niche? |
| SLA gate and worst-case scoring | should a new rule be added, and what? |
| reproducibility: config plus seed recorded | |
| determinism: identical fingerprint on replay | |

Everything that can be a check should be a check. The agent handles only the residual, and its
output is a **proposed rule change** that a human accepts, not an immediate score adjustment.

**Rule changes apply forward, never backward.** A new rule invalidates old scores, so the archive
records which rule set each score was earned under, and a round is only ever compared within one
rule set. Without that, "the referee makes new rules" quietly destroys the ability to tell whether
anything is improving.

---

## 3. Rated capacity: the mechanism that makes honesty pay

Issao's design, and the cleverest part of it:

> The policy is allowed to spill traffic above its rated capacity without SLO cost. The policy can
> dynamically update its rated capacity at warm up for the initial phase of simulation, the referee
> is measuring the metrics after that initial phase.

So the policy declares a number. Above it, shedding is free. Below it, the SLA gate applies.

That makes the declaration a genuine commitment with a two-sided cost:

| The policy declares | What happens |
|---|---|
| too low | it sheds traffic it could have served, so goodput and therefore score fall |
| too high | it must meet the SLA gate on load it cannot handle, breaches, and scores zero |
| honestly | maximum score |

No judgement required. The incentive does the work, which is exactly what an anti-gaming rule
should look like.

**One consequence worth designing for.** Because capacity is declared during warm-up, a policy
could inspect the warm-up load and declare a number tuned to it. Issao's design already handles
this, though it is worth making explicit: **the load generator is free to change the load after
warm-up**, so an over-claim tuned to an easy warm-up is punished in the measured phase. That
tension is the core adversarial dynamic of the whole game, and the load generator should be
explicitly told to exploit it.

**A second consequence.** Warm-up must be long enough for the fleet to reach steady state, or a
policy is declaring capacity from a transient. `docs/ARCHITECTURE.md` section 8 already excludes
warm-up from statistics; the arena additionally requires that autoscaling have settled, which
means warm-up longer than a cold start, so at least several minutes of simulated time.

---

## 4. A round

```
for each round:
  1. Referee assembles the load slate: every candidate in the load archive, plus the mandatory
     vanilla shapes, plus the fixed held-out suite from section 5.
  2. Referee assigns seeds. The same seeds for every policy, so a comparison is a comparison.
  3. For each (policy candidate, load) pair: run, with the physics referee in strict mode.
     Embarrassingly parallel; this is what Cloud Run Jobs are for.
  4. Mechanical checks: physics violations, realism envelope, determinism replay on a sample.
  5. Score each policy as the worst case over loads, gated on the SLA cap.
  6. Score each load by how much it degrades the *best* policy, so a load that defeats a weak
     policy and not a strong one is worth little.
  7. Update both archives, keeping ten each.
  8. Judging referee reviews the round and may propose a rule change, for a human to accept.
  9. Both generators read the full results and propose their next candidates.
```

Same seeds across policies is not a detail. Named independent RNG streams, already in the design,
mean the workload a policy faces does not shift because the policy changed, so a difference in
score is a difference in policy.

---

## 5. The failure mode Issao's design does not yet cover

Co-evolutionary systems fail in a characteristic way: both sides get very good at each other and
lose all generality. The mandatory vanilla shapes help, but they are a floor rather than a
yardstick, because the generators are free to treat them as a tax and optimise around them.

**Add a fixed held-out benchmark suite that never changes.** A dozen scenarios, frozen before the
arena starts, drawn from `docs/ARCHITECTURE.md` section 12's phase-1 dynamics: the rolling hotspot,
the preemption cascade, the retry storm, the prefix-affinity tension, a diurnal multi-region run.
Never added to, never tuned, and no generator may propose changes to it.

Its only job is to answer one question that the arena cannot answer about itself: **is the current
best policy actually better than the one from ten rounds ago, or has the game merely moved?** If
held-out performance is flat while arena scores climb, the arena is measuring its own drift.

That check is cheap, it is mechanical, and without it there is no way to distinguish progress from
co-evolutionary noise.

---

## 5b. Two flaws in this specification, found by implementing it

`src/arena.rs` implements the mechanical half against the working engine. Two things in the
specification above do not survive contact with it. Both need Issao, because both are rule changes
rather than implementation choices, and `docs/arena-implementation.md` has the measurements.

### The objective as written always scores the *easiest* load

Section 1 defines a policy's score as the minimum of its goodput across the load archive. Measured
across the frozen suite, every top-ranked policy takes its minimum on the two **lightest** loads,
because those offer twenty times less work than the heaviest. Absolute goodput is not comparable
across loads that differ in offered volume, so the minimum is set by the smallest load rather than by
the hardest one, and the adversarial intent of the objective is inverted: a load generator would score
best by proposing *trivial* loads.

The fix is to normalise before taking the minimum, and there are two defensible choices. Goodput as a
**share of offered work**, which asks how much of what arrived was served well. Or goodput as a share
of what the **best policy on that load** achieved, which is a relative measure and is how tournaments
usually handle heterogeneous rounds. The implementation reports the first as a diagnostic alongside the
raw objective, and deliberately does not change the objective, because that is Issao's call.

### A 99.9% cap and a length-independent first-token target cannot both hold

Section 1 suggests an SLA cap of 99.9%. Measured: **no policy reaches it on any chat or long-context
mixture at any offered rate, including a sixteenth of rated capacity.** The ceiling on the reference
mixture is about 0.9992 and the cause is arithmetic rather than policy. Prompt lengths are lognormal,
so roughly 1% of long-mode prompts exceed 56,500 tokens, which is two seconds of pure prefill against
a two-second first-token budget, before any queueing at all.

It is reachable only where outputs are short and prompts are small: on the code-completion load,
round-robin attains 0.9991 at 71 requests/s.

So the cap and the SLO are jointly unsatisfiable, and there are two honest resolutions. **Per-class
targets**, so the long mode gets a first-token budget proportional to its prompt, which is the right
answer and is why SLO classes are on the roadmap. Or **a cap of 0.95**, which is reachable across the
whole suite and is what the implementation uses as its default, with 0.99 reachable on individual
loads.

Under a 0.95 cap the ranking is power-of-two-choices, then round robin, then random, with both
global least-loaded policies scoring zero. Under 0.99 or 0.999 every policy scores zero, which is a
useless tournament rather than a demanding one.

Issao, 16:05: *"SLA 0.95 ok for now, but we need to figure out how to do better."* So 0.95 is the
rule of record, and the open design task is a target that a 0.99 cap can be held against: a
first-token budget that scales with prompt length, or per-class budgets. Either makes the gate
measure policy rather than prompt-length arithmetic.

## 6. What this requires from earlier milestones

Two things that are cheap now and expensive to retrofit, which is the reason this document exists
before the arena does.

1. **A policy must be expressible as data, not only as code.** `PolicySpec` in `scenario.proto` now
   gives each slot a typed configuration, so a generator can propose a policy choice and every one
   of its parameters as data, and the schema states exactly what is tunable. What it cannot do is
   propose a new policy *structure*: that would need a small interpreted decision language.
   Issao overrode that limitation, 16:05: *"Arena policy generator should actually have full power
   to write code to write new policies, as well as tuning parameters on existing policies."* So a
   candidate is either a `PolicySpec` with new parameters or a new implementation of the policy
   trait, written by the generator as code. The referee's strict mode and the `Observation`/`Intent`
   boundary are what make that safe: generated code cannot express a physics violation. `PolicySpec`
   now carries a `GeneratedPolicy` variant in every policy slot, naming the policy, its source path
   and the sha256 of that source, so a result always says which code produced it (947649b).

   A consequence of typing them, worth naming: adding a policy is now a schema change. For the
   arena that is fine, since it tunes what ships. It would matter if the arena were ever allowed to
   invent policies, which is the same boundary as above.
2. **Rated capacity needs a home in the interfaces.** A policy must be able to declare and update
   it, and the referee must be able to read it. It is one field on the policy configuration plus
   one intent, and adding it later means a proto change during a live experiment.

Everything else the arena needs, the strict referee, per-decision cost measurement, determinism
fingerprints, stratified traces, is already required by earlier milestones for its own reasons.
