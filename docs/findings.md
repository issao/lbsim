# Findings

Seven results from the simulator as it stands. Every number here is reproducible with
`./run-demos.sh`, which writes a self-contained HTML report per experiment into `out/`.

Reference fleet throughout: 256 replicas of a 70-billion-parameter model on 8x H100, cost model
calibrated in `bench/validate_epochs.py` against a published batch-1 measurement and reproducing its
step-time table to within 0.05 ms. Rated capacity 1,878 requests/s. Unless stated, offered load is
**560 requests/s, which is 30% of capacity**: none of what follows is an overload artefact.

What is *not* modelled yet, and therefore not claimed: preemption, prefix caching, memory tiering,
autoscaling, and multi-cluster. `docs/scope-today.md` says why each was cut.

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
cliff moves left as the fleet grows, which is the mechanism above, not a new one. A fleet-size sweep is
queued as demo 13 (U98) to turn that relationship into a curve rather than two points.

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

---

## What these nine have in common

Every one is a case where **the obvious metric moves the wrong way, or not at all**:

- Throughput is flat while goodput varies 8x in result 3 and 51x in result 1.
- Load imbalance improves while service collapses, in result 5, and is identical across a collapse in
  result 6.
- More offered load produces less delivered work, in result 4.
- The best-informed policy performs worst, in result 1.
- The cause of a collapse is gone while the collapse continues, in result 6.
- A fleet idle by compute serves one sequence at a time, at a twentieth of rated load, in result 8.
- A 2.8x per-sequence speed-up is a 2% capacity change at the fleet's batch size, in result 9.

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
3. **Preemption is absent.** In a real engine, results 5 and 6 would additionally trigger key-value
   eviction and a recompute cascade, which is itself a positive feedback loop, so both collapses are if
   anything understated.
4. **The workload is synthetic**, calibrated against the distributions in `docs/calibration.md`. The
   prefix-sharing structure, which nothing public measures, is not modelled at all.
