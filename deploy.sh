#!/usr/bin/env bash
# Build and deploy to Cloud Run. Every flag here is load-bearing; see docs/execution-plan.md 3.4.
set -euo pipefail
cd "$(dirname "$0")"
export PATH="$HOME/local/bin:$HOME/.local/bin:$PATH"

PROJECT=${PROJECT:-lbsim-gcp}
REGION=${REGION:-us-central1}
SERVICE=${SERVICE:-lbsim}
TAG="$REGION-docker.pkg.dev/$PROJECT/lbsim/$SERVICE:$(git rev-parse --short HEAD)"

echo "building $TAG"
gcloud builds submit --tag "$TAG" --project "$PROJECT" --quiet

echo "deploying $SERVICE"
gcloud run deploy "$SERVICE" \
  --image "$TAG" \
  --project "$PROJECT" --region "$REGION" \
  --cpu 1 --memory 512Mi \
  --concurrency 80 \
  --min-instances 0 \
  --max-instances 10 \
  --timeout 300 \
  --no-allow-unauthenticated \
  --quiet
# Notes on what is deliberately absent:
#   --no-cpu-throttling  : not set. This is a static server, so CPU only during a request is correct
#                          and cheaper. It becomes necessary when a simulation advances between
#                          requests, which is the real Ingress service, not this.
#   --session-affinity   : not set, for the same reason. Nothing here holds per-user state.
# min-instances 0 and max-instances 10 are the cost controls, per the standing authorization.

URL=$(gcloud run services describe "$SERVICE" --project "$PROJECT" --region "$REGION" \
        --format='value(status.url)' --quiet)
echo
echo "deployed: $URL"
echo "it is private. To view it locally, authenticated:"
echo "  gcloud run services proxy $SERVICE --project $PROJECT --region $REGION --port 8080"
echo "then open http://localhost:8080"
