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

Run `python3 tools/inbox.py` to fetch from origin and list pending markers and unseen commits.
Do this at the start of every turn before anything else, after every commit, whenever waiting
on something rather than idling, and before ending a turn.

The user commits on their own branch and pushes to GitHub. Fetch and merge regularly: their
edits are the other half of this conversation.

## Ground rules

- **Rust** for the simulator and backends, **Node.js + React** for the frontend, per VISION.md.
  Interfaces are defined in `proto/`; the user reviews every interface.
- Keep the core simulation dependency-light.
- Determinism is non-negotiable: every random draw comes from an explicitly seeded stream.
  Same config + same seed must produce byte-identical output.
- Policies are pluggable. Adding a policy must not require touching the engine.
- No `time.time()`, no wall-clock, no threads in simulation code. Simulated clock only.

## Current state

Design phase. **Nothing is built.** The user must bless `docs/ARCHITECTURE.md` and the
interfaces in `proto/` before any implementation starts. Do not write simulator code
until then.

`TASKS.md` tracks what is waiting on the user, stack ranked. `STATUS.md` tracks what is done
and live. Keep both current.

## Commands

- Check the inbox: `python3 tools/inbox.py`
- Validate interfaces: `protoc --proto_path=proto --descriptor_set_out=/dev/null $(find proto -name "*.proto")`
- Validate the diagram against the protos: `python3 tools/check_diagram.py`
- Toolchain setup, if `$HOME` was wiped: see `docs/toolchain.md`

## Style

- Rust: no `unsafe` in simulation code. Prefer data-oriented layout (struct-of-arrays) in hot paths.
- Comments explain *why*, never *what*.
