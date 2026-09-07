// SubscriptionUpdate row -> the panels' Frame.
//
// The panels were written against the mock engine's per-tick `Frame`, and they never see the wire.
// This is the one place the two meet: a fleet-scope `MetricRow` becomes a `Frame` with the fields the
// engine really produced filled in and everything else left honest. Per docs/dashboard-plan.md
// section 3 and WIRE.md's metric table:
//
//   - a metric on the wire is read as it is (`METRIC_KV_UTILIZATION` is already a fraction);
//   - a metric the engine does not simulate at all is NaN, which the charts draw as a gap and the
//     tiles as a dash, rather than a zero that would look like a measurement;
//   - the three lifecycle counts are 0 and the replica list is empty, because the fleet is static by
//     construction today (no warming, draining or ejected replicas exist to count) and per-replica
//     rows arrive with U23;
//   - `tierUtilization.hbm` is the KV utilization, because HBM is the only tier the engine has.
//
// Distributions are the delicate part. hist.ts's `Histogram` is a log-linear bucketed histogram and
// the panels derive percentiles and SLO attainment from it at render time, which is what makes an
// SLO threshold a view-only change. The wire carries `count`, `mean`, `min`, `max` and a few exact
// percentiles per sample window, not the buckets. So the histogram here is *reconstructed*: the
// percentiles define a piecewise-linear CDF and each bucket receives the mass that CDF puts in it.
// Its quantiles land within one bucket of the wire's, and its `fractionBelow` is the linear
// interpolation of the wire's CDF. The wire's own values are kept on the frame as `exact`, so a
// panel that wants the measured p99 rather than the bucketed one can read it without a second
// decoder. A window with no completions has no distribution at all: the histogram stays empty and
// `exact` stays absent, which `latencyMs` reports as NaN. Zero would be a lie.

import type { Frame, ReplicaSample } from './engine';
import type { MetricName, SubscriptionUpdate, WireDistribution } from './api';
import { relSeconds } from './api';
import { type Histogram, HIST_BUCKETS, newHistogram, record } from './hist';

/** One wire distribution in the panels' unit (milliseconds), kept exactly as measured. */
export interface ExactPercentiles {
  count: number;
  meanMs: number;
  minMs: number;
  maxMs: number;
  percentile: number[];
  valueMs: number[];
  fromMergedHistogram: boolean;
}

export type LatencyKind = 'ttft' | 'itl' | 'e2e' | 'queueWait';

export interface ReplayFrame extends Frame {
  /** The absolute simulated instant, unrounded. `simS` is this relative to the run's origin. */
  simTimeUnixNs: bigint;
  /** The wire's exact percentiles per latency, absent when the window had no completions. */
  exact: Partial<Record<LatencyKind, ExactPercentiles>>;
  /** `METRIC_GOODPUT_TOKENS_PER_S`, NaN when absent. The panels derive goodput; this is the measured one. */
  goodputTokensPerS: number;
  /** `METRIC_SLO_ATTAINMENT` over the window, NaN when the window ended no requests. */
  sloAttainment: number;
  /** Fleet sums of `METRIC_QUEUED_SEQS` and `METRIC_RUNNING_SEQS`. */
  queuedSeqs: number;
  runningSeqs: number;
}

const LATENCY_METRIC: Record<LatencyKind, MetricName> = {
  ttft: 'METRIC_TTFT',
  itl: 'METRIC_ITL',
  e2e: 'METRIC_E2E',
  queueWait: 'METRIC_QUEUE_WAIT',
};

const NS_PER_MS = 1e6;

/**
 * One replica-scope update as the heatmap's row. The engine records queued, running, KV and the last
 * step; everything else the interface has is NaN, so a column the wire never carried reads as
 * missing rather than as a measured zero. `present` and `state` are what a static fleet is: there
 * is no lifecycle in the engine yet, so every replica in the row exists and is ready.
 */
export function replicaFromUpdate(u: SubscriptionUpdate): ReplicaSample {
  const v = (m: MetricName): number => u.row.values[m] ?? NaN;
  const runningSeqs = v('METRIC_RUNNING_SEQS');
  const stepTimeS = v('METRIC_STEP_TIME');
  return {
    id: Number(u.row.target.replicaId ?? -1n),
    present: true,
    state: 'READY',
    weight: 1,
    queuedSeqs: v('METRIC_QUEUED_SEQS'),
    runningSeqs,
    batchSize: runningSeqs,
    kvTokensResident: v('METRIC_KV_TOKENS_RESIDENT'),
    kvUtilization: v('METRIC_KV_UTILIZATION'),
    stepTimeMs: stepTimeS * 1000,
    queueWaitMs: NaN,
    ttftMeanMs: NaN,
    itlMeanMs: NaN,
    prefixHitRate: NaN,
    admittedRps: NaN,
    completedRps: NaN,
    preemptionsPerS: NaN,
    trueSpeedMultiplier: NaN,
    telemetryStalenessMs: NaN,
  };
}

/**
 * One fleet-scope update as a frame. `tick` is the row's index in the run, `originUnixNs` the run's
 * start, `replicas` the SCOPE_REPLICA updates recorded at the same instant, when the export has them.
 */
export function frameFromUpdate(
  u: SubscriptionUpdate,
  originUnixNs: bigint,
  tick: number,
  replicas: readonly SubscriptionUpdate[] = []
): ReplayFrame {
  const v = (m: MetricName): number => u.row.values[m] ?? NaN;
  const dist = (k: LatencyKind): { hist: Histogram; exact?: ExactPercentiles } => {
    const d = u.row.distributions[LATENCY_METRIC[k]];
    if (d === undefined || d.count === 0n) return { hist: newHistogram() };
    return { hist: histogramFromDistribution(d), exact: exactFromDistribution(d) };
  };
  const ttft = dist('ttft');
  const itl = dist('itl');
  const e2e = dist('e2e');
  const queueWait = dist('queueWait');
  const exact: ReplayFrame['exact'] = {};
  if (ttft.exact) exact.ttft = ttft.exact;
  if (itl.exact) exact.itl = itl.exact;
  if (e2e.exact) exact.e2e = e2e.exact;
  if (queueWait.exact) exact.queueWait = queueWait.exact;

  const kv = v('METRIC_KV_UTILIZATION');
  return {
    tick,
    simS: relSeconds(u.simTimeUnixNs, originUnixNs),
    simTimeUnixNs: u.simTimeUnixNs,
    offeredRps: v('METRIC_OFFERED_RPS'),
    admittedRps: v('METRIC_ADMITTED_RPS'),
    completedRps: v('METRIC_COMPLETED_RPS'),
    rejectedRps: v('METRIC_REJECTED_RPS'),
    outputTokensPerS: v('METRIC_OUTPUT_TOKENS_PER_S'),
    goodputTokensPerS: v('METRIC_GOODPUT_TOKENS_PER_S'),
    sloAttainment: v('METRIC_SLO_ATTAINMENT'),
    preemptionsPerS: v('METRIC_PREEMPTIONS_PER_S'),
    loadImbalanceCv: v('METRIC_LOAD_IMBALANCE_CV'),
    wastedGpuFraction: v('METRIC_WASTED_GPU_FRACTION'),
    readyReplicas: v('METRIC_READY_REPLICAS'),
    warmingReplicas: u.row.values.METRIC_WARMING_REPLICAS ?? 0,
    drainingReplicas: u.row.values.METRIC_DRAINING_REPLICAS ?? 0,
    ejectedReplicas: 0,
    kvUtilization: kv,
    prefixHitRate: v('METRIC_PREFIX_HIT_RATE'),
    tierUtilization: { hbm: kv, dram: NaN, ssd: NaN },
    tierBandwidth: { dram: NaN, ssd: NaN },
    queuedSeqs: v('METRIC_QUEUED_SEQS'),
    runningSeqs: v('METRIC_RUNNING_SEQS'),
    ttft: ttft.hist,
    itl: itl.hist,
    e2e: e2e.hist,
    queueWait: queueWait.hist,
    exact,
    replicas: replicas.map(replicaFromUpdate),
    events: [],
  };
}

/**
 * The measured percentile `p` of one latency, in milliseconds: the wire's value when the window
 * carried that percentile, otherwise NaN. Never an interpolation, and never zero for an empty window.
 */
export function latencyMs(f: ReplayFrame, which: LatencyKind, p: number): number {
  const e = f.exact[which];
  if (!e) return NaN;
  const i = e.percentile.indexOf(p);
  return i === -1 ? NaN : e.valueMs[i];
}

export function exactFromDistribution(d: WireDistribution): ExactPercentiles {
  return {
    count: Number(d.count),
    meanMs: d.mean / NS_PER_MS,
    minMs: d.min / NS_PER_MS,
    maxMs: d.max / NS_PER_MS,
    percentile: d.percentile.slice(),
    valueMs: d.value.map((x) => x / NS_PER_MS),
    fromMergedHistogram: d.fromMergedHistogram,
  };
}

// ---------------------------------------------------------------------------
// Histogram reconstruction
// ---------------------------------------------------------------------------

// hist.ts's bucket geometry, restated: 4 buckets per octave from 0.5 ms. hist.ts does not export its
// edges, and this file must not change it; the self-test checks these against `bins()` so the two
// cannot drift silently.
const MIN_LOG = Math.log2(0.5);
const BUCKETS_PER_OCTAVE = 4;

export function bucketLow(b: number): number {
  return Math.pow(2, MIN_LOG + b / BUCKETS_PER_OCTAVE);
}

/**
 * A `Histogram` whose bucket masses follow the piecewise-linear CDF through the wire's `min`, its
 * percentiles and its `max`. `count`, `sum`, `min` and `max` are the wire's own.
 */
export function histogramFromDistribution(d: WireDistribution): Histogram {
  const h = newHistogram();
  const count = Number(d.count);
  if (count <= 0) return h;
  const minMs = d.min / NS_PER_MS;
  const maxMs = d.max / NS_PER_MS;
  const cdf = cdfKnots(minMs, maxMs, d.percentile, d.value.map((x) => x / NS_PER_MS));

  if (cdf === null) {
    record(h, minMs, count);
  } else {
    const F = (x: number) => cdfAt(cdf, x);
    let prev = 0;
    for (let b = 0; b < HIST_BUCKETS; b++) {
      const last = b === HIST_BUCKETS - 1;
      const hi = last ? Infinity : bucketLow(b + 1);
      const f = last ? 1 : F(hi);
      const mass = (f - prev) * count;
      prev = f;
      if (mass <= 0) continue;
      // Any value inside the bucket lands the mass in it; the geometric middle keeps `record`'s own
      // bookkeeping harmless, and the true moments are restored below.
      const lo = b === 0 ? 0.25 : bucketLow(b);
      const mid = last ? lo * 1.1 : Math.sqrt(lo * hi);
      record(h, mid, mass);
    }
  }
  h.count = count;
  h.sum = (d.mean / NS_PER_MS) * count;
  h.min = minMs;
  h.max = maxMs;
  return h;
}

interface Knots {
  x: number[];
  p: number[];
}

/** The CDF's knots in ascending x, or null when the distribution is a single point. */
function cdfKnots(minMs: number, maxMs: number, percentile: number[], valueMs: number[]): Knots | null {
  if (!(maxMs > minMs)) return null;
  const order = percentile.map((_, i) => i).sort((a, b) => percentile[a] - percentile[b]);
  const x = [minMs];
  const p = [0];
  for (const i of order) {
    const q = percentile[i] / 100;
    if (!(q > 0 && q < 1)) continue;
    // Monotone by construction on the wire; clamped here so a rounding wobble cannot invert the CDF.
    x.push(Math.min(Math.max(valueMs[i], x[x.length - 1]), maxMs));
    p.push(Math.max(q, p[p.length - 1]));
  }
  x.push(maxMs);
  p.push(1);
  return { x, p };
}

function cdfAt(k: Knots, v: number): number {
  const n = k.x.length;
  if (v <= k.x[0]) return 0;
  if (v >= k.x[n - 1]) return 1;
  let i = 1;
  while (i < n && k.x[i] < v) i++;
  // Two knots at one x are a jump; the value at the jump is the upper one.
  let j = i;
  while (j + 1 < n && k.x[j + 1] === k.x[i]) j++;
  if (k.x[i] === v) return k.p[j];
  const x0 = k.x[i - 1];
  const x1 = k.x[i];
  const p0 = k.p[i - 1];
  const p1 = k.p[i];
  return x1 > x0 ? p0 + ((v - x0) / (x1 - x0)) * (p1 - p0) : p1;
}
