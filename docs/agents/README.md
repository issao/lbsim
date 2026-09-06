# Agent briefs: how to restart the fleet from files

Every agent in this project is restartable from the repository alone. A session restart kills every
agent (they live only as long as the session), so each one's memory is a file it owns, and each one's
spawn brief is here. The main session re-spawns them in this order, each with `Agent` and the brief
file's contents as the prompt:

| Order | Agent | Brief | Its memory | Model |
|---|---|---|---|---|
| 1 | Monitor and housekeeping | `housekeeping.md` | `TASKS.md`, `STATUS.md` | default |
| 2 | Tech lead | `tech-lead.md` | `docs/execution-graph.md` | Fable (`model: fable`), high effort |
| as needed | Cloud | `cloud.md` | `docs/deploy.md` | default; only when a deploy or cloud change is needed |

The main session's own memory is `CLAUDE.md`, this directory, and `docs/dashboard-plan.md`. Its first
actions after a restart: `tools/sync.sh`, read `STATUS.md`, re-spawn 1 and 2, re-arm the commit watcher.
