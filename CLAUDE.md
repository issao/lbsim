# lbsim — instructions for Claude

<!-- Keep this file SHORT. It is prepended to every session. Long files get skimmed. -->
<!-- Owner: the user. Claude may propose edits but should ask before rewriting. -->

## What this is

A discrete-event simulator for a cloud LLM inference service. Two purposes:
reproduce real-world load dynamics, and evaluate scheduling / load-balancing /
traffic-shaping policies for performance, service quality, and failure robustness.

See VISION.md for the authoritative scope. See docs/diagrams/README.md for the
diagram-to-proto convention.

## Workflow rules

- **Never leave the working tree dirty.** Every unit of work ends in a commit.
- Work on a branch named `claude/<topic>`. The user keeps their own edits on their
  own branch.
- **Always push to `origin master`.** The user pulls `master` into a local clone, so work
  that is not on `origin/master` does not reach them. When a unit of work is complete:
  merge the working branch into `master`, then `git push origin master` (and the branch).
  A session must never end with unpushed commits on `master`.
  If the push fails for lack of credentials, say so prominently rather than
  reporting the work as delivered.
- Merge with `--no-ff` so each unit of work stays visible as a group in history.
- If `master` has moved, rebase the working branch onto it before merging, so history
  stays linear and the user's own branch does not collide.
- Commit at every completed unit of work, not at the end of a session.

## Two files are the primary channel to the user

The user may not read the chat. **Anything that needs their attention goes in a file, not only
in a reply.**

- **`TASKS.md`** — everything Claude needs from the user, stack ranked, most important and
  most blocking first. Every item states what Claude will assume if the user says nothing, so
  no item stalls the work indefinitely. Update it the moment a need appears or is met.
- **`STATUS.md`** — what is finished, what is live on `origin/master`, what Claude is doing
  right now, and which assumptions Claude is running on. Update it at every unit of work,
  before pushing.

Both carry a "last updated" line. Stale files are worse than none, because the user will act
on them.

## The Issao inbox — check constantly

The user leaves instructions for Claude inside the files themselves, on a line beginning with
their name followed by a colon, in a code comment or on a markdown line. Each one is a direct
instruction. The protocol is:

1. **Act on it.** Fold the instruction into the state of the repo: change the design, the
   interfaces, the code, the docs, whatever it asks for.
2. **Delete the line.** Once the repo reflects it, remove the marker block entirely.
   `python3 tools/inbox.py --resolve <file>:<line>` does this safely.
3. **Say so in the commit message**, quoting the instruction, so the exchange survives in
   history rather than only in a deleted line.

A marker left in the tree means the work is not done. Never edit a marker in place, and never
leave one behind as answered: acting on it and deleting it are one unit of work.

### Reacting to a push

**Run `tools/sync.sh`.** One command, so no step gets forgotten. It fetches, integrates
`origin/master` when the integration is unambiguous, lists pending instructions, and checks
that the protos compile, the diagram still matches them, and the epoch math is still exact. It
exits non-zero when anything needs attention, and refuses to touch a dirty tree.

Run it at the start of every turn before anything else, whenever the commit watcher fires,
after every commit, whenever waiting on something rather than idling, and before ending a turn.

Then, for each instruction it lists:

1. Act on it.
2. `python3 tools/inbox.py --resolve <file>:<line>` to remove the marker.
3. Commit, quoting the instruction. Merge to `master` and push.
4. Re-run `tools/sync.sh` to confirm nothing is left.

### The watcher

A background monitor polls `git ls-remote origin` every 60 seconds and reports new commits
plus any instruction markers inside them. **It lives only as long as this session.** If the
session restarts, re-arm it, and until then rely on `tools/sync.sh` at every turn boundary.
Never claim the watcher is running without checking.

## Cloud: what Claude may and may not do

**The credential here is a scoped deploy identity, not the user's account.** As of 2026-09-06 the
sandbox holds only `lbsim-deployer@lbsim-gcp`, with Cloud Run admin, Artifact Registry writer, Cloud
Build editor, service account user, monitoring viewer, and object admin on `gs://lbsim-gcp-runs`. The
user's own credential was revoked from here.

That history matters, because it is why the boundary exists. Before the switch `gcloud` was
authenticated as the user with full permissions, and a subagent used it to delete one of the user's
budgets while cleaning up a duplicate it had created. The boundary is now verified rather than
promised: an attempted self-grant of a role was refused, because the identity cannot even read the
policy it would need to modify.

**Standing authorization, granted 2026-09-06:** *"you can actually deploy without asking for
permission. As long as it helps development, feel free to deploy as needed for up to 10 replicas at a
time."*

- **Allowed unattended:** build images, push to Artifact Registry, create and update Cloud Run services
  and jobs, create domain mappings, read monitoring. Cap every service at `--max-instances 10` and
  every job at `--parallelism 10`, and always `--min-instances 0`.
- **Ask first:** deleting anything Claude did not create in this session, and anything outside
  `lbsim-gcp`.
- **Billing is off limits, and now also impossible.** Budgets, billing accounts, project links, quotas.
  The user asked for exactly this: deploy powers without billing powers. Claude can no longer even
  *read* the budget, which is the correct trade and means budget verification belongs to the user.
- **Any delete filter must be an exact match, never a prefix or substring.** That is precisely how the
  budget was lost: a filter meant for one name matched two.
- **Every `gcloud` call takes `--quiet`.** It offers to enable APIs interactively, and unattended that
  prompt hangs a deploy rather than failing it. A hang is harder to diagnose than an error.
- **Delegating cloud work to a subagent is now acceptable, and only because the credential is scoped.**
  The earlier rule forbade it, and the reason was that a subagent inherited the user's full account and
  deleted a budget. That harm is now impossible: the only credential here has no billing role and cannot
  read IAM. The user has since asked for cloud work to be delegated entirely. The rule that remains is
  the one above about deletes and exact-match filters.
- **Watch instance-hours, not the bill.** Non-zero while nobody is using the dashboard means the idle
  shutdown is broken. That is the real cost control; the budget is only the tripwire behind it.

**Secrets never enter the repo or the conversation.** The deploy key lives at
`~/.config/gcloud/lbsim-deployer.json`, mode 600, outside any git repository. `.gitignore` carries
credential-shaped patterns and `tools/sync.sh` runs a secret scan over the tree on every invocation. A
credential pasted into a transcript is disclosed rather than transient, whatever its expiry, so it is
never the right mechanism.

## Ground rules

- **Rust** for the simulator and backends, **Node.js + React** for the frontend, per VISION.md.
  Interfaces are defined in `proto/`; the user reviews every interface.
- Keep the core simulation dependency-light.
- Determinism is non-negotiable: every random draw comes from an explicitly seeded stream.
  Same config + same seed must produce byte-identical output.
- Policies are pluggable. Adding a policy must not require touching the engine.
- No `time.time()`, no wall-clock, no threads in simulation code. Simulated clock only.

## Current state

Building. The simulator runs, six dynamics reproduce, the workspace test suite passes, and the stand-in dashboard builds. Counts live in `STATUS.md`, not here, because a number in this file goes stale within the hour.
See `STATUS.md`. Work is delegated across three long-running agents with strict file ownership:

| Agent | Owns | Must not touch |
|---|---|---|
| Tech lead | `src/`, `tests/`, `scenarios/`, `web/src/lib/` | docs, `TASKS.md`, `STATUS.md`, cloud files |
| Cloud | `Dockerfile`, `deploy.sh`, `cloudbuild.yaml`, `.dockerignore`, `docs/deploy.md` | everything else |
| Monitor and housekeeping | `TASKS.md`, `STATUS.md`, `README.md`, `docs/*.md` except deploy | code, cloud files, this file |

The main agent coordinates, owns this file and `proto/`, and stays out of the areas above. Any agent that
needs a file it does not own stops and says so rather than editing it. `git add` is always by explicit
path, never `-A`, because that is how one agent's in-flight files ended up in another's commit today.

That rule turned out to be insufficient. **Commit with an explicit pathspec too: `git commit -m "..." -- <paths>`, message first, then the
separator, then the paths.** Getting the order wrong makes git read the message as a pathspec and fail
loudly, which is at least the safe direction.
Another agent had *staged* its files in the shared checkout without committing, and a plain `git commit`
swept everything in the index, explicit `git add` or not. `git commit -- <paths>` commits only those
paths regardless of what else is staged. Before committing in a shared checkout, `git diff --cached
--name-only` shows whether the index holds anyone else's work. The real cure is the worktree rule below,
which the main agent now follows as well.

**The tech lead breaks dependencies before accepting them**, per Issao: *"look at the overall execution
plan graph and whenever there is a dependency, consider if we can make a small refactor or propose a
stand in interface to break up the dependency."* The tech lead keeps an explicit dependency graph of the
in-flight and queued units, and for every edge asks whether a small refactor or a stand-in interface (a
trait with a trivial implementation, a fixture in the final file format, a shape fixed in prose like
`crates/sim-ingress/WIRE.md`) lets the downstream unit start now and reconcile at integration. A unit's
brief states which upstream it would have waited for and what stand-in removed the wait. Edges that could
not be broken are reported with the reason, so the wait is a decision rather than a default.

**A simplification pass runs on a cadence**, per the user: after every wave of merges or every four
merges, the tech lead spawns one simplification agent, never more than one at a time. Its mandate is
the `/simplify` skill's: reuse, simplification, efficiency and altitude cleanups, no bug hunting, no
features, prefer deletion over rewriting. Zero behavior change, proven by every test passing unchanged
and by `bench/validate_epochs.py`, `check-sensitivity.sh` and the determinism fingerprints producing
byte-identical output. One crate per pass, own worktree, and it reports what it deliberately did not
simplify, because the temptation in that kind of pass is to keep going. A change to `proto/` or a
crate boundary is a decision rather than a cleanup and is reported, not made.

**Each agent works in its own git worktree**, not the shared checkout. A shared checkout means one
agent's dirty files make `tools/sync.sh` refuse for everyone. The housekeeping agent already works in
`/home/agents/repo/lbsim-docs`; the tech lead is moving its subagents to worktrees. Memory is safe
because `.cargo/config.toml` points every worktree at one absolute target directory, so cargo's file
lock serializes builds. `tools/sync.sh` keeps its state files under `git rev-parse --git-dir`, which is
per-worktree.

`TASKS.md` tracks what is waiting on the user, stack ranked. `STATUS.md` tracks what is done
and live. Keep both current.

## Commands

- React to a push, and check every invariant: `tools/sync.sh`
- Check the inbox only: `python3 tools/inbox.py`
- Validate interfaces: `protoc --proto_path=proto --descriptor_set_out=/dev/null $(find proto -name "*.proto")`
- Validate the diagram against the protos: `python3 tools/check_diagram.py`
- Toolchain setup, if `$HOME` was wiped: see `docs/toolchain.md`

## Style

- Rust: no `unsafe` in simulation code. Prefer data-oriented layout (struct-of-arrays) in hot paths.
- Comments explain *why*, never *what*.
