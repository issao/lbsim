// The shape every panel reads: one `Frame` per sample instant, decoded from the wire by adapter.ts
// (live and replay alike). A field the engine does not produce yet is NaN, an empty list or null,
// never a placeholder value, so a panel can render it as "not simulated yet" instead of a number.

import type { ReplicaState } from './types';
import { type Histogram, merge, newHistogram } from './hist';

/**
 * Percentiles of one fraction-valued metric across the replicas present at an instant. The wire's
 * `Distribution` in the metric's own unit (0..1), never milliseconds. `value[i]` is the
 * `percentile[i]`-th percentile.
 */
export interface FractionPercentiles {
  count: number;
  mean: number;
  min: number;
  max: number;
  percentile: number[];
  value: number[];
}

export interface ReplicaSample {
  id: number;
  present: boolean;
  /** `UNKNOWN` when the row did not carry `METRIC_REPLICA_STATE` (an older server or recording). */
  state: ReplicaState;
  weight: number;
  queuedSeqs: number;
  runningSeqs: number;
  batchSize: number;
  kvTokensResident: number;
  kvUtilization: number;
  /** Busy share of the sample window (in a step, as opposed to idle with an empty batch). */
  gpuUtilization: number;
  /** Share of busy time under the compute roofline; the complement of the wasted fraction. */
  gpuComputeBoundFraction: number;
  stepTimeMs: number;
  queueWaitMs: number;
  ttftMeanMs: number;
  itlMeanMs: number;
  prefixHitRate: number;
  admittedRps: number;
  completedRps: number;
  preemptionsPerS: number;
  trueSpeedMultiplier: number;
  telemetryStalenessMs: number;
}

export type EventKind = 'gray-failure' | 'ejected' | 'scale-up' | 'ready' | 'draining' | 'queue-overflow';

export interface FleetEvent {
  simS: number;
  kind: EventKind;
  replicaId: number;
  text: string;
  severity: 'info' | 'warning' | 'critical';
}

export interface Frame {
  tick: number;
  simS: number;
  offeredRps: number;
  admittedRps: number;
  completedRps: number;
  rejectedRps: number;
  outputTokensPerS: number;
  preemptionsPerS: number;
  loadImbalanceCv: number;
  wastedGpuFraction: number;
  readyReplicas: number;
  warmingReplicas: number;
  drainingReplicas: number;
  ejectedReplicas: number;
  kvUtilization: number;
  /** Fleet mean of the replicas' `gpuUtilization`, and its spread across them; null when unknown. */
  gpuUtilization: number;
  gpuComputeBoundFraction: number;
  gpuUtilizationP: FractionPercentiles | null;
  kvUtilizationP: FractionPercentiles | null;
  prefixHitRate: number;
  tierUtilization: { hbm: number; dram: number; ssd: number };
  tierBandwidth: { dram: number; ssd: number };
  ttft: Histogram;
  itl: Histogram;
  e2e: Histogram;
  queueWait: Histogram;
  replicas: ReplicaSample[];
  events: FleetEvent[];
}

/** Merge a window of per-sample histograms, the way Ingress merges across shards. */
export function mergeWindow(frames: Frame[], pick: (f: Frame) => Histogram): Histogram {
  const out = newHistogram();
  for (const f of frames) merge(out, pick(f));
  return out;
}
