# Findings

Seven results from the simulator as it stands. Every number here is reproducible with
`./run-demos.sh`, which writes a self-contained HTML report per experiment into `out/`.

Reference fleet throughout: 32 replicas of a 70-billion-parameter model on 8x H100, cost model
calibrated in `bench/validate_epochs.py` against a published batch-1 measurement and reproducing its
step-time table to within 0.05 ms. Rated capacity 235 requests/s. Unless stated, offered load is
**70 requests/s, which is 30% of capacity**: none of what follows is an overload artefact.

What is *not* modelled yet, and therefore not claimed: preemption, prefix caching, memory tiering,
autoscaling, and multi-cluster. `docs/scope-today.md` says why each was cut.

---

## 1. Reading the whole fleet is worse than sampling two of it

| Policy | Goodput tok/s | First-token p99 | Attainment | Load spread |
|---|---|---|---|---|
| power of two choices | **18,127** | 3,859 ms | 97.0% | 0.33 |
| round robin | 17,361 | 8,321 ms | 93.1% | 0.43 |
| random | 16,961 | 8,590 ms | 92.2% | 0.55 |
| least requests | **6,943** | 20,133 ms | 39.4% | 1.27 |

**Least-requests is 2.6x worse than sampling two replicas at random, and worse than round robin,
which ignores load entirely.** It inspects all 32 replicas and picks the least loaded, which is the
obvious thing to do and is the trap: its snapshot is up to 1.2 seconds old, so every routing decision
in that window sees the same apparently-idle replica and sends to it. Sampling bounds the stampede by
construction, because only a fraction of decisions consider any one replica at a time.

**Throughput barely moves across all four**, 17,480 to 18,744. The work gets done either way; under
the worse policy it arrives too late to count. That is why ranking policies on throughput selects the
wrong one, and why goodput leads every table in this project.

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

## 3. Prefill and decode contend for one device, and no setting wins both

Chunked prefill token budget swept, with power-of-two-choices routing.

| Chunk budget | Worst gap between tokens p99 | First-token p99 | Goodput | Attainment |
|---|---|---|---|---|
| 512 | **33 ms** | 4,631 ms | 17,776 | 95.8% |
| 1,024 | 50 ms | 3,859 ms | **18,127** | 97.0% |
| 2,048 | 86 ms | 3,523 ms | 2,181 | 31.8% |
| 4,096 | 157 ms | 3,355 ms | 2,205 | 32.2% |
| 8,192 | 302 ms | 3,288 ms | 2,437 | 34.1% |
| 16,384 | **587 ms** | **3,255 ms** | 2,354 | 33.4% |

The two latencies move in opposite directions, monotonically, across an eighteenfold range of worst
gap. A bigger chunk gets the first token out sooner and inserts a longer stall into every stream
already decoding, because both compete for the same device in the same step.

**The goodput cliff between 1,024 and 2,048 is not a modelling artefact, it is the SLO.** The
inter-token target here is 80 ms; at a 2,048-token chunk the worst gap is 86 ms, so almost every
request breaches and earns no goodput while still consuming capacity. Throughput is flat at about
18,700 across the whole sweep. **The fleet does identical work and delivers a tenth of the value**,
which is the sharpest illustration in this set of why the two measures must not be confused.

## 4. Past the knee, offering more load delivers less work

Offered rate swept with power-of-two-choices.

| Offered | Goodput | Throughput | First-token p99 | Attainment |
|---|---|---|---|---|
| 30 rps | 7,919 | 8,106 | 2,819 ms | 98.1% |
| 70 rps | 18,127 | 18,744 | 3,859 ms | 97.0% |
| 110 rps | 26,457 | 27,863 | 4,496 ms | 95.4% |
| 150 rps | **33,068** | 35,350 | 5,637 ms | 93.3% |
| 190 rps | 31,761 | **37,976** | 8,590 ms | 83.3% |
| 230 rps | 18,128 | 30,175 | 38,655 ms | 53.2% |

Two knees, in different places, and that is the point. **Goodput peaks at 150 requests/s**, which is
64% of the rated 235. **Throughput peaks later, at 190.** Between those two points the fleet is doing
more work and delivering less value.

Past 190 both collapse: at 230 requests/s throughput itself falls 20% below its peak while first-token
latency goes to 38 seconds. Offering 53% more load than the goodput optimum yields **45% less
goodput**. Admission control is not a refinement here, it is the difference between the peak and the
right-hand column.

## 5. Capacity is a token budget, and a load-balancer metric can improve while service collapses

Share of long-context requests swept, everything else fixed. The long mode averages 24,000 prompt
tokens against 1,200 for the short mode.

| Long share | Goodput | First-token p99 | Attainment | Load spread |
|---|---|---|---|---|
| 0% | 19,037 | **407 ms** | 100.0% | 0.38 |
| 4% | 18,750 | 2,215 ms | 98.7% | 0.35 |
| 8% | 18,127 | 3,859 ms | 97.0% | 0.33 |
| 16% | 16,748 | 5,033 ms | 93.6% | 0.28 |
| 32% | **8,515** | **13,824 ms** | 64.9% | **0.24** |

**Load spread falls from 0.38 to 0.24 while goodput halves and tail latency gets 34 times worse.** The
balancer looks like it is doing a better job precisely as service falls apart, because a few enormous
requests make every replica uniformly saturated. A dashboard watching imbalance would report
improvement throughout.

The mechanism is the token budget. One 24,000-token context consumes what eight chat turns consume, so
raising the long share consumes admission capacity that a request count would have said was free. At
32% long, first-token latency at the 99th percentile is 13.8 seconds even though the fleet is at 30%
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
| none | 0 | 20,161 | 84.5% | 2 → 2 | yes |
| 3 attempts, 10% budget | 3,841 | 17,373 | 81.1% | 2 → 2 | yes |
| 3 attempts, no budget | **35,806** | **7,344** | 46.3% | **2 → 6** | **no** |

**The unbudgeted run never comes back.** Offered load returned to normal after forty seconds and its
queue is still three times pre-spike depth at the end of a four-minute run. That is a metastable
collapse: the system is in a state it sustains on its own, and the original cause is gone.

The mechanism is visible in the retry count. Unbudgeted retries reach 35,806 against 3,841 with a 10%
cap, a ninefold difference, because each timeout adds load precisely when the fleet is least able to
absorb it. Retries here are far more expensive than in a stateless service: a request that times out
after twenty seconds has already consumed twenty seconds of device time producing tokens nobody will
read.

**A 10% budget recovers 86% of the no-retry goodput and turns the collapse into a recovery.** That is
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
the unit's own, quoted rather than re-run.

---

## What these eight have in common

Every one is a case where **the obvious metric moves the wrong way, or not at all**:

- Throughput is flat while goodput varies 8x, in results 1 and 3.
- Load imbalance improves while service collapses, in result 5, and is identical across a collapse in
  result 6.
- More offered load produces less delivered work, in result 4.
- The best-informed policy performs worst, in result 1.
- The cause of a collapse is gone while the collapse continues, in result 6.
- A fleet idle by compute serves one sequence at a time, at a twentieth of rated load, in result 8.

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
   together, and the policy ranking is **identical in all seven cases**. Goodput moves by up to 30%,
   as it should, while power-of-two-choices stays first and least-requests stays last throughout.

   | Perturbation | goodput, p2c | goodput, least_requests | ranking |
   |---|---|---|---|
   | nominal | 18,127 | 6,943 | unchanged |
   | bandwidth −30% | 17,029 | 6,236 | unchanged |
   | bandwidth +30% | 19,212 | 7,600 | unchanged |
   | prefill −30% | 17,165 | 4,844 | unchanged |
   | prefill +30% | 18,695 | 8,515 | unchanged |

   So the orderings are conclusions and the magnitudes are illustration. The script exits non-zero if
   any ordering ever flips, which makes this a regression test rather than a one-off observation.
3. **Preemption is absent.** In a real engine, results 5 and 6 would additionally trigger key-value
   eviction and a recompute cascade, which is itself a positive feedback loop, so both collapses are if
   anything understated.
4. **The workload is synthetic**, calibrated against the distributions in `docs/calibration.md`. The
   prefix-sharing structure, which nothing public measures, is not modelled at all.
