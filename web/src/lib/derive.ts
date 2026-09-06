// Everything the panels compute from recorded frames rather than from the physics.
//
// Keeping this separate is the point of the exercise: attainment and goodput are derived here, so
// moving an SLO threshold changes the charts without touching the engine, and the UI can say
// truthfully that no re-simulation happened.

import type { Frame } from './engine';
import type { ScenarioConfig, Slo } from './config';
import { fractionBelow, type Histogram, quantile } from './hist';

export const WINDOW_S = 60;

export function windowFrames(engine: { window: (a: number, b: number) => Frame[] }, cursorS: number): Frame[] {
  return engine.window(Math.max(0, cursorS - WINDOW_S), cursorS);
}

/**
 * Joint attainment: within the TTFT target and within the inter-token target. Treated as
 * independent, which it is not; the real engine counts requests that met both. Marked as mock
 * everywhere it is shown, like everything else here.
 */
export function attainment(f: Frame, slo: Slo): number {
  return fractionBelow(f.ttft, slo.ttftMs) * fractionBelow(f.itl, slo.itlMs) * fractionBelow(f.e2e, slo.e2eS * 1000);
}

/** Tokens per second delivered within SLO. The headline number. */
export function goodput(f: Frame, slo: Slo): number {
  return f.outputTokensPerS * attainment(f, slo);
}

export function series(frames: Frame[], pick: (f: Frame) => number): number[] {
  return frames.map(pick);
}

export function percentileSeries(frames: Frame[], pick: (f: Frame) => Histogram, p: number): number[] {
  return frames.map((f) => quantile(pick(f), p));
}

export function xs(frames: Frame[]): number[] {
  return frames.map((f) => f.simS);
}

export interface HealthCounts {
  ready: number;
  warming: number;
  draining: number;
  ejected: number;
  gray: number;
}

export function healthCounts(f: Frame): HealthCounts {
  let gray = 0;
  for (const r of f.replicas) if (r.present && r.state === 'READY' && r.trueSpeedMultiplier < 0.9) gray++;
  return {
    ready: f.readyReplicas,
    warming: f.warmingReplicas,
    draining: f.drainingReplicas,
    ejected: f.ejectedReplicas,
    gray,
  };
}

export type ReplicaColumn =
  | 'id'
  | 'state'
  | 'queuedSeqs'
  | 'kvTokensResident'
  | 'batchSize'
  | 'stepTimeMs'
  | 'prefixHitRate'
  | 'ttftMeanMs'
  | 'weight';

export const REPLICA_COLUMNS: { key: ReplicaColumn; label: string; metric?: string }[] = [
  { key: 'id', label: 'replica' },
  { key: 'state', label: 'state' },
  { key: 'queuedSeqs', label: 'queue', metric: 'METRIC_QUEUED_SEQS' },
  { key: 'kvTokensResident', label: 'resident kv', metric: 'METRIC_KV_TOKENS_RESIDENT' },
  { key: 'batchSize', label: 'batch', metric: 'METRIC_BATCH_SIZE' },
  { key: 'stepTimeMs', label: 'step', metric: 'METRIC_STEP_TIME' },
  { key: 'prefixHitRate', label: 'prefix hit', metric: 'METRIC_PREFIX_HIT_RATE' },
  { key: 'ttftMeanMs', label: 'ttft mean', metric: 'METRIC_TTFT' },
];

/** Whether the fleet is past the knee, for a status colour that is not guesswork. */
export function loadStatus(f: Frame, c: ScenarioConfig): 'good' | 'serious' | 'critical' {
  const a = attainment(f, c.slo);
  if (a < 0.8) return 'critical';
  if (a < 0.97) return 'serious';
  return 'good';
}
