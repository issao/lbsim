#!/usr/bin/env bash
# One command, idempotent: build the container and put it on Cloud Run.
#
#   ./deploy.sh
#
# Every flag below is load-bearing; docs/deploy.md explains each one and what is deliberately
# absent. Re-running with an unchanged HEAD rebuilds the same tag and deploys a fresh revision,
# which is harmless.
set -euo pipefail
cd "$(dirname "$0")"
export PATH="$HOME/local/bin:$HOME/.local/bin:$PATH"

PROJECT=${PROJECT:-lbsim-gcp}
REGION=${REGION:-us-central1}
SERVICE=${SERVICE:-lbsim}
REPO=${REPO:-lbsim}
BUCKET=${BUCKET:-gs://${PROJECT}-runs}

# --check-idle: read the instance_count metric back out of Cloud Monitoring, so the cost control is
# measured rather than assumed. A single instance held warm is ~$274/month; nothing else about this
# deploy costs real money, so this is the only number worth checking.
#
# Done with curl against the Monitoring API because gcloud has no `monitoring time-series` command.
# roles/monitoring.viewer is enough. The metric carries a `state` label of "active" or "idle";
# both must read zero once traffic stops. Note the sampling lag: Cloud Run reports instance_count
# on a ~60s cadence and Monitoring ingests it with a delay of a minute or two, so a check run
# immediately after a request will still show the instance that served it. That is correct, not a
# failure. What matters is the tail of the series.
if [ "${1:-}" = "--check-idle" ]; then
  END=$(date -u +%Y-%m-%dT%H:%M:%SZ)
  START=$(date -u -d '40 minutes ago' +%Y-%m-%dT%H:%M:%SZ)
  TS=$(mktemp /tmp/lbsim-ts-XXXXXX.json)
  trap 'rm -f "$TS"' EXIT
  curl -sS -G "https://monitoring.googleapis.com/v3/projects/$PROJECT/timeSeries" \
    -H "Authorization: Bearer $(gcloud auth print-access-token --quiet)" \
    --data-urlencode "filter=metric.type=\"run.googleapis.com/container/instance_count\" AND resource.labels.service_name=\"$SERVICE\"" \
    --data-urlencode "interval.start_time=$START" \
    --data-urlencode "interval.end_time=$END" \
    --data-urlencode "aggregation.alignmentPeriod=60s" \
    --data-urlencode "aggregation.perSeriesAligner=ALIGN_MAX" \
    -o "$TS"
  python3 - "$TS" <<'PY'
import json, sys

with open(sys.argv[1]) as f:
    d = json.load(f)
if "error" in d:
    print("monitoring API error:", d["error"].get("message"))
    sys.exit(1)
series = d.get("timeSeries", [])
if not series:
    print("no instance_count samples in the window.")
    print("Cloud Run stops emitting the metric while a service has no instances, so an empty")
    print("series after traffic stopped is the scale-to-zero result, not a missing metric.")
    print("If it is empty *right after* a request, wait two minutes for ingestion and re-run.")
    sys.exit(0)

rows = {}
for s in series:
    state = s["metric"]["labels"].get("state", "unknown")
    for p in s["points"]:
        v = p["value"]
        rows.setdefault(p["interval"]["endTime"], {})[state] = float(
            v.get("doubleValue", v.get("int64Value", 0))
        )

print("time (UTC)              active    idle")
for t in sorted(rows)[-30:]:
    r = rows[t]
    print("%-22s %7.2f %7.2f" % (t, r.get("active", 0.0), r.get("idle", 0.0)))

last = rows[sorted(rows)[-1]]
total = last.get("active", 0.0) + last.get("idle", 0.0)
print()
verdict = "SCALED TO ZERO." if total == 0 else "still warm; re-check in a few minutes."
print("most recent sample: %.2f instances. %s" % (total, verdict))
PY
  exit 0
fi

REV=$(git rev-parse --short HEAD)
TAG="$REGION-docker.pkg.dev/$PROJECT/$REPO/$SERVICE:$REV"
SRC="$BUCKET/cloudbuild-source/$SERVICE-$REV.tgz"

if [ -n "$(git status --porcelain)" ]; then
  echo "warning: working tree is dirty. The image is built from the tree, but is tagged :$REV," >&2
  echo "         so the tag will not identify what is inside it. Commit first if that matters." >&2
fi

# Upload the build context ourselves. `gcloud builds submit` would stage it for us, but staging
# calls storage.buckets.get, which this account deliberately does not have; writing a plain object
# does not. See the header of cloudbuild.yaml.
#
# The excludes mirror .gcloudignore and .dockerignore. They are spelled out here because tar does
# not understand gitignore syntax, so the three lists have to be kept in step by hand.
TARBALL=$(mktemp /tmp/lbsim-src-XXXXXX.tgz)
trap 'rm -f "$TARBALL"' EXIT
tar czf "$TARBALL" \
  --exclude=./.git \
  --exclude=./target \
  --exclude=./out \
  --exclude=./bench \
  --exclude=./tests \
  --exclude=./web/node_modules \
  --exclude=./web/dist \
  -C . .
echo "source: $(du -h "$TARBALL" | cut -f1) -> $SRC"
gcloud storage cp "$TARBALL" "$SRC" --project "$PROJECT" --quiet

echo "building $TAG"
if ! gcloud builds submit --no-source \
      --config cloudbuild.yaml \
      --substitutions="_TAG=$TAG,_SRC=$SRC" \
      --project "$PROJECT" --quiet; then
  echo >&2
  echo "build failed. Logs go to Cloud Logging, not Cloud Storage, so they are not printed above." >&2
  echo "The last build's log:" >&2
  echo "  gcloud builds list --project $PROJECT --limit 1 --format='value(id)' --quiet" >&2
  echo "  gcloud builds log <ID> --project $PROJECT --quiet" >&2
  exit 1
fi

echo "deploying $SERVICE"
gcloud run deploy "$SERVICE" \
  --image "$TAG" \
  --project "$PROJECT" --region "$REGION" \
  --cpu 1 --memory 512Mi \
  --cpu-throttling \
  --concurrency 80 \
  --min-instances 0 \
  --max-instances 10 \
  --timeout 300 \
  --allow-unauthenticated \
  --quiet
# Notes on what is deliberately absent:
#   --no-cpu-throttling  : not set, and --cpu-throttling is set explicitly instead. This is a static
#                          server, so CPU only during a request is correct and cheaper. It becomes
#                          necessary when a simulation advances between requests, which is the real
#                          Ingress service, not this one.
#   --session-affinity   : not set, for the same reason. Nothing here holds per-user state.
#   --cpu-boost          : not set. Cold start is a static binary; there is nothing to warm.
# min-instances 0 and max-instances 10 are the cost controls, per the standing authorization.

URL=$(gcloud run services describe "$SERVICE" --project "$PROJECT" --region "$REGION" \
        --format='value(status.url)' --quiet)
echo
echo "deployed: $URL"
echo "public, per Issao: mock data and published findings, nothing sensitive."
curl -fsS -o /dev/null -w 'healthz: HTTP %{http_code} in %{time_total}s\n' "$URL/healthz" || \
  echo "healthz did not answer; check 'gcloud run services logs read $SERVICE --region $REGION'"
echo
echo "idle instance count is the cost control. To check it returns to zero:"
echo "  ./deploy.sh --check-idle    (or see docs/deploy.md)"
