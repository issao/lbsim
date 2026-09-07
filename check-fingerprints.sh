#!/usr/bin/env bash
# The golden-scenario check: did any number move?
#
# Runs every demo, the held-out suite and a probe_live run with telemetry on, and prints one line per run
# holding the determinism fingerprint, the event count and an md5 of the whole summary.csv, which
# carries every reported metric. `bench/golden-fingerprints.txt` is the committed baseline.
#
#   ./check-fingerprints.sh            compare against the baseline; exit non-zero on any difference
#   ./check-fingerprints.sh --update   rewrite the baseline. A commit that does this must say why every
#                                      changed number changed; that is the whole point of the file.
#
# A unit of work that claims "no behaviour change" is verified by this exiting zero, not by the claim.
# A unit that changes behaviour deliberately updates the baseline in the same commit as the change.
set -uo pipefail
cd "$(dirname "$0")"
export PATH="$HOME/local/bin:$HOME/.local/bin:$PATH"
GOLDEN=bench/golden-fingerprints.txt
S="tools/build.sh run --release --quiet --bin sim-run --"
OUT=$(mktemp -d)
trap 'rm -rf "$OUT"' EXIT

run() { # label, args...
  local label=$1; shift
  $S "$@" --out "$OUT/$label.html" --telemetry "$OUT/$label" >/dev/null 2>&1 || echo "$label: RUN FAILED"
}
run 1-routing compare scenarios/route_round_robin.txt scenarios/route_p2c.txt scenarios/route_least_requests.txt scenarios/route_random.txt
run 2-staleness sweep scenarios/route_least_requests.txt --over telemetry_interval_ms=100,250,500,1000,2000,4000
run 3-chunking sweep scenarios/route_p2c.txt --over step_token_budget=512,1024,2048,4096,8192,16384
run 4-load-curve sweep scenarios/route_p2c.txt --over arrival_rps=30,70,110,150,190,230
run 5-long-context sweep scenarios/route_p2c.txt --over long_probability=0.0,0.04,0.08,0.16,0.32
run 6-retry compare scenarios/retry_none.txt scenarios/retry_budget.txt scenarios/retry_storm.txt
run 7-least-queue-tokens run scenarios/route_least_queue_tokens.txt
run 8-probe-live run scenarios/route_p2c.txt --set probe_live=true
run 7-no-decode compare scenarios/route_round_robin_no_decode.txt scenarios/route_p2c_no_decode.txt
run 8-admission compare scenarios/admit_accept_all.txt scenarios/admit_deadline_aware.txt
run 9-fair-share compare scenarios/admit_tenants_accept_all.txt scenarios/admit_fair_share.txt
run 10-probes compare scenarios/route_p2c.txt scenarios/route_least_kv_probe.txt
run 11-preemption compare scenarios/kv_spiral_never.txt scenarios/kv_spiral_swap.txt
run 12-forecast-load run scenarios/route_forecast_load.txt
run 13-forecast-latency run scenarios/route_forecast_latency.txt
for h in scenarios/holdout/*.txt; do run "h-$(basename "$h" .txt)" run "$h"; done

lines=$( {
  for f in $(find "$OUT" -name '*.summary.csv' | sort); do
    rel=${f#"$OUT"/}
    fp=$(grep '^fingerprint,' "$f" | cut -d, -f2)
    ev=$(grep '^events,' "$f" | cut -d, -f2)
    sum=$(md5sum "$f" | cut -c1-12)
    printf '%-60s fingerprint=%-22s events=%-9s summary_md5=%s\n' "$rel" "$fp" "$ev" "$sum"
  done
  # The rendered reports too, so a change to the report crate is held to the same standard.
  for f in $(find "$OUT" -maxdepth 1 -name '*.html' | sort); do
    printf '%-60s html_md5=%s\n' "${f#"$OUT"/}" "$(md5sum "$f" | cut -c1-12)"
  done
} )

if [ "${1:-}" = "--update" ]; then
  printf '%s\n' "$lines" > "$GOLDEN"
  echo "baseline rewritten: $GOLDEN ($(printf '%s\n' "$lines" | wc -l) runs)"
  exit 0
fi
if diff <(printf '%s\n' "$lines") "$GOLDEN" >"$OUT/diff.txt"; then
  echo "PASS: every fingerprint, event count and summary metric matches $GOLDEN ($(wc -l < "$GOLDEN") runs)"
else
  echo "FAIL: numbers moved. If deliberate, rerun with --update and say why in the commit."
  cat "$OUT/diff.txt"
  exit 1
fi
