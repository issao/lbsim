// U52: which Frame / ReplicaSample fields the engine actually populates from real telemetry today
// (decided by U17 and U23), and whether a given frame + the fields a panel reads add up to real
// data, invented data, or a mix. This is the only source of truth for the mock tag: a panel that
// reads only wired fields off a wire frame has earned the right to stop calling itself mock.
//
// "Wired" is a property of the pipeline, not of any one frame: it lists the fields the engine sets
// from measured telemetry today. A mock frame invents plausible values for the same field names, so
// wired-ness alone can't tell mock from real -- that's what isWireFrame is for.

import type { Frame, ReplicaSample } from './engine';
import { DATA_SOURCE_LABEL, type DataMode } from './mode';

/** Frame fields the engine populates from real telemetry on a wire frame. Everything else is NaN or empty. */
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
 * ReplicaSample fields the engine populates from real telemetry on a wire frame. `state` (the
 * announced health state MachineLevel and ClusterHealth's gray-failure tile read against true
 * speed) is deliberately absent: per docs/dashboard-plan.md section 3 it is still invented, so a
 * panel reading it stays 'partial' rather than wrongly earning 'real'.
 */
export const WIRED_REPLICA_FIELDS: ReadonlySet<keyof ReplicaSample> = new Set<keyof ReplicaSample>([
  'id',
  'present',
  'queuedSeqs',
  'runningSeqs',
  'batchSize',
  'kvTokensResident',
  'kvUtilization',
  'gpuUtilization',
  'gpuComputeBoundFraction',
  'stepTimeMs',
]);

/** A frame that came from the wire (replay or live) carries `simTimeUnixNs`; a mock frame never does. */
export function isWireFrame(f: Frame): boolean {
  return 'simTimeUnixNs' in f;
}

export interface Realness {
  kind: 'mock' | 'partial' | 'real';
  /** Names of the fields this panel reads that are not wired -- empty unless kind is 'partial'. */
  mockFields: string[];
}

/**
 * What a panel may honestly claim about the frame it is drawing, given the specific Frame and
 * ReplicaSample fields it reads. A mock frame is always 'mock', regardless of which fields a panel
 * happens to read: the frame was never touched by the engine, so there is nothing to be partial about.
 * On a wire frame, a panel is 'real' only if every field it reads is wired; otherwise 'partial', naming
 * the unwired fields so the tag's tooltip says exactly what part of the panel is still invented.
 */
export function realness(
  frame: Frame,
  reads: readonly (keyof Frame)[],
  replicaReads: readonly (keyof ReplicaSample)[] = []
): Realness {
  if (!isWireFrame(frame)) return { kind: 'mock', mockFields: [] };
  const mockFields: string[] = [];
  for (const field of reads) {
    if (!WIRED_FRAME_FIELDS.has(field)) mockFields.push(field as string);
  }
  for (const field of replicaReads) {
    if (!WIRED_REPLICA_FIELDS.has(field)) mockFields.push(field as string);
  }
  return mockFields.length === 0 ? { kind: 'real', mockFields: [] } : { kind: 'partial', mockFields };
}

/**
 * U70/U95: the word a panel's tag should show, given what it may honestly claim and the active
 * data source. Unknown (no `Realness` computed yet) or a genuinely mock frame always says `mock`.
 * A partial or real panel on a live or replay run borrows the mode's own word -- a partial panel
 * is not lying about the run, only about a handful of columns on it, and that exception belongs
 * in the tag's title and the panel's inline note (see `partialNote`), not in the top-line word.
 * In mock mode this collapses back to the old behaviour: `DATA_SOURCE_LABEL.mock` is itself `mock`.
 */
export function panelTagWord(data: Realness | undefined, mode: DataMode): 'mock' | 'replay' | 'live' {
  if (!data || data.kind === 'mock') return 'mock';
  return DATA_SOURCE_LABEL[mode];
}

/** Human label for every `Frame` and `ReplicaSample` field a panel might name in a mock/partial tag. */
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

/**
 * U95: the sentence a partial panel shows in its body and in its tag's title, when the panel is
 * otherwise honestly claiming the run's own word (live or replay). "values" rather than "columns"
 * because the same note serves a table's columns and a chart's series alike.
 */
export function partialNote(word: 'live' | 'replay', fields: readonly string[]): string {
  const n = fields.length;
  const labels = fields.map(fieldLabel).join(', ');
  return `${word} — ${n} ${n === 1 ? 'value' : 'values'} not simulated yet, shown as placeholders: ${labels}`;
}
