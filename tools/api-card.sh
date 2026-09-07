#!/usr/bin/env bash
# api-card.sh prints exact declaration lines and code excerpts from crate/tests/web
# sources, so a unit brief can paste real excerpts without reading whole files by hand.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

crate="${1:-}"; pattern="${2:-}"; ctx="${3:-12}"
usage() { echo "usage: $(basename "$0") <crate>|tests|web [pattern] [context-lines]" >&2; }
[[ -z "$crate" ]] && { usage; exit 1; }

CRATE_AWK='FNR==1{prev=""} {t=$0; sub(/^[ \t]*/,"",t); m=0
  if (t~/^pub\(crate\) /) m=1; else if (t~/^pub /) m=1
  else if (t~/^impl/) m=1; else if (t~/^mod /) m=1
  else if (prev=="#[test]" && t~/^fn /) m=1
  if (m) printf "%s:%d: %s\n", FILENAME, FNR, t; prev=t}'
TESTS_AWK='FNR==1{prev=""} {t=$0; sub(/^[ \t]*/,"",t); m=0
  if (FILENAME~/^tests\/common\//) { if (t~/^pub fn /) m=1 }
  else if (prev=="#[test]" && t~/^(pub )?fn /) m=1
  if (m) printf "%s:%d: %s\n", FILENAME, FNR, t; prev=t}'
WEB_AWK='{t=$0; sub(/^[ \t]*/,"",t); if (t~/^export /) printf "%s:%d: %s\n", FILENAME, FNR, t}'

case "$crate" in
  tests)
    files="$( { find tests -maxdepth 1 -name '*.rs'; find tests/common -name '*.rs' 2>/dev/null; } | sort)"
    select_awk="$TESTS_AWK" ;;
  web)
    files="$(find web/src/lib -maxdepth 1 -name '*.ts' | sort)"
    select_awk="$WEB_AWK" ;;
  *)
    [[ -d "crates/$crate/src" ]] || { usage; exit 1; }
    files="$(find "crates/$crate/src" -name '*.rs' | sort)"
    select_awk="$CRATE_AWK" ;;
esac

[[ -z "${files// /}" ]] && { usage; exit 1; }
listing="$(awk "$select_awk" $files | sort -t: -k1,1 -k2,2n)"

if [[ -z "$pattern" ]]; then
  printf '%s\n' "$listing"
  exit 0
fi

matches="$(printf '%s\n' "$listing" | grep -E -- "$pattern" || true)"
[[ -z "$matches" ]] && exit 0

first=1
while IFS= read -r entry; do
  fpath="${entry%%:*}"; rest="${entry#*:}"; lno="${rest%%:*}"
  [[ $first -eq 0 ]] && echo "--"
  first=0
  sed -n "${lno},$((lno + ctx))p" "$fpath"
done <<< "$matches"
