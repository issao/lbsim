#!/usr/bin/env bash
# Exercises the five behaviors api-card.sh must have so a change to it can't silently
# break the briefs that depend on its output shape.
set -uo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
card="$root/tools/api-card.sh"
fail=0

check() { # name, condition (0=pass)
  if [[ "$2" -eq 0 ]]; then echo "PASS: $1"; else echo "FAIL: $1"; fail=1; fi
}

out_a="$("$card" sim-model)"
n_a=$(printf '%s\n' "$out_a" | grep -c .)
[[ "$n_a" -ge 15 ]]; c1=$?
printf '%s\n' "$out_a" | grep -qF 'crates/sim-model/src/lib.rs:97: pub fn step(&mut self, sc: &Scenario, cost: &CostModel, now: Nanos) -> Option<StepOutcome> {'; c2=$?
check "(a) sim-model has >=15 items and the step() anchor" "$(( c1 != 0 || c2 != 0 ))"

out_b="$("$card" sim-metrics 'pub struct Frame')"
printf '%s\n' "$out_b" | grep -qF 'pub struct Frame {' && printf '%s\n' "$out_b" | grep -qF 'pub t: Nanos,'
check "(b) sim-metrics Frame struct excerpt" "$?"

out_c="$("$card" sim-physics step_ns 3)"
block_c="$(printf '%s\n' "$out_c" | awk '/pub fn step_ns/{f=1} f{print; c++} f && c==4{exit}')"
[[ "$(printf '%s\n' "$block_c" | wc -l)" -eq 4 ]]
check "(c) sim-physics step_ns with 3 lines of context" "$?"

"$card" no-such-crate >/dev/null 2>&1
check "(d) unknown crate exits 1" "$(( $? != 1 ))"

out_e="$("$card" web)"
n_e=$(printf '%s\n' "$out_e" | grep -c '^web/src/lib/api.ts')
[[ "$n_e" -ge 20 ]]
check "(e) web api.ts has >=20 exports" "$?"

exit "$fail"
