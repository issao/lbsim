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

# A stash nobody created deliberately means something outside this session moved the tree.
# A VS Code Git extension attached to this working directory did exactly that on 2026-09-06,
# stashing uncommitted work and switching branches, which silently reverted a large edit.
if [ -n "$(git stash list 2>/dev/null)" ]; then
  attention=1
  say "UNEXPECTED STASH — work may have been moved out of the tree by something else:"
  git stash list | sed 's/^/    /'
  say "    inspect with: git stash show --stat 'stash@{0}'"
  say "    recover with: git checkout 'stash@{0}' -- <paths>"
  rule
fi

# HEAD moving without this script doing it is the other half of the same symptom.
head_now=$(git rev-parse HEAD)
head_file=".git/lbsim-last-head"
if [ -f "$head_file" ]; then
  head_prev=$(cat "$head_file")
  if [ "$head_prev" != "$head_now" ]; then
    say "NOTE: HEAD moved since the last sync: ${head_prev:0:8} -> ${head_now:0:8}"
    say "      Expected after a commit or merge. Unexpected otherwise; check the reflog."
    rule
  fi
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

# --- comments the user added, prefixed or not ------------------------------
# Marker scanning finds lines that name him. It does not find an instruction written as a plain
# comment, and he writes those too: one arrived as a bare trailing comment on a proto field and
# would have gone unread. So diff his commits and show every comment line they added.
seen_file=".git/lbsim-last-remote"
remote_now=$(git rev-parse --verify --quiet origin/master || echo "")
if [ -n "$remote_now" ] && [ -f "$seen_file" ]; then
  remote_prev=$(cat "$seen_file")
  if [ "$remote_prev" != "$remote_now" ] && git cat-file -e "$remote_prev" 2>/dev/null; then
    added=$(git diff "$remote_prev".."$remote_now" -- '*.proto' '*.md' '*.rs' '*.toml' 2>/dev/null \
      | grep -E '^\+' | grep -vE '^\+\+\+' \
      | grep -E '(//|#|<!--|/\*)' \
      | grep -vE 'Co-Authored-By|Claude-Session' | head -40)
    if [ -n "$added" ]; then
      attention=1
      say "COMMENTS ADDED upstream since the last sync (${remote_prev:0:8}..${remote_now:0:8}):"
      printf '%s\n' "$added" | sed 's/^/    /'
      say "    -> read these as instructions even when they do not name him"
      rule
    fi
  fi
fi
[ -n "$remote_now" ] && echo "$remote_now" > "$seen_file"

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

# Secret scan on what is about to be, or has just been, committed. A one-off audit answers a question;
# a check that runs every time answers it continuously, and the failure mode here is silent and
# permanent, since a secret in git history survives deletion.
if git rev-parse --git-dir >/dev/null 2>&1; then
  leak=$(git grep -I -n -E \
    -e '-----BEGIN [A-Z ]*PRIVATE KEY-----' \
    -e '"private_key_id"[[:space:]]*:' \
    -e '"type"[[:space:]]*:[[:space:]]*"service_account"' \
    -e 'ya29\.[A-Za-z0-9_-]{40,}' \
    -e 'AIza[0-9A-Za-z_-]{35}' \
    -e 'AKIA[0-9A-Z]{16}' \
    -e 'xox[baprs]-[0-9A-Za-z-]{20,}' \
    -- HEAD 2>/dev/null | head -5)
  if [ -n "$leak" ]; then
    attention=1
    say "SECRET-SHAPED CONTENT IN THE TREE:"
    printf '%s\n' "$leak" | sed 's/^/    /'
    say "    -> do not just delete it. It is in history; rotate the credential first"
  else
    say "secret scan: clean"
  fi
fi

if [ -f tools/inbox.py ]; then
  if python3 tools/inbox.py --selftest >/dev/null 2>&1; then
    say "inbox scanner: regression cases pass"
  else
    say "inbox scanner: SELFTEST FAILING — instructions may be going unread"
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

git rev-parse HEAD > "$head_file" 2>/dev/null || true

say "======================================================================"
exit "$attention"
