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
#
# REDUCE_SUM across series matters: every *revision* emits its own series per state, so a service
# that has just rolled over has two of each, and reading one of them understates the instance count
# that is actually being billed.
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
    --data-urlencode "aggregation.crossSeriesReducer=REDUCE_SUM" \
    --data-urlencode "aggregation.groupByFields=metric.label.state" \
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
        bucket = rows.setdefault(p["interval"]["endTime"], {})
        bucket[state] = bucket.get(state, 0.0) + float(
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
# bench/ is kept except bench/queue (half a megabyte of queue benchmarks): run-demos.sh calls
# bench/bode.py inside the image, and this tar is the context, so .dockerignore's re-include of that
# one file could never take effect. Three builds failed on it before the served build log said why.
# The excludes mirror .gcloudignore and .dockerignore. They are spelled out here because tar does
# not understand gitignore syntax, so the three lists have to be kept in step by hand. .cargo is the
# one that is not merely about upload size: .cargo/config.toml pins an absolute target-dir on this
# machine, and a container that inherits it writes the binary outside ./target.
TARBALL=$(mktemp /tmp/lbsim-src-XXXXXX.tgz)
trap 'rm -f "$TARBALL"' EXIT
tar czf "$TARBALL" \
  --exclude=./.cargo \
  --exclude=./.git \
  --exclude=./target \
  --exclude=./out \
  --exclude=./bench/queue \
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
  --cpu 2 --memory 8Gi \
  --set-env-vars "LBSIM_MEMORY_BUDGET_MB=6000" \
  --cpu-throttling \
  --no-cpu-boost \
  --concurrency 80 \
  --session-affinity \
  --min-instances 0 \
  --max-instances 10 \
  --min 0 \
  --max 10 \
  --timeout 300 \
  --allow-unauthenticated \
  --quiet
# Cloud Run has two instance caps and gcloud spells them differently, which is a trap:
#   --max-instances 10   caps the *revision*  (autoscaling.knative.dev/maxScale)
#   --max 10             caps the *service*   (run.googleapis.com/maxScale)
# The service cap defaults to 20 whether you ask for it or not, so setting only --max-instances
# leaves a service that will scale to 20 the moment anyone deploys a revision without the flag. Both
# are set here so the cost control does not depend on remembering the flag next time. Same for
# --min-instances / --min at zero.
#
# What is deliberately absent, or deliberately off:
#   --no-cpu-throttling  : not used; --cpu-throttling stays. A paced run advances only while a request
#                          is in flight, and an open subscription is one, so the dashboard case has CPU;
#                          a run nobody is watching stands still, which is WIRE.md's idle rule.
#   --cpu 2 --memory 8Gi : Issao hit the engine's memory budget at 2 GiB (long-prompt and 10k-replica
#                          runs), so the container is 8 GiB and LBSIM_MEMORY_BUDGET_MB is 6000, ~75% of
#                          it, the engine's own guard so a runaway run aborts with a reason rather than
#                          an OOM kill. 2 vCPU is the most Cloud Run needs for 8 GiB and the engine is
#                          single-threaded. Nothing runs, and nothing bills, while no instance exists.
#   --session-affinity   : SET since the live Ingress (U18). A run lives in one instance's memory,
#                          so a StartRun answered by instance A followed by an OpenSubscription routed
#                          to instance B leaves the browser reconnecting forever; the browser gate saw
#                          exactly that on lbsim.ai (13 of 15 walkthroughs stuck at "stream opening"
#                          while every one passed locally). The affinity cookie keeps a viewer on the
#                          instance that holds its run; it is best effort, which is why a run the
#                          instance does not hold answers 404 and the client reopens from scratch.
#   --cpu-boost          : explicitly *off*. It is on by default on new services, and it is billed.
#                          The container is a static binary that binds its port in milliseconds, so
#                          there is nothing for extra startup CPU to accelerate.
#   --execution-environment : left at gen1, which has the faster cold start. gen2 buys a full Linux
#                          syscall surface that a file server does not use.

URL=$(gcloud run services describe "$SERVICE" --project "$PROJECT" --region "$REGION" \
        --format='value(status.url)' --quiet)
echo
echo "deployed: $URL"
echo "public, per Issao: mock data and published findings, nothing sensitive."
# Probed on "/" rather than "/healthz". Google Front End intercepts /healthz on *.run.app and answers
# its own branded 404, so the request never reaches the container and the check would always fail.
# Measured, not guessed: the /healthz 404 carries no x-cloud-trace-context and no nosniff header,
# while /nope and /health both come back as the server's own plain-text "not found", and the same
# binary answers /healthz with 200 when run locally. The container's health endpoint is therefore
# correct but unreachable from outside under this hostname. See docs/deploy.md, "The /healthz trap".
curl -fsS -o /dev/null -w 'GET /: HTTP %{http_code} in %{time_total}s (cold start included)\n' "$URL/" || \
  echo "the service did not answer; check 'gcloud run services logs read $SERVICE --region $REGION'"
echo
echo "idle instance count is the cost control. To check it returns to zero:"
echo "  ./deploy.sh --check-idle    (or see docs/deploy.md)"
