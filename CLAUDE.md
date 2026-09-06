# lbsim — instructions for Claude

<!-- Keep this file SHORT. It is prepended to every session. Long files get skimmed. -->
<!-- Owner: the user. Claude may propose edits but should ask before rewriting. -->

## What this is

A discrete-event simulator for a cloud LLM inference service. Two purposes:
reproduce real-world load dynamics, and evaluate scheduling / load-balancing /
traffic-shaping policies for performance, service quality, and failure robustness.

See VISION.md for the authoritative scope. See docs/diagrams/README.md for the
diagram-to-proto convention.

## Ground rules

- Do not add a dependency without asking. Environment has no pip; stdlib only until told otherwise.
- Determinism is non-negotiable: every random draw comes from an explicitly seeded stream.
  Same config + same seed must produce byte-identical output.
- Policies are pluggable. Adding a policy must not require touching the engine.
- No `time.time()`, no wall-clock, no threads in simulation code. Simulated clock only.

## Commands

<!-- fill in as they exist -->
- Tests: `python3 -m unittest discover -s tests`
- Run a scenario: `python3 -m llmsim.cli run scenarios/<name>.yaml`

## Style

- Type hints on public functions. Dataclasses for state. No inheritance deeper than one level.
- Comments explain *why*, never *what*.
