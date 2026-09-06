#!/usr/bin/env bash
# Does the policy ordering survive a 30% error in the cost model?
#
# The cost model is calibrated at two points and interpolates a roofline between them, so its absolute
# numbers carry real uncertainty. What must survive that uncertainty is the *ordering*: if a policy
# ranking flips when the utilization constants move by 30%, it was never a conclusion.
#
# Exits non-zero if any perturbation reorders the policies.
set -uo pipefail
cd "$(dirname "$0")"
export PATH="$HOME/local/bin:$HOME/.local/bin:$PATH"
# Through cargo rather than ./target: .cargo/config.toml points every worktree at one shared target
# directory, so a relative path is wrong in all but the original checkout.
S="cargo run --release --quiet --bin sim-run --"
SCN="scenarios/route_p2c.txt scenarios/route_round_robin.txt scenarios/route_random.txt scenarios/route_least_requests.txt"

# label : step_base_ms : step_per_kv_ktoken_ms : prefill_tokens_per_s
CASES=(
  "nominal:10.2:0.0175:28286"
  "bandwidth -30%:13.26:0.02275:28286"
  "bandwidth +30%:7.14:0.01225:28286"
  "prefill -30%:10.2:0.0175:19800"
  "prefill +30%:10.2:0.0175:36772"
  "both -30%:13.26:0.02275:19800"
  "both +30%:7.14:0.01225:36772"
)

expected=""
fail=0
printf "%-16s %s\n" "perturbation" "ranking by goodput, best first"
printf "%s\n" "----------------------------------------------------------------------"
for c in "${CASES[@]}"; do
  label="${c%%:*}"; rest="${c#*:}"
  base="${rest%%:*}"; rest="${rest#*:}"
  kv="${rest%%:*}"; pf="${rest#*:}"
  ranking=$($S compare $SCN \
      --set step_base_ms="$base" --set step_per_kv_ktoken_ms="$kv" \
      --set prefill_tokens_per_s="$pf" --out /dev/null 2>/dev/null \
    | sed -n '4,20p' | awk '$2+0 == $2 && $2 != "" {print $1, $2}' | sort -k2 -nr | awk '{printf "%s ", $1}')
  printf "%-16s %s\n" "$label" "$ranking"
  # An empty ranking is a broken run, not an agreement. Without this check a missing binary produced
  # seven empty rankings that all matched, and the script said PASS.
  if [ -z "$ranking" ]; then
    echo "    NO RANKING PRODUCED; the run failed"
    fail=1
  fi
  if [ -z "$expected" ]; then
    expected="$ranking"
  elif [ "$ranking" != "$expected" ]; then
    echo "    ORDERING CHANGED under this perturbation"
    fail=1
  fi
done
echo
if [ "$fail" -eq 0 ]; then
  echo "PASS: the ordering is invariant to +/-30% in the cost constants, so it is a conclusion"
  echo "      rather than an artefact of the calibration."
else
  echo "FAIL: at least one ordering flipped. The affected comparison is not a conclusion."
fi
exit "$fail"
