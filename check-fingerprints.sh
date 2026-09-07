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
# The named runs live in bench/fingerprint-runs.txt (one label + args per line, `#` comments and
# blank lines skipped) so two agents adding scenarios in parallel append lines instead of
# colliding on this script; `.gitattributes` unions that file across a merge.
while IFS= read -r fr_line; do
  case "$fr_line" in ''|'#'*) continue ;; esac
  read -r -a fr_args <<<"$fr_line"
  run "${fr_args[@]}"
done < bench/fingerprint-runs.txt
for h in scenarios/holdout/*.txt; do run "h-$(basename "$h" .txt)" run "$h"; done

lines=$( {
  for f in $(find "$OUT" -name '*.summary.csv' | sort); do
    rel=${f#"$OUT"/}
    fp=$(grep '^fingerprint,' "$f" | cut -d, -f2)
    ev=$(grep '^events,' "$f" | cut -d, -f2)
    sum=$(md5sum "$f" | cut -c1-12)
    printf '%-60s fingerprint=%-22s events=%-9s summary_md5=%s\n' "$rel" "$fp" "$ev" "$sum"
  done
  # The rendered reports too, so a change to the report crate is held to the same standard. The
  # verbatim-scenario block is stripped first: it embeds Scenario::to_text(), so a new Scenario
  # field alone would otherwise move every html_md5 row with no run behaviour having changed.
  for f in $(find "$OUT" -maxdepth 1 -name '*.html' | sort); do
    printf '%-60s html_md5=%s\n' "${f#"$OUT"/}" "$(sed '/<!-- scenario -->/,/<!-- \/scenario -->/d' "$f" | md5sum | cut -c1-12)"
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
