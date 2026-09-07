# Iteration profile: where a unit of work spends its time

Measured 2026-09-06 16:45–17:05 PDT from the transcripts of the tech lead and its ten worktree-era
subagents (15:42–16:35 PDT), plus a physical measurement of the build cycle in a throwaway worktree on
`origin/master` at that time. Issao asked for this, verbatim: *"profile the developer flows of tl and other
coding agents and identify any significant opportunities for improving iteration cycle."* Every number
below is measured; estimates are marked as such.

## 1. The headline

**A unit is 68% model time, 32% tool time, and the tool time is dominated by one 79-second test.**

The physical cycle is already fast: a cold worktree builds the whole workspace in 6 s, one crate's tests
re-run in 0.17 s after an edit, the fingerprint check takes 2.4 s with a warm release build, and the web
bundle builds in 2.5 s. What is slow is `test --workspace` at 85–89 s, of which **79 s is `sim-arena`'s
unit test running a full arena round in debug mode**; the other 15 test binaries and 10 crates together
take under 5 s. That one test was run 18 times by the ten subagents and about 15 more times by the tech
lead at integration.

The other big costs are not physical at all: 4.7 min per unit reading before the first edit, 5.9 min per
unit waiting for the tech lead to integrate, and turns that carry 89k tokens of context on average.

## 2. The ten units, measured

| Unit | Wall min | Turns | Ctx k tok/turn | Model % | Full `test` runs (n, min) | First edit at min | Files read before first edit | Report→merge min |
|---|---|---|---|---|---|---|---|---|
| idle/lease | 5.5 | 19 | 56 | 66 | 1, 1.0 | 2.4 | 8 | 8.0 |
| simplify-1 | 20.1 | 45 | 112 | 87 | 4, 1.2 | 8.0 | 4 | 3.5 |
| web-transport | 23.3 | 60 | 165 | 77 | 0 (web) | 8.3 | 21 | 3.4 |
| engine-core | 21.0 | 48 | 100 | 62 | 5, 6.2 | 2.3 | 23 | 6.6 |
| physics-oracle | 14.3 | 14 | 78 | 77 | 1, 1.3 | 7.1 | 9 | 6.3 |
| wire-export | 12.9 | 39 | 96 | 79 | 2, 1.2 | 5.4 | 31 | 6.6 |
| least-kv-probe | 5.4 | 28 | 57 | 56 | 1, 1.3 | 1.0 | 13 | 6.6 |
| deadline-adm | 12.8 | 41 | 71 | 56 | 3, 3.4 | 1.5 | 7 | 2.4 |
| fair-share | 10.4 | 29 | 80 | 67 | 1, 1.7 | 4.6 | 22 | 4.3 |
| simplify-2 | 12.6 | 25 | 71 | 56 | 2, 1.2 | 6.3 | 10 | 11.6 |
| **mean** | **13.8** | 35 | 89 | **68** | 2.0, **1.8** | **4.7** | 15 | **5.9** |

Definitions. *Model %* is wall time not inside a tool call, i.e. the agent thinking and writing. *Ctx* is
input tokens per turn (cache reads included). *Full test* is `test` without `-p` or `--test`. *First
edit* is the first Write/Edit/heredoc after spawn. *Report→merge* is the agent's last transcript line to
the `--no-ff` merge commit on `master`.

Failures across the ten units: 7 test failures, 2 compile errors, 1 real fingerprint diff, 1 cargo lock
wait. Builds do not fail much; they are cheap, and they are not where the time goes.

The tech lead over the same period: 81 min wall, 101 turns, **508k tokens of context per turn**, 75 Bash
calls totalling 86 min (calls overlap), of which 28 of the last 32 min were integration. Its first build
under a fresh worktree took 3 s. It hit the build-slot lock 12 times, competing with its own subagents.

The cloud agent (39.5 min, 120 Bash calls, 42% tool time) iterated against Cloud Build at 1.5–3 min per
round trip with two IAM permission denials; its cycle is bounded by the cloud, not by anything here.

## 3. The physical cycle, measured in a throwaway worktree

| Step | Time |
|---|---|
| `git worktree add` | 17 ms |
| cold `tools/build.sh test --workspace` (fresh target dir) | 89.3 s |
| warm `test --workspace`, nothing changed | 88.1 s |
| warm `test --workspace` after touching `sim-core` (everything rebuilds) | 84.7 s |
| warm `test -p sim-policy` after touching one policy file | **0.17 s** |
| cold `build --release` | 6.0 s |
| `./check-fingerprints.sh`, warm release | 2.4 s (agents saw 8–13 s under load; 80–94 s when it had to rebuild release) |
| `test --release --workspace` | 56.1 s |
| `npm ci` (offline cache) / `npm run build` | 1.0 s / 2.5 s |
| target directory after a full debug+release build | 592 MB |

Per test binary and crate, warm:

| | ms |
|---|---|
| **`sim-arena` lib tests** | **79,361** |
| `wire_export` | 1,260 |
| `determinism` | 732 |
| `sim-physics` lib | 706 |
| `policy_ordering` | 638 |
| `load_response` | 468 |
| `stream_independence` | 278 |
| the other 9 test binaries and 10 crates, together | ~700 |

The compile is not the cost. Three near-identical 85–89 s numbers for cold, warm-noop and warm-after-core
mean the wall is test *execution*, and one test is 90% of it.

Memory: the cgroup's historical `memory.peak` is 25.6 GB, the OOM event from earlier today; during this
measurement `memory.current` (which includes page cache) sat at 16.5 GB with all agents running. The
per-build peak could not be isolated without resetting the cgroup counter, but a zero-dependency 10k-line
workspace whose full target directory is 592 MB does not need 4 GB per build; the current bound of two
concurrent builds at `jobs = 4` is conservative by a wide margin. Estimate, not measured.

## 4. Repeated failures, fix-once items

| Signature | Hits | Agents | Cause |
|---|---|---|---|
| `missing fields admission, admission_headroom, fair_share_burst ...` / `could not compile lbsim (test scenario_parse)` | 7 | 2 | tests build `Scenario` as a struct literal, so every branch that adds a scenario key breaks every other branch's tests at rebase |
| `command not found` | 3 | 3 | `PATH` lacks `~/local/bin`; `tools/build.sh` exports it for cargo but nothing does for `protoc`/`python3` calls outside it |
| `Not possible to fast-forward` | 2 | 2 | agents merging `origin/master` into a worktree whose `master` is checked out elsewhere |
| registry-table conflicts in `crates/sim-policy/src/lib.rs` | 4 rebase conflicts | tech lead | three policy briefs each added "exactly one line" to the same table |

Also read by nearly every agent before its first edit, none of which the unit needed to *read*:
`tools/build.sh` (7 agents), `check-fingerprints.sh` (9), `.cargo/config.toml` (5), `tests/layering.rs`
(4), `Cargo.toml` (4). And `crates/sim-leaf/src/lib.rs`, 700+ lines, was read **28 times** by 10 agents.

## 5. Opportunities, ranked by minutes saved per unit × units per hour

Units per hour at the observed peak (16:10–16:35): about 8 subagent units merged per hour. Savings are
per unit unless stated; "covered" means another change already in flight delivers it and it is not
double-counted here.

| # | Change | Saves per unit | Per hour at 8 units | Evidence | Status |
|---|---|---|---|---|---|
| 1 | **Take the arena round out of `cargo test`'s default path.** Mark `sim-arena`'s round test `#[ignore]` and run it with `--ignored --release` only inside the integration gate; or move it to a `bench/`-style binary. Full workspace test drops from 88 s to ~6 s. | 2.5 min (1.8 min of full tests per unit, plus the same at integration, plus the tech lead's 15 full runs) | ~20 min, and the integration gate runs 3× faster | §3 | **new** |
| 2 | **Briefs carry the code, not pointers to it.** A unit's brief includes the exact struct definitions, function signatures and the 20–40 lines it will touch, plus a fixed preamble with the commands (`tools/build.sh test -p X`, `--test Y`, `./check-fingerprints.sh`, the worktree line) so nobody reads `build.sh`, `check-fingerprints.sh`, `.cargo/config.toml` or `layering.rs` again. Target: first edit ≤ 2 min. `tools/api-card.sh <crate>` (pub items with line numbers, ~30 lines) makes this cheap to produce. | 2.5 min of the 4.7 min setup tax; also fewer turns and smaller context | ~20 min | §2 first-edit and files-read columns, §4 | **new** |
| 3 | **Subagents integrate themselves** through `tools/integrate.sh` (flock queue; rebase, test, fingerprints, merge, push, delete branch). Removes the 5.9 min mean report→merge wait and the tech lead's 28-of-32-minute integration load, so the fleet stops emptying between waves. | 5.9 min of latency per unit; tech lead freed | fleet stays full | §2 last column | covered, fork in flight |
| 4 | **Inner loop is one crate; the full gate runs once.** With #3, the subagent's own `test --workspace` + fingerprints before reporting is redundant with the gate. The brief says: iterate with `-p <crate>` / `--test <name>` (0.2–1.3 s), run `tools/integrate.sh --dry-run` once at the end. | ~1.5 min (overlaps with #1; after #1 the full test is 6 s and this matters less) | ~10 min | §2, §3 | new, one line in the brief template |
| 5 | **`Scenario` built by `Scenario::default()` + struct update in every test**, and a `tests/common` builder, so adding a key never breaks another branch's tests. Fix once. | 2–4 min per affected agent; two of ten this hour, and every future policy or dynamic adds keys | ~5 min now, growing | §4 row 1 | **new** |
| 6 | Generated policy registry, so a policy branch touches only its own file. | the 4 rebase conflicts and their resolution time (~4 min each for the tech lead) | ~8 min of tech-lead time | §4 row 4 | covered, same fork |
| 7 | **Shrink per-turn context.** Bash output goes to a file with `tail -20`; `Read` uses `offset`/`limit`; briefs say which files not to read. Per-unit Bash output pulled into context was 94–138 KB and one agent read a 60 KB file whole; mean context 89k tokens per turn, tech lead 508k. Turn latency is 10–47 s and grows with context. | estimate 1–2 min via faster turns and fewer of them; not directly measured | ~10 min (estimate) | §2 ctx column, TL figure | new, brief template + CLAUDE.md line |
| 8 | Continuous pipeline instead of waves (spawn on completion; ≥6 in flight while the backlog is non-empty). | the ~30 min the fleet sat empty between 16:08 and 16:38 | | prior analysis | covered, tech-lead operating mode |
| 9 | `tools/env.sh` sourced by every command line in the brief preamble (`PATH`, `LBSIM_*`), and the worktree line in the brief written as `git -C /home/agents/repo/lbsim worktree add ...` so nobody merges `origin/master` into a checked-out `master`. | ~1 min per hit, 5 hits this hour | ~3 min | §4 rows 2–3 | new, trivial |
| 10 | Raise `LBSIM_MAX_BUILDS` to 4 with `jobs = 2`. | the tech lead's 12 lock waits (short) | small | §2, §3 memory | new; measure peak RSS first by resetting `memory.peak` |

**Not worth doing, and why.** A prebuilt target directory cloned per worktree: cold build is 6 s and
`worktree add` is 17 ms; with #1 the cold `test --workspace` is ~15 s. Nothing to save. Nightly
`--report-time`: the per-binary table above already localises the cost. A second tech lead: the tech lead
is idle once #3 lands; the constraint was integration, not span of control.

**Expected result.** Applying #1, #2, #4, #5, #9 to the brief template and the test suite takes under an
hour of one agent's time and removes roughly 6–7 minutes from a 13.8-minute unit, on top of the 5.9 minutes
of integration latency #3 removes. Estimate: a unit goes from ~20 minutes spawn-to-master to ~8, and the
tech lead's share of each drops from about 6 minutes to under 1.

## 6. Method, so it can be rerun

Transcripts: `/tmp/claude-1000/-home-agents-repo-lbsim/<session>/tasks/*.output`, one JSONL per agent.
Each `assistant` record's `tool_use` blocks were paired with the matching `tool_result` by id to get
per-call durations and outcomes; `usage` fields gave turns and context. Classification of a Bash result:
`error[E` → compile error; `test result: FAILED` or `panicked at` → test failure; `FAIL: numbers moved` →
fingerprint diff; `Blocking waiting for file lock` → lock wait; `CONFLICT` → rebase conflict. Merge times
from `git log --merges --format=%ct` on `origin/master` matched by branch name. Physical numbers from
`date +%s%N` around each command in a fresh worktree on `origin/master`, one build at a time through
`tools/build.sh`, and `/sys/fs/cgroup/memory.{current,peak}`. The scripts are not committed; they were
~150 lines of Python and shell in the job's scratch directory.

## 2026-09-06 17:12 PDT — baseline before the new tech lead's first wave

The productivity agent (a09d1073) now continues this file every 20 minutes; the parser is
`~/.prod/profile.py`, same method as §6. This section is the baseline the first delta is measured
against: the four units the old tech lead spawned at 16:40, none of them with the brief template
(v1 landed at 16:56), all four merged at the 17:04–17:06 checkpoint by main rather than through
`tools/integrate.sh`, so there is no report→merge figure for them.

| Unit | Wall min | Turns | Ctx k/turn | Model % | Bash out KB | First edit min | Files read before it | Full/partial tests | Failures |
|---|---|---|---|---|---|---|---|---|---|
| ingress-server (a77e7ce) | 22.8+ | 30 | 116 | 52 | 104 | 7.1 | 26 | 0 / 2 (607 s) | one 603 s hang |
| arena-rules (a973e61) | 23.4 | 39 | 84 | 26 | 125 | 7.0 | 19 | 1 / 5 (261 s) | 2 test failures, one 603 s hang |
| trace-wire (aababe7) | 25.3 | 46 | 120 | 44 | 205 | 4.8 | 21 | 1 (604 s) / 7 (171 s) | 3 compile, 2 test failures |
| replay (af6eac7, web) | 23.4 | 112 | 212 | 82 | 380 | 7.2 | 30 | web only | 2 NaN render errors |
| **mean** | **23.7** | 57 | 133 | 51 | 204 | **6.5** | **24** | | |

Against §2's ten units (13.8 min, 4.7 min to first edit, 15 files read before it, 68% model time):
these four are bigger units, and every setup number is worse. All four read `tools/build.sh`,
`check-fingerprints.sh`, `.cargo/config.toml` and `tests/layering.rs` again, which the template's
"do not read" line exists to stop; `crates/sim-ingress/src/lib.rs` and `sim-leaf/src/lib.rs` were
read whole, repeatedly. Model share dropped to 51% for one reason:

**Three of the four Rust units lost ten minutes each to a single hung test.** At 16:52:57,
16:53:01 and 16:53:10 three agents ran `tools/build.sh test ...` and all three returned at 17:03,
at the Bash tool's 600 s limit. The cause is `tests/ingress_http.rs`, the ingress-server unit's
own test: it starts the HTTP server and never exits. It held build slot 1, and `tools/build.sh`'s
fallback ("every slot busy: wait on the first one") queued the other two agents behind it, on the
slot that would never free, while slot 2 sat idle. The same thing is happening as this is written:
`ingress_http-1b347def` has run 241 s under a `timeout 400` the agent added by hand, and the cloud
agent's `build --release` has waited 161 s on slot 1 with slot 2 free. 30 agent-minutes lost to
the first occurrence; every build on the machine is serialised to one slot until the test is
fixed. Two fix-once proposals go to the tech lead with this section: `build.sh` waits round-robin
(`flock -w 1` over both slots in a loop) instead of pinning to slot 1, and it wraps cargo in
`timeout -k 5 ${LBSIM_BUILD_TIMEOUT:-420}` so a hang returns an error inside the agent's turn
instead of a silent tool timeout that leaves the process running. The ingress test itself needs a
deadline in the test (a `recv_timeout`, or a server built with a shutdown handle).
*Footnote, 17:35:* the hang was that agent's first uncommitted draft (`StepForward` on a paused run
waited forever), fixed before its first commit; the committed suite on master (c0e9ea8) runs in about
10 s, so there is no open test item. The `build.sh` fallback was real regardless and landed as U45
(35c7918): round-robin slot wait, cargo bounded by `timeout`.

Smaller items seen in this wave. The arena unit ran the full arena round in debug three times
(80–92 s each) before its 603 s loss, which §5 item 1 addresses and which the tech lead's
`arena-rules` merge ("fast arena tests") may already have. The trace-wire unit ran the workspace
tests itself before reporting, which the template now forbids. Context per turn is 84–212k; the
replay unit's 112 turns pulled 380 KB of Bash output into context, mostly whole-file `cat`s.

Next section: the first wave under brief template v1, spawned by tech lead a86e5fcc (17:07), at
about 17:40.

## 2026-09-06 17:35 PDT — first wave under the template (v2 from the first spawn)

Tech lead a86e5fcc spawned 14 agents between 17:18 and 17:27, every brief pasted from
`docs/agents/brief-template.md` v2 with excerpts filled in. Seven had merged by 17:29 through
`tools/integrate.sh`; three large units and three small ones were still running at 17:31; one is a
review agent. Model is "sonnet" where the spawn said so, "default" otherwise.

| Unit | Model | Brief KB / excerpt lines / files owned | Wall min | Turns | Ctx k | s per turn (median) | First edit min | Reads before it | integrate.sh s (lock wait) |
|---|---|---|---|---|---|---|---|---|---|
| build-slots | sonnet | 4.9 / 17 / 2 | 4.0 | 22 | 51 | 5.3 | 0.4 | 1 | 54 (21) |
| wire-decisions | sonnet | 5.0 / 1 / 1 | 4.7 | 27 | 61 | 5.0 | 1.7 | 1 | 41 (6) |
| scenario-default | sonnet | 4.5 / 3 / 11 | 4.8 | 39 | 57 | 3.7 | 2.3 | 12 | 53 (19) |
| trace-fields | sonnet | 5.4 / 13 / 4 | 5.2 | 35 | 69 | 4.3 | 2.6 | 7 | 34 (0) |
| replica-rows | default | 7.3 / 21 / 11 | 5.2 | 29 | 64 | 5.0 | 1.6 | 29 | 37 (0) |
| api-card | sonnet | 4.5 / 1 / 2 | 5.3 | 27 | 59 | 5.3 | 2.1 | 4 | 33 (0) |
| generator | default | 7.6 / 20 / 6 | 6.8 | 33 | 58 | 9.1 | 3.0 | 15 | 41 (5) |
| **merged, mean** | | | **5.1** | 30 | 60 | 5.4 | **2.0** | **10** | **42 (7)** |
| preemption | default | 8.6 / 30 / 13 | 11.2+ | 45 | 77 | 14.8 | 4.7 | 36 | in flight |
| trace-engine | default | 9.0 / 16 / 9 | 9.9+ | 38 | 78 | 17.8 | 6.7 | 48 | in flight |
| web-live | default | 8.9 / 10 / 13 | 12.1+ | 42 | 91 | 13.1 | 7.8 | 50 | in flight |
| walkthroughs | sonnet | 7.1 / 7 / 8 | 7.8+ | 32 | 70 | 6.5 | 5.9 | 11 | in flight |
| export-demos, mock-tags | sonnet | 5–7 / 6–9 / 2–13 | 2–3 | | | 4–5 | 1.5 | 9–11 | in flight |

**Delta against §2 (the ten pre-template units).** Wall 13.8 → 5.1 min for the merged units.
First edit 4.7 → 2.0 min. Files read before it 15 → 10, and `tools/build.sh`,
`check-fingerprints.sh`, `.cargo/config.toml` and `tests/layering.rs` were read by one agent
between them (preemption, twice, `check-fingerprints.sh`), against 25 reads across ten agents
before. Report→merge 5.9 min → 0.7 min: the agent runs `integrate.sh` itself, the gate is 33–54 s
end to end (workspace tests 7–22 s now that the arena round is out of the default path), and the
integration lock waited at most 21 s with seven agents finishing inside six minutes. No full
workspace test was run by any agent before the gate; no 600 s timeouts. The tech lead: 23 min,
69 turns, 152k tokens per turn (was 508k), 15 spawns and 6 messages, its Bash calls 55–61 s
only when a graph update carries its own integration.

**What the numbers say about unit size.** The variable that predicts the rest is the number of
files a unit owns. The seven merged units own 1–11 files and reached their first edit in 0.4–3.0
min at 4–9 s per turn. The three large units own 9–13 files, read 36–50 file slices before the
first edit (4.7–7.8 min), and run at 13–18 s per turn, three times the small units, because each
turn carries 77–91k tokens. Excerpts in the brief did not close the gap: 16–30 pasted lines cover
one seam, and a 13-file unit has several. `crates/sim-leaf/src/lib.rs` (1047 lines) was read in
15 slices by preemption, `sim-scenario/src/lib.rs` (379 lines) in 10 slices over three turns by
trace-engine, where one whole read would have been one turn and 4k tokens.

Ranked, at the current rate of about 20 merged units per hour:

| # | Change | Saves | Owner | Status |
|---|---|---|---|---|
| 1 | **Units own at most ~6 files.** A unit that needs 9–13 files is two or three units with a stand-in seam between them. When one cannot be split, the brief carries `tools/api-card.sh <crate>` output (U47, landed 17:26) for every crate it touches, not just the seam. | 3–5 min on each large unit, and 3× cheaper turns for its whole life; 3 of 13 units this wave | tech lead | proposed 17:35 |
| 2 | Read a file under 400 lines once, whole; slice only longer ones, several ranges per command. | ~1 min on units touching `sim-scenario`, `sim-model` or any mid-sized file; 4 of 13 | template | **v3, this section** |
| 3 | Model choice written into the template header: sonnet for a unit with ≤4 owned files and one test (measured 4–5 s per turn, 4–5 min wall); default otherwise. Already the tech lead's practice; recorded so it survives a restart. | keeps the 5 s turn | template | **v3** |
| 4 | Integration lock: waits of 0–21 s at seven finishes in six minutes. Nothing to do until it exceeds a minute. | | | measured, no action |

Failures this wave, none repeated across agents: preemption one borrow error and one test
failure of its own test; trace-engine nine compile errors in `sim-leaf` and one failing test of
its own; export-demos one failing test of its own; the generator one intentional parse error. No
compile failure came from another branch's change, which is what U46 (Scenario by `default()`)
was for.

Next section at about 17:55, after the three large units finish, with their report→merge.
