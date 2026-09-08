# Findings

Seventeen results from the simulator as it stands, from twenty demos. Every number here is
reproducible with `./run-demos.sh`, which writes a self-contained HTML report per experiment into
`out/`.

Reference fleet throughout: 256 replicas of a 70-billion-parameter model on 8x H100, cost model
calibrated in `bench/validate_epochs.py` against a published batch-1 measurement and reproducing its
step-time table to within 0.05 ms. Rated capacity 1,878 requests/s. Unless stated, offered load is
**560 requests/s, which is 30% of capacity**: none of what follows is an overload artefact.

What is *not* modelled yet, and therefore not claimed: prefill/decode disaggregation and
multi-cluster. `docs/scope-today.md` says why each was cut. Preemption (result 8), prefix caching
(results 11 and 14), memory tiering (16) and single-cluster autoscaling (17) were cut on day one and
have since landed.

---

## 1. Reading the whole fleet is worse than sampling two of it

| Policy | Goodput tok/s | First-token p99 | Attainment | Load spread |
|---|---|---|---|---|
| power of two choices | **143,882** | 3,355 ms | 97.2% | 0.33 |
| round robin | 138,849 | 6,711 ms | 94.5% | 0.45 |
| random | 137,430 | 6,778 ms | 93.5% | 0.54 |
| least requests | **2,800** | 54,224 ms | 4.5% | 2.78 |

**Least-requests is 51x worse than sampling two replicas at random, and worse than round robin,
which ignores load entirely.** It inspects all 256 replicas and picks the least loaded, which is the
obvious thing to do and is the trap: its snapshot is up to 1.2 seconds old, so every routing decision
in that window sees the same apparently-idle replica and sends to it. Sampling bounds the stampede by
construction, because only a fraction of decisions consider any one replica at a time. **At this fleet
size the policy is not merely worse, it is catastrophic**: attainment falls from 39.4% (at 32
replicas) to 4.5%, load spread reaches a CV of 2.78, first-token p99 reaches 54 seconds, and 13,817
requests now time out mid-run — a bigger fleet gives the same stale snapshot more replicas to herd
onto at once, so the shape of the failure is unchanged but its severity is not.

**Throughput among the other three barely moves**, 146,921 to 148,032. The work gets done either way;
under the worse policy it arrives too late to count. Least-requests' own throughput now collapses too,
to 29,005, because the requests that time out mid-run have already consumed device time without
completing. That is why ranking policies on throughput selects the wrong one, and why goodput leads
every table in this project.

Second-order but worth noting: **round robin beats random.** Round robin is perfectly even in request
*count*, which is not the right unit, but it is still less lumpy than independent random choice.

## 2. Herding is a smooth function of staleness, and it is steep

Telemetry publication interval swept with least-requests routing, everything else fixed.

| Interval | Goodput tok/s | Attainment | Load spread |
|---|---|---|---|
| 100 ms | 14,912 | 79.3% | 0.67 |
| 250 ms | 14,941 | 79.5% | 0.71 |
| 500 ms | 11,771 | 64.8% | 0.95 |
| 1 s | 6,943 | 39.4% | 1.27 |
| 2 s | 3,494 | 22.6% | 1.54 |
| 4 s | 1,264 | 10.6% | 1.88 |

**A twelvefold goodput collapse across a range of scrape intervals that all look reasonable.** Nothing
about the fleet changed; only how old the information was when the decision was made.

Note the shape. Between 100 and 250 ms almost nothing happens, then it falls off a cliff. Staleness
has a threshold rather than a gradient, and the threshold is set by how fast queues change, which is
the loop's own time constant. That is the control-theoretic reading, and it is why the report computes
a dominant oscillation frequency rather than only an average.

The table above is written against the 32-replica numbers, ahead of a queued follow-up that moves this
scenario's default back to 32 replicas to match; until that lands, running `route_least_requests.txt`
as it stands defaults to 256, and doing so shows why the follow-up matters: **at 256 replicas even
100 ms of staleness is past the cliff.** The same sweep at 256 replicas collapses at every interval
instead of only the slow ones — attainment falls from 0.79 to 0.15 at the 100 ms point alone, and from
0.10 to 0.009 at the 4 s end of the sweep, load imbalance CV rises as high as 5.46, and by 2 s the
queue cap starts rejecting requests outright, 42,000 of them by 4 s, where none were rejected before.
A larger fleet at the same offered/capacity ratio drains and refills its queues faster, so the same
loop time constant that put the cliff at 1 s here puts it inside the first bucket at 256 replicas: the
cliff moves left as the fleet grows, which is the mechanism above, not a new one. Demo 13, result 10,
turns that relationship into a curve rather than two points.

## 3. Prefill and decode contend for one device, and no setting wins both

Chunked prefill token budget swept, with power-of-two-choices routing.

| Chunk budget | Worst gap between tokens p99 | First-token p99 | Goodput | Attainment |
|---|---|---|---|---|
| 512 | **32 ms** | 3,993 ms | 142,202 | 96.4% |
| 1,024 | 50 ms | 3,355 ms | **143,882** | 97.2% |
| 2,048 | 86 ms | 3,020 ms | 18,497 | 32.9% |
| 4,096 | 157 ms | 2,852 ms | 19,155 | 33.7% |
| 8,192 | 302 ms | 2,919 ms | 19,252 | 33.8% |
| 16,384 | **587 ms** | **2,852 ms** | 18,973 | 33.5% |

The two latencies move in opposite directions, monotonically, across an eighteenfold range of worst
gap. A bigger chunk gets the first token out sooner and inserts a longer stall into every stream
already decoding, because both compete for the same device in the same step.

**The goodput cliff between 1,024 and 2,048 is not a modelling artefact, it is the SLO.** The
inter-token target here is 80 ms; at a 2,048-token chunk the worst gap is 86 ms, so almost every
request breaches and earns no goodput while still consuming capacity. Throughput is flat, 147,941 to
148,252, across the whole sweep. **The fleet does identical work and delivers a tenth of the value**,
which is the sharpest illustration in this set of why the two measures must not be confused.

## 4. Past the knee, offering more load delivers less work

Offered rate swept with power-of-two-choices.

| Offered | Goodput | Throughput | First-token p99 | Attainment |
|---|---|---|---|---|
| 240 rps | 63,967 | 65,407 | 2,819 ms | 98.2% |
| 560 rps | 143,882 | 148,032 | 3,355 ms | 97.2% |
| 880 rps | 212,782 | 221,681 | 3,792 ms | 96.1% |
| 1,200 rps | **260,424** | 278,417 | 4,966 ms | 93.4% |
| 1,520 rps | 253,970 | **294,403** | 7,852 ms | 85.0% |
| 1,840 rps | 151,260 | 246,993 | 35,970 ms | 55.9% |

Two knees, in different places, and that is the point. **Goodput peaks at 1,200 requests/s**, which is
64% of the rated 1,878. **Throughput peaks later, at 1,520.** Between those two points the fleet is
doing more work and delivering less value.

Past 1,520 both collapse: at 1,840 requests/s throughput itself falls 16% below its peak while
first-token latency goes to 36 seconds. Offering 53% more load than the goodput optimum yields **42%
less goodput**. Admission control is not a refinement here, it is the difference between the peak and
the right-hand column.

## 5. Capacity is a token budget, and a load-balancer metric can improve while service collapses

Share of long-context requests swept, everything else fixed. The long mode averages 24,000 prompt
tokens against 1,200 for the short mode.

| Long share | Goodput | First-token p99 | Attainment | Load spread |
|---|---|---|---|---|
| 0% | 149,373 | **411 ms** | 100.0% | 0.39 |
| 4% | 147,009 | 2,215 ms | 98.8% | 0.36 |
| 8% | 143,882 | 3,355 ms | 97.2% | 0.33 |
| 16% | 134,001 | 4,631 ms | 93.7% | 0.29 |
| 32% | **72,865** | **12,616 ms** | 67.3% | **0.24** |

**Load spread falls from 0.39 to 0.24 while goodput halves and tail latency gets 31 times worse.** The
balancer looks like it is doing a better job precisely as service falls apart, because a few enormous
requests make every replica uniformly saturated. A dashboard watching imbalance would report
improvement throughout.

The mechanism is the token budget. One 24,000-token context consumes what eight chat turns consume, so
raising the long share consumes admission capacity that a request count would have said was free. At
32% long, first-token latency at the 99th percentile is 12.6 seconds even though the fleet is at 30%
of its rated request rate.

This is the clearest argument in this set for the design decision that capacity, rate limits and
routing signals must all be denominated in tokens. A system that counts requests cannot see this
happening.

## 6. A retry budget is the difference between a bad minute and an outage

A load spike to three times normal for forty seconds, then a return to normal. All three runs are
identical except for retry policy. Queue depth is compared before the spike against the final quarter
of the run, which is the only window that answers the question.

| Retry policy | Retries | Goodput | Attainment | Queue before → after | Recovered |
|---|---|---|---|---|---|
| none | 0 | 165,492 | 62.8% | 13 → 12 | yes |
| 3 attempts, 10% budget | 30,734 | 145,271 | 53.1% | 15 → 16 | yes |
| 3 attempts, no budget | **273,801** | **62,019** | 15.6% | **15 → 48** | **no** |

**The unbudgeted run never comes back.** Offered load returned to normal after forty seconds and its
queue is still three times pre-spike depth at the end of a four-minute run. That is a metastable
collapse: the system is in a state it sustains on its own, and the original cause is gone.

The mechanism is visible in the retry count. Unbudgeted retries reach 273,801 against 30,734 with a
10% cap, a ninefold difference, because each timeout adds load precisely when the fleet is least able
to absorb it. Retries here are far more expensive than in a stateless service: a request that times
out after twenty seconds has already consumed twenty seconds of device time producing tokens nobody
will read.

**A 10% budget recovers 88% of the no-retry goodput and turns the collapse into a recovery.** That is
the whole intervention. Not smarter routing, not more capacity, just a cap on how much of the offered
load may be retries.

Note also that **load imbalance is 0.22 to 0.25 in all three runs**, essentially identical. The
collapse is invisible in the balancer's own metric, which is the same lesson as result 5 arriving by a
different route.

---

## 7. The routing ordering does not need the decode physics

The same two policies as result 1, round robin and power of two choices, with the scenario key
`disable_decode = true`, which Issao described as *"basically by setting HBM to infinity"*: the
bandwidth term of the step cost is zero, so a decode step costs its fixed overhead and nothing per
resident token. KV accounting is unchanged, so capacity still binds in tokens. Rated capacity rises
from 235 to 269 requests/s; offered load stays 70. Demo 7, `out/7-no-decode.html`.

Demo 1 is now the 256-replica default fleet; this pair is kept at its original 32 replicas, on
purpose, because the comparison below is against demo 1's small-fleet numbers, not its current ones —
otherwise switching off decode and scaling the fleet by 8x would be two changes at once instead of
one.

| Policy | Goodput tok/s | Throughput | First-token p99 | Inter-token p99 | Attainment | Load spread |
|---|---|---|---|---|---|---|
| power of two choices | **18,360** | 18,984 | **3,825 ms** | 46 ms | 97.0% | 0.34 |
| round robin | 17,784 | 18,986 | 7,181 ms | 46 ms | 93.7% | 0.40 |

**The ordering holds, and by the same margin.** With decode: p2c 97.0% attainment against round
robin's 93.1%, first-token p99 3.9 s against 8.3 s. Without: 97.0% against 93.7%, 3.8 s against
7.2 s. Throughput is identical to within two tokens per second, as in result 1, and inter-token
latency is the same 46 ms for both, because with the bandwidth term gone a decode step costs the same
whatever is resident.

That is what `VISION.md` §3a predicted: the rolling hotspot is a property of heterogeneous request
sizes meeting a router that ignores them, not of LLM physics. Round robin is even in request *count*,
and a queue of a few long prompts behind one short one is what creates the first-token tail. Removing
the decode cost leaves that mechanism untouched, which is why the first-token gap barely moves while
everything downstream of it gets cheaper.

The knob's value is as a control: any later dynamic that *disappears* under `disable_decode` is a
decode-physics effect, and any that survives is a queueing effect. Results 1 and 2 are the second kind.

---

## 8. The KV spiral: parked session context fills the cache, and swapping it out cures it

Demo 11, `out/11-preemption.html`, `scenarios/kv_spiral_never.txt` against `kv_spiral_swap.txt`
(U22, d48191f, merged dcf77c8). Capacity is a token budget, and until this unit nothing ever gave
tokens back except a finished request. Real engines evict: `recompute` drops a context and prefills
it again, `swap` copies it over the host link and back; the scenario chooses one, with the victim
rule separate (newest, largest context, most slack), and every cost goes through `CostModel`. `never`
is the default and is proven invisible: every earlier golden row is unchanged, and `route_p2c` under
`recompute` has the same fingerprint because nothing there is ever under pressure.

The pressure comes from sessions. A finished turn parks its context on the replica and the next
turn, after the think time, goes straight back to it carrying the whole context. In the tech lead's
words, forwarded verbatim from the unit's agent:

> At two sessions a second on four replicas, a load the fleet is rated to serve twenty times over,
> parked context fills a 30k-token cache inside a minute; without eviction admission is blocked by
> memory nobody is computing on and the replica serves one sequence at a time (late attainment 2%,
> p99 TTFT 36 s). Swapping the same load to DRAM at 50 GB/s serves it at 95% with an 84 ms p99 TTFT
> and 2.2 preemptions a second: demo 11. A lone running sequence is never evicted, because admission
> let it in over the cap and evicting it would only re-admit it next step. Parked context goes before
> a running sequence, since dropping idle context stalls nobody. Swaps are charged to the step that
> performs them, which is why the scenario keeps contexts short: a 4k-token swap is 26 ms, and several
> in one step breach the 80 ms inter-token SLO on their own.

| Eviction | Attainment | First-token p99 | Preemptions/s |
|---|---|---|---|
| `never` | 2% | 36 s | 0 |
| `swap` to DRAM, 50 GB/s | **95%** | **84 ms** | 2.2 |

This is result 5's token budget seen from the other side: there the budget bound because requests were
long, here it binds because idle context is never released, at a load twenty times below rated
capacity. The metric that misleads is the one a load balancer would watch: the fleet is nearly idle by
compute while it serves one sequence at a time. The full table is the report's; the numbers here are
the unit's own, quoted rather than re-run. Goodput in the report, as quoted by demo 11's walkthrough
script (U60, c16675b): 780 tok/s without eviction, 1,426 with swap. Review R2 found that session turns
bypassed admission and that decode-growth eviction could swap the queue head's parked context out and
straight back; U62 (51968a7, merged 2896377) fixed both, and the numbers held: swap late attainment
0.953, p99 TTFT 84 ms, 2.24 preemptions/s, both spiral runs' step fingerprints unchanged, only the
events and `first_attempts` summary rows moved, by one gateway event per session turn.

---

## 9. Speculative decoding pays at small batch and costs at large

Scope item 16, `VISION.md` §3a's value-and-cost question (U26, 6fd648e, merged 0039c28). Scenario
keys `spec_draft_tokens` and `spec_accept_rate`, off by default; `scenarios/spec_off.txt` against
`spec_n4.txt`; demo 12, `out/12-spec-decode.html`, since U26b (b827d84, merged 02e5dcb). The gain is modelled as the expected tokens a
step, (1 − a^(N+1))/(1 − a), through a deterministic per-sequence accumulator rather than a random
draw, so every existing run is byte-identical and the `route_p2c` golden row proves it; a geometric
draw from a named stream is the documented refinement. The cost is verify compute added to the step;
the bandwidth term is untouched because the weights are read once a step either way. In the tech
lead's words, forwarded verbatim from the unit's agent:

> With N=4 drafts at α=0.7 a sequence advances 2.77 expected tokens a step. At the defaults (10.2 ms
> step, 28,286 prefill tok/s) the verify compute B·N/prefill_tokens_per_s equals the fixed step cost
> at B ≈ 72 sequences. Measured on one replica: batch 4 goes 392 → 1,024 tok/s (2.61×), batch 36 is
> 1.84×, batch 72 is 1.38×, batch 144 is 0.92× (a net loss), batch 256 is 0.61×. On the route_p2c
> fleet (effective batch 256, bandwidth term present) rated_rps moves only 234.8 → 240.2, so
> speculation is a small-batch/latency tool, not a capacity tool.

| Batch | Tokens/s, off → N=4 at α=0.7 | Ratio |
|---|---|---|
| 4 | 392 → 1,024 | 2.61× |
| 36 | | 1.84× |
| 72 | | 1.38× |
| 144 | | 0.92× |
| 256 | | 0.61× |

The metric that misleads here is the one a serving team would reach for first: a per-sequence
speed-up of 2.8x in expected tokens a step, which becomes a fleet capacity change of 2% because the
fleet runs near the batch where verify compute has eaten the gain. The numbers are the unit's own,
quoted rather than re-run; demo 12's report is the table of record once `./run-demos.sh` has been run on it.

## 10. Herding's staleness cliff moves left as the fleet grows

Demo 13, `out/13-herd-fleet.html`, `scenarios/herd_fleet.txt` (U98, cbf800d, merged d94fbf0).
Result 2 measured herding against telemetry age at one fleet size. U92's before/after showed the
same `least_requests` policy at the same cadence going from 79% attainment at 32 replicas to 15% at
256, so this sweep holds the cadence at 250 ms and the offered/capacity ratio at 0.30 (a new key,
`arrival_rps_per_replica`, keeps offered load following the fleet) and varies only the fleet.

| Replicas | Offered | Goodput tok/s | Attainment | First-token p99 | Load spread | Timeouts running |
|---|---|---|---|---|---|---|
| 32 | 70 rps | 14,941 | **79.5%** | 8.0 s | 0.71 | 3 |
| 64 | 140 rps | 21,058 | 56.5% | 12.8 s | 1.01 | 10 |
| 128 | 280 rps | 20,068 | 29.5% | 18.3 s | 1.34 | 20 |
| 256 | 560 rps | 17,388 | 14.5% | 35.4 s | 1.64 | 176 |
| 512 | 1,120 rps | 11,937 | **7.0%** | **50.5 s** | **2.03** | **4,050** |

**At one scrape interval that looked safe at 32 replicas, attainment falls from 79.5% to 7.0% as the
fleet grows sixteenfold, with nothing else changed.** Throughput rises 18,617 to 157,577 tok/s
across the same rows, because the fleet is eight times bigger; goodput per replica falls at every
step, 467 to 23 tok/s. The mechanism is result 2's: every router reads the same snapshot and the
least-loaded replica takes every arrival in the window. What changes with fleet size is how many
arrivals a window holds: at a fixed offered/capacity ratio that number scales with the fleet, so the
same 250 ms of staleness herds sixteen times as many requests onto one replica. The cliff is a
function of fleet size as much as of scrape interval, and a staleness budget set on a small fleet
does not survive growth.

Caveat: this is `least_requests`, the policy that scans, at one cadence; the 32-replica point's
fingerprint is byte-identical to result 2's 250 ms row, which is the check that the two sweeps are
the same experiment. Load spread's 2.03 at 512 replicas comes with 4,050 requests timing out
mid-run, so the right-hand column is a fleet that has started shedding work by timeout, not by
policy.

## 11. Prefix affinity against load spreading: a monotone knob, and at this workload the cache never repays the concentration

Demo 14, `out/14-affinity.html`, `scenarios/affinity_off.txt` against `affinity_spread.txt` and
`affinity_sticky.txt` (U27a engine 820e752, U27b policy d2c117e, merged 485f368; hit rate on the
wire and in the report since U27c, 1ee2f80). The workload carries a prefix topology derived from a
session process, 100 roots of 800 tokens under a Zipf(1.0) draw with a fork rate of 0.1 and a
400k-token prefix cache per replica; `prefix_affinity` routes to the replica already holding the
request's prefix until that replica's load exceeds `affinity_max_load_ratio` times the fleet mean,
then falls back to power of two choices. 256 replicas at 560 rps.

| Policy | Goodput tok/s | Attainment | First-token p99 | Load spread | Prefix hit |
|---|---|---|---|---|---|
| p2c, no affinity | **147,043** | **98.7%** | **2,282 ms** | **0.32** | 17.4% |
| affinity, ratio 1.05 | 145,303 | 97.8% | 3,221 ms | 0.37 | 18.9% |
| affinity, ratio 2.2 | 141,314 | 96.4% | 3,959 ms | **0.75** | **22.2%** |

**The ratio is a monotone knob from hit rate to hotspots, and every step along it loses.** Hit rate
climbs 17 to 22%, load spread climbs 0.32 to 0.75 (at a ratio of 4.0 it reaches 1.36), and goodput,
attainment and first-token tail all move the wrong way. The arithmetic is the reason: a hit saves an
800-token prefill, about 28 ms, in a run that is decode-bound, so the most a hit can buy is 28 ms of
first-token latency while the concentration it costs shows up as queueing on the holder. Affinity
pays when the prefix is long relative to the step and the replica is prefill-bound; here it is
neither, and the knob only chooses how much to lose.

Caveat, and it is the interesting one: the three scenarios run 100 ms / 10 ms telemetry, not the
baseline's 1 s / 200 ms. At the baseline's staleness every ratio from 0.5 to 2.2 herds to a load
spread of about 1.2 and loses to p2c on every metric, because a holder receives a full second of a
root's traffic before the view that would refuse it arrives, so the ceiling never acts. That is
result 2's cliff reached through a different policy, and it is why the demo isolates the ratio on
fresh telemetry first. The topology's parameters are guesses, per `docs/calibration.md` §9.1, and
the prefix cache has its own budget rather than competing with live KV.

## 12. A gray replica is invisible in the fleet aggregate, and ejecting it on the one tell the delayed view carries takes 4.3 seconds

Demo 15, `out/15-gray-failure.html`, `scenarios/gray_failure_none.txt` against
`gray_failure_eject.txt` (U31b, 206cb85, merged 02039ea). At t = 60 s replica 5 of 256 slows to 0.3x
and keeps announcing healthy; telemetry still reports it, and only its step time grows, in a view
that is 200 ms old and refreshed every second. `HealthPolicy` is the third pluggable kind: `outlier`
ejects a replica whose step time exceeds `ejection_ratio` times the fleet median on `ejection_views`
consecutive views, for `ejection_cooldown_s`, and writes the verdict into the same delayed
`ReplicaView` every router and admission policy already reads. p2c routing, 560 rps.

| Ejection | Attainment | First-token p99 | Goodput tok/s | Requests to the gray replica after onset | Of those, missed SLO |
|---|---|---|---|---|---|
| none | 97.1% | 3,355 ms | 143,739 | **49** | **48** |
| outlier, ratio 8, 3 views | 97.1% | 3,322 ms | 143,791 | **20** | — |

**The fleet numbers do not move: one replica in 256 at 0.3x is 0.4% of capacity, and every fleet
percentile absorbs it.** The failure lives in the per-replica view, where 49 requests reached the
slow replica after onset without ejection and 48 of them missed their SLO; with ejection, 20. The
timeline is the finding: ejected at 64.3 s, a detection latency of 4.3 s on a 1 s scrape, re-admitted
at 94.3 s when the cooldown ended, re-ejected at 100.3 s because the replica was still slow. A
cooldown is a re-assessment, not a pardon.

Caveat: the demo runs at `ejection_ratio = 8`, not the briefed 3, because a healthy fleet's step
time is bimodal, 11 ms decoding against 48 ms carrying a prefill chunk, a 4.4x spread, and a ratio
of 3 ejected 146 healthy replicas by t = 20 s. The price of the safe ratio is that the gray
replica's decode-only steps, at three times the median, look healthy, which is why detection takes
several views. The `Scenario` default stays 3.0 until the main agent decides. The demo routes with
p2c rather than `least_requests`, because `least_requests` is already at 4.5% attainment on this
fleet with no failure at all (result 1) and would hide the mechanism.

## 13. Local scheduling only bites when the prefill budget is the contended resource

Demo 16, `out/16-scheduling.html`, `scenarios/sched_fifo.txt`, `sched_class.txt`,
`sched_deadline.txt` (U108, 96a6456, c43b56e, db6547d, merged ce4a0bb). Issao asked whether policies
covered *"the host and gpu local scheduling decision"*; they did not, so `SchedulingPolicy` is the
third seam, at replica scope, deciding per step which queued sequences join the batch and in what
order, the prefill chunk budget, the order in which the batch's unfinished prompts are prefilled,
and the preemption victim. `fifo_chunked` is the engine's old behaviour and is proven byte-identical
on every golden row; `class_priority` admits interactive before agent before batch and evicts the
newest of the lowest class; `deadline_first` admits by least slack and evicts the latest deadline.
Three SLO classes at 70/20/10, 256 replicas, 1,120 rps, twice the baseline's offered load.

| Scheduler | Attainment, all | Interactive attainment | Goodput tok/s | Throughput tok/s | First-token p99 |
|---|---|---|---|---|---|
| `fifo_chunked` | 94.3% | 0.926 | 229,943 | 266,101 | **4,563 ms** |
| `class_priority` | **95.2%** | **0.943** | **232,141** | 265,587 | 5,100 ms |
| `deadline_first` | 91.1% | **0.883** | 222,272 | 265,414 | 6,174 ms |

**At 1.2x and 1.5x of the baseline nothing separated the three schedulers, because nothing queued.**
256 replicas at batch 256 hold thirty-odd sequences each, so a replica queue never forms and the
admission order has nothing to order. The contention in this fleet is inside the batch, where
chunked prefill spends its 1,024 tokens a step in batch order, and a 24k-token prompt admitted one
step earlier is 24 steps before your first token. That is why the seam gained the prefill-order
decision, and why the demo runs at twice the load. There `class_priority` lifts interactive
attainment 0.926 to 0.943 and interactive goodput 156,563 to 159,615 tok/s; agent pays, 0.972 to
0.960; batch stays at 0.997 because its 60 s target has slack to spare. `deadline_first` drops
interactive to 0.883: least slack charges the prefill still owed against the slack, so the longest
prompts look most urgent, and earliest-deadline-first over chunked prefill is longest-job-first in
disguise. Throughput is within a quarter of a percent across all three. **A replica scheduler
orders what it holds; it does not create capacity.**

Caveat: the 70/20/10 class shares are a guess per `docs/calibration.md` §9.1; the report has no
per-class table yet, so the interactive column comes from the unit's ignored test
`demo_16_class_table`, which prints it reproducibly. A policy never sees the whole queue, only
`max_batch` entries at its head, so each decision is bounded and a sequence beyond the head waits
its turn whatever its class.

## 14. The affinity failover cascade is a per-replica event the fleet aggregate hides

Demo 17, `out/17-cascade.html`, `scenarios/cascade_p2c.txt` against `cascade_affinity.txt` (U27d,
e942298, merged 8b2348f). Dynamic 10, no new mechanism: result 11's sticky run (ratio 2.2) with its
measured hottest prefix holder, replica 0 at 29.6 running against a fleet mean of 10.5, crashed at
60 s and returned cold at 90 s; p2c on the same seed and the same crash beside it.

| Routing | Attainment | First-token p99 | Goodput tok/s | Load spread | Holder at the crash, running / KV tokens | Timeouts running |
|---|---|---|---|---|---|---|
| p2c | **98.7%** | **2,282 ms** | **147,083** | **0.32** | 8 / 39k | 33 |
| affinity, ratio 2.2 | 96.2% | 4,027 ms | 141,117 | 0.75 | **36 / 145k** | **73** |

**The fleet aggregate barely moves: 96.2% with the crash against 96.4% without it in result 11.**
The cascade is in the per-replica view. The holder dies with 36 in-flight sequences and 145k KV
tokens, four and a half times what p2c's replica 0 was carrying, and every one of those is a timeout
charged to the client. Its roots land cold on the fallback replicas: replica 156's step time goes
15.7 to 28.7 ms, replica 35's running count 4.3 to 14.4, and the inheriting replicas' queues peak at
9, 5 and 2 where p2c's never exceed 1. On return the router's affinity map outlives the replica's
cache, so replica 0 is back at 90% of its load within half a second, serving every request as a
cache miss, and runs 27% slower than before the crash for about fifteen seconds. That is
`VISION.md` §3a's *"hot spot of machine fail over"*, at 560 of 1,878 rps, where the fleet has
capacity to spare and the percentile cannot see it.

Caveat: the ready count on the wire stayed at 256 through the outage in both runs at the time of
the demo, so the crash showed only in replica 0's state metric; U111 (8d956c9) has since made the
count exclude crashed replicas. The walkthrough points at the heatmap rather than the fleet
percentile, and says why.

## 15. The staleness loop rolls off like an integrator with a resonance of its own, not like the pure delay a controller would be designed against

Demo 18, `out/18-bode.html` and `out/18-bode-plot.html`, `scenarios/bode.txt` swept over
`perturb_frequency_hz` (M7, U33, 3a7246d and 156799f, merged 32859aa). Three workload keys,
default-off, put a sine on offered load, `1 + a·sin(2πft)`; the demo drives a 30% sine from 0.01 to
1 Hz through `least_requests` on a 1 s / 200 ms delayed snapshot over 32 replicas, and
`bench/bode.py` fits gain, phase, harmonic content and the dominant frequency of the fleet's
in-flight count and of the per-replica spread. The delay-only model, D + I/2 = 0.70 s, predicts a
180° crossing at 1 / (2 (D + I/2)) = 0.71 Hz.

| Input f | Gain | Gain dB | Phase | Harmonic content | Output dominant f |
|---|---|---|---|---|---|
| 0.01 Hz | **1.59** | **+4.0** | −39° | 0.29 | 0.010 Hz |
| 0.02 Hz | 0.97 | −0.3 | −61° | 0.42 | 0.020 Hz |
| 0.05 Hz | 0.37 | −8.6 | −104° | 1.06 | 0.050 Hz |
| 0.1 Hz | 0.115 | −18.8 | −102° | 3.5 | **0.020 Hz** |
| 0.2 Hz | 0.084 | −21.6 | −91° | 4.1 | **0.025 Hz** |
| 0.5 Hz | 0.031 | −30.2 | −99° | 11.9 | **0.025 Hz** |
| 1.0 Hz | **0.014** | **−36.8** | −103° | **34.6** | **0.020 Hz** |

**The predicted 180° crossing is never reached inside the sweep.** Gain falls 41 dB across two
decades, an integrator's roll-off, and the phase plateaus near −100° instead of winding on toward
−180° the way a delay's would. Above 0.1 Hz the output stops following the input at all: harmonic
content climbs from 0.3 to 35, and the fleet's dominant frequency sits at 0.02 to 0.025 Hz whatever
is driving it, a 40 to 50 second period set by how long requests stay resident rather than by the
telemetry delay. The input becomes a ripple on the herd. For a controller that is the useful
reading: the plant a proportional autoscaler or shaper closes its loop through is not the delay
alone, and its oscillation is a property of request residency that a faster scrape would not
remove.

Caveat: the output is `fleet_running`, not `fleet_queue`, because at this fleet an arrival is
admitted straight into a batch and the waiting queue averages 1.7 requests; 32 replicas at 70 rps,
where result 2's cliff already has `least_requests` at 39 to 42% attainment throughout, so the
sweep measures the herd's dynamics rather than a healthy loop's; the lowest frequency has only two
cycles in a 240 s run.

## 16. Swap beats recompute until the shared fabric saturates: a quarter-sized DRAM pool wins, a slow tier loses, and a contended fabric gives the advantage back

Demo 19, `out/19-tiering.html`, `scenarios/tier_dram.txt`, `tier_dram_ssd.txt`, `tier_contended.txt`
(U30, 3eb8495 and 38b3fc2, merged 192c6e0). Dynamic 14, `docs/ARCHITECTURE.md` §7.2 with numbers:
DRAM and SSD are pooled at cluster scope, every migration debits one shared fabric container and
runs at the slower of its tier and the fabric, so contention emerges rather than being assumed.
Four keys, `dram_pool_tokens`, `ssd_pool_tokens`, `ssd_gbps`, `fabric_gbps`, all defaulting to the
per-replica DRAM path of result 8, which is why every golden row before this demo is byte-identical.
The workload is result 8's session spiral on its four replicas, kept small on purpose.

| Tiers | Goodput tok/s | Attainment | First-token p99 | Worst gap p99 |
|---|---|---|---|---|
| swap everything to per-replica DRAM (result 8) | 1,376 | 97.6% | **84 ms** | 82 ms |
| one DRAM pool of 28k tokens, overflow recomputed | **1,421** | **100%** | 233 ms | **70 ms** |
| DRAM, then a 120k-token SSD pool at 10 GB/s | 893 | 70.3% | 323 ms | 361 ms |
| the same pools behind a 10 GB/s fabric | **666** | **59.6%** | **4,228 ms** | **2,416 ms** |

**A DRAM pool a quarter the size of what the swap-everything run parks at peak beats it**, 100%
attainment against 97.6%, because dropping the overflow for recompute costs first-token latency
(233 against 84 ms) while a synchronous copy costs the decoders' inter-token gap, and under an 80 ms
inter-token objective that is the trade that wins. **A tier slower than the inter-token budget is
worse than no tier**: the SSD run drops nothing and holds throughput at 1,411, and attainment falls
to 70% because every restore from it is a 361 ms stall in someone's stream. **A fabric every
migration crosses inverts the advantage**: pinned at 100% in 12% of windows, goodput falls to 666,
barely above the 608 of the run that never swaps at all. That is §7.2's *"until the shared bandwidth
saturates"*, measured.

Caveat: four replicas and two sessions a second, the fleet result 8 needed to show the spiral; the
migration policy is fixed for this unit (DRAM while there is room, SSD while there is room, otherwise
drop for recompute), with the victim choice staying at the scheduling seam of result 13; the SSD
figure is a single drive's 10 GB/s, the striped default is 50. The swap-everything row is result 8's
run, quoted from its own report.

## 17. Autoscaling with a cold start: a saturating utilization signal caps a proportional controller at 1/target per cycle, and a 30 s turn-up makes the cycle the turn-up

Demo 20, `out/20-autoscaling.html`, `scenarios/autoscale_none.txt`, `autoscale_cold30.txt`,
`autoscale_cold5.txt` (U32, 65a407e and 0996c17, merged e703f6a). Dynamic 12's single-cluster half
and the fifth pluggable seam: `AutoscalingPolicy` reads the same delayed views every other policy
reads and returns a number; the engine owns what the number costs. A slot the autoscaler fills is
WARMING for `warmup_delay_s` and serves nothing; one it empties is DRAINING, finishes what it holds
and loses the rest at `drain_timeout_s`. A fixed 256-replica fleet against `target_utilization`
scaling between 64 and 384 (target 0.7, step 32, cooldown 30 s, a decision every 10 s), over a
0.005 Hz sine from 224 to 896 rps: a 200 s day.

| Fleet | Attainment | Goodput tok/s | First-token p99 | Queue wait p99 | Timeouts running |
|---|---|---|---|---|---|
| fixed, 256 replicas | **96.7%** | **150,566** | **3,590 ms** | **47 ms** | 317 |
| scaled, 5 s cold start | 91.6% | 140,765 | 4,631 ms | 3,221 ms | 431 |
| scaled, 30 s cold start | **51.3%** | **77,974** | **53,150 ms** | **59,593 ms** | **9,020** |

**Both scaled fleets spend 29% fewer replica-seconds than the fixed one; the 5 s turn-up gives back
five points of attainment for that, the 30 s turn-up gives back forty-five.** The mechanism is in
the signal. Utilization is `running / max_batch`, and it saturates at one, so however deep the
queue a proportional controller can only ask for `ceil(serving × 1 / target)` replicas, a growth of
at most 1/target, here 1.43x, per decision. The cold start is the cycle: a replica turned up at one
decision is still warming at the next, so the fleet grows by that factor once per turn-up rather
than once per interval. From the 64-replica floor the 30 s fleet took four cycles and 150 s to reach
the day's peak; the queue reached 30,437 and 9,020 requests timed out. The drain's cost is bounded,
about a hundred requests in 190,000. This is the actuator-lag problem `docs/policy-catalog.md`'s
autoscaling section names, with the controller's gain limit made explicit: a proportional controller
on a saturating signal cannot be tuned out of it, and result 15's plant is what a predictive one
would be designed against.

Caveat: `max_batch` is 32 here rather than the base fleet's 256, because the autoscaler's signal is
`running / max_batch` and 32 is where this workload's prefill budget binds anyway; at 256 the target
would be a queue of thousands, which is the same saturation seen from the other side. The rated
capacity column in the report reads 1,152 rps for the same reason. Multi-geo, dynamic 13, stays
queued.

---

## What these seventeen have in common

Every one is a case where **the obvious metric moves the wrong way, or not at all**:

- Throughput is flat while goodput varies 8x in result 3 and 51x in result 1.
- Load imbalance improves while service collapses, in result 5, and is identical across a collapse in
  result 6.
- More offered load produces less delivered work, in result 4.
- The best-informed policy performs worst, in result 1.
- The cause of a collapse is gone while the collapse continues, in result 6.
- A fleet idle by compute serves one sequence at a time, at a twentieth of rated load, in result 8.
- A 2.8x per-sequence speed-up is a 2% capacity change at the fleet's batch size, in result 9.
- A scrape interval that was safe at 32 replicas is past the cliff at 512, in result 10.
- The best hit rate is the worst service, in result 11.
- A replica at a third of its speed leaves every fleet percentile unmoved, in result 12, and so does
  the crash of the hottest replica, in result 14.
- The scheduler that orders by deadline serves the deadline-bound class worst, in result 13.
- The controller-design number, a 180° crossing, never arrives, while the loop oscillates at a
  frequency of its own, in result 15.
- The faster tier is the one that loses, in result 16.
- The fleet that scales spends less and serves less, and no gain setting fixes it, in result 17.

- The same ordering with the device physics switched off, in result 7, so the effect is the queue's.

That is the argument for building this at all. Each of these is discoverable in production only by
degrading it, and three of the four would be invisible on a conventional dashboard.

## One correction to these numbers

The attainment column was overstated at high load until 14:35, and the arena harness caught why.
`slo_attainment` divided by *successful* requests rather than by all of them, so a policy that shed
nine requests in ten and served the tenth well would have reported perfect attainment. That is exactly
the trade the SLO gate exists to forbid, and the metric was blind to it.

Fixed: the denominator is now every measured request. A shed request did not get service, whatever the
merits of shedding it early. The old view survives as a diagnostic under a different name, because "of
what we served, how much was good" is sometimes the question, just never the score.

Impact on what is published above: attainment falls by 0.1 to 6 points, entirely at the high-load end
where shedding actually happens, and 230 requests/s moves from 59.1% to 53.2%. **No ordering and no
conclusion changes.** The figures in the tables are the corrected ones.

## Caveats, stated rather than buried

1. **The cost model is calibrated at two points**, batch-1 and batch-256 decode, against published
   figures. Between and beyond them it is an interpolation of a roofline, not a measurement.
2. **Absolute numbers should not be quoted; ratios should.** Now checked rather than asserted:
   `./check-sensitivity.sh` perturbs the bandwidth and prefill constants by ±30%, separately and
   together, and the policy ranking is **identical in all seven cases**. Goodput moves by up to 36%,
   as it should, while power-of-two-choices stays first and least-requests stays last throughout.

   | Perturbation | goodput, p2c | goodput, least_requests | ranking |
   |---|---|---|---|
   | nominal | 143,882 | 2,800 | unchanged |
   | bandwidth −30% | 136,062 | 2,405 | unchanged |
   | bandwidth +30% | 151,405 | 3,313 | unchanged |
   | prefill −30% | 137,283 | 1,796 | unchanged |
   | prefill +30% | 146,844 | 3,481 | unchanged |

   So the orderings are conclusions and the magnitudes are illustration. The script exits non-zero if
   any ordering ever flips, which makes this a regression test rather than a one-off observation.
3. **Preemption is absent from results 5 and 6.** Their scenarios still run `preemption = never`, the
   default; in a real engine both would additionally trigger key-value eviction and a recompute
   cascade, which is itself a positive feedback loop (result 8 measures it), so both collapses are if
   anything understated.
4. **The workload is synthetic**, calibrated against the distributions in `docs/calibration.md`. The
   prefix-sharing structure, which nothing public measures, is derived from a session process whose
   parameters are guesses (`docs/calibration.md` §9.1), so results 11 and 14 are about the mechanism,
   not the magnitude.
