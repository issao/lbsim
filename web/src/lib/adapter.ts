// SubscriptionUpdate row -> the panels' Frame.
//
// The panels read a per-sample `Frame`, and they never see the wire. This is the one place the two
// meet: a fleet-scope `MetricRow` becomes a `Frame` with the fields the
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

import type { FractionPercentiles, Frame, ReplicaSample } from './frame';
import type { ReplicaState } from './types';
import type { MetricName, SubscriptionUpdate, WireDistribution } from './api';
import { relSeconds } from './api';
import { type Histogram, HIST_BUCKETS, histMean, newHistogram, quantile, record } from './hist';

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

/** `METRIC_REPLICA_STATE` is the state's enum number on the wire; anything else is a row that did not carry it. */
const REPLICA_STATE: Record<number, ReplicaState> = { 1: 'READY', 2: 'DEGRADED', 3: 'EJECTED' };

/**
 * One replica-scope update as the heatmap's row. The engine records queued, running, KV, the last
 * step, GPU utilization, the state, the true speed and the window's TTFT; everything else the
 * interface has is NaN (or `UNKNOWN`), so a column the wire never carried reads as missing rather
 * than as a measured zero. `present` is what a static fleet is: every replica in the row exists.
 */
export function replicaFromUpdate(u: SubscriptionUpdate): ReplicaSample {
  const v = (m: MetricName): number => u.row.values[m] ?? NaN;
  const runningSeqs = v('METRIC_RUNNING_SEQS');
  const stepTimeS = v('METRIC_STEP_TIME');
  const ttft = u.row.distributions.METRIC_TTFT;
  return {
    id: Number(u.row.target.replicaId ?? -1n),
    present: true,
    state: REPLICA_STATE[v('METRIC_REPLICA_STATE')] ?? 'UNKNOWN',
    weight: 1,
    queuedSeqs: v('METRIC_QUEUED_SEQS'),
    runningSeqs,
    batchSize: runningSeqs,
    kvTokensResident: v('METRIC_KV_TOKENS_RESIDENT'),
    kvUtilization: v('METRIC_KV_UTILIZATION'),
    gpuUtilization: v('METRIC_GPU_UTILIZATION'),
    gpuUsefulFraction: v('METRIC_GPU_USEFUL_FRACTION'),
    gpuComputeBoundFraction: v('METRIC_GPU_COMPUTE_BOUND_FRACTION'),
    stepTimeMs: stepTimeS * 1000,
    queueWaitMs: NaN,
    // A window with no completions has no TTFT at all, NaN rather than a zero that reads as fast.
    ttftMeanMs: ttft !== undefined && ttft.count > 0n ? ttft.mean / NS_PER_MS : NaN,
    itlMeanMs: NaN,
    prefixHitRate: NaN,
    admittedRps: NaN,
    completedRps: NaN,
    preemptionsPerS: v('METRIC_PREEMPTIONS_PER_S'),
    trueSpeedMultiplier: v('METRIC_TRUE_SPEED_MULTIPLIER'),
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
  // A fraction across replicas is read as it is; `exactFromDistribution` would divide it by 1e6.
  const fraction = (m: MetricName): FractionPercentiles | null => {
    const d = u.row.distributions[m];
    return d === undefined || d.count === 0n ? null : fractionPercentilesFromDistribution(d);
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
    gpuUtilization: v('METRIC_GPU_UTILIZATION'),
    gpuUsefulFraction: v('METRIC_GPU_USEFUL_FRACTION'),
    gpuComputeBoundFraction: v('METRIC_GPU_COMPUTE_BOUND_FRACTION'),
    gpuUtilizationP: fraction('METRIC_GPU_UTILIZATION'),
    gpuUsefulFractionP: fraction('METRIC_GPU_USEFUL_FRACTION'),
    kvUtilizationP: fraction('METRIC_KV_UTILIZATION'),
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

/** A distribution of a 0..1 metric across replicas, kept in its own unit. */
export function fractionPercentilesFromDistribution(d: WireDistribution): FractionPercentiles {
  return {
    count: Number(d.count),
    mean: d.mean,
    min: d.min,
    max: d.max,
    percentile: d.percentile.slice(),
    value: d.value.slice(),
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

// ---------------------------------------------------------------------------
// Smoothing: the trailing window over recorded frames
// ---------------------------------------------------------------------------
//
// The replay half of `OpenSubscriptionRequest.smoothing_window_ns`. A live run asks the server for
// smoothed rows; a recording is smoothed here, by the same definition (WIRE.md "Smoothing",
// `run::row_over`): frame `i` becomes the window of the frames whose instant lies in
// `(t_i − window, t_i]`, which is `ceil(window / interval)` frames ending at `i`, fewer at the start
// of a run. Every gauge and rate is the mean of its per-frame values; every latency histogram is the
// merge of the frames' histograms, so its p99 is the p99 of every request that finished in the
// window; the replica counts by state, each replica's own state and the frame's instant are the
// sample's own. One frame per input frame, so a smoothed series lines up with the raw one point for
// point, and a window of one frame is the input itself.
//
// Two places the recording cannot match the server exactly, both documented rather than hidden: the
// server takes SLO attainment, the compute-bound share and the prefix hit rate as ratios of the
// window's sums, and a recording has no per-window counts to weigh with, so here they are means of
// the per-frame ratios; and the wire's exact percentiles cannot be merged, so a smoothed frame's
// `exact` is read off the merged histogram and says so with `fromMergedHistogram: true`.

/** `ceil(windowS / intervalS)` frames, never fewer than one: the server's `frames_in_window`. */
export function framesInWindow(windowS: number, intervalS: number): number {
  // In whole milliseconds, as the scenario states its interval, so a 100 ms interval recovered from
  // float seconds cannot round a 30 s window up to 301 frames.
  const ivMs = Math.round(intervalS * 1000);
  if (!(windowS > 0) || !(ivMs > 0)) return 1;
  return Math.max(1, Math.ceil(Math.round(windowS * 1000) / ivMs));
}

const FRAME_MEANS = [
  'offeredRps', 'admittedRps', 'completedRps', 'rejectedRps', 'outputTokensPerS', 'goodputTokensPerS',
  'sloAttainment', 'preemptionsPerS', 'loadImbalanceCv', 'wastedGpuFraction', 'kvUtilization',
  'gpuUtilization', 'gpuUsefulFraction', 'gpuComputeBoundFraction', 'prefixHitRate', 'queuedSeqs', 'runningSeqs',
] as const;
const REPLICA_MEANS = [
  'queuedSeqs', 'runningSeqs', 'batchSize', 'kvTokensResident', 'kvUtilization', 'gpuUtilization',
  'gpuUsefulFraction', 'gpuComputeBoundFraction', 'stepTimeMs', 'queueWaitMs', 'ttftMeanMs', 'itlMeanMs', 'prefixHitRate',
  'admittedRps', 'completedRps', 'preemptionsPerS', 'trueSpeedMultiplier', 'telemetryStalenessMs',
] as const;
const LATENCIES: LatencyKind[] = ['ttft', 'itl', 'e2e', 'queueWait'];
const DEFAULT_EXACT_PERCENTILES = [50, 90, 99, 99.9];

/** A running mean over the finite values added and not yet removed; NaN while it holds none. */
class Mean {
  private sum = 0;
  private n = 0;
  add(v: number): void {
    if (Number.isFinite(v)) {
      this.sum += v;
      this.n++;
    }
  }
  remove(v: number): void {
    if (Number.isFinite(v)) {
      this.sum -= v;
      this.n--;
    }
  }
  value(): number {
    return this.n > 0 ? this.sum / this.n : NaN;
  }
}

function meanOf(values: number[]): number {
  const m = new Mean();
  for (const v of values) m.add(v);
  return m.value();
}

/**
 * A histogram's counts summed in float64 as the window slides, so that adding a frame and later
 * subtracting it leaves the buckets where they were; the panels' `Histogram` keeps float32 counts,
 * and a float32 sum is not undone by a float32 subtraction over thousands of frames.
 */
class SlidingHistogram {
  readonly counts = new Float64Array(HIST_BUCKETS);
  count = 0;
  sum = 0;
  add(h: Histogram): void {
    for (let i = 0; i < HIST_BUCKETS; i++) this.counts[i] += h.counts[i];
    this.count += h.count;
    this.sum += h.sum;
  }
  remove(h: Histogram): void {
    for (let i = 0; i < HIST_BUCKETS; i++) this.counts[i] = Math.max(0, this.counts[i] - h.counts[i]);
    this.count = Math.max(0, this.count - h.count);
    this.sum -= h.sum;
  }
  /** The window's histogram, its bounds read off the frames the window still holds. */
  snapshot(window: ReplayFrame[], kind: LatencyKind): Histogram {
    const h = newHistogram();
    h.counts.set(this.counts);
    h.count = this.count;
    h.sum = this.count > 0 ? this.sum : 0;
    for (const f of window) {
      if (f[kind].count > 0) {
        h.min = Math.min(h.min, f[kind].min);
        h.max = Math.max(h.max, f[kind].max);
      }
    }
    return h;
  }
}

/** The per-slot mean of the fraction distributions the window's frames carry, null when none does. */
function meanFractionPercentiles(window: ReplayFrame[], pick: (f: ReplayFrame) => FractionPercentiles | null): FractionPercentiles | null {
  const carried = window.map(pick).filter((p): p is FractionPercentiles => p !== null);
  if (carried.length === 0) return null;
  const template = carried[carried.length - 1];
  const same = carried.filter((p) => p.percentile.length === template.percentile.length && p.percentile.every((q, i) => q === template.percentile[i]));
  return {
    count: template.count,
    mean: meanOf(same.map((p) => p.mean)),
    min: meanOf(same.map((p) => p.min)),
    max: meanOf(same.map((p) => p.max)),
    percentile: template.percentile.slice(),
    value: template.percentile.map((_, i) => meanOf(same.map((p) => p.value[i]))),
  };
}

/**
 * Every frame smoothed over the trailing window of `windowS` simulated seconds, at the recording's
 * own interval (inferred from the frames when not given). The input itself when the window is one
 * frame or shorter, so a caller can tell "nothing to do" by identity.
 */
export function smoothFrames(frames: ReplayFrame[], windowS: number, intervalS?: number): ReplayFrame[] {
  const n = frames.length;
  const dt = intervalS ?? (n > 1 ? (frames[n - 1].simS - frames[0].simS) / (n - 1) : 0);
  const m = framesInWindow(windowS, dt);
  if (m <= 1 || n === 0) return frames;

  // Sliding state for what is too big to recompute per frame: the four histograms (72 buckets each)
  // and the per-replica means (a fleet of hundreds). The scalars are recomputed over the window,
  // which is exact and cheap.
  const hists: Record<LatencyKind, SlidingHistogram> = {
    ttft: new SlidingHistogram(),
    itl: new SlidingHistogram(),
    e2e: new SlidingHistogram(),
    queueWait: new SlidingHistogram(),
  };
  const replicaMeans = new Map<number, Mean[]>();
  const meansFor = (id: number): Mean[] => {
    let ms = replicaMeans.get(id);
    if (!ms) replicaMeans.set(id, (ms = REPLICA_MEANS.map(() => new Mean())));
    return ms;
  };
  const enter = (f: ReplayFrame) => {
    for (const k of LATENCIES) hists[k].add(f[k]);
    for (const r of f.replicas) {
      const ms = meansFor(r.id);
      REPLICA_MEANS.forEach((key, i) => ms[i].add(r[key]));
    }
  };
  const leave = (f: ReplayFrame) => {
    for (const k of LATENCIES) hists[k].remove(f[k]);
    for (const r of f.replicas) {
      const ms = meansFor(r.id);
      REPLICA_MEANS.forEach((key, i) => ms[i].remove(r[key]));
    }
  };

  const out: ReplayFrame[] = new Array(n);
  for (let i = 0; i < n; i++) {
    enter(frames[i]);
    if (i >= m) leave(frames[i - m]);
    const window = frames.slice(Math.max(0, i + 1 - m), i + 1);
    const last = frames[i];

    const smoothed: ReplayFrame = { ...last, exact: {}, replicas: [] };
    for (const key of FRAME_MEANS) smoothed[key] = meanOf(window.map((f) => f[key]));
    smoothed.tierUtilization = {
      hbm: meanOf(window.map((f) => f.tierUtilization.hbm)),
      dram: meanOf(window.map((f) => f.tierUtilization.dram)),
      ssd: meanOf(window.map((f) => f.tierUtilization.ssd)),
    };
    smoothed.tierBandwidth = { dram: meanOf(window.map((f) => f.tierBandwidth.dram)), ssd: meanOf(window.map((f) => f.tierBandwidth.ssd)) };
    smoothed.gpuUtilizationP = meanFractionPercentiles(window, (f) => f.gpuUtilizationP);
    smoothed.gpuUsefulFractionP = meanFractionPercentiles(window, (f) => f.gpuUsefulFractionP);
    smoothed.kvUtilizationP = meanFractionPercentiles(window, (f) => f.kvUtilizationP);

    for (const k of LATENCIES) {
      const h = hists[k].snapshot(window, k);
      smoothed[k] = h;
      if (h.count > 0) {
        const percentile = window.map((f) => f.exact[k]?.percentile).find((p) => p !== undefined) ?? DEFAULT_EXACT_PERCENTILES;
        smoothed.exact[k] = {
          count: h.count,
          meanMs: histMean(h),
          minMs: h.min,
          maxMs: h.max,
          percentile: percentile.slice(),
          valueMs: percentile.map((p) => quantile(h, p)),
          fromMergedHistogram: true,
        };
      }
    }

    // The rows are the sample's own replicas, each with its values averaged over the window it was
    // present in; its state, presence and weight are read at the sample.
    smoothed.replicas = last.replicas.map((r) => {
      const ms = meansFor(r.id);
      const row: ReplicaSample = { ...r };
      REPLICA_MEANS.forEach((key, j) => {
        row[key] = ms[j].value();
      });
      return row;
    });
    out[i] = smoothed;
  }
  return out;
}
