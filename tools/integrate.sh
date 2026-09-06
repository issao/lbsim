#!/usr/bin/env bash
# The merge queue, as a script any agent can run.
#
#   tools/integrate.sh <branch>                       rebase, verify, merge to master, push, clean up
#   tools/integrate.sh <branch> --dry-run             everything except merge, push and deletion
#   tools/integrate.sh <branch> --remove-worktree P   on success also remove the agent's worktree P
#
# Why this exists. Measured on 2026-09-06: the tech lead spent 28 of 32 minutes as a serial merge
# queue, rebasing, building, testing and fingerprinting one branch after another, while its fleet of
# subagents sat finished and nothing new was spawned. Every one of those steps is mechanical. The
# judgment in this project is applied twice, in the brief that scopes a unit and in the review of the
# merged diff afterwards, and neither needs the tech lead to be the one typing `git merge`. So the
# agent that finished the unit runs this itself; the lock below makes that safe with ten of them.
#
# What it checks, in order, and the exit code when it refuses:
#   2  the branch does not rebase cleanly onto origin/master (conflicting files are printed; the
#      branch is left exactly as it was, and its author resolves the rebase)
#   3  `tools/build.sh test --workspace` fails (this includes the layering test)
#   4  `./check-fingerprints.sh` reports that a number moved. A deliberate behaviour change updates
#      bench/golden-fingerprints.txt on the branch, in which case this passes by construction; an
#      accidental one is caught here, which is the whole point of that file
#   5  the branch touched web/ and `npm run build` fails
#   6  the master checkout at $MAIN is dirty, or is not a fast-forward of origin/master
#   7  the push did not land, or master on origin does not contain the branch afterwards
#   1  anything else: bad arguments, missing branch, worktree trouble
#
# Where it runs. All verification happens in one persistent worktree, $QUEUE, which no agent edits by
# hand. Persistent, not per-branch, so its target directory stays warm across integrations; one, not
# many, because the flock serialises integrations anyway. The merge itself is made in $MAIN, the only
# checkout that has master, after fast-forwarding it to origin/master; it is refused if that checkout
# is dirty, because a stray file there would be swept into the merge commit.
#
# Whole-branch, not whole-tree: the fingerprint and test runs cover the rebased branch, so a branch
# that passes here passes on master as merged, since master is an ancestor.
set -uo pipefail
export PATH="$HOME/local/bin:$HOME/.local/bin:$PATH"

MAIN=${LBSIM_MAIN:-/home/agents/repo/lbsim}
QUEUE=${LBSIM_QUEUE:-/home/agents/repo/lbsim-integ-queue}
LOCK=${LBSIM_INTEGRATE_LOCK:-/home/agents/repo/.integrate.lock}

branch=""; dry=0; remove_wt=""
while [ $# -gt 0 ]; do
  case "$1" in
    --dry-run) dry=1 ;;
    --remove-worktree) shift; remove_wt=${1:-} ;;
    -h|--help) sed -n '2,40p' "$0"; exit 0 ;;
    -*) echo "unknown option $1" >&2; exit 1 ;;
    *) branch=$1 ;;
  esac
  shift
done
[ -n "$branch" ] || { echo "usage: tools/integrate.sh <branch> [--dry-run] [--remove-worktree PATH]" >&2; exit 1; }
branch=${branch#origin/}
slug=$(printf '%s' "$branch" | tr -c 'A-Za-z0-9' '-')
work="integ/$slug"

say() { printf '%s integrate %s: %s\n' "$(date '+%H:%M:%S')" "$branch" "$*"; }
die() { local code=$1; shift; say "REFUSED ($code): $*"; exit "$code"; }

# One integration at a time, machine-wide. flock waits in the kernel; nobody polls.
exec {lockfd}>"$LOCK"
say "waiting for the integration lock"
flock "$lockfd"
say "lock held"

git -C "$MAIN" fetch -q origin || die 1 "fetch failed"
git -C "$MAIN" rev-parse -q --verify "origin/$branch" >/dev/null || die 1 "origin/$branch does not exist"

# The queue worktree: create once, then reset to the branch under test every time.
if [ ! -d "$QUEUE/.git" ] && [ ! -f "$QUEUE/.git" ]; then
  git -C "$MAIN" worktree add -q --detach "$QUEUE" origin/master || die 1 "cannot create $QUEUE"
fi
git -C "$QUEUE" checkout -q --detach origin/master 2>/dev/null
git -C "$QUEUE" branch -q -D "$work" 2>/dev/null
git -C "$QUEUE" checkout -q -B "$work" "origin/$branch" || die 1 "cannot check out origin/$branch"

# Rebase, so history stays linear and the merge below is a clean --no-ff group.
if ! git -C "$QUEUE" rebase -q origin/master >/dev/null 2>&1; then
  say "rebase conflict in:"
  git -C "$QUEUE" diff --name-only --diff-filter=U | sed 's/^/    /'
  git -C "$QUEUE" rebase --abort
  git -C "$QUEUE" checkout -q --detach origin/master
  die 2 "does not rebase cleanly onto origin/master; resolve on the branch and rerun"
fi
n=$(git -C "$QUEUE" rev-list --count origin/master..HEAD)
if [ "$n" = 0 ]; then
  say "nothing to integrate: every commit on $branch is already on master"
  git -C "$QUEUE" checkout -q --detach origin/master
  if [ "$dry" = 0 ]; then
    git -C "$MAIN" push -q origin --delete "$branch" 2>/dev/null && say "deleted origin/$branch"
    git -C "$MAIN" branch -q -D "$branch" 2>/dev/null
  fi
  exit 0
fi
say "rebased: $n commit(s) on top of origin/master $(git -C "$MAIN" rev-parse --short origin/master)"

# Verification. All of it is mechanical, and all of it runs on the rebased branch.
( cd "$QUEUE" && tools/build.sh test --workspace -q >"$QUEUE/.integrate-test.log" 2>&1 ) \
  || { tail -40 "$QUEUE/.integrate-test.log"; die 3 "workspace tests fail"; }
say "tests pass"
( cd "$QUEUE" && ./check-fingerprints.sh >"$QUEUE/.integrate-fp.log" 2>&1 ) \
  || { cat "$QUEUE/.integrate-fp.log"; die 4 "fingerprints moved without a baseline update"; }
say "fingerprints match"
if [ -n "$(git -C "$QUEUE" diff --name-only origin/master...HEAD -- web/)" ]; then
  ( cd "$QUEUE/web" && npm ci --prefer-offline --no-audit --no-fund >/dev/null 2>&1 && npm run build >"$QUEUE/.integrate-web.log" 2>&1 ) \
    || { tail -40 "$QUEUE/.integrate-web.log"; die 5 "web build fails"; }
  say "web builds"
fi

if [ "$dry" = 1 ]; then
  say "DRY RUN OK: $branch would merge as $n commit(s); nothing pushed"
  git -C "$QUEUE" checkout -q --detach origin/master
  exit 0
fi

# The merge, in the one checkout that has master.
[ -z "$(git -C "$MAIN" status --porcelain)" ] || die 6 "$MAIN is dirty; someone is editing the shared checkout"
git -C "$MAIN" merge -q --ff-only origin/master || die 6 "$MAIN master is not a fast-forward of origin/master"
subject=$(git -C "$QUEUE" log -1 --format=%s)
# The rebased commits now live on the local branch $work, visible from $MAIN since worktrees share refs.
git -C "$MAIN" merge --no-ff -q --no-edit -m "Merge $branch: $subject" "$work" || {
  git -C "$MAIN" merge --abort 2>/dev/null; die 6 "merge into master failed after a clean rebase"; }
git -C "$MAIN" push -q origin master || { git -C "$MAIN" reset -q --hard origin/master; die 7 "push of master rejected; master reset, rerun"; }
git -C "$MAIN" fetch -q origin
git -C "$MAIN" merge-base --is-ancestor "$work" origin/master || die 7 "origin/master does not contain $branch after push"
sha=$(git -C "$MAIN" rev-parse --short origin/master)

# Clean up: the branch has served its purpose, and a stale one misleads the next reader.
git -C "$MAIN" push -q origin --delete "$branch" 2>/dev/null || true
git -C "$QUEUE" checkout -q --detach origin/master
git -C "$MAIN" branch -q -D "$work" 2>/dev/null
git -C "$MAIN" branch -q -D "$branch" 2>/dev/null
if [ -n "$remove_wt" ]; then
  git -C "$MAIN" worktree remove --force "$remove_wt" 2>/dev/null && say "removed worktree $remove_wt" \
    || say "could not remove worktree $remove_wt (not a worktree, or not ours)"
fi
echo "INTEGRATED $sha $branch"
