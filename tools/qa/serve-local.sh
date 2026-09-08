#!/usr/bin/env bash
# Build everything the dashboard needs, serve it locally, run the browser gate against it, tear down.
#
# Usage: tools/qa/serve-local.sh
#   QA_PORT=8181         port the local sim-run serve listens on
#   QA_SKIP_REPORTS=1    skip run-demos.sh (one to two minutes); home's report links then 404
#   QA_BASE              honoured by qa.js only; this script always points it at localhost:$QA_PORT
#   QA_SCRIPT=screens.js run the screenshot walkthrough (tools/qa/screens.js) instead of the gate
# Exit status is qa.js's.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
export PATH="$HOME/local/bin:$HOME/.local/bin:$PATH"
PORT=${QA_PORT:-8181}
LOG=/tmp/lbsim-qa-$PORT.log

tools/build.sh build --release --bin sim-run

# Worktrees carry no node_modules; borrow the primary checkout's rather than install a second copy.
if [ ! -e web/node_modules ]; then
  ln -sfn /home/agents/repo/lbsim/web/node_modules web/node_modules
fi
(cd web && npm run build)
./target/release/sim-run export --demos --dir web/dist

if [ "${QA_SKIP_REPORTS:-0}" != 1 ]; then
  ./run-demos.sh
  mkdir -p web/dist/reports
  cp out/*.html web/dist/reports/
fi

PORT=$PORT ./target/release/sim-run serve --dir web/dist >"$LOG" 2>&1 &
SERVER=$!
trap 'kill $SERVER 2>/dev/null || true' EXIT

for _ in $(seq 1 30); do
  if curl -s -X POST "localhost:$PORT/v1/ingress/ListRuns" -d '{}' >/dev/null; then break; fi
  sleep 1
done
if ! curl -s -X POST "localhost:$PORT/v1/ingress/ListRuns" -d '{}' >/dev/null; then
  echo "serve-local: server on :$PORT never answered; see $LOG" >&2
  exit 1
fi

QA_BASE="http://localhost:$PORT" node "tools/qa/${QA_SCRIPT:-qa.js}"
