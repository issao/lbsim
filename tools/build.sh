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
# directory above the worktrees, so a build waits in the kernel rather than polling. The lock is held
# by the exec'd cargo process itself and released when it exits, however it exits.
#
# Every agent runs cargo through this, including the scripts in the repo root. A bare `cargo`
# invocation is the memory runaway this exists to prevent.
set -u
MAX_BUILDS=${LBSIM_MAX_BUILDS:-2}
LOCK_DIR=${LBSIM_LOCK_DIR:-/home/agents/repo}
export PATH="$HOME/local/bin:$HOME/.local/bin:$PATH"

for slot in $(seq 1 "$MAX_BUILDS"); do
  exec {fd}>"$LOCK_DIR/.cargo-build.$slot.lock"
  if flock -n "$fd"; then
    exec cargo "$@"
  fi
  exec {fd}>&-
done
# Every slot busy: wait on the first one. Others may free up sooner, but a single wait is simple and
# starvation-free, and a build queue of a few seconds is not the problem being solved here.
exec {fd}>"$LOCK_DIR/.cargo-build.1.lock"
flock "$fd"
exec cargo "$@"
