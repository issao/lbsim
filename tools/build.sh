#!/usr/bin/env bash
# Run cargo under a machine-wide concurrency bound.
#
#   tools/build.sh test
#   tools/build.sh run --release --quiet --bin sim-run -- compare a.txt b.txt
#
# Each git worktree has its own target directory (see .cargo/config.toml, and the wrong build that
# a shared one produced). That removed the accidental memory bound the shared directory's file lock
# provided, so this puts an explicit one back: at most MAX_BUILDS cargo processes run at once across
# every worktree on the machine, each capped at jobs=4 by the config. Slots are flock(2) files in the
# directory above the worktrees.
#
# At 16:53 three agents hit this at once: a hung `tests/ingress_http.rs` (a server that never exited)
# held slot 1, and the old "wait on slot 1 only" fallback pinned the other two waiters behind it while
# slot 2 sat idle free. All three returned only at the calling tool's 600s limit, and the hung cargo
# kept the slot even after that. So: every slot is polled round-robin with `flock -n` once a second
# until one is free, instead of blocking on a single slot, and every cargo runs under `timeout` so a
# hang can't hold a slot indefinitely. LBSIM_BUILD_TIMEOUT (default 420s, five times the slowest
# legitimate debug test) bounds that hold; a hang returns exit 124 well inside the calling tool's
# turn instead of holding the slot until something outside the script kills it. The lock fd is
# inherited by both `timeout` and the cargo it execs, so the slot is released when either exits,
# whichever happens first.
#
# Every agent runs cargo through this, including the scripts in the repo root. A bare `cargo`
# invocation is the memory runaway this exists to prevent.
set -u
MAX_BUILDS=${LBSIM_MAX_BUILDS:-2}
LOCK_DIR=${LBSIM_LOCK_DIR:-/home/agents/repo}
BUILD_TIMEOUT=${LBSIM_BUILD_TIMEOUT:-420}
export PATH="$HOME/local/bin:$HOME/.local/bin:$PATH"

while :; do
  for slot in $(seq 1 "$MAX_BUILDS"); do
    exec {fd}>"$LOCK_DIR/.cargo-build.$slot.lock"
    if flock -n "$fd"; then exec timeout -k 5 "$BUILD_TIMEOUT" cargo "$@"; fi
    exec {fd}>&-
  done
  sleep 1
done
