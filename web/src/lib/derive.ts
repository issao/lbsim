// Everything the panels compute from recorded frames rather than from the physics.
//
// Keeping this separate is the point of the exercise: attainment and goodput are derived here, so
// moving an SLO threshold changes the charts without touching the engine, and the UI can say
// truthfully that no re-simulation happened.

import type { FractionPercentiles, Frame } from './frame';
import type { ScenarioConfig, Slo } from './config';
import { fractionBelow, type Histogram, quantile } from './hist';

export const WINDOW_S = 60;

export function windowFrames(engine: { window: (a: number, b: number) => Frame[] }, cursorS: number): Frame[] {
  return engine.window(Math.max(0, cursorS - WINDOW_S), cursorS);
}

/**
 * Joint attainment: within the TTFT target and within the inter-token target. Treated as
 * independent, which it is not; the engine's own `METRIC_SLO_ATTAINMENT` counts requests that met
 * both, and the headline should move to it.
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

/**
 * Percentiles of a fraction across replicas by sorting, nearest rank. This is the wire's
 * `distributions["67"]` recomputed client-side, for a replay frame that carries per-replica rows
 * but predates the fleet distribution. Non-finite entries are absent replicas
 * and are dropped; null when nothing is left, so the chart draws a gap rather than a zero.
 */
export function percentilesOver(values: number[], ps: number[]): FractionPercentiles | null {
  const xs = values.filter(Number.isFinite).sort((a, b) => a - b);
  if (xs.length === 0) return null;
  const at = (p: number): number => xs[Math.min(xs.length - 1, Math.max(0, Math.ceil((p / 100) * xs.length) - 1))];
  return {
    count: xs.length,
    mean: xs.reduce((a, b) => a + b, 0) / xs.length,
    min: xs[0],
    max: xs[xs.length - 1],
    percentile: ps.slice(),
    value: ps.map(at),
  };
}

/** The `p`-th percentile per frame from a `FractionPercentiles`, NaN (a gap) where the frame has none or lacks `p`. */
export function fractionPercentileSeries(frames: Frame[], pick: (f: Frame) => FractionPercentiles | null, p: number): number[] {
  return frames.map((f) => {
    const fp = pick(f);
    if (!fp) return NaN;
    const i = fp.percentile.indexOf(p);
    return i === -1 ? NaN : fp.value[i];
  });
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
  // Gray failure is read off the per-replica rows; a frame without them (a live fleet frame, an
  // older recording) has nothing to count, and NaN says so rather than a reassuring zero.
  const rows = f.replicas.filter((r) => r.present && r.state !== 'UNKNOWN');
  let gray = rows.length === 0 ? NaN : 0;
  for (const r of rows) if (r.state === 'DEGRADED' || (r.state === 'READY' && r.trueSpeedMultiplier < 0.9)) gray++;
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
