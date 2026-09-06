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
- **Always merge back to `origin`.** Do not leave work stranded on a local branch:
  when a unit of work is complete, merge the branch into `master` and push both
  `master` and the branch to `origin`. A session must never end with unpushed commits.
- Merge with `--no-ff` so each unit of work stays visible as a group in history.
- If `master` has moved, rebase the working branch onto it before merging, so history
  stays linear and the user's own branch does not collide.
- Commit at every completed unit of work, not at the end of a session.

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

`TODO.md` tracks what is waiting on the user. Keep it current.

## Commands

- Validate interfaces: `protoc --proto_path=proto --descriptor_set_out=/dev/null $(find proto -name "*.proto")`
- Toolchain setup, if `$HOME` was wiped: see `docs/toolchain.md`

## Style

- Rust: no `unsafe` in simulation code. Prefer data-oriented layout (struct-of-arrays) in hot paths.
- Comments explain *why*, never *what*.
