// U52: which Frame / ReplicaSample fields the engine actually populates from real telemetry today
// (decided by U17 and U23), and whether a given frame + the fields a panel reads add up to real
// data, invented data, or a mix. This is the only source of truth for the mock tag: a panel that
// reads only wired fields off a wire frame has earned the right to stop calling itself mock.
//
// "Wired" is a property of the pipeline, not of any one frame: it lists the fields the engine sets
// from measured telemetry today. A mock frame invents plausible values for the same field names, so
// wired-ness alone can't tell mock from real -- that's what isWireFrame is for.

import type { Frame, ReplicaSample } from './engine';

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
  'ttft',
  'itl',
  'e2e',
  'queueWait',
  'replicas',
]);

/** ReplicaSample fields the engine populates from real telemetry on a wire frame. */
export const WIRED_REPLICA_FIELDS: ReadonlySet<keyof ReplicaSample> = new Set<keyof ReplicaSample>([
  'id',
  'present',
  'state',
  'queuedSeqs',
  'runningSeqs',
  'batchSize',
  'kvTokensResident',
  'kvUtilization',
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
