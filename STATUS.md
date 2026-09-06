# STATUS

What is live on `origin/master`, what each agent is doing now, and the assumptions being acted on.
For things that need *you*, see `TASKS.md`.

**Last updated:** 2026-09-06 16:20 by Claude.

---

## Phase

**Building, delegated.** Three long-lived agents with disjoint file ownership, recorded in `CLAUDE.md`.
Scope is `docs/scope-today.md` package B plus item 7, and both side missions Issao asked for.

## Live on `origin/master`

| What | Where | Verified by |
|---|---|---|
| Simulator, six dynamics reproducing | `src/`, `scenarios/` | `./run-demos.sh`, six HTML reports in `out/` |
| Test suite: 45 pass, 2 ignored as known defects | `tests/` | `cargo test` |
| Policy ordering survives ±30% cost-model error | `check-sensitivity.sh` | run it; exits non-zero on a flip |
| Arena, mechanical half, eight-scenario held-out suite | `src/arena.rs`, `scenarios/holdout/` | `cargo test --release --lib -- --nocapture` |
| Stand-in dashboard, three surfaces, mock data | `web/` | `cd web && npm run build` |
| **Deployed, public**: <https://lbsim-irpwc2yaoa-uc.a.run.app>, dashboard at `/`, reports at `/reports/1-routing.html` to `6-retry.html` | `Dockerfile`, `cloudbuild.yaml`, `deploy.sh` | `curl -sI` on the URL returns 200; `./deploy.sh --check-idle` reads the instance count from Cloud Monitoring |
| Reference cost model, exact against a naive oracle | `bench/validate_epochs.py` | `tools/sync.sh` runs it |
| Interfaces, twelve files, reviewed | `proto/lbsim/v1/` | `tools/sync.sh` compiles them |
| Findings, one section per dynamic | `docs/findings.md` | every table from `./run-demos.sh` |

The six dynamics, one line each, all measured at 30% of rated capacity unless the sweep is over load:
reading the whole fleet routes worse than sampling two of it; herding has a staleness threshold, not a
gradient; prefill and decode contend and no chunk size wins both; past the knee more offered load
delivers less; a load-balancer metric improves while service collapses; a retry budget is the
difference between a bad minute and an outage. Numbers in `docs/findings.md`.

## What each agent is doing now

| Agent | Owns | Now |
|---|---|---|
| Tech lead | `src/`, `tests/`, `scenarios/`, `web/src/lib/` | branch `claude/tl-ingress`; nothing merged beyond the above yet |
| Cloud | `Dockerfile`, `deploy.sh`, `cloudbuild.yaml`, `docs/deploy.md` | first deploy done at 15:45, merged as 72dfb16. No further IAM grant was needed: the blocker was two bucket permissions, worked around in `cloudbuild.yaml`. `docs/deploy.md` not written yet |
| Monitor and housekeeping | `TASKS.md`, `STATUS.md`, `README.md`, `docs/*.md` | inbox and PR loop every 90 s; tidying the documents; next `README.md`, then `docs/` contradictions |

The main agent coordinates and owns `CLAUDE.md` and `proto/`.

## Assumptions being acted on

- Package B of `docs/scope-today.md` plus item 7 is today's scope; the dashboard and the arena were
  reinstated by Issao and are built.
- Arena: SLA cap 0.95 is Issao's rule for now; the raw objective stays with goodput share beside it
  until he rules on `TASKS.md` item 2. The policy generator may write code, not only parameters.
- Prefix-sharing topology comes from a session model with fork-off and merge-back rates, swept.
- Leaf shards are separate processes; memory tiers are Ingress-owned with a seeded bloom-filter
  residency hint. Both are design of record, `docs/ARCHITECTURE.md` §14, not yet built.
- `lbsim.ai` is dark: its DNS zone went with `lbsim-prod`. `TASKS.md` item 1.
- Deployment scales to zero within a replica budget of ten, public, on the `run.app` URL until the
  domain steps in `TASKS.md` item 1 are done. Scale-to-zero is measured by `deploy.sh --check-idle`,
  not assumed.
- Reference hardware is a 70-billion-parameter model on eight H100s.
- Everything in `docs/ARCHITECTURE.md` section 14 stands as recorded there.

## Tree state

Each agent commits on its own `claude/<topic>` branch, merges to `master` with `--no-ff`, and pushes.
The housekeeping agent works in a separate worktree at `/home/agents/repo/lbsim-docs` so that another
agent's uncommitted files in `/home/agents/repo/lbsim` never enter its commits.
