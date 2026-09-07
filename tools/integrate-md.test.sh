#!/usr/bin/env bash
# Unit test for the docs_only decision function used by tools/integrate.sh (U68). Sources
# nothing from integrate.sh on purpose — integrate.sh runs its gate on load (flock, fetch, ...),
# so it is not safe to source. This carries an identical copy of docs_only and tests it alone.
#
#   bash tools/integrate-md.test.sh
set -uo pipefail

docs_only() {
  local paths=$1 p n=0
  while IFS= read -r p; do
    [ -n "$p" ] || continue
    case "$p" in
      *.md) n=$((n + 1)) ;;
      *) return 1 ;;
    esac
  done <<<"$paths"
  [ "$n" -gt 0 ]
}

fail=0
check() {
  local name=$1 input=$2 want=$3 got
  if docs_only "$input"; then got=0; else got=1; fi
  if [ "$got" = "$want" ]; then
    printf 'ok   %s\n' "$name"
  else
    printf 'FAIL %s: want %s got %s\n' "$name" "$want" "$got"
    fail=1
  fi
}

check "single md file"        $'docs/a.md'              0
check "two md files"          $'docs/a.md\nREADME.md'    0
check "mixed md and code"     $'docs/a.md\nsrc/x.rs'     1
check "empty diff"            ''                         1

if [ "$fail" = 0 ]; then
  echo "all docs_only cases passed"
else
  echo "docs_only test FAILED"
fi
exit "$fail"
