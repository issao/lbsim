# lbsim

A discrete-event simulator for a cloud LLM inference service. It exists to reproduce the load
dynamics a real fleet shows, and to score scheduling, load-balancing and traffic-shaping policies for
performance, service quality and robustness. Rust engine, React dashboard, protobuf interfaces.
`VISION.md` is the authoritative scope.

## Run it

```bash
export PATH="$HOME/local/bin:$HOME/.local/bin:$PATH"   # this sandbox; see docs/toolchain.md
./run-demos.sh              # six experiments, six self-contained HTML reports in out/
cargo test                  # 45 tests, 2 ignored as known defects
./check-sensitivity.sh      # the policy ordering must survive 30% cost-model error
cargo test --release --lib -- --nocapture   # one arena round on the held-out suite
cd web && npm install && npm run dev        # the stand-in dashboard, mock data, localhost:5173
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
| the policy arena | `docs/arena.md` spec, `docs/arena-implementation.md` measured |
| the dashboard | `docs/ui-spec.md`, `web/README.md` |
| where the numbers come from | `docs/calibration.md`, `docs/llm-serving-primer.md` |
| how the agents are staffed | `docs/agent-architecture.md`, ownership table in `CLAUDE.md` |
| deploying | `docs/deploy.md`, `deploy.sh` |

## Working rules

`CLAUDE.md` has them. The short version: every unit of work ends in a commit on a `claude/<topic>`
branch, merged to `master` with `--no-ff` and pushed. Instructions to Claude are lines inside any file
that begin with his name and a colon; `tools/sync.sh` finds them, and the tree itself is the queue.
