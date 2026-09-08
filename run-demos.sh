#!/usr/bin/env bash
# Every demonstration in one command. Each writes a self-contained HTML report to out/.
set -euo pipefail
cd "$(dirname "$0")"
export PATH="$HOME/local/bin:$HOME/.local/bin:$PATH"
# Through cargo rather than ./target: .cargo/config.toml points every worktree at one shared target
# directory, so a relative path is wrong in all but the original checkout.
S="tools/build.sh run --release --quiet --bin sim-run --"

echo "== 1. routing policies at identical load and seed =="
$S compare scenarios/route_round_robin.txt scenarios/route_p2c.txt \
           scenarios/route_least_requests.txt scenarios/route_random.txt \
           --out out/1-routing.html

echo; echo "== 2. telemetry staleness: how herding grows with it =="
$S sweep scenarios/staleness_least_requests.txt \
         --over telemetry_interval_ms=100,250,500,1000,2000,4000 \
         --out out/2-staleness.html

echo; echo "== 3. chunked prefill: first-token latency against stall length =="
$S sweep scenarios/route_p2c.txt \
         --over step_token_budget=512,1024,2048,4096,8192,16384 \
         --out out/3-chunking.html

echo; echo "== 4. load curve: where the knee is =="
$S sweep scenarios/route_p2c.txt \
         --over arrival_rps=240,560,880,1200,1520,1840 \
         --out out/4-load-curve.html

echo; echo "== 5. long-context share: capacity is a token budget, not a request count =="
$S sweep scenarios/route_p2c.txt \
         --over long_probability=0.0,0.04,0.08,0.16,0.32 \
         --out out/5-long-context.html

echo; echo "== 6. retry storm: whether the fleet comes back after the spike =="
$S compare scenarios/retry_none.txt scenarios/retry_budget.txt scenarios/retry_storm.txt \
           --out out/6-retry.html

echo; echo "== 7. decode disabled (HBM to infinity): does the rolling hotspot need LLM physics? =="
$S compare scenarios/route_round_robin_no_decode.txt scenarios/route_p2c_no_decode.txt \
           --out out/7-no-decode.html

echo; echo "== 8. admission under overload: shed early or time out late =="
$S compare scenarios/admit_accept_all.txt scenarios/admit_deadline_aware.txt \
           --out out/8-admission.html

echo; echo "== 9. tenants: weighted fair share against an over-share tenant =="
$S compare scenarios/admit_tenants_accept_all.txt scenarios/admit_fair_share.txt \
           --out out/9-fair-share.html

echo; echo "== 10. live probes: least-KV on fresh state against p2c on the snapshot =="
$S compare scenarios/route_p2c.txt scenarios/route_least_kv_probe.txt \
           --out out/10-probes.html

echo; echo "== 11. the KV spiral: parked session context fills the cache, and swapping it out cures it =="
$S compare scenarios/kv_spiral_never.txt scenarios/kv_spiral_swap.txt \
           --out out/11-preemption.html

echo; echo "== 12. speculative decoding: a small-batch win at N=4 that shrinks and reverses as batch grows =="
$S compare scenarios/spec_off.txt scenarios/spec_n4.txt \
           --out out/12-spec-decode.html

echo; echo "== 13. herding gets worse as the fleet grows: least_requests over 32-512 replicas at one offered/capacity ratio =="
$S sweep scenarios/herd_fleet.txt --over replicas=32,64,128,256,512 \
           --out out/13-herd-fleet.html

echo; echo "== 14. prefix affinity against load spreading: p2c, then affinity at max load ratio 1.05 and 2.2 =="
$S compare scenarios/affinity_off.txt scenarios/affinity_spread.txt scenarios/affinity_sticky.txt \
           --out out/14-affinity.html

echo; echo "== 15. gray failure: a replica at 0.3x that announces healthy, with and without outlier ejection on the delayed view =="
$S compare scenarios/gray_failure_none.txt scenarios/gray_failure_eject.txt \
           --out out/15-gray-failure.html

echo; echo "== 16. the replica's own scheduler: fifo_chunked, class_priority and deadline_first over three SLO classes at 2x load =="
$S compare scenarios/sched_fifo.txt scenarios/sched_class.txt scenarios/sched_deadline.txt \
           --out out/16-scheduling.html

echo; echo "== 17. affinity hotspot failover cascade: the hottest holder crashes at 60 s and returns cold at 90 s, p2c against affinity =="
$S compare scenarios/cascade_p2c.txt scenarios/cascade_affinity.txt \
           --out out/17-cascade.html

echo; echo "== 18. staleness loop Bode plot: a 30% sine on offered load swept from 0.01 to 1 Hz, least_requests on a 1 s / 200 ms delayed snapshot =="
$S sweep scenarios/bode.txt --over perturb_frequency_hz=0.01,0.02,0.05,0.1,0.2,0.5,1.0 \
           --out out/18-bode.html --telemetry out/18-bode --telemetry-budget-mb 64
python3 bench/bode.py

echo; echo "== 19. KV memory tiering: a cluster DRAM pool, an SSD pool under it, and a 10 GB/s fabric they share =="
$S compare scenarios/tier_dram.txt scenarios/tier_dram_ssd.txt scenarios/tier_contended.txt \
           --out out/19-tiering.html

echo; echo "reports in out/"
echo "for machine-readable telemetry, add: --telemetry out/NAME.tele [--telemetry-budget-mb N]"
