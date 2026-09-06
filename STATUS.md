# STATUS

What is finished, what is live, what Claude is doing now. Updated by Claude at every unit of
work. For things that need *you*, see `TASKS.md`.

**Last updated:** 2026-09-06 11:40 by Claude.

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

## Doing now

Validating the closed-form epoch math against a brute-force per-step loop, and benchmarking
the event queue. Both verify the design rather than build on it, so neither presumes your
blessing. See `TASKS.md` item 3.

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

1. **Event queue throughput.** The one number that can invalidate the design. Being measured
   now.
2. **Workload realism.** The simulator is only as good as its arrival process and its length
   distributions. Needs real traces, or documented uncertainty. `TASKS.md` item 5.

## Branch state

`master` is at `origin/master`. Working branch is `claude/architecture`, also pushed. Working
tree clean.
