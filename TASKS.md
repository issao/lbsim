# TASKS — things that need Issao

Stack ranked. Highest first. Item 1 is the most valuable thing you can do for the project
right now. Claude keeps this file current; anything Claude can do alone is not here.

Each item says what happens if you do not answer, so nothing stalls indefinitely.

Last updated: 2026-09-06 11:40 by Claude.

---

## 1. BLOCKING — bless the architecture, or veto parts of it

Everything downstream depends on this. Read `docs/ARCHITECTURE.md`, section 13 lists six
decisions. Fastest path: read section 0, the five findings, then section 13, then say yes or
name what you disagree with. Fifteen minutes.

The one that matters most is decision 1. If analytic epoch advancement is wrong, the scale
target is unreachable and the design changes shape. Claude is validating the math numerically
in the meantime, since that is verification rather than implementation.

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

## 3. Should Claude do groundwork while gated?

Three items are not implementation and cannot be invalidated by your review:

- **Benchmark the event queue.** This is open risk 1 and the single number that can
  invalidate the plan. Recommended: yes, do it first.
- **Validate the closed-form epoch math** against a brute-force per-step loop. Recommended:
  yes. If it does not match exactly, the central claim is wrong.
- **Cargo workspace skeleton** with crate boundaries and proto codegen, no logic, so blessing
  unblocks immediately rather than after setup. Recommended: yes.

**Default being taken:** Claude is doing the first two now, under `bench/`, because they
verify the design rather than build on it. The workspace skeleton waits for your word, since
it presumes the crate structure you have not yet blessed.

## 4. Confirm the reference hardware and model for calibration

`docs/llm-serving-primer.md` section 10.7 assumes a 70-billion-parameter model on eight H100s.
Every number in the analysis follows from that pair.

**If you say nothing:** that pair stays the default.

## 5. Do you have real traces for calibration, even aggregated?

Workload realism is the weakest link in the whole design and the least glamorous thing to get
right. Arrival burstiness, prompt and output length distributions, and session continuation
rates are what determine whether any conclusion transfers to reality. Published numbers exist
but are thin.

**If you say nothing:** Claude calibrates against published vLLM and SGLang benchmark figures
and documents the resulting uncertainty as a named risk.

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
