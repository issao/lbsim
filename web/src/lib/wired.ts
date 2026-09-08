// Which Frame / ReplicaSample fields the engine populates from measured telemetry today. This is
// the only source of truth for the "—" rule: a panel reading a field outside these sets renders
// `Unwired` ("—", hover "not simulated yet") rather than a number, and a table never sorts by one.

import type { Frame, ReplicaSample } from './frame';

/** Frame fields the engine populates from real telemetry. Everything else is NaN or empty. */
export const WIRED_FRAME_FIELDS: ReadonlySet<keyof Frame> = new Set<keyof Frame>([
  'offeredRps',
  'admittedRps',
  'completedRps',
  'rejectedRps',
  'outputTokensPerS',
  'loadImbalanceCv',
  'readyReplicas',
  'kvUtilization',
  'gpuUtilization',
  'gpuComputeBoundFraction',
  'gpuUtilizationP',
  'kvUtilizationP',
  'ttft',
  'itl',
  'e2e',
  'queueWait',
  'replicas',
]);

/**
 * ReplicaSample fields the engine populates from real telemetry. `state`, `trueSpeedMultiplier`
 * and `ttftMeanMs` arrived with `METRIC_REPLICA_STATE`, `METRIC_TRUE_SPEED_MULTIPLIER` and the
 * replica-scope `METRIC_TTFT` distribution; a row from an older server or recording carries
 * `UNKNOWN` / NaN there, which the table renders as "—" like any absent value.
 */
export const WIRED_REPLICA_FIELDS: ReadonlySet<keyof ReplicaSample> = new Set<keyof ReplicaSample>([
  'id',
  'present',
  'state',
  'queuedSeqs',
  'runningSeqs',
  'batchSize',
  'kvTokensResident',
  'kvUtilization',
  'gpuUtilization',
  'gpuComputeBoundFraction',
  'stepTimeMs',
  'ttftMeanMs',
  'trueSpeedMultiplier',
]);

/** Human label for every `Frame` and `ReplicaSample` field an `Unwired` hover might name. */
export const FIELD_LABEL: Record<string, string> = {
  // Frame
  tick: 'tick',
  simS: 'simulated time',
  offeredRps: 'offered RPS',
  admittedRps: 'admitted RPS',
  completedRps: 'completed RPS',
  rejectedRps: 'rejected RPS',
  outputTokensPerS: 'output tokens per second',
  preemptionsPerS: 'preemptions per second',
  loadImbalanceCv: 'load imbalance (CV)',
  wastedGpuFraction: 'wasted GPU fraction',
  readyReplicas: 'ready replica count',
  warmingReplicas: 'warming count',
  drainingReplicas: 'draining count',
  ejectedReplicas: 'ejected count',
  kvUtilization: 'KV utilization',
  kvUtilizationP: 'KV utilization percentiles',
  gpuUtilization: 'GPU utilization',
  gpuUtilizationP: 'GPU utilization percentiles',
  gpuComputeBoundFraction: 'compute-bound fraction',
  prefixHitRate: 'prefix hit rate',
  tierUtilization: 'memory-tier occupancy',
  tierBandwidth: 'tier bandwidth',
  ttft: 'TTFT',
  itl: 'ITL',
  e2e: 'end-to-end latency',
  queueWait: 'queue wait',
  replicas: 'replica samples',
  events: 'events',
  // ReplicaSample
  id: 'replica id',
  present: 'presence',
  state: 'replica state',
  weight: 'weight',
  queuedSeqs: 'queued sequences',
  runningSeqs: 'running sequences',
  batchSize: 'batch size',
  kvTokensResident: 'KV tokens resident',
  stepTimeMs: 'step time',
  queueWaitMs: 'queue wait',
  ttftMeanMs: 'TTFT mean',
  itlMeanMs: 'ITL mean',
  trueSpeedMultiplier: 'speed multiplier',
  telemetryStalenessMs: 'telemetry staleness',
};

/** `FIELD_LABEL[id]`, falling back to splitting an unlisted camelCase identifier into words. */
export function fieldLabel(id: string): string {
  const known = FIELD_LABEL[id];
  if (known) return known;
  return id
    .replace(/([a-z0-9])([A-Z])/g, '$1 $2')
    .replace(/([A-Z]+)([A-Z][a-z])/g, '$1 $2')
    .toLowerCase();
}
