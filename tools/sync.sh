#!/usr/bin/env bash
# React to Issao's commits: fetch, integrate, and report what needs acting on.
#
# The reaction to a push must be one command, not a remembered sequence, because a missed
# step means an instruction sits unnoticed in the tree. Run this whenever the commit monitor
# fires, at the start of a turn, and before ending one.
#
# Read-only with respect to history: it fetches and reports, and merges only when the merge
# is a clean fast-forward or a trivially clean merge. Anything requiring judgement is left
# for Claude to do deliberately.
#
# Exit codes:
#   0  nothing to do
#   1  something needs Claude's attention (new commits, pending instructions, or a conflict)
#   2  the repository or the remote is in a state this script will not touch

set -uo pipefail
cd "$(dirname "$0")/.." || exit 2

say() { printf '%s\n' "$*"; }
rule() { printf '%s\n' "------------------------------------------------------------------------"; }

attention=0

say "======================================================================"
say "sync: $(date '+%Y-%m-%d %H:%M:%S')  branch=$(git rev-parse --abbrev-ref HEAD)"
rule

# A dirty tree means an earlier unit of work did not finish. Stop rather than merge into it.
if [ -n "$(git status --porcelain)" ]; then
  say "DIRTY WORKING TREE — refusing to merge. Commit or discard first:"
  git status --short | sed 's/^/    /'
  exit 2
fi

if ! git fetch --all --prune --quiet 2>/dev/null; then
  say "WARNING: fetch failed; reporting from local refs only"
fi

# --- their commits ---------------------------------------------------------

incoming=$(git log --oneline --decorate --all --not HEAD --max-count=40 2>/dev/null)
if [ -n "$incoming" ]; then
  attention=1
  say "COMMITS not on HEAD:"
  printf '%s\n' "$incoming" | sed 's/^/    /'
  rule

  # Only integrate what is unambiguous. origin/master is the shared trunk.
  upstream="origin/$(git rev-parse --abbrev-ref HEAD 2>/dev/null)"
  target="origin/master"
  if git rev-parse --verify --quiet "$target" >/dev/null; then
    base=$(git merge-base HEAD "$target" 2>/dev/null)
    head_sha=$(git rev-parse HEAD)
    target_sha=$(git rev-parse "$target")
    if [ "$base" = "$target_sha" ]; then
      say "origin/master is already contained in HEAD; nothing to merge"
    elif [ "$base" = "$head_sha" ]; then
      say "fast-forwarding to origin/master"
      git merge --ff-only --quiet "$target" && say "    ok" || say "    FAILED"
    else
      say "HEAD and origin/master have diverged."
      say "Merging origin/master into this branch; conflicts are left for Claude to resolve."
      if git merge --no-edit --quiet "$target" 2>/dev/null; then
        say "    merged cleanly"
      else
        say "    CONFLICTS — resolve these before doing anything else:"
        git diff --name-only --diff-filter=U | sed 's/^/      /'
        exit 1
      fi
    fi
  fi
  rule
else
  say "COMMITS: none pending"
  rule
fi

# --- their in-file instructions -------------------------------------------
# Run after merging, since a new commit is exactly where a new instruction arrives.

if ! python3 tools/inbox.py --no-fetch; then
  attention=1
fi

# --- invariants that must hold before any further work --------------------

rule
if [ -d proto ]; then
  protoc_bin=$(command -v protoc || echo "$HOME/local/bin/protoc")
  if [ -x "$protoc_bin" ]; then
    if "$protoc_bin" --proto_path=proto --descriptor_set_out=/dev/null \
        $(find proto -name '*.proto' | sort) 2>/dev/null; then
      say "protos: compile"
    else
      say "protos: DO NOT COMPILE — fix before anything else"
      attention=1
    fi
  else
    say "protos: skipped, protoc not found (see docs/toolchain.md)"
  fi
fi

if [ -f tools/check_diagram.py ]; then
  if python3 tools/check_diagram.py >/dev/null 2>&1; then
    say "diagram: consistent with proto/"
  else
    say "diagram: DRIFTED from proto/ — run python3 tools/check_diagram.py"
    attention=1
  fi
fi

if [ -f bench/validate_epochs.py ]; then
  if python3 bench/validate_epochs.py >/dev/null 2>&1; then
    say "epoch math: still exact"
  else
    say "epoch math: FAILING — the core design claim broke"
    attention=1
  fi
fi

say "======================================================================"
exit "$attention"
