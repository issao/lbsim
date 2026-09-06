# STATUS

What is finished, what is live, what Claude is doing now. Updated by Claude at every unit of
work. For things that need *you*, see `TASKS.md`.

**Last updated:** 2026-09-06 12:50 by Claude.

---

## Phase

**Design, one gate left.** No simulator code exists, by your instruction. The architecture is
reviewed and its decisions are recorded in `docs/ARCHITECTURE.md` section 14. The remaining gate
is the interface review, `TASKS.md` item 1.

## Live on `origin/master`

| What | Where | State |
|---|---|---|
| Vision | `VISION.md` | yours, sections 1-8 complete; section 9 points at the architecture |
| Domain primer | `docs/llm-serving-primer.md` | complete, 585 lines, includes hardware reference numbers |
| Architecture and fidelity analysis | `docs/ARCHITECTURE.md` | **reviewed**; decisions in section 14, your three-layer deployment in section 10 |
| Interfaces | `proto/lbsim/v1/*.proto` | thirteen files, all compile; subscription and telemetry feedback folded in |
| Calibration reference | `docs/calibration.md` | complete; four replayable traces identified, seven primer corrections |
| Agent architecture | `docs/agent-architecture.md` | proposal, **awaiting your review** |
| Reference cost model | `bench/validate_epochs.py` | four properties pass, including the compute branch and speculation |
| Architecture diagram | `docs/diagrams/system.drawio` | three pages, uncompressed, verified against the protos |
| Reaction tooling | `tools/sync.sh`, `tools/inbox.py` | end-to-end verified against a real push |
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

## Your feedback, folded in

You left five marker instructions on `master` at 11:53. The watcher caught them within a minute
and all five are now reflected in the repo, with the markers removed:

- Analytic epochs are the plan of record.
- Cohorts move to a parking lot rather than being dropped, with an explicit trigger and a knob
  design spanning fully-discrete to fully-fluid. `docs/ARCHITECTURE.md` section 4 is rewritten.
- Prefix caching accepted.
- DRAM and SSD both fully disaggregated at cluster level.
- Your three-layer architecture is now `docs/ARCHITECTURE.md` section 10, worked out in full,
  plus two new interface files and a third diagram page.

Three things came out of working through your sketch that need your eye, all in `TASKS.md`:

1. **One departure from your sketch.** A frontend-requested time sampling factor does not hold
   the O(1) bound on its own, because raising simulation speed raises the wire rate with it. A
   subscription now declares a wall-clock budget and the server derives the interval. Your
   preference survives as a hint.
2. **Ingress state is fine; Ingress compute is the risk.** Memory is single-digit megabytes for
   the fleet view and about 165 MB for in-flight requests. But at 20x realtime Ingress makes
   2.25 million routing decisions per second, so a full fleet scan per request would cost 14
   cores. O(N) routing has to be banned, not merely discouraged.
3. **Leaf shards should be threads, not processes.** A process boundary costs 50 to 100
   microseconds per barrier against a 0.2 to 1 millisecond lookahead, which breaks the target;
   threads cost 1 to 5 microseconds. The proto boundary is preserved either way.

## Since your last look

**Your 13 architecture instructions and 8 proto instructions are all folded in**, markers removed,
each quoted in a commit message. Two were real errors in my model:

- **The 9,000 output tokens per second figure was decode-only at full batch**, as you suspected.
  With prefill included it is 4,533 at a chat-like mix and 889 at a prompt-heavy one, a span of 39x.
  The fleet budget is redone at 56,700 requests per second rather than 112,500, which also means
  the fleet is prefill-limited rather than KV-limited at that mix, so adding KV capacity buys
  nothing.
- **The cost model was bandwidth-only.** It now takes the worse of bandwidth and compute. The
  closed form survives, because two lines cross at most once. Bandwidth-only would have
  systematically overstated speculative decoding and quantization, which are exactly the policies
  under study.

Your speculative-decoding design works and is better than what it replaced: verification multiplies
compute while leaving weight reads alone, so the erosion of speedup at large batch is emergent
rather than coded. It also removed an incorrect claim of mine that acceptance rate falls with batch
size.

**Two tooling failures worth knowing about, both fixed.** Four of your seven proto instructions were
invisible to the scanner because they trailed code as comments; it anchored only to line starts.
And one instruction carried no name prefix at all, so `sync.sh` now diffs upstream commits and
surfaces every comment line they add. A missed instruction is the worst failure mode here, so both
have regression tests.

## Doing now

Nothing blocking. Awaiting review of `docs/agent-architecture.md` and the remaining interfaces.

## Assumptions Claude is running on

These will be acted upon unless you say otherwise. Each is listed in `TASKS.md` with the
consequence of silence.

- Analytic epoch advancement is the core mechanism.
- Request cohorts are dropped; fluid mode survives only for out-of-focus regions.
- Prefix caching is in scope for phase 2.
- The memory tier is pooled per cluster, not per host.
- Leaf shards are threads within one process per run, not separate processes.
- Simulated time is absolute Unix epoch nanoseconds in a uint64; no timestamp passes through a float.
- Routing policies must be O(1) or O(log N); a full fleet scan per request is banned.
- Reference calibration pair is a 70-billion-parameter model on eight H100s.

## Known risks

The full list is `docs/ARCHITECTURE.md` section 12. The two that could change the plan:

1. ~~Event queue throughput~~ **closed by measurement**, see above.
2. **Workload realism.** Now the top risk. The simulator is only as good as its arrival process
   and its prompt and output length distributions. Needs real traces, or documented
   uncertainty. `TASKS.md` item 4.

## One incident, resolved

Something outside this session, most likely a VS Code Git extension attached to this working
directory, stashed uncommitted work and switched branches at 11:59. That silently reverted the
`docs/ARCHITECTURE.md` rewrite. It was recovered from the stash and reassembled with the newer
interface work. Nothing is lost and every check passes. `tools/sync.sh` now warns on an
unexpected stash and on HEAD moving between runs.

## Branch state

`master` is at `origin/master`. Working branch is `claude/architecture`, also pushed. Working
tree clean.
