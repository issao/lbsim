# Findings

Five results from the simulator as it stands. Every number here is reproducible with
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
| random | 16,961 | 8,590 ms | 92.3% | 0.55 |
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
| 110 rps | 26,457 | 27,863 | 4,496 ms | 95.6% |
| 150 rps | **33,068** | 35,350 | 5,637 ms | 93.8% |
| 190 rps | 31,761 | **37,976** | 8,590 ms | 84.5% |
| 230 rps | 18,128 | 30,175 | 38,655 ms | 59.1% |

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

---

## What these five have in common

Every one is a case where **the obvious metric moves the wrong way, or not at all**:

- Throughput is flat while goodput varies 8x, in results 1 and 3.
- Load imbalance improves while service collapses, in result 5.
- More offered load produces less delivered work, in result 4.
- The best-informed policy performs worst, in result 1.

That is the argument for building this at all. Each of these is discoverable in production only by
degrading it, and three of the four would be invisible on a conventional dashboard.

## Caveats, stated rather than buried

1. **The cost model is calibrated at two points**, batch-1 and batch-256 decode, against published
   figures. Between and beyond them it is an interpolation of a roofline, not a measurement.
2. **Absolute numbers should not be quoted; ratios should.** A ±30% error in the utilization constants
   moves every latency here without changing any ordering. A sensitivity sweep confirming that has not
   been run yet, and until it has, the orderings are the claim and the magnitudes are illustration.
3. **Preemption is absent.** In a real engine, result 5 would additionally trigger key-value eviction
   and a recompute cascade, so the collapse there is if anything understated.
4. **The workload is synthetic**, calibrated against the distributions in `docs/calibration.md`. The
   prefix-sharing structure, which nothing public measures, is not modelled at all.
