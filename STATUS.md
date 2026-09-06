# STATUS

What is finished, what is live, what Claude is doing now. Updated by Claude at every unit of
work. For things that need *you*, see `TASKS.md`.

**Last updated:** 2026-09-06 11:50 by Claude.

---

## Phase

**Design, awaiting blessing.** No simulator code exists, by your instruction. The gate is
`TASKS.md` items 1 and 2.

## Live on `origin/master`

| What | Where | State |
|---|---|---|
| Vision | `VISION.md` | yours, sections 1-8 complete; section 9 points at the architecture |
| Domain primer | `docs/llm-serving-primer.md` | complete, 585 lines, includes hardware reference numbers |
| Architecture and fidelity analysis | `docs/ARCHITECTURE.md` | complete, **awaiting your blessing** |
| Interfaces | `proto/lbsim/v1/*.proto` | nine files, all compile, **awaiting your review** |
| Architecture diagram | `docs/diagrams/system.drawio` | two pages, uncompressed, verified against the protos |
| Diagram checker | `tools/check_diagram.py` | passing, zero problems |
| Inbox scanner | `tools/inbox.py` | working |
| Toolchain notes | `docs/toolchain.md` | Rust and protoc working in this sandbox |
| Design validation | `bench/validate_epochs.py` | passing; proves the core claim exactly |
| Queue benchmark | `bench/queue/` | run; open risk 1 now closed |
| Simulator code | — | **none, deliberately** |

## Finished since the session started

1. Domain primer on LLM serving mechanics, with accelerator specs, the memory locality
   hierarchy from on-package memory out to object storage, prefill throughput, a decode
   step-time table, KV transfer costs per tier, and cold-start budgets.
2. A Python prototype of the engine core with 71 passing tests. **Deleted**, at your
   instruction, once the vision specified Rust and a 50,000-GPU target that Python cannot
   reach. The design carried over; the code did not.
3. Fixed the Rust toolchain. This sandbox had no C compiler, so nothing could link. Resolved
   without root by downloading and extracting Debian packages into the home directory.
   `cargo build`, `cargo test`, proc macros, and `protoc` all work now. See `docs/toolchain.md`.
4. Architecture and fidelity analysis answering the open questions in your vision. Five
   findings, six decisions needed from you.
5. Nine proto interface files, all compiling under protoc 31.1.
6. The architecture diagram, plus a checker that verifies every arrow against the protos. The
   checker found three real drift problems on its first run, including a policy service the
   diagram referenced that the protos did not declare. All fixed.

## Two design risks measured, both resolved

**The core claim holds.** `bench/validate_epochs.py` proves the closed-form epoch advance is
*exactly* equivalent to iterating every decode step, not an approximation. Checked in rational
arithmetic across batch sizes 1 to 256 and up to 30,000 steps, plus 20,000 randomised trials of
the inverse solve, plus whole replica runs where sequences finish at different times. Those
runs needed 23,343 per-step iterations versus 285 epoch iterations, a factor of 82. A bonus
finding: the closed form is also more numerically accurate than the loop, because it does not
accumulate rounding over n additions.

**Open risk 1 is closed, and my proposed mitigation was wrong.** The event queue measures 54 ns
per event for a 6,250-entry standard-library binary heap, giving roughly 2x headroom against the
budget, so one core reaches the 20x stretch target for the whole fleet. Heap *size* is what
matters: at 1.7 million entries the same structure costs 441 ns, 4.4x over budget. That confirms
the two-level design, keeping per-request timers out of the global queue. But the hand-rolled
4-ary heap I proposed as the fix is *slower* than the standard library at every size tested, so
that recommendation is dropped. `docs/ARCHITECTURE.md` section 1.4 is corrected, including an
earlier prediction of 1 to 2 microseconds that was pessimistic by about 3x.

One input correction while measuring: the scale budget assumed 9,000 output tokens per second
per replica, and the validated cost model gives 7,676. The analysis therefore overstates fleet
request rate by about 15%, which makes every cost figure conservative rather than optimistic.

## Doing now

Nothing. Waiting on `TASKS.md` items 1 and 2, the two blocking gates.

## Assumptions Claude is running on

These will be acted upon unless you say otherwise. Each is listed in `TASKS.md` with the
consequence of silence.

- Analytic epoch advancement is the core mechanism.
- Request cohorts are dropped; fluid mode survives only for out-of-focus regions.
- Prefix caching is in scope for phase 2.
- The memory tier is pooled per cluster, not per host.
- One thread per cluster, no intra-cluster parallelism.
- Reference calibration pair is a 70-billion-parameter model on eight H100s.

## Known risks

The full list is `docs/ARCHITECTURE.md` section 12. The two that could change the plan:

1. ~~Event queue throughput~~ **closed by measurement**, see above.
2. **Workload realism.** Now the top risk. The simulator is only as good as its arrival process
   and its prompt and output length distributions. Needs real traces, or documented
   uncertainty. `TASKS.md` item 4.

## Branch state

`master` is at `origin/master`. Working branch is `claude/architecture`, also pushed. Working
tree clean.
