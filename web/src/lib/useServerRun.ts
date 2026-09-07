// The run controller, backed by the Ingress server instead of the in-browser mock.
//
// `ServerRunEngine` is the whole controller with no React in it: it starts a run, subscribes to the
// fleet stream, decodes each update into the same `ReplayFrame` replay decodes from `fleet.jsonl`
// (through the same `frameFromUpdate`, so live and replay agree by construction), and answers the
// three reads the panels make (`frameAt`, `window`, `eventsUpTo`) over those frames. `useServerRun`
// is a thin hook over it, shaped to `RunHandle` so wiring a panel is a swap of the hook rather than
// a rewrite of the panel. The self-test drives the engine directly against a fake server.
//
// Two fields differ from the mock's, and pretending otherwise would be the dishonesty this whole
// exercise is trying to avoid:
//
//   - `update` returns `null` rather than an `UpdateResponse`. A round trip decides the answer, so it
//     arrives on `lastUpdate` later, exactly as `lastRewind` already does.
//   - `rewindTo` is refused and named: the first server has no Rewind. `scrubTo` pins the cursor
//     inside what has already streamed, which is a local read and needs no server.
//
// Everything else -- cursor, playback, step, restart, the banners the UI is obliged to show -- is
// the same shape and the same units (relative simulated seconds) as the mock's.

import { useEffect, useMemo, useReducer, useRef, useState } from 'react';
import type { FrameSource, RunHandle, RunSourceInfo } from './useRun';
import { STEP_S } from './useRun';
import type { FleetEvent, Frame } from './engine';
import type { ScenarioConfig } from './config';
import { cloneConfig, diffConfig, FIELD_LABEL } from './config';
import type { RewindResponse, Target as UiTarget, UpdateResponse } from './types';
import { frameFromUpdate, type ReplayFrame } from './adapter';
import {
  IngressClient,
  IngressError,
  type MetricName,
  type RunStatus,
  type StreamPhase,
  type SubscriptionHandle,
  type WireDistribution,
  fleetTarget,
  relSeconds,
  scenarioConfigToWire,
  scenarioEnvelope,
  secondsToNs,
  subscribeToTarget,
  policiesToWire,
  toOverrides,
  workloadToWire,
  uiTargetToWire,
} from './api';
import { modeBanner, serverMode } from './mode';
import { LEASE_MS } from './subscriptions';

/** How often the run's status is polled. The stream carries metrics; this carries state and speed. */
export const STATUS_POLL_MS = 500;
/** Bounded history, because a long run at a fine sample rate is otherwise unbounded browser memory. */
export const MAX_SAMPLES = 4000;
/** Why rewind is refused. Constant, so a panel can show it without importing the transport. */
export const SERVER_DISABLED_REASON = 'the server does not support Rewind yet';
/** What `lastUpdate` says when the server answers 501: the control is wired, the server is not. */
export const SERVER_NOT_YET = 'the server does not support this yet';
/** The lease asked for, from the stand-in registry's figure so the status bar's number stays true. */
export const LEASE_NS = BigInt(LEASE_MS) * 1_000_000n;

/** The fleet metrics the dashboard's top-level panels read. One subscription, one entity. */
export const FLEET_METRICS: MetricName[] = [
  'METRIC_OFFERED_RPS', 'METRIC_ADMITTED_RPS', 'METRIC_COMPLETED_RPS', 'METRIC_REJECTED_RPS',
  'METRIC_OUTPUT_TOKENS_PER_S', 'METRIC_GOODPUT_TOKENS_PER_S', 'METRIC_PREEMPTIONS_PER_S',
  'METRIC_RETRIES_PER_S', 'METRIC_KV_UTILIZATION', 'METRIC_RUNNING_SEQS', 'METRIC_QUEUED_SEQS',
  'METRIC_LOAD_IMBALANCE_CV', 'METRIC_WASTED_GPU_FRACTION', 'METRIC_SLO_ATTAINMENT',
  'METRIC_READY_REPLICAS', 'METRIC_WARMING_REPLICAS', 'METRIC_DRAINING_REPLICAS',
  'METRIC_TTFT', 'METRIC_ITL', 'METRIC_E2E', 'METRIC_QUEUE_WAIT',
];

export const DEFAULT_PERCENTILES = [50, 90, 99, 99.9];

/** One decoded update, with time in the two forms a caller needs and no third. */
export interface ServerSample {
  /** Simulated seconds since the run's first observed sample. The only float form of time. */
  simS: number;
  /** The absolute instant, unrounded, for anything that must not lose nanoseconds. */
  simTimeUnixNs: bigint;
  realtimeFactor: number;
  values: Partial<Record<MetricName, number>>;
  distributions: Partial<Record<MetricName, WireDistribution>>;
}

export interface ServerRunEngineOptions {
  client: IngressClient;
  metrics?: MetricName[];
  recordTraces?: boolean;
  /** Called after every observable change, so a hook can re-render. */
  onChange?: () => void;
  /** 0 disables the status poll; the self-test calls `pollStatus()` itself. */
  statusPollMs?: number;
  leaseNs?: bigint;
  /** Seams handed to `subscribeToTarget`, so the self-test reconnects without a clock. */
  rnd?: () => number;
  sleep?: (ms: number) => Promise<void>;
  now?: () => number;
}

/**
 * The panels' view of a live run. Frames arrive over the stream and accumulate; the reads are the
 * replay engine's over a growing array. Nothing here is simulated in the browser.
 */
export class ServerRunEngine implements FrameSource {
  config: ScenarioConfig;
  /** Oldest first, capped at MAX_SAMPLES. Same array for the engine's whole life. */
  readonly frames: ReplayFrame[] = [];
  runId: string | null = null;
  status: RunStatus | null = null;
  connection: StreamPhase = 'opening';
  /** A transport or server error worth showing, rather than a blank chart. */
  error: string | null = null;
  /** Control-panel fields the engine has no equivalent for, so the UI can say they are inert. */
  dropped: string[] = [];
  lastUpdate: UpdateResponse | null = null;
  /** Always null: nothing is ever rewound. Kept so the handle keeps its shape. */
  readonly lastRewind: RewindResponse | null = null;
  /** The last change that was refused, named, so a banner can say what did not happen. */
  refused: string | null = null;
  resimulating = false;
  revision = 0;
  /** The origin `simS` is measured from: the first sample observed. Absolute, so it never passes through a float. */
  originUnixNs: bigint | null = null;
  /** Scrubbing pins the cursor: the server keeps advancing, and a chart that snapped back to live
   *  the moment the user let go of the scrubber would be unusable. */
  pinnedS: number | null = null;

  private readonly opts: ServerRunEngineOptions;
  private metrics: MetricName[];
  /** Metrics this server refused for the fleet scope, dropped from the subscription and named in the banner. */
  unserved: MetricName[] = [];
  private sub: SubscriptionHandle | null = null;
  private poll: ReturnType<typeof setInterval> | null = null;
  /** Bumped by every start and dispose, so a StartRun that lands after either is stopped again. */
  private generation = 0;
  private disposed = false;
  /** The speed a pause resumes to, since the wire has no "unpause at the old speed". */
  private lastFactor = 1;

  constructor(initial: ScenarioConfig, opts: ServerRunEngineOptions) {
    this.config = cloneConfig(initial);
    this.opts = opts;
    this.metrics = opts.metrics ?? FLEET_METRICS;
  }

  // -- FrameSource ----------------------------------------------------------

  get recordedToS(): number {
    const live = this.frames.length ? this.frames[this.frames.length - 1].simS : 0;
    const s = this.status && this.originUnixNs !== null ? relSeconds(this.status.simTimeUnixNs, this.originUnixNs) : 0;
    return Math.max(live, s);
  }

  get cursorS(): number {
    return this.pinnedS ?? this.recordedToS;
  }

  get durationS(): number {
    return this.config.durationS;
  }

  get paused(): boolean {
    return this.status?.state === 'STATE_PAUSED' || this.status?.state === 'STATE_COMPLETE' || this.connection === 'complete';
  }

  get speed(): number {
    return this.status?.realtimeFactor ?? 0;
  }

  get subscriptionId(): string | null {
    return this.sub?.subscriptionId() ?? null;
  }

  /** The stream's sample interval, from the frames themselves. */
  private get dtS(): number {
    const n = this.frames.length;
    return n > 1 ? (this.frames[n - 1].simS - this.frames[0].simS) / (n - 1) : 1 / this.config.samplesPerSimSecond;
  }

  /** Index of the last frame at or before `simS`; the first frame before any sample. */
  indexAt(simS: number): number {
    let lo = 0;
    let hi = this.frames.length - 1;
    if (hi < 0 || simS < this.frames[0].simS) return 0;
    while (lo < hi) {
      const mid = (lo + hi + 1) >> 1;
      if (this.frames[mid].simS <= simS) lo = mid;
      else hi = mid - 1;
    }
    return lo;
  }

  frameAt(simS: number): Frame | undefined {
    return this.frames[this.indexAt(simS)];
  }

  /** Same decimation as the replay engine's, always ending on the cursor's own frame. */
  window(fromS: number, toS: number): Frame[] {
    const stride = Math.max(1, Math.round(1 / (this.config.samplesPerSimSecond * this.dtS)));
    const out: Frame[] = [];
    let a = 0;
    while (a < this.frames.length && this.frames[a].simS < fromS) a++;
    let b = this.frames.length;
    while (b > 0 && this.frames[b - 1].simS > toS) b--;
    for (let i = a; i < b; i += stride) out.push(this.frames[i]);
    if (b > a) {
      const last = this.frames[b - 1];
      if (out[out.length - 1] !== last) out.push(last);
    }
    return out;
  }

  /** The server has no failure injection yet, so a live run carries no events. */
  eventsUpTo(_simS: number): FleetEvent[] {
    return [];
  }

  // -- lifecycle ------------------------------------------------------------

  private changed(): void {
    this.opts.onChange?.();
  }

  private fail(e: unknown): void {
    this.error = e instanceof Error ? e.message : String(e);
    this.changed();
  }

  private origin(t: bigint): bigint {
    if (this.originUnixNs === null) this.originUnixNs = t;
    return this.originUnixNs;
  }

  /**
   * StartRun, then the fleet subscription and the status poll. Resolves with the run id, or null
   * when the server refused, in which case `error` says why. Safe to call again after `dispose`,
   * which is what React's development double-mount does.
   */
  async start(play = true): Promise<string | null> {
    this.disposed = false;
    const gen = ++this.generation;
    const client = this.opts.client;
    const wire = scenarioConfigToWire(this.config);
    this.dropped = wire.dropped;
    let id: string;
    try {
      // The whole config goes as scenario text, per WIRE.md; overrides are for edits on top of a
      // served file, and this client has no served file to edit. Unset speed means as fast as
      // possible; a pause is a SetSpeed rather than a cap of zero, so the speed survives unpausing.
      id = await client.startRun({
        scenario: scenarioEnvelope(wire.fields),
        // A run that starts paused is paced at the last speed, so it cannot race to completion in
        // the gap before the pause lands; on Cloud Run an unpaced 300 s scenario finished before
        // the SetSpeed arrived and the pause answered 409. A run that starts playing is unpaced.
        maxRealtimeFactor: play ? 0 : this.lastFactor,
        recordTraces: this.opts.recordTraces ?? true,
      });
    } catch (e) {
      if (gen === this.generation) this.fail(e);
      return null;
    }
    if (gen !== this.generation) {
      // Disposed or restarted while the request was in flight: this run has no owner.
      void client.stopRun(id).catch(() => undefined);
      return null;
    }
    this.runId = id;
    this.error = null;
    this.changed();
    if (!play) {
      await client
        .setSpeed(id, this.lastFactor, true)
        .then((s) => this.setStatus(s))
        // Already finished is not a failure: every sample is recorded and the stream replays it.
        .catch((e) => (String(e).includes('STATE_COMPLETE') ? undefined : this.fail(e)));
    }
    this.subscribe();
    const every = this.opts.statusPollMs ?? STATUS_POLL_MS;
    if (every > 0) {
      void this.pollStatus();
      this.poll = setInterval(() => void this.pollStatus(), every);
    }
    return id;
  }

  private setStatus(s: RunStatus): void {
    this.status = s;
    if (s.error) this.error = s.error;
    this.changed();
  }

  async pollStatus(): Promise<void> {
    const id = this.runId;
    if (!id || this.disposed) return;
    try {
      const s = await this.opts.client.getRun(id);
      if (this.runId === id && !this.disposed) this.setStatus(s);
    } catch (e) {
      if (this.runId === id && !this.disposed) this.fail(e);
    }
  }

  private subscribe(): void {
    const id = this.runId;
    if (!id) return;
    this.sub?.close();
    this.sub = subscribeToTarget(this.opts.client, {
      runId: id,
      target: fleetTarget(),
      metrics: this.metrics,
      samplesPerSimSecond: this.config.samplesPerSimSecond,
      percentiles: DEFAULT_PERCENTILES,
      leaseNs: this.opts.leaseNs ?? LEASE_NS,
      rnd: this.opts.rnd,
      sleep: this.opts.sleep,
      now: this.opts.now,
      onPhase: (p, detail) => {
        this.connection = p;
        if (p === 'failed' && detail) {
          // "METRIC_X is not served for this scope": the server is older or narrower than this
          // client's wish list. Drop that metric and open again; the panel for it stays mock.
          // Without this the open answered 200 with a rejected_reason and the dashboard waited
          // forever for a first sample, which is how the showcase got stuck.
          const m = /^(METRIC_[A-Z0-9_]+) is not served/.exec(detail);
          if (m && this.metrics.includes(m[1] as MetricName) && this.runId === id && !this.disposed) {
            this.unserved.push(m[1] as MetricName);
            this.metrics = this.metrics.filter((x) => x !== m[1]);
            this.changed();
            setTimeout(() => {
              if (this.runId === id && !this.disposed) this.subscribe();
            }, 0);
            return;
          }
          this.error = detail;
        }
        this.changed();
      },
      onUpdate: (u) => {
        if (this.runId !== id) return;
        const t0 = this.origin(u.simTimeUnixNs);
        // A resumed stream replays from `Last-Event-ID`, and a reopened one from wherever the
        // server starts; either way a sample at or before the last one held is already here.
        const last = this.frames.length ? this.frames[this.frames.length - 1] : null;
        if (last !== null && u.simTimeUnixNs <= last.simTimeUnixNs) return;
        this.frames.push(frameFromUpdate(u, t0, last === null ? 0 : last.tick + 1));
        if (this.frames.length > MAX_SAMPLES) this.frames.splice(0, this.frames.length - MAX_SAMPLES);
        this.changed();
      },
    });
  }

  /** Stop polling, close the subscription, and stop the run. */
  dispose(): void {
    this.disposed = true;
    this.generation++;
    if (this.poll !== null) clearInterval(this.poll);
    this.poll = null;
    this.sub?.close();
    this.sub = null;
    const id = this.runId;
    if (id) void this.opts.client.stopRun(id).catch(() => undefined);
  }

  // -- controls -------------------------------------------------------------

  setPaused(p: boolean): void {
    const id = this.runId;
    if (!id) return;
    if (!p) this.pinnedS = null;
    void this.opts.client
      .setSpeed(id, this.lastFactor, p)
      .then((s) => this.setStatus(s))
      .catch((e) => this.fail(e));
  }

  /** A speed of 0 is a pause; any other speed sets the factor and plays at it. */
  setSpeed(f: number): void {
    if (f <= 0) {
      this.setPaused(true);
      return;
    }
    this.lastFactor = f;
    const id = this.runId;
    if (!id) return;
    void this.opts.client
      .setSpeed(id, f, false)
      .then((s) => this.setStatus(s))
      .catch((e) => this.fail(e));
  }

  step(): void {
    const id = this.runId;
    if (!id) return;
    this.pinnedS = null;
    // StepForward is bounded server-side; the status it returns says where it actually stopped.
    void this.opts.client
      .stepForward(id, secondsToNs(STEP_S))
      .then((s) => this.setStatus(s))
      .catch((e) => this.fail(e));
  }

  rewindTo(_s: number): void {
    this.refused = `rewind: not applied. ${SERVER_DISABLED_REASON}.`;
    this.changed();
  }

  /** A log read over what has streamed: the cursor pins there until play or step releases it. */
  scrubTo(s: number): void {
    this.pinnedS = Math.min(Math.max(0, s), this.recordedToS);
    this.changed();
  }

  /**
   * The answer to an update. A rejection is mirrored onto `refused`, because the generic update
   * banner reads only `requiredResimulation` and would otherwise print "nothing was re-simulated"
   * over a change the server never applied; the "not applied" banner is the one that prints a reason.
   */
  private settle(u: UpdateResponse): void {
    this.lastUpdate = u;
    this.refused = u.accepted ? null : `${u.changed.join(', ')}: not applied. ${u.rejectedReason}`;
  }

  /**
   * UpdateWorkload and UpdatePolicies, the only two live-tunable calls in ingress.proto. The answer
   * lands on `lastUpdate`; a 501 lands there too, named, because the control being wired and the
   * server not yet honouring it are two different facts and the banner should say which.
   */
  async update(next: ScenarioConfig): Promise<void> {
    const prev = this.config;
    const d = diffConfig(prev, next);
    const changed = d.paths.map((p) => FIELD_LABEL[p] ?? p);
    this.config = cloneConfig(next);
    this.revision++;
    const id = this.runId;
    if (!id || d.paths.length === 0) {
      if (d.paths.length) this.settle({ accepted: true, requiredResimulation: false, rewoundToS: this.cursorS, rejectedReason: '', changed });
      else this.lastUpdate = null;
      this.changed();
      return;
    }
    const client = this.opts.client;
    const calls: Promise<{ accepted: boolean; requiredResimulation: boolean; rewoundToUnixNs: bigint; rejectedReason: string }>[] = [];
    if (d.paths.some((p) => p.startsWith('workload.'))) calls.push(client.updateWorkload(id, toOverrides(workloadToWire(next).fields)));
    if (d.paths.some((p) => p.startsWith('routing.'))) calls.push(client.updatePolicies(id, toOverrides(policiesToWire(next).fields)));
    if (calls.length === 0) {
      // Anything else -- fleet shape, seed, duration -- is a new run by design.
      this.settle({ accepted: false, requiredResimulation: false, rewoundToS: this.cursorS, rejectedReason: 'only workload and policy are live-tunable; restart the run for this change', changed });
      this.changed();
      return;
    }
    this.resimulating = true;
    this.changed();
    try {
      const rs = await Promise.all(calls);
      const t0 = this.originUnixNs;
      const accepted = rs.every((r) => r.accepted);
      const resim = rs.some((r) => r.requiredResimulation);
      const rewoundNs = rs.map((r) => r.rewoundToUnixNs).filter((n) => n > 0n).sort((a, b) => (a < b ? -1 : a > b ? 1 : 0))[0];
      this.settle({
        accepted,
        requiredResimulation: resim,
        rewoundToS: rewoundNs !== undefined && t0 !== null ? relSeconds(rewoundNs, t0) : this.cursorS,
        rejectedReason: rs.map((r) => r.rejectedReason).filter(Boolean).join('; '),
        changed,
      });
      // A refusal from the server means `next` was never applied: leaving it on `config` shows a
      // value the fleet does not hold, and a second, identical submission would diff to nothing
      // and never be resent. Restore what the server still holds instead.
      if (!accepted) {
        this.config = prev;
        this.revision++;
      }
      if (resim && rewoundNs !== undefined) {
        // The history after the rewind point is a different future now, so drop it rather than
        // leaving a chart that mixes two runs.
        let keep = this.frames.length;
        while (keep > 0 && this.frames[keep - 1].simTimeUnixNs > rewoundNs) keep--;
        this.frames.length = keep;
      }
    } catch (e) {
      if (e instanceof IngressError && e.httpStatus === 501) {
        this.settle({ accepted: false, requiredResimulation: false, rewoundToS: this.cursorS, rejectedReason: `${SERVER_NOT_YET}: ${e.message}`, changed });
        this.config = prev;
        this.revision++;
      } else {
        this.error = e instanceof Error ? e.message : String(e);
      }
    } finally {
      this.resimulating = false;
      this.changed();
    }
  }

  /** Stop this run and start another from `next`. Everything observed so far is dropped. */
  async restart(next: ScenarioConfig): Promise<void> {
    const old = this.runId;
    this.generation++;
    if (this.poll !== null) clearInterval(this.poll);
    this.poll = null;
    this.sub?.close();
    this.sub = null;
    this.runId = null;
    this.status = null;
    this.frames.length = 0;
    this.lastUpdate = null;
    this.refused = null;
    this.pinnedS = null;
    this.originUnixNs = null;
    this.connection = 'opening';
    this.config = cloneConfig(next);
    this.revision++;
    this.changed();
    const gen = this.generation;
    if (old) await this.opts.client.stopRun(old).catch(() => undefined);
    // Disposed, or superseded by a later start/restart/dispose, while StopRun was in flight: the
    // run this would start has no owner left to receive it, so starting it would leak a
    // subscription and a status poll that nothing ever tears down.
    if (this.disposed || gen !== this.generation) return;
    await this.start(true);
  }

  dismissUpdate(): void {
    this.lastUpdate = null;
    this.changed();
  }

  dismissRefused(): void {
    this.refused = null;
    this.changed();
  }
}

// ---------------------------------------------------------------------------
// The hook
// ---------------------------------------------------------------------------

export type ServerRunHandle = Omit<RunHandle, 'engine' | 'update' | 'source'> & {
  engine: ServerRunEngine;
  /** The answer arrives on `lastUpdate` a round trip later; see the header. */
  update: (next: ScenarioConfig) => null;
  source: RunSourceInfo & { kind: 'server'; disabledReason: string };
  originUnixNs: bigint | null;
  runId: string | null;
  status: RunStatus | null;
  connection: StreamPhase;
  subscriptionId: string | null;
  dropped: string[];
  unserved: MetricName[];
  error: string | null;
  /** The reason every refused control gives. Constant; here so a panel need not import this file. */
  disabledReason: string;
  refused: string | null;
  dismissRefused: () => void;
};

/**
 * Compile-time proof that the server handle is a `RunHandle`: every panel that takes one takes it.
 * If a field is added to `RunHandle` and not here, this stops compiling.
 */
export type HandleShapesAgree = ServerRunHandle extends RunHandle ? true : false;
export const HANDLE_SHAPES_AGREE: HandleShapesAgree = true;

export interface ServerRunOptions {
  autoplay?: boolean;
  client?: IngressClient;
  metrics?: MetricName[];
  recordTraces?: boolean;
}

export function useServerRun(initial: ScenarioConfig, opts: ServerRunOptions = {}): ServerRunHandle {
  const autoplay = opts.autoplay ?? true;
  const mode = useMemo(() => serverMode(), []);
  const client = useMemo(() => opts.client ?? new IngressClient({ baseUrl: mode.baseUrl }), [opts.client, mode.baseUrl]);
  const [version, bump] = useReducer((n: number) => n + 1, 0);

  // One engine per mount, seeded from the initial config, exactly as useRun builds one mock engine.
  const engineRef = useRef<ServerRunEngine | null>(null);
  if (engineRef.current === null) {
    engineRef.current = new ServerRunEngine(initial, { client, metrics: opts.metrics, recordTraces: opts.recordTraces, onChange: bump });
  }
  const engine = engineRef.current;

  useEffect(() => {
    void engine.start(autoplay);
    // A run the browser started and then navigated away from would keep a simulation going, so the
    // unmount stops it. The subscription lease covers the case where the tab dies instead.
    return () => engine.dispose();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [engine]);

  return useMemo<ServerRunHandle>(
    () => ({
      engine,
      config: engine.config,
      cursorS: engine.cursorS,
      recordedToS: engine.recordedToS,
      durationS: engine.durationS,
      paused: engine.paused,
      speed: engine.speed,
      resimulating: engine.resimulating,
      lastRewind: engine.lastRewind,
      lastUpdate: engine.lastUpdate,
      revision: engine.revision,
      setPaused: (p) => engine.setPaused(p),
      setSpeed: (f) => engine.setSpeed(f),
      step: () => engine.step(),
      rewindTo: (s) => engine.rewindTo(s),
      scrubTo: (s) => engine.scrubTo(s),
      update: (next) => {
        void engine.update(next);
        return null;
      },
      restart: (next) => void engine.restart(next),
      dismissUpdate: () => engine.dismissUpdate(),
      source: { kind: 'server', label: modeBanner(mode), disabledReason: SERVER_DISABLED_REASON, runId: engine.runId ?? undefined },
      originUnixNs: engine.originUnixNs,
      runId: engine.runId,
      status: engine.status,
      connection: engine.connection,
      subscriptionId: engine.subscriptionId,
      dropped: engine.dropped,
      unserved: engine.unserved,
      error: engine.error,
      disabledReason: SERVER_DISABLED_REASON,
      refused: engine.refused,
      dismissRefused: () => engine.dismissRefused(),
    }),
    // `version` is what the engine bumps; every field above is read from it at that moment.
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [engine, mode, version]
  );
}

/**
 * One subscription for one entity, for a panel that draws a single replica, pool or tenant. Separate
 * from the run hook because the protos make it separate: there is no "all replicas" call, and a page
 * showing twenty rows opens twenty of these and closes them when the page changes.
 */
export function useServerTarget(
  runId: string | null,
  target: UiTarget,
  metrics: MetricName[],
  samplesPerSimSecond: number,
  client?: IngressClient
): ServerSample[] {
  const mode = useMemo(() => serverMode(), []);
  const c = useMemo(() => client ?? new IngressClient({ baseUrl: mode.baseUrl }), [client, mode.baseUrl]);
  const [samples, setSamples] = useState<ServerSample[]>([]);
  const originRef = useRef<bigint | null>(null);
  // Keyed on the entity, not on the object's identity: re-rendering with an equal target must not
  // close and reopen the subscription, which is the whole bounded-ness claim in the ui spec.
  const key = `${target.scope}:${target.id ?? ''}`;
  const targetRef = useRef(target);
  targetRef.current = target;

  useEffect(() => {
    if (!runId) return;
    setSamples([]);
    originRef.current = null;
    const handle = subscribeToTarget(c, {
      runId,
      target: uiTargetToWire(targetRef.current),
      metrics,
      samplesPerSimSecond,
      percentiles: DEFAULT_PERCENTILES,
      leaseNs: LEASE_NS,
      onUpdate: (u) => {
        if (originRef.current === null) originRef.current = u.simTimeUnixNs;
        const t0 = originRef.current;
        setSamples((prev) => {
          const next = prev.length >= MAX_SAMPLES ? prev.slice(prev.length - MAX_SAMPLES + 1) : prev.slice();
          next.push({
            simS: relSeconds(u.simTimeUnixNs, t0),
            simTimeUnixNs: u.simTimeUnixNs,
            realtimeFactor: u.realtimeFactor,
            values: u.row.values,
            distributions: u.row.distributions,
          });
          return next;
        });
      },
    });
    return () => handle.close();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [c, runId, key, samplesPerSimSecond]);

  return samples;
}
