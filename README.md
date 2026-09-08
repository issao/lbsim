# lbsim

A discrete-event simulator for a cloud LLM inference service. It exists to reproduce the load
dynamics a real fleet shows, and to score scheduling, load-balancing and traffic-shaping policies for
performance, service quality and robustness. Rust engine, React dashboard, protobuf interfaces.
`VISION.md` is the authoritative scope. The dashboard's two data modes are live, a run on the Ingress
server, and replay of a recorded run; the browser mock that stood in for the engine on day one was
removed on 2026-09-07 (U100), so nothing on it invents a number.

## Run it

```bash
export PATH="$HOME/local/bin:$HOME/.local/bin:$PATH"   # this sandbox; see docs/toolchain.md
./run-demos.sh              # twenty experiments, twenty self-contained HTML reports in out/
tools/build.sh test --workspace             # 73 tests, 3 ignored as known defects; build.sh bounds cargo to two processes machine-wide
./check-fingerprints.sh     # every report number byte-identical to bench/golden-fingerprints.txt
./check-sensitivity.sh      # the policy ordering must survive 30% cost-model error
tools/build.sh test --release -p sim-arena -- --nocapture arena_round   # one arena round on the held-out suite
cd web && npm install && npm run dev        # the dashboard: live with a server, replay of exported runs otherwise, localhost:5173
```

Every run is deterministic: same scenario and seed, byte-identical output. Scenarios are plain
`key = value` files in `scenarios/`; every number in a report traces to one of them.

## Read next

| If you want | Read |
|---|---|
| what is finished and live | `STATUS.md` |
| what is waiting on Issao | `TASKS.md` |
| the results, with the tables | `docs/findings.md` |
| what was cut today and why | `docs/scope-today.md` |
| the design, and every decision taken | `docs/ARCHITECTURE.md`, section 14 for decisions |
| the whole-project plan | `docs/execution-plan.md` |
| what is being built right now, unit by unit | `docs/execution-graph.md` |
| the policy arena | `docs/arena.md` spec, `docs/arena-implementation.md` measured, `docs/policy-catalog.md` every policy idea |
| the dashboard | `docs/ui-spec.md`, `web/README.md`; how it came to show real runs, `docs/dashboard-plan.md` |
| the wire between browser and server | `crates/sim-ingress/WIRE.md` |
| where the numbers come from | `docs/calibration.md`, `docs/llm-serving-primer.md` |
| how the agents are staffed | `docs/agent-architecture.md`, ownership table in `CLAUDE.md` |
| deploying | `docs/deploy.md`, `deploy.sh` |

## Working rules

`CLAUDE.md` has them. The short version: every agent works in its own worktree on a `claude/<topic>`
branch; subagents push their branch and report, and the owning long-running agent rebases, merges to
`master` with `--no-ff` and pushes. Every cargo call goes through `tools/build.sh`. "No behaviour
change" means `./check-fingerprints.sh` prints `PASS`. Timestamps in the documents come from the clock
or the commit that carried the event, never from estimation. Instructions to Claude are lines inside
any file that begin with his name and a colon; `tools/sync.sh` finds them, the tree itself is the
queue, and a marker that asks for another agent's work is routed under "Routed to" in `TASKS.md`.
