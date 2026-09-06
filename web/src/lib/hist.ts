// Log-linear bucketed histogram, the same shape subscription.proto puts on the Leaf-to-Ingress hop.
//
// The stand-in stores histograms rather than percentiles for a reason that shows up in the UI:
// SLO attainment and the percentile table are then both *derived* at render time, so moving an SLO
// threshold is a view-only change that needs no re-simulation, and the panel can say so honestly.

/** 4 buckets per octave from 0.5 ms to ~2^17 ms (36 hours), 72 buckets. */
const MIN_LOG = Math.log2(0.5);
const BUCKETS_PER_OCTAVE = 4;
export const HIST_BUCKETS = 72;

export interface Histogram {
  counts: Float32Array;
  count: number;
  sum: number;
  min: number;
  max: number;
}

export function newHistogram(): Histogram {
  return { counts: new Float32Array(HIST_BUCKETS), count: 0, sum: 0, min: Infinity, max: 0 };
}

function bucketOf(v: number): number {
  if (v <= 0.5) return 0;
  const b = Math.floor((Math.log2(v) - MIN_LOG) * BUCKETS_PER_OCTAVE);
  return b < 0 ? 0 : b >= HIST_BUCKETS ? HIST_BUCKETS - 1 : b;
}

function bucketLow(b: number): number {
  return Math.pow(2, MIN_LOG + b / BUCKETS_PER_OCTAVE);
}

function bucketHigh(b: number): number {
  return bucketLow(b + 1);
}

export function record(h: Histogram, value: number, weight = 1): void {
  h.counts[bucketOf(value)] += weight;
  h.count += weight;
  h.sum += value * weight;
  if (value < h.min) h.min = value;
  if (value > h.max) h.max = value;
}

/** Bucket-wise addition. Mergeability is the whole point of this shape. */
export function merge(into: Histogram, from: Histogram): void {
  for (let i = 0; i < HIST_BUCKETS; i++) into.counts[i] += from.counts[i];
  into.count += from.count;
  into.sum += from.sum;
  into.min = Math.min(into.min, from.min);
  into.max = Math.max(into.max, from.max);
}

/** Linear interpolation inside the containing bucket. */
export function quantile(h: Histogram, p: number): number {
  if (h.count <= 0) return 0;
  const want = (p / 100) * h.count;
  let acc = 0;
  for (let b = 0; b < HIST_BUCKETS; b++) {
    const c = h.counts[b];
    if (acc + c >= want) {
      const f = c > 0 ? (want - acc) / c : 0;
      const lo = bucketLow(b);
      return lo + (bucketHigh(b) - lo) * f;
    }
    acc += c;
  }
  return h.max;
}

/** Fraction of the distribution at or below `threshold`. This is how attainment is computed. */
export function fractionBelow(h: Histogram, threshold: number): number {
  if (h.count <= 0) return 1;
  let acc = 0;
  for (let b = 0; b < HIST_BUCKETS; b++) {
    const hi = bucketHigh(b);
    const c = h.counts[b];
    if (c === 0) continue;
    if (hi <= threshold) {
      acc += c;
    } else {
      const lo = bucketLow(b);
      if (threshold > lo) acc += (c * (threshold - lo)) / (hi - lo);
      break;
    }
  }
  return acc / h.count;
}

export function histMean(h: Histogram): number {
  return h.count > 0 ? h.sum / h.count : 0;
}

/** Bucket edges and counts, for the histogram panel. */
export function bins(h: Histogram): { lo: number; hi: number; count: number }[] {
  const out: { lo: number; hi: number; count: number }[] = [];
  for (let b = 0; b < HIST_BUCKETS; b++) {
    if (h.counts[b] > 0) out.push({ lo: bucketLow(b), hi: bucketHigh(b), count: h.counts[b] });
  }
  return out;
}
