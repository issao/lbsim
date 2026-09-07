#!/usr/bin/env bash
# Exercises the U45 fix to tools/build.sh (round-robin slot wait, timeout-bounded holds) against a
# private lock dir, so it never touches the real build slots other agents are using.
set -u

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BUILD_SH="$REPO_ROOT/tools/build.sh"

export LBSIM_LOCK_DIR="$(mktemp -d)"
export LBSIM_MAX_BUILDS=2

FAIL=0
BG_PIDS=()

cleanup() {
  for pid in "${BG_PIDS[@]:-}"; do
    [ -n "$pid" ] && kill "$pid" >/dev/null 2>&1 || true
  done
  wait >/dev/null 2>&1 || true
  rm -rf "$LBSIM_LOCK_DIR"
}
trap cleanup EXIT

elapsed() { awk -v a="$1" -v b="$2" 'BEGIN { printf "%.2f", b - a }'; }

report() {
  local name="$1" ok="$2" detail="$3"
  if [ "$ok" = "1" ]; then
    echo "PASS: $name ($detail)"
  else
    echo "FAIL: $name ($detail)"
    FAIL=1
  fi
}

# (a) slot 1 busy for 30s, slot 2 free: build.sh --version must grab slot 2 immediately, not wait.
flock "$LBSIM_LOCK_DIR/.cargo-build.1.lock" sleep 30 &
slot1_pid=$!
BG_PIDS+=("$slot1_pid")

start=$EPOCHREALTIME
"$BUILD_SH" --version >/tmp/build-slots-test-a.log 2>&1
rc=$?
end=$EPOCHREALTIME
dur_a=$(elapsed "$start" "$end")
ok=1
[ "$rc" -eq 0 ] || ok=0
awk -v d="$dur_a" 'BEGIN{exit !(d<2)}' || ok=0
report "(a) one slot busy, other free -> immediate" "$ok" "${dur_a}s, exit $rc"

# (b) both slots busy; slot 2 frees after 3s. build.sh --version must pick up slot 2 at ~3s, not
# wait out slot 1's full 30s the way the old single-slot fallback would have.
flock "$LBSIM_LOCK_DIR/.cargo-build.2.lock" sleep 3 &
BG_PIDS+=("$!")

start=$EPOCHREALTIME
"$BUILD_SH" --version >/tmp/build-slots-test-b.log 2>&1
rc=$?
end=$EPOCHREALTIME
dur_b=$(elapsed "$start" "$end")
ok=1
[ "$rc" -eq 0 ] || ok=0
awk -v d="$dur_b" 'BEGIN{exit !(d>=3 && d<=6)}' || ok=0
report "(b) both busy, second frees first -> ~3s not 30s" "$ok" "${dur_b}s, exit $rc"

# Free slot 1 now; its 30s hold has served both timed cases above and would otherwise block (c).
kill "$slot1_pid" >/dev/null 2>&1 || true
wait "$slot1_pid" 2>/dev/null || true

# Warm the compile cache so (c) times the timeout, not a cold build.
"$BUILD_SH" build -p sim-arena >/tmp/build-slots-test-warmup.log 2>&1 || true

# (c) a hung test must be killed at LBSIM_BUILD_TIMEOUT rather than holding its slot forever.
start=$EPOCHREALTIME
LBSIM_BUILD_TIMEOUT=2 "$BUILD_SH" test -p sim-arena -- --ignored arena_round_over_the_holdout_suite \
  >/tmp/build-slots-test-c.log 2>&1
rc=$?
end=$EPOCHREALTIME
dur_c=$(elapsed "$start" "$end")
ok=1
[ "$rc" -eq 124 ] || ok=0
awk -v d="$dur_c" 'BEGIN{exit !(d<=8)}' || ok=0
report "(c) hung test killed at LBSIM_BUILD_TIMEOUT" "$ok" "${dur_c}s, exit $rc"

echo "---"
echo "timings: a=${dur_a}s b=${dur_b}s c=${dur_c}s"
exit "$FAIL"
