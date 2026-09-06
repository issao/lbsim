// The mock engine.
//
// A stateful loop over sample ticks, snapshotted every SNAPSHOT_S simulated seconds. It is not
// physically accurate and does not try to be; it is arranged so that the *directions* are right,
// because the layout can only be judged if the panels move the way the real thing would:
//
//   - raising the arrival rate raises queue depth, then time-to-first-token, then loses goodput;
//   - round robin with a heavy tail produces a hotspot that walks the fleet;
//   - least-KV-tokens with stale telemetry produces a travelling wave of herding;
//   - power-of-two-choices is flat and stays flat when telemetry goes stale.
//
// The snapshot structure is not decoration either: it is what lets a physics change rewind to a
// snapshot boundary and re-simulate forward, which is the behaviour UpdateResponse describes and
// the UI has to show.

import type { ReplicaState, RewindResponse, UpdateResponse } from './types';
import type { ScenarioConfig } from './config';
import { cloneConfig, diffConfig, FIELD_LABEL } from './config';
import { clamp, coeffOfVariation, drift, lognormalQuantile, normal, uniform } from './rng';
import { type Histogram, merge, newHistogram, record } from './hist';

export const SNAPSHOT_S = 10;
/**
 * Seconds a session keeps its context resident between turns. This is what makes key-value
 * capacity bind at modest request rates: a replica serving four requests can be holding the
 * contexts of a dozen conversations, and it is the sessions rather than the batch that fill it.
 */
const PARK_S = 12;
/** Extra slots for replicas an autoscale event brings up mid-run. */
const SPARE_SLOTS = 2;
/** Deterministic quantile probes per replica when building a fleet histogram. */
const PROBES = 12;

export interface ReplicaSample {
  id: number;
  present: boolean;
  state: ReplicaState;
  weight: number;
  queuedSeqs: number;
  runningSeqs: number;
  batchSize: number;
  kvTokensResident: number;
  kvUtilization: number;
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

interface State {
  q: Float64Array;
  batch: Float64Array;
  kvPenalty: Float64Array;
  speed: Float64Array;
  state: ReplicaState[];
  present: boolean[];
  overflowAnnounced: boolean[];
}

function initialState(slots: number, replicas: number): State {
  const st: State = {
    q: new Float64Array(slots),
    batch: new Float64Array(slots),
    kvPenalty: new Float64Array(slots).fill(1),
    speed: new Float64Array(slots).fill(1),
    state: new Array(slots).fill('READY'),
    present: new Array(slots).fill(true),
    overflowAnnounced: new Array(slots).fill(false),
  };
  for (let i = replicas; i < slots; i++) {
    st.present[i] = false;
    st.state[i] = 'WARMING';
  }
  return st;
}

function copyState(s: State): State {
  return {
    q: s.q.slice(),
    batch: s.batch.slice(),
    kvPenalty: s.kvPenalty.slice(),
    speed: s.speed.slice(),
    state: s.state.slice(),
    present: s.present.slice(),
    overflowAnnounced: s.overflowAnnounced.slice(),
  };
}

/** Mean prompt and output tokens across the mixture. */
export function tokenMeans(c: ScenarioConfig): { prompt: number; output: number } {
  const p = c.workload.longProbability;
  return {
    prompt: (1 - p) * c.workload.promptMean + p * c.workload.longPromptMean,
    output: (1 - p) * c.workload.outputMean + p * c.workload.longOutputMean,
  };
}

function decodeTokensPerS(c: ScenarioConfig, batch: number): number {
  const b = Math.max(batch, 1);
  return (b * 1000) / (c.fleet.stepBaseMs + c.fleet.stepPerSeqMs * b);
}

/**
 * Nameplate: requests per second one replica retires at its maximum batch, prefill and decode
 * together. The achievable rate is lower whenever key-value capacity caps the batch below
 * `max_batch`, which is most of the time and the whole of the interesting part.
 */
export function ratedRpsPerReplica(c: ScenarioConfig): number {
  const t = tokenMeans(c);
  const costS = t.prompt / c.fleet.prefillTokensPerS + t.output / decodeTokensPerS(c, c.fleet.maxBatch);
  return 1 / costS;
}

/** What a replica can retire once the batch is capped by whatever KV is left after parked sessions. */
function achievableRps(c: ScenarioConfig, batchCap: number): number {
  const t = tokenMeans(c);
  const costS = t.prompt / c.fleet.prefillTokensPerS + t.output / decodeTokensPerS(c, Math.max(batchCap, 1));
  return 1 / costS;
}

export function ratedFleetRps(c: ScenarioConfig): number {
  return ratedRpsPerReplica(c) * c.fleet.replicas;
}

function perturbationFactor(c: ScenarioConfig, t: number): number {
  const w = c.workload;
  if (w.perturbation === 'none') return 1;
  const start = c.warmupS;
  if (t < start) return 1;
  if (w.perturbation === 'step') return 1 + w.perturbAmplitude;
  return 1 + w.perturbAmplitude * Math.sin(2 * Math.PI * w.perturbFrequencyHz * (t - start));
}

/**
 * Relative arrival share per replica, mean 1 over the replicas that can receive work.
 * This function is where the policy differences live, and therefore where most of the point is.
 */
function routingWeights(c: ScenarioConfig, t: number, st: State, out: Float64Array): void {
  const R = out.length;
  const r = c.routing;
  const seed = c.seed;
  const longScale = c.workload.longProbability / 0.08;
  const loopDelayS = (c.telemetryDelayMs + c.telemetryIntervalMs / 2) / 1000;

  for (let i = 0; i < R; i++) {
    let w = 1;
    switch (r.kind) {
      case 'round_robin': {
        // Equal request counts, unequal work. Heavy requests are dealt around the ring, so the
        // resulting hotspot walks it: dynamic 1.
        const period = 40;
        const centre = ((t / period) * R) % R;
        const d = Math.min(Math.abs(i - centre), R - Math.abs(i - centre));
        const width = Math.max(1.1, R * 0.055);
        const amp = clamp(1.5 * longScale, 0, 2.6);
        w = 1 + amp * Math.exp(-0.5 * (d / width) ** 2) + 0.09 * drift(6, t, seed, i);
        break;
      }
      case 'random':
        w = 1 + 0.42 * drift(3, t, seed + 1, i) + 0.28 * normal(seed + 7, i);
        break;
      case 'least_requests':
        // Balances the count and therefore not the load: the bias is static, because a replica
        // that drew long requests keeps them.
        w = 1 + 0.18 * drift(4, t, seed + 2, i) + 0.40 * clamp(longScale, 0, 3) * normal(seed + 11, i);
        break;
      case 'least_kv_tokens': {
        // Right unit, stale input. Every router sees the same delayed snapshot and picks the same
        // apparently-idle replica, so the load sloshes: dynamic 2.
        const f = 1 / Math.max(2 * loopDelayS, 0.4);
        const amp = clamp(0.05 + 0.55 * (loopDelayS - 0.2), 0.05, 1.2);
        w = 1 + amp * Math.cos(2 * Math.PI * f * t - (2 * Math.PI * i) / R) + 0.05 * drift(5, t, seed + 3, i);
        break;
      }
      case 'power_of_two_choices': {
        const base = (r.probeLive ? 0.045 : 0.085) * Math.sqrt(2 / clamp(r.choices, 1, 8));
        w = 1 + base * drift(2.5, t, seed + 4, i) + 0.03 * normal(seed + 13, i);
        break;
      }
      case 'prefix_affinity': {
        const sd = clamp(0.05 + 0.5 * (r.maxLoadRatio - 1), 0.04, 1.1);
        w = 1 + sd * drift(12, t, seed + 5, i) + 0.05 * drift(3, t, seed + 6, i);
        break;
      }
    }
    out[i] = st.present[i] && st.state[i] === 'READY' ? Math.max(w, 0.05) : 0;
  }

  let sum = 0;
  let n = 0;
  for (let i = 0; i < R; i++) if (out[i] > 0) { sum += out[i]; n++; }
  if (sum <= 0 || n === 0) return;
  const k = n / sum;
  for (let i = 0; i < R; i++) out[i] *= k;
}

function prefixHitRate(c: ScenarioConfig, t: number, i: number): number {
  if (c.routing.kind === 'prefix_affinity') {
    return clamp(0.72 - 0.18 * (c.routing.maxLoadRatio - 1) + 0.04 * drift(7, t, c.seed + 21, i), 0.1, 0.95);
  }
  const base = c.routing.kind === 'round_robin' || c.routing.kind === 'random' ? 0.07 : 0.11;
  return clamp(base + 0.03 * drift(7, t, c.seed + 21, i), 0.01, 0.5);
}

/** Deterministic fleet events, so cluster health has something honest to draw. */
function scheduleFor(c: ScenarioConfig): {
  grayAt: number; grayId: number; ejectAt: number; scaleAt: number; readyAt: number;
  drainAt: number; drainId: number;
} {
  const R = c.fleet.replicas;
  const grayId = Math.floor(uniform(c.seed, 101) * R);
  let drainId = Math.floor(uniform(c.seed, 103) * R);
  if (drainId === grayId) drainId = (drainId + 1) % R;
  return { grayAt: 52, grayId, ejectAt: 71, scaleAt: 88, readyAt: 116, drainAt: 100, drainId };
}

export class MockEngine {
  config: ScenarioConfig;
  readonly dtS: number;
  readonly slots: number;
  frames: Frame[] = [];
  /** Simulated seconds recorded so far. Scrubbing inside this is a log read. */
  recordedToS = 0;
  private snapshots = new Map<number, State>();
  private state: State;
  private weights: Float64Array;
  private wasted = 0;
  private totalGpuS = 0;

  constructor(config: ScenarioConfig) {
    this.config = cloneConfig(config);
    this.dtS = 1 / 2; // physics step; independent of the chart sample rate
    this.slots = this.config.fleet.replicas + SPARE_SLOTS;
    this.state = initialState(this.slots, this.config.fleet.replicas);
    this.weights = new Float64Array(this.slots);
    this.snapshots.set(0, copyState(this.state));
  }

  get ticksTotal(): number {
    return Math.floor(this.config.durationS / this.dtS);
  }

  /** Extend the recording to at least `toS` simulated seconds. Returns whether it did work. */
  simulateTo(toS: number): boolean {
    const target = Math.min(Math.ceil(toS / this.dtS), this.ticksTotal);
    if (target <= this.frames.length) return false;
    while (this.frames.length < target) this.stepOnce();
    this.recordedToS = this.frames.length * this.dtS;
    return true;
  }

  frameAt(simS: number): Frame | undefined {
    const i = clamp(Math.floor(simS / this.dtS), 0, Math.max(this.frames.length - 1, 0));
    return this.frames[i];
  }

  /** Frames covering [fromS, toS], decimated to the configured chart sample rate. */
  window(fromS: number, toS: number): Frame[] {
    const stride = Math.max(1, Math.round(1 / (this.config.samplesPerSimSecond * this.dtS)));
    const a = clamp(Math.floor(fromS / this.dtS), 0, this.frames.length);
    const b = clamp(Math.ceil(toS / this.dtS), 0, this.frames.length);
    const out: Frame[] = [];
    for (let i = a; i < b; i += stride) out.push(this.frames[i]);
    const last = this.frames[b - 1];
    if (last && out[out.length - 1] !== last) out.push(last);
    return out;
  }

  eventsUpTo(simS: number): FleetEvent[] {
    const out: FleetEvent[] = [];
    for (const f of this.frames) {
      if (f.simS > simS) break;
      for (const e of f.events) out.push(e);
    }
    return out;
  }

  /** ingress.proto Rewind. `fromLog` is false when the target is past the recording. */
  rewind(toS: number): RewindResponse {
    const beyond = toS > this.recordedToS + 1e-9;
    if (beyond) {
      this.simulateTo(toS);
      return { simTimeS: Math.min(toS, this.recordedToS), fromLog: false, restoredFromSnapshotS: this.recordedToS };
    }
    return { simTimeS: toS, fromLog: true, restoredFromSnapshotS: toS };
  }

  /**
   * ingress.proto UpdateWorkload / UpdatePolicies, collapsed into one call because the stand-in
   * edits one config object. A physics change rewinds to the last snapshot at or before `atS` and
   * re-simulates; a view-only change does not.
   */
  applyConfig(next: ScenarioConfig, atS: number): UpdateResponse {
    const d = diffConfig(this.config, next);
    const changed = d.paths.map((p) => FIELD_LABEL[p] ?? p);
    if (d.paths.length === 0) {
      return { accepted: true, requiredResimulation: false, rewoundToS: atS, rejectedReason: '', changed };
    }
    if (next.fleet.replicas !== this.config.fleet.replicas) {
      // Fleet size changes the slot count, so this stand-in restarts rather than pretending a
      // snapshot from a differently shaped fleet can be restored.
      this.config = cloneConfig(next);
      this.reset();
      this.simulateTo(atS);
      return { accepted: true, requiredResimulation: true, rewoundToS: 0, rejectedReason: '', changed };
    }
    this.config = cloneConfig(next);
    if (d.physicsPaths.length === 0) {
      return { accepted: true, requiredResimulation: false, rewoundToS: atS, rejectedReason: '', changed };
    }
    const snapS = Math.max(0, Math.floor(atS / SNAPSHOT_S) * SNAPSHOT_S);
    const snap = this.snapshots.get(snapS);
    const keepTicks = Math.floor(snapS / this.dtS);
    this.frames.length = Math.min(this.frames.length, keepTicks);
    this.state = snap ? copyState(snap) : initialState(this.slots, this.config.fleet.replicas);
    for (const key of [...this.snapshots.keys()]) if (key > snapS) this.snapshots.delete(key);
    this.recordedToS = snapS;
    this.simulateTo(Math.max(atS, snapS + this.dtS));
    return { accepted: true, requiredResimulation: true, rewoundToS: snapS, rejectedReason: '', changed };
  }

  reset(): void {
    this.frames = [];
    this.snapshots.clear();
    this.state = initialState(this.slots, this.config.fleet.replicas);
    this.snapshots.set(0, copyState(this.state));
    this.recordedToS = 0;
    this.wasted = 0;
    this.totalGpuS = 0;
  }

  // -------------------------------------------------------------------------
  // One physics step
  // -------------------------------------------------------------------------
  private stepOnce(): void {
    const c = this.config;
    const st = this.state;
    const dt = this.dtS;
    const tick = this.frames.length;
    const t = tick * dt;
    const R = this.slots;
    const tk = tokenMeans(c);
    const sched = scheduleFor(c);
    const events: FleetEvent[] = [];

    // --- lifecycle, from the deterministic schedule ------------------------
    if (crossed(t, dt, sched.grayAt) && sched.grayId < R) {
      st.speed[sched.grayId] = 0.28;
      events.push({
        simS: t, kind: 'gray-failure', replicaId: sched.grayId, severity: 'critical',
        text: `replica ${sched.grayId} degraded to 0.28x true speed, still announcing healthy`,
      });
    }
    if (crossed(t, dt, sched.ejectAt) && sched.grayId < R) {
      st.state[sched.grayId] = 'EJECTED';
      events.push({
        simS: t, kind: 'ejected', replicaId: sched.grayId, severity: 'warning',
        text: `replica ${sched.grayId} ejected from routing, ${(sched.ejectAt - sched.grayAt).toFixed(0)} s after onset`,
      });
    }
    if (crossed(t, dt, sched.scaleAt)) {
      for (let i = c.fleet.replicas; i < R; i++) { st.present[i] = true; st.state[i] = 'WARMING'; }
      events.push({
        simS: t, kind: 'scale-up', replicaId: c.fleet.replicas, severity: 'info',
        text: `+${SPARE_SLOTS} replicas requested, cold start ${sched.readyAt - sched.scaleAt} s`,
      });
    }
    if (crossed(t, dt, sched.readyAt)) {
      for (let i = c.fleet.replicas; i < R; i++) if (st.present[i]) st.state[i] = 'READY';
      events.push({ simS: t, kind: 'ready', replicaId: c.fleet.replicas, severity: 'info', text: `warm replicas ready` });
    }
    if (crossed(t, dt, sched.drainAt) && sched.drainId < R) {
      st.state[sched.drainId] = 'DRAINING';
      events.push({
        simS: t, kind: 'draining', replicaId: sched.drainId, severity: 'info',
        text: `replica ${sched.drainId} announced draining`,
      });
    }

    // --- offered load ------------------------------------------------------
    const offered = Math.max(
      0,
      c.workload.arrivalRps * perturbationFactor(c, t) * (1 + 0.035 * drift(4, t, c.seed + 31))
    );
    routingWeights(c, t, st, this.weights);

    let activeCount = 0;
    for (let i = 0; i < R; i++) if (this.weights[i] > 0) activeCount++;
    const perReplicaOffered = activeCount > 0 ? offered / activeCount : 0;

    const ttft = newHistogram();
    const itl = newHistogram();
    const e2e = newHistogram();
    const queueWait = newHistogram();
    const samples: ReplicaSample[] = [];
    const loadForCv: number[] = [];

    let admitted = 0;
    let completed = 0;
    let rejected = 0;
    let preemptions = 0;
    let kvSum = 0;
    let kvN = 0;
    let hitSum = 0;
    let dramTokens = 0;
    let ssdTokens = 0;
    let stepGpuS = 0;
    let stepWastedS = 0;

    for (let i = 0; i < R; i++) {
      const present = st.present[i];
      const state = st.state[i];
      const receiving = present && state === 'READY';
      const arrivals = receiving ? perReplicaOffered * this.weights[i] * dt : 0;

      // Key-value capacity, not compute, is usually what caps the batch: parked sessions hold
      // context between turns, and what is left over is what can be decoding at once.
      const hitRate = prefixHitRate(c, t, i);
      const ctxTokens = tk.prompt * (1 - hitRate * 0.6) + tk.output / 2;
      const parkedSeqs = c.workload.longProbability * (arrivals / dt) * PARK_S;
      const kvBudgetSeqs = c.fleet.kvTokensPerReplica / Math.max(ctxTokens, 1);
      const batchCap = clamp(kvBudgetSeqs - parkedSeqs, 1, c.fleet.maxBatch);

      const capRps = achievableRps(c, batchCap) * st.speed[i] * st.kvPenalty[i] * (state === 'DRAINING' ? 0.6 : 1);
      const canServe = present && state !== 'EJECTED' && state !== 'WARMING' ? capRps * dt : 0;

      let q = st.q[i] + arrivals;
      const done = Math.min(q, canServe);
      q -= done;
      let overflow = 0;
      if (q > c.fleet.maxQueue) {
        overflow = q - c.fleet.maxQueue;
        q = c.fleet.maxQueue;
        if (!st.overflowAnnounced[i] && overflow > 0.5) {
          st.overflowAnnounced[i] = true;
          events.push({
            simS: t, kind: 'queue-overflow', replicaId: i, severity: 'warning',
            text: `replica ${i} queue hit max_queue=${c.fleet.maxQueue}, shedding at the door`,
          });
        }
      }
      st.q[i] = q;

      const throughputRps = done / dt;
      const serviceS = (tk.output * (c.fleet.stepBaseMs + c.fleet.stepPerSeqMs * Math.max(st.batch[i], 1))) / 1000
        / Math.max(st.speed[i], 0.05);
      const wantBatch = clamp(throughputRps * serviceS, 0, batchCap);
      st.batch[i] += (wantBatch - st.batch[i]) * 0.45;
      const batch = st.batch[i];

      const kvTokens = (batch + parkedSeqs) * ctxTokens;
      const rawKv = kvTokens / c.fleet.kvTokensPerReplica;
      const kvUtil = clamp(rawKv, 0, 1);
      const excess = Math.max(0, rawKv - 0.96);
      const preemptRate = excess > 0 ? excess * 14 * batch * 0.06 : 0;
      st.kvPenalty[i] = clamp(1 / (1 + 1.8 * excess), 0.25, 1);
      if (excess > 0) {
        dramTokens += kvTokens * clamp(excess * 1.5, 0, 0.6);
        ssdTokens += kvTokens * clamp(excess * 0.4, 0, 0.2);
      }

      const stepMs = (c.fleet.stepBaseMs + c.fleet.stepPerSeqMs * batch) / Math.max(st.speed[i], 0.05);
      const queueWaitMs = (q / Math.max(capRps, 0.02)) * 1000;
      const prefillContention = 1 + 0.6 * (batch / c.fleet.maxBatch);
      const prefillMs = (tk.prompt * (1 - hitRate * 0.8) / c.fleet.prefillTokensPerS) * 1000 * prefillContention
        / Math.max(st.speed[i], 0.05);
      const ttftMs = queueWaitMs + prefillMs;
      // Prefill and decode share the step budget, so admitting fast costs everyone already
      // decoding. This is the interference in dynamic 3, expressed as a multiplier.
      const prefillShare = clamp((arrivals / dt) * tk.prompt / c.fleet.prefillTokensPerS, 0, 0.95);
      const itlMs = stepMs * (1 + 0.9 * prefillShare) * (1 + (preemptRate > 0 ? 1.4 : 0));
      const e2eMs = ttftMs + itlMs * tk.output;

      // Fleet histograms: mix each replica's within-replica spread, weighted by its arrival share.
      if (receiving || q > 0) {
        const wgt = Math.max(arrivals, 1e-3);
        const cvTtft = clamp(0.45 * c.workload.promptCv + 0.25 + 0.014 * q, 0.4, 2.4);
        const cvItl = clamp(0.2 + 0.35 * (batch / c.fleet.maxBatch), 0.15, 1.2);
        const cvE2e = clamp(0.35 + 0.65 * c.workload.outputCv, 0.4, 2.6);
        const cvQ = clamp(0.6 + 0.02 * q, 0.5, 2.5);
        for (let k = 0; k < PROBES; k++) {
          const p = (k + 0.5) / PROBES;
          record(ttft, lognormalQuantile(Math.max(ttftMs, 1), cvTtft, p), wgt / PROBES);
          record(itl, lognormalQuantile(Math.max(itlMs, 0.5), cvItl, p), wgt / PROBES);
          record(e2e, lognormalQuantile(Math.max(e2eMs, 1), cvE2e, p), wgt / PROBES);
          record(queueWait, lognormalQuantile(Math.max(queueWaitMs, 0.5), cvQ, p), wgt / PROBES);
        }
      }

      if (present && state !== 'EJECTED') {
        const busy = clamp((batch / c.fleet.maxBatch) * 0.85 + (q > 0 ? 0.15 : 0), 0, 1);
        stepGpuS += busy * dt;
        stepWastedS += busy * dt * clamp(excess * 1.2, 0, 0.5) + (overflow > 0 ? busy * dt * 0.1 : 0);
        kvSum += kvUtil;
        kvN++;
        hitSum += hitRate;
        loadForCv.push(batch * ctxTokens);
      }

      admitted += done / dt;
      completed += done / dt;
      rejected += overflow / dt;
      preemptions += preemptRate;

      samples.push({
        id: i,
        present,
        state,
        weight: this.weights[i],
        queuedSeqs: q,
        runningSeqs: batch,
        batchSize: batch,
        kvTokensResident: kvTokens,
        kvUtilization: kvUtil,
        stepTimeMs: stepMs,
        queueWaitMs,
        ttftMeanMs: ttftMs,
        itlMeanMs: itlMs,
        prefixHitRate: hitRate,
        admittedRps: arrivals / dt,
        completedRps: done / dt,
        preemptionsPerS: preemptRate,
        trueSpeedMultiplier: st.speed[i],
        telemetryStalenessMs: c.telemetryDelayMs + c.telemetryIntervalMs * uniform(c.seed, i, tick),
      });
    }

    this.totalGpuS += stepGpuS;
    this.wasted += stepWastedS;

    let ready = 0, warming = 0, draining = 0, ejected = 0;
    for (let i = 0; i < R; i++) {
      if (!st.present[i]) continue;
      if (st.state[i] === 'READY') ready++;
      else if (st.state[i] === 'WARMING') warming++;
      else if (st.state[i] === 'DRAINING') draining++;
      else ejected++;
    }

    const kvUtilFleet = kvN > 0 ? kvSum / kvN : 0;
    const frame: Frame = {
      tick,
      simS: t,
      offeredRps: offered,
      admittedRps: admitted,
      completedRps: completed,
      rejectedRps: rejected,
      outputTokensPerS: completed * tk.output,
      preemptionsPerS: preemptions,
      loadImbalanceCv: coeffOfVariation(loadForCv),
      wastedGpuFraction: this.totalGpuS > 0 ? clamp(this.wasted / this.totalGpuS, 0, 1) : 0,
      readyReplicas: ready,
      warmingReplicas: warming,
      drainingReplicas: draining,
      ejectedReplicas: ejected,
      kvUtilization: kvUtilFleet,
      prefixHitRate: kvN > 0 ? hitSum / kvN : 0,
      tierUtilization: {
        hbm: kvUtilFleet,
        dram: clamp(dramTokens / (c.fleet.kvTokensPerReplica * Math.max(kvN, 1) * 0.5), 0, 1),
        ssd: clamp(ssdTokens / (c.fleet.kvTokensPerReplica * Math.max(kvN, 1) * 2), 0, 1),
      },
      tierBandwidth: {
        dram: clamp(dramTokens / 1.2e6, 0, 1),
        ssd: clamp(ssdTokens / 3e5, 0, 1),
      },
      ttft,
      itl,
      e2e,
      queueWait,
      replicas: samples,
      events,
    };
    this.frames.push(frame);

    const nextT = (tick + 1) * dt;
    if (Math.abs(nextT % SNAPSHOT_S) < 1e-9) this.snapshots.set(nextT, copyState(st));
  }
}

function crossed(t: number, dt: number, at: number): boolean {
  return t >= at && t - dt < at;
}

/** Merge a window of per-tick histograms, the way Ingress merges across shards. */
export function mergeWindow(frames: Frame[], pick: (f: Frame) => Histogram): Histogram {
  const out = newHistogram();
  for (const f of frames) merge(out, pick(f));
  return out;
}
