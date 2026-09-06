#!/usr/bin/env bash
# Every demonstration in one command. Each writes a self-contained HTML report to out/.
set -euo pipefail
cd "$(dirname "$0")"
export PATH="$HOME/local/bin:$HOME/.local/bin:$PATH"
# Through cargo rather than ./target: .cargo/config.toml points every worktree at one shared target
# directory, so a relative path is wrong in all but the original checkout.
S="cargo run --release --quiet --bin sim-run --"

echo "== 1. routing policies at identical load and seed =="
$S compare scenarios/route_round_robin.txt scenarios/route_p2c.txt \
           scenarios/route_least_requests.txt scenarios/route_random.txt \
           --out out/1-routing.html

echo; echo "== 2. telemetry staleness: how herding grows with it =="
$S sweep scenarios/route_least_requests.txt \
         --over telemetry_interval_ms=100,250,500,1000,2000,4000 \
         --out out/2-staleness.html

echo; echo "== 3. chunked prefill: first-token latency against stall length =="
$S sweep scenarios/route_p2c.txt \
         --over step_token_budget=512,1024,2048,4096,8192,16384 \
         --out out/3-chunking.html

echo; echo "== 4. load curve: where the knee is =="
$S sweep scenarios/route_p2c.txt \
         --over arrival_rps=30,70,110,150,190,230 \
         --out out/4-load-curve.html

echo; echo "== 5. long-context share: capacity is a token budget, not a request count =="
$S sweep scenarios/route_p2c.txt \
         --over long_probability=0.0,0.04,0.08,0.16,0.32 \
         --out out/5-long-context.html

echo; echo "== 6. retry storm: whether the fleet comes back after the spike =="
$S compare scenarios/retry_none.txt scenarios/retry_budget.txt scenarios/retry_storm.txt \
           --out out/6-retry.html

echo; echo "reports in out/"
echo "for machine-readable telemetry, add: --telemetry out/NAME.tele [--telemetry-budget-mb N]"
