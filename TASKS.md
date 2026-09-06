# TASKS — things that need Issao

Stack ranked. Highest first. Item 1 is the most valuable thing you can do for the project
right now. Claude keeps this file current; anything Claude can do alone is not here.

Each item says what happens if you do not answer, so nothing stalls indefinitely.

Last updated: 2026-09-06 11:50 by Claude.

---

## 1. BLOCKING — bless the architecture, or veto parts of it

Everything downstream depends on this. Read `docs/ARCHITECTURE.md`, section 13 lists six
decisions. Fastest path: read section 0, the five findings, then section 13, then say yes or
name what you disagree with. Fifteen minutes.

Decision 1, analytic epoch advancement, is **now backed by proof rather than argument**:
`bench/validate_epochs.py` shows the closed form is exactly equivalent to per-step iteration,
in rational arithmetic, with 82x fewer iterations. Section 3.5 has the detail. That should make
decision 1 easy to accept.

**If you say nothing:** Claude proceeds on the assumption that all six are accepted, and says
so loudly in `STATUS.md`. Reversing later costs rework proportional to how much got built.

## 2. BLOCKING — review the interfaces in `proto/`

You asked to review every interface. Nine files. Order by consequence:

1. `policy.proto` — the engine/policy seam and the referee. Decides whether the agent arena
   can be trusted, because it is what makes a cheating policy unrepresentable rather than
   merely forbidden.
2. `telemetry.proto` — the staleness boundary. Decides whether the control-theory dynamics
   you care about are reachable at all.
3. `scenario.proto` — the whole configuration surface, and the largest file. This is the one
   you will live in.
4. `common.proto` and `request.proto` — vocabulary. Note `Truth`: it holds what the simulator
   knows and no policy may ever see.
5. `serving.proto`, `kv.proto`, `capacity.proto` — the modelled data plane, one method per
   diagram arrow.
6. `metrics.proto`, `control.proto` — results and the dashboard API.

Leave corrections as `Issao:` lines directly in the proto files. Claude will act on each and
delete the line.

**If you say nothing:** Claude treats the interfaces as accepted and starts generating code
from them. Interface changes after that are cheap for messages and expensive for services.

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

## Done

- [x] Write `VISION.md` sections 1-8.
- [x] Publish the repository to GitHub.
- [x] Install and authenticate `gh` so Claude can push.
- [x] Wire diagram: you opted out of drawing, Claude drew `docs/diagrams/system.drawio`.
- [x] `VISION.md` section 9 now points at `docs/ARCHITECTURE.md` section 10.
