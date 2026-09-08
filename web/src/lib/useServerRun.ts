// The run controller, backed by the Ingress server.
//
// `ServerRunEngine` is the whole controller with no React in it: it starts a run, subscribes to the
// fleet stream, decodes each update into the same `ReplayFrame` replay decodes from `fleet.jsonl`
// (through the same `frameFromUpdate`, so live and replay agree by construction), and answers the
// three reads the panels make (`frameAt`, `window`, `eventsUpTo`) over those frames. `useServerRun`
// is a thin hook over it, shaped to `RunHandle` so wiring a panel is a swap of the hook rather than
// a rewrite of the panel. The self-test drives the engine directly against a fake server.
//
// Two fields differ from the replay handle's, and pretending otherwise would be the dishonesty
// this whole exercise is trying to avoid:
//
//   - `update` returns `null` rather than an `UpdateResponse`. A round trip decides the answer, so it
//     arrives on `lastUpdate` later, exactly as `lastRewind` already does.
//   - `rewindTo` is refused and named: the first server has no Rewind. `scrubTo` pins the cursor
//     inside what has already streamed, which is a local read and needs no server.
//
// Everything else -- cursor, playback, step, restart, the banners the UI is obliged to show -- is
// the same shape and the same units (relative simulated seconds) as the replay handle's.

import { useEffect, useMemo, useReducer, useRef, useState, useSyncExternalStore } from 'react';
import type { FrameSource, RunHandle, RunSourceInfo } from './useRun';
import { STEP_S } from './useRun';
import type { FleetEvent, Frame, ReplicaSample } from './frame';
import type { ScenarioConfig } from './config';
import { cloneConfig, diffConfig, FIELD_LABEL } from './config';
import type { RewindResponse, Target as UiTarget, UpdateResponse } from './types';
import { frameFromUpdate, replicaFromUpdate, type ReplayFrame } from './adapter';
import {
  IngressClient,
  IngressError,
  type MetricName,
  type RunStatus,
  type StreamPhase,
  type SubscriptionUpdate,
  type SubscribeOptions,
  type SubscriptionHandle,
  type WireDistribution,
  type WireRequestTrace,
  type OutcomeName,
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
import { smoothingWindowNs, useSmoothing } from './smoothing';

// ---------------------------------------------------------------------------
// Stream budget
// ---------------------------------------------------------------------------

/**
 * How many SSE streams this page may hold open at once, the fleet stream included.
 *
 * Every RPC (SetSpeed, GetRun, Renew) is a `fetch` to the same origin as the streams, and over
 * HTTP/1.1 Chrome allows six connections per host. The Machines page opened one stream per visible
 * row, so ten rows plus the fleet stream held eleven, and every RPC after them queued forever: the
 * showcase's pause reached the server only sometimes (U102). Four is the fleet stream, three replica
 * streams, and two connections left for RPCs. HTTP/2 and HTTP/3 multiplex every stream on one
 * connection, so only the server's own limit applies there; lbsim.ai answers over HTTP/2 through
 * Cloud Run, the local `sim-run serve` over HTTP/1.1.
 */
export const STREAM_BUDGET_H1 = 4;
export const STREAM_BUDGET_H2 = 32;

export function streamBudget(): number {
  const nav =
    typeof performance === 'undefined'
      ? undefined
      : (performance.getEntriesByType('navigation')[0] as { nextHopProtocol?: string } | undefined);
  const proto = nav?.nextHopProtocol ?? '';
  return proto === 'h2' || proto === 'h3' ? STREAM_BUDGET_H2 : STREAM_BUDGET_H1;
}

let openStreams = 0;
const streamListeners = new Set<() => void>();
function countStream(delta: number): void {
  openStreams += delta;
  for (const l of streamListeners) l();
}
/** Subscriptions open right now, from every hook and engine in the page. */
export function openStreamCount(): number {
  return openStreams;
}
/** The same number, as a React value, for the status bar. */
export function useOpenStreams(): number {
  return useSyncExternalStore(
    (l) => {
      streamListeners.add(l);
      return () => streamListeners.delete(l);
    },
    () => openStreams,
    () => 0
  );
}

/**
 * `subscribeToTarget`, counted: every stream this module opens goes through here, because the
 * budget is enforced against the count. Released once, on `close()` or when the loop ends on its
 * own (`complete`, `failed`), whichever is first.
 */
function countedSubscribe(client: IngressClient, o: SubscribeOptions): SubscriptionHandle {
  const inner = subscribeToTarget(client, o);
  let counted = true;
  const release = () => {
    if (!counted) return;
    counted = false;
    countStream(-1);
  };
  countStream(1);
  void inner.done.then(release, release);
  return {
    ...inner,
    close() {
      inner.close();
      release();
    },
  };
}

/** How often the run's status is polled. The stream carries metrics; this carries state and speed. */
export const STATUS_POLL_MS = 500;
/** Bounded history, because a long run at a fine sample rate is otherwise unbounded browser memory. */
export const MAX_SAMPLES = 4000;
/** Window `achievedFactor` averages over: recent enough to react, wide enough not to be one frame's jitter. */
export const PACE_WINDOW_MS = 2000;
/** Why rewind is refused. Constant, so a panel can show it without importing the transport. */
export const SERVER_DISABLED_REASON = 'the server does not support Rewind yet';
/** What `lastUpdate` says when the server answers 501: the control is wired, the server is not. */
export const SERVER_NOT_YET = 'the server does not support this yet';
/** The lease asked for on every subscription; renewed at a third of it while the page is visible. */
export const LEASE_MS = 30_000;
export const LEASE_NS = BigInt(LEASE_MS) * 1_000_000n;

/** The fleet metrics the dashboard's top-level panels read. One subscription, one entity. */
export const FLEET_METRICS: MetricName[] = [
  'METRIC_OFFERED_RPS', 'METRIC_ADMITTED_RPS', 'METRIC_COMPLETED_RPS', 'METRIC_REJECTED_RPS',
  'METRIC_OUTPUT_TOKENS_PER_S', 'METRIC_GOODPUT_TOKENS_PER_S', 'METRIC_PREEMPTIONS_PER_S',
  'METRIC_RETRIES_PER_S', 'METRIC_KV_UTILIZATION', 'METRIC_RUNNING_SEQS', 'METRIC_QUEUED_SEQS',
  'METRIC_LOAD_IMBALANCE_CV', 'METRIC_WASTED_GPU_FRACTION', 'METRIC_SLO_ATTAINMENT',
  'METRIC_READY_REPLICAS', 'METRIC_WARMING_REPLICAS', 'METRIC_DRAINING_REPLICAS',
  'METRIC_GPU_UTILIZATION', 'METRIC_GPU_COMPUTE_BOUND_FRACTION',
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
  /** The smoothing window every subscription opens with; `setSmoothing` changes it live. */
  smoothingWindowNs?: bigint;
  /** Seams handed to `subscribeToTarget`, so the self-test reconnects without a clock. */
  rnd?: () => number;
  sleep?: (ms: number) => Promise<void>;
  now?: () => number;
}

/** A config path the server reads only at StartRun: no Update call carries it. */
const isStructural = (p: string): boolean => p === 'seed' || p === 'durationS' || p === 'warmupS' || p.startsWith('fleet.');

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
  /**
   * Edits the server takes only at StartRun -- fleet shape, physics, seed, duration -- staged on
   * the config the panel shows until `restart(pendingRestart)` starts a run from them. Null when
   * the running fleet holds every structural value the panel does. Issao: "where do i tune step
   * token budget?": the knob was a slider that ended in a refusal, so it read as inert.
   */
  pendingRestart: ScenarioConfig | null = null;
  /** Human labels of the staged keys, for the banner that asks for the restart. */
  pendingKeys: string[] = [];
  /** The config the current run was started from: what `pendingKeys` is measured against. */
  private startedFrom: ScenarioConfig;
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
  /** The pace actually observed, over the last `PACE_WINDOW_MS` of fleet updates: (Δsim ns / 1e9)
   *  / (Δwall ms / 1000). NaN until two samples exist, so a stalled or fresh run reads as unknown
   *  rather than as a false zero. */
  achievedFactor = NaN;
  /** Wall/sim pairs backing `achievedFactor`, oldest first, trimmed to `PACE_WINDOW_MS` of wall time. */
  private paceSamples: { wallMs: number; simNs: bigint }[] = [];

  private readonly opts: ServerRunEngineOptions;
  private metrics: MetricName[];
  /** `OpenSubscriptionRequest.smoothing_window_ns` on the fleet stream; 0n is the raw cadence. */
  smoothingWindowNs: bigint;
  /**
   * Frames arriving from a subscription reopened under a new window, held here until they have
   * caught up with the frames on screen and then swapped in at once, so the chart never empties
   * while the server re-streams the run's history smoothed. Null when no reopen is in progress.
   */
  private staging: ReplayFrame[] | null = null;
  /** Metrics this server refused for the fleet scope, dropped from the subscription and named in the banner. */
  unserved: MetricName[] = [];
  private sub: SubscriptionHandle | null = null;
  private poll: ReturnType<typeof setInterval> | null = null;
  /** Bumped by every start and dispose, so a StartRun that lands after either is stopped again. */
  private generation = 0;
  private disposed = false;
  /** The speed a pause resumes to, since the wire has no "unpause at the old speed". */
  private lastFactor = 1;
  /**
   * Whether the run should be paused, as last asked. A control that lands before StartRun has
   * answered has no id to send it to; it is remembered here and applied once the id exists, so the
   * showcase's "speed 2x, play" issued on the first render is not lost to a pause that lands later.
   */
  private wantPaused = false;
  /**
   * A control arrived while no run id existed. The showcase's walkthrough issues its first step
   * from a child effect, which React runs before the parent effect that calls `start`, so `start`
   * must not reset `wantPaused` to its own default when a control already spoke.
   */
  private pendingControl = false;
  /**
   * Every control for the run, one in flight at a time, in the order it was issued. Two SetSpeeds
   * in flight together reach the server in either order: on lbsim.ai the walkthrough's play was
   * overtaken by start's own pause about once in fourteen cards, and the run stood still with its
   * stream open. Each link reads the wish current when its turn comes, so a stale pause never wins.
   */
  private controls: Promise<void> = Promise.resolve();

  constructor(initial: ScenarioConfig, opts: ServerRunEngineOptions) {
    this.config = cloneConfig(initial);
    this.startedFrom = this.config;
    this.opts = opts;
    this.metrics = opts.metrics ?? FLEET_METRICS;
    this.smoothingWindowNs = opts.smoothingWindowNs ?? 0n;
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
    // Before the first status, and for a run an older client started unpaced, the pressed speed
    // button is the speed this client starts runs at; a factor of 0 would press nothing.
    const f = this.status?.realtimeFactor ?? 0;
    return f > 0 ? f : this.lastFactor;
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
    if (!this.pendingControl) this.wantPaused = !play;
    this.pendingControl = false;
    const gen = ++this.generation;
    const client = this.opts.client;
    const wire = scenarioConfigToWire(this.config);
    this.dropped = wire.dropped;
    this.startedFrom = cloneConfig(this.config);
    this.stage();
    const startedAt = this.lastFactor;
    let id: string;
    try {
      // The whole config goes as scenario text, per WIRE.md; overrides are for edits on top of a
      // served file, and this client has no served file to edit. A pause is a SetSpeed rather
      // than a cap of zero, so the speed survives unpausing.
      id = await client.startRun({
        scenario: scenarioEnvelope(wire.fields),
        // Every run starts paced at the speed control's value, whether it starts playing or
        // paused. Playing: an unpaced 120 s scenario finishes on the server in about a second, so
        // the viewer opens on "stream complete" with the cursor parked at the end, live but not
        // looking it; paced, the cursor follows the live edge at wall-clock pace and the speed
        // buttons mean what they say from the first second. Paused: the run cannot race to
        // completion in the gap before the pause lands; on Cloud Run an unpaced 300 s scenario
        // finished before the SetSpeed arrived and the pause answered 409.
        maxRealtimeFactor: startedAt,
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
    // The run is already paced at the factor StartRun carried, so a playing run at that factor
    // needs no call; a pause, or a speed chosen while the request was in flight, needs one. It is
    // queued before the id is published, so a control issued by the render that first sees the id
    // (the walkthrough's first step) lines up behind it instead of racing it on the wire.
    let settled: Promise<void> = Promise.resolve();
    if (this.wantPaused || this.lastFactor !== startedAt) {
      settled = this.control(
        () => (this.runId === id ? client.setSpeed(id, this.lastFactor, this.wantPaused) : null),
        // Already finished is not a failure: every sample is recorded and the stream replays it.
        (e) => (String(e).includes('STATE_COMPLETE') ? undefined : this.fail(e))
      );
    }
    this.changed();
    await settled;
    // Disposed or restarted while that control was in flight, which is where a closing page lands:
    // a subscription and a poll set now would have no owner, and the next start would overwrite
    // the interval's handle and leave it ticking for good.
    if (gen !== this.generation) return null;
    this.subscribe();
    const every = this.opts.statusPollMs ?? STATUS_POLL_MS;
    if (every > 0) {
      if (this.poll !== null) clearInterval(this.poll);
      void this.pollStatus();
      this.poll = setInterval(() => void this.pollStatus(), every);
    }
    return id;
  }

  /**
   * Append a control to the run's queue. `send` runs once every earlier control has answered and
   * returns null when the run it was meant for is gone. Resolves when this control has answered.
   */
  private control(send: () => Promise<RunStatus> | null, onError: (e: unknown) => void = (e) => this.fail(e)): Promise<void> {
    const turn = this.controls.then(async () => {
      if (this.disposed) return;
      try {
        const s = await send();
        if (s) this.setStatus(s);
      } catch (e) {
        onError(e);
      }
    });
    this.controls = turn;
    return turn;
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
    this.sub = countedSubscribe(this.opts.client, {
      runId: id,
      target: fleetTarget(),
      metrics: this.metrics,
      samplesPerSimSecond: this.config.samplesPerSimSecond,
      percentiles: DEFAULT_PERCENTILES,
      leaseNs: this.opts.leaseNs ?? LEASE_NS,
      smoothingWindowNs: this.smoothingWindowNs,
      rnd: this.opts.rnd,
      sleep: this.opts.sleep,
      now: this.opts.now,
      onPhase: (p, detail) => {
        this.connection = p;
        if (p === 'failed' && detail) {
          // "METRIC_X is not served for this scope": the server is older or narrower than this
          // client's wish list. Drop that metric and open again; the panel for it reads "—".
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
        if (this.staging !== null) {
          this.stageFrame(u, t0);
          return;
        }
        // A resumed stream replays from `Last-Event-ID`, and a reopened one from wherever the
        // server starts; either way a sample at or before the last one held is already here.
        const last = this.frames.length ? this.frames[this.frames.length - 1] : null;
        if (last !== null && u.simTimeUnixNs <= last.simTimeUnixNs) return;
        this.frames.push(frameFromUpdate(u, t0, last === null ? 0 : last.tick + 1));
        if (this.frames.length > MAX_SAMPLES) this.frames.splice(0, this.frames.length - MAX_SAMPLES);
        this.recordPace(u.simTimeUnixNs);
        this.changed();
      },
    });
  }

  /**
   * A fresh subscription streams the run from its first frame, now built over the new window. The
   * frames on screen keep their place until the new stream has reached the last of them, then the
   * whole history is replaced in one splice (same array, so every reader sees the swap), and from
   * there the stream appends as usual. The pace window is not fed with the backlog, which arrives
   * as fast as the server can encode it and is not the run's pace.
   */
  private stageFrame(u: SubscriptionUpdate, t0: bigint): void {
    const s = this.staging as ReplayFrame[];
    const prev = s.length ? s[s.length - 1] : null;
    if (prev !== null && u.simTimeUnixNs <= prev.simTimeUnixNs) return;
    s.push(frameFromUpdate(u, t0, prev === null ? 0 : prev.tick + 1));
    if (s.length > MAX_SAMPLES) s.splice(0, s.length - MAX_SAMPLES);
    const held = this.frames.length ? this.frames[this.frames.length - 1] : null;
    if (held !== null && u.simTimeUnixNs < held.simTimeUnixNs) return;
    this.frames.splice(0, this.frames.length, ...s);
    this.staging = null;
    this.changed();
  }

  /**
   * Change the smoothing window live: the fleet subscription is reopened with it, and nothing else
   * moves. The run keeps running or stays paused as it was (a fresh open lifts only an idle stop,
   * never a pause), the cursor stays pinned where it is, and the history is re-streamed smoothed
   * rather than left half raw. A change before the run exists is applied by the first subscribe.
   */
  setSmoothing(windowNs: bigint): void {
    if (windowNs === this.smoothingWindowNs) return;
    this.smoothingWindowNs = windowNs;
    if (!this.runId || this.sub === null || this.disposed) return;
    this.staging = [];
    this.subscribe();
  }

  /**
   * Feeds `achievedFactor` from a fleet update's sim time: keeps the last `PACE_WINDOW_MS` of
   * wall/sim pairs and divides the elapsed sim time by the elapsed wall time across that window.
   * This is the pace the run actually made, as distinct from `status.realtimeFactor`, which is
   * only the target asked of it.
   */
  private recordPace(simNs: bigint): void {
    const wallMs = this.opts.now?.() ?? Date.now();
    this.paceSamples.push({ wallMs, simNs });
    while (this.paceSamples.length > 1 && wallMs - this.paceSamples[0].wallMs > PACE_WINDOW_MS) {
      this.paceSamples.shift();
    }
    if (this.paceSamples.length < 2) {
      this.achievedFactor = NaN;
      return;
    }
    const first = this.paceSamples[0];
    const dWallMs = wallMs - first.wallMs;
    this.achievedFactor = dWallMs > 0 ? Number(simNs - first.simNs) / 1e9 / (dWallMs / 1000) : NaN;
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
    if (!id) return;
    void this.opts.client.stopRun(id).catch(() => undefined);
    // On pagehide the browser abandons an ordinary fetch with the page, so the run outlives its
    // tab until the server's cap of live runs answers 503 to everyone. `keepalive` lets this one
    // request finish after unload; the client call above stays so a fake still sees the StopRun.
    if (typeof window !== 'undefined' && typeof fetch !== 'undefined') {
      void fetch(`${this.opts.client.baseUrl}/v1/ingress/StopRun`, { method: 'POST', body: JSON.stringify({ run_id: id }), keepalive: true }).catch(() => undefined);
    }
  }

  // -- controls -------------------------------------------------------------

  setPaused(p: boolean): void {
    this.wantPaused = p;
    if (!p) this.pinnedS = null;
    const id = this.runId;
    if (!id) {
      this.pendingControl = true;
      return;
    }
    void this.control(() => (this.runId === id ? this.opts.client.setSpeed(id, this.lastFactor, this.wantPaused) : null));
  }

  /** A speed of 0 is a pause; any other speed sets the factor and plays at it. */
  setSpeed(f: number): void {
    if (f <= 0) {
      this.setPaused(true);
      return;
    }
    this.lastFactor = f;
    this.wantPaused = false;
    const id = this.runId;
    if (!id) {
      this.pendingControl = true;
      return;
    }
    void this.control(() => (this.runId === id ? this.opts.client.setSpeed(id, this.lastFactor, this.wantPaused) : null));
  }

  step(): void {
    const id = this.runId;
    if (!id) return;
    this.pinnedS = null;
    // StepForward is bounded server-side; the status it returns says where it actually stopped.
    void this.control(() => (this.runId === id ? this.opts.client.stepForward(id, secondsToNs(STEP_S)) : null));
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

  /** Recompute what a restart would change: the structural keys the panel holds and the run does not. */
  private stage(): void {
    const keys = diffConfig(this.startedFrom, this.config).paths.filter(isStructural);
    this.pendingKeys = keys.map((p) => FIELD_LABEL[p] ?? p);
    this.pendingRestart = keys.length ? cloneConfig(this.config) : null;
  }

  /**
   * UpdateWorkload and UpdatePolicies, the only two live-tunable calls in ingress.proto. The answer
   * lands on `lastUpdate`; a 501 lands there too, named, because the control being wired and the
   * server not yet honouring it are two different facts and the banner should say which.
   * A structural key -- fleet shape, physics, seed, duration -- is a new run by design: it is
   * staged on `pendingRestart` for the banner's restart rather than sent, so the knob keeps the
   * value the user set instead of ending in a refusal.
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
    const structural = d.paths.some(isStructural);
    if (structural) this.stage();
    const client = this.opts.client;
    const calls: Promise<{ accepted: boolean; requiredResimulation: boolean; rewoundToUnixNs: bigint; rejectedReason: string }>[] = [];
    if (d.paths.some((p) => p.startsWith('workload.'))) calls.push(client.updateWorkload(id, toOverrides(workloadToWire(next).fields)));
    if (d.paths.some((p) => p.startsWith('routing.'))) calls.push(client.updatePolicies(id, toOverrides(policiesToWire(next).fields)));
    if (calls.length === 0) {
      if (structural) this.lastUpdate = null;
      else this.settle({ accepted: false, requiredResimulation: false, rewoundToS: this.cursorS, rejectedReason: 'only workload and policy are live-tunable; restart the run for this change', changed });
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
        this.stage();
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
        this.stage();
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
    this.staging = null;
    this.lastUpdate = null;
    this.refused = null;
    this.pinnedS = null;
    this.originUnixNs = null;
    this.connection = 'opening';
    this.config = cloneConfig(next);
    // The new run starts from `next`, so nothing is pending against it; `start` measures again.
    this.startedFrom = this.config;
    this.pendingRestart = null;
    this.pendingKeys = [];
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
  /** The pace actually measured client-side; see `ServerRunEngine.achievedFactor`. */
  achievedFactor: number;
  connection: StreamPhase;
  subscriptionId: string | null;
  dropped: string[];
  unserved: MetricName[];
  error: string | null;
  /** Structural edits waiting for `restart(pendingRestart)`; see `ServerRunEngine.pendingRestart`. */
  pendingRestart: ScenarioConfig | null;
  pendingKeys: string[];
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

/**
 * The banner's word for the run's pace. A factor of 0 is a run an older client started unpaced;
 * this client never starts one, so "speed 0×" is never the right thing to print. `achievedFactor`
 * is what the client actually measured (see `ServerRunEngine.recordPace`), not the target the
 * server was asked for: a fleet too large to keep up shows the shortfall rather than the wish.
 */
export function speedLabel(status: RunStatus | null, paused: boolean, achievedFactor: number): string {
  if (paused) return 'paused';
  if (status === null) return '…';
  const achieved = Number.isFinite(achievedFactor) ? achievedFactor.toFixed(1) : null;
  if (status.realtimeFactor <= 0) return achieved !== null ? `unpaced (${achieved}×)` : 'unpaced';
  if (achieved !== null && achievedFactor < status.realtimeFactor * 0.9) {
    return `${status.realtimeFactor}× (achieving ${achieved}×)`;
  }
  return `${status.realtimeFactor}×`;
}

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
  const smooth = useSmoothing();

  // One engine per mount, seeded from the initial config and the smoothing window in force, so the
  // first subscription already carries it rather than opening raw and reopening a render later.
  const engineRef = useRef<ServerRunEngine | null>(null);
  if (engineRef.current === null) {
    engineRef.current = new ServerRunEngine(initial, {
      client,
      metrics: opts.metrics,
      recordTraces: opts.recordTraces,
      smoothingWindowNs: smoothingWindowNs(smooth),
      onChange: bump,
    });
  }
  const engine = engineRef.current;

  useEffect(() => {
    engine.setSmoothing(smoothingWindowNs(smooth));
  }, [engine, smooth]);

  useEffect(() => {
    void engine.start(autoplay);
    // A run the browser started and then navigated away from would keep a simulation going, so the
    // unmount stops it. A closed tab never unmounts, so pagehide stops it too; the subscription
    // lease covers the case where the tab dies without either.
    const onHide = () => engine.dispose();
    window.addEventListener('pagehide', onHide);
    return () => {
      window.removeEventListener('pagehide', onHide);
      engine.dispose();
    };
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
      // Distinct from `paused`: STATE_PAUSED and a stream that finished before the poll caught up
      // (`connection === 'complete'`) both pause without the run being over.
      ended: engine.status?.state === 'STATE_COMPLETE',
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
      achievedFactor: engine.achievedFactor,
      connection: engine.connection,
      subscriptionId: engine.subscriptionId,
      dropped: engine.dropped,
      unserved: engine.unserved,
      error: engine.error,
      pendingRestart: engine.pendingRestart,
      pendingKeys: engine.pendingKeys,
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
    const handle = countedSubscribe(c, {
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

/** How often the Traces tab asks `GetTraces` again while it is open. A poll, not a stream: the
 * proto has no trace subscription, and a page of a hundred journeys every two seconds is bounded. */
export const TRACES_POLL_MS = 2000;
/** One page of the Traces tab; the server's own default when unset, and the ring holds twenty. */
export const TRACES_PAGE = 100;

export interface TraceFilters {
  outcome?: OutcomeName;
  minE2eNs?: bigint;
  tenantId?: bigint;
  limit?: number;
}

export interface ServerTraces {
  /** The last page the server answered, newest first, exactly as decoded. */
  traces: WireRequestTrace[];
  /** How many polls have answered, so a panel can tell "none yet" from "none sampled". */
  answered: number;
  error: string | null;
}

/**
 * The sampled journeys of one run, for the Traces tab: `GetTraces` every `TRACES_POLL_MS` while
 * mounted, cancelled on unmount or when the run or the filters change. Sibling of `useServerTarget`:
 * one entity, one bounded stream of reads, closed by the component's own lifecycle.
 */
export function useServerTraces(runId: string | null, filters: TraceFilters = {}, client?: IngressClient): ServerTraces {
  const mode = useMemo(() => serverMode(), []);
  const c = useMemo(() => client ?? new IngressClient({ baseUrl: mode.baseUrl }), [client, mode.baseUrl]);
  const [state, setState] = useState<ServerTraces>({ traces: [], answered: 0, error: null });
  // Keyed on the filter values, not the object, so a re-render with equal filters keeps the poll.
  const key = `${filters.outcome ?? ''}:${filters.minE2eNs ?? ''}:${filters.tenantId ?? ''}:${filters.limit ?? TRACES_PAGE}`;
  const filtersRef = useRef(filters);
  filtersRef.current = filters;

  useEffect(() => {
    setState({ traces: [], answered: 0, error: null });
    if (!runId) return;
    const ac = new AbortController();
    let timer: ReturnType<typeof setTimeout> | null = null;
    const tick = async () => {
      try {
        const f = filtersRef.current;
        const traces = await c.getTraces(runId, { ...f, limit: f.limit ?? TRACES_PAGE }, ac.signal);
        if (ac.signal.aborted) return;
        setState((prev) => ({ traces, answered: prev.answered + 1, error: null }));
      } catch (e) {
        if (ac.signal.aborted) return;
        const message = e instanceof IngressError && e.httpStatus === 501 ? SERVER_NOT_YET : e instanceof Error ? e.message : String(e);
        setState((prev) => ({ ...prev, answered: prev.answered + 1, error: message }));
      }
      if (!ac.signal.aborted) timer = setTimeout(() => void tick(), TRACES_POLL_MS);
    };
    void tick();
    return () => {
      ac.abort();
      if (timer !== null) clearTimeout(timer);
    };
  }, [c, runId, key]);

  return state;
}

/** What the server serves on a `SCOPE_REPLICA` target (`REPLICA_METRICS` in sim-ingress/src/run.rs). */
const REPLICA_ROW_METRICS: MetricName[] = [
  'METRIC_QUEUED_SEQS',
  'METRIC_RUNNING_SEQS',
  'METRIC_KV_UTILIZATION',
  'METRIC_KV_TOKENS_RESIDENT',
  'METRIC_STEP_TIME',
  'METRIC_GPU_UTILIZATION',
  'METRIC_GPU_COMPUTE_BOUND_FRACTION',
  'METRIC_REPLICA_STATE',
  'METRIC_TRUE_SPEED_MULTIPLIER',
  'METRIC_TTFT',
];
/** Enough history for a sparkline and a heatmap column per row without holding the run. */
export const REPLICA_HISTORY = 240;

export interface ServerReplicas {
  /** The most recent row per replica this hook has ever streamed for the run; a replica that left
   * the page keeps its last row, so a sort by a live column stays put instead of flapping. */
  latest: Map<number, ReplicaSample>;
  /** Oldest first, capped at `REPLICA_HISTORY` per replica. */
  history: Map<number, ReplicaSample[]>;
  /** Replica streams open right now, and how many the budget lets this page hold at once. */
  streaming: number[];
  slots: number;
}

const EMPTY_REPLICAS: ServerReplicas = { latest: new Map(), history: new Map(), streaming: [], slots: 0 };

/** How long each replica stream of a page larger than its slots lives before the next id takes its slot. */
export const REPLICA_ROTATE_MS = 2000;

export interface ReplicaStreamsOptions {
  client: IngressClient;
  runId: string;
  /** The page, in the order the slots go round. */
  ids: number[];
  samplesPerSimSecond: number;
  /** The same window the fleet stream is on, so a row and the chart above it agree. */
  smoothingWindowNs?: bigint;
  budget: number;
  onRow: (id: number, row: ReplicaSample) => void;
  openStreamImpl?: SubscribeOptions['openStreamImpl'];
}

/**
 * The replica streams of one page, held within the budget: at most `budget - 1` at once, and fewer
 * when other streams (the fleet's, an A/B pair's) already hold slots. A page larger than that takes
 * turns: `tick()` closes the oldest open stream and opens the next id round-robin, so with a tick
 * every `REPLICA_ROTATE_MS / slots` each stream lives `REPLICA_ROTATE_MS` and every row has streamed
 * within `ceil(page / slots)` rotations, keeping its last row in between. No React and no clock,
 * so the self-test drives it directly.
 */
export class ReplicaStreams {
  private open: Array<{ id: number; handle: SubscriptionHandle }> = [];
  private cursor = 0;
  private closed = false;
  private readonly o: ReplicaStreamsOptions;

  constructor(o: ReplicaStreamsOptions) {
    this.o = o;
    this.fill();
  }

  /** Replica streams this page may hold at once, given what else is open. */
  slots(): number {
    const others = openStreams - this.open.length;
    return Math.max(1, Math.min(this.o.budget - 1, this.o.budget - others));
  }

  /** A page that fits its slots streams whole and never rotates. */
  rotates(): boolean {
    return this.o.ids.length > this.slots();
  }

  streaming(): number[] {
    return this.open.map((s) => s.id);
  }

  tick(): void {
    if (this.closed || !this.rotates()) return;
    this.open.shift()?.handle.close();
    this.fill();
  }

  close(): void {
    this.closed = true;
    for (const s of this.open) s.handle.close();
    this.open = [];
  }

  private fill(): void {
    const { ids } = this.o;
    while (!this.closed && this.open.length < this.slots() && this.open.length < ids.length) {
      const id = ids[this.cursor % ids.length];
      this.cursor++;
      const handle = countedSubscribe(this.o.client, {
        runId: this.o.runId,
        target: uiTargetToWire({ scope: 'REPLICA', id }),
        metrics: REPLICA_ROW_METRICS,
        samplesPerSimSecond: this.o.samplesPerSimSecond,
        percentiles: DEFAULT_PERCENTILES,
        leaseNs: LEASE_NS,
        smoothingWindowNs: this.o.smoothingWindowNs,
        openStreamImpl: this.o.openStreamImpl,
        onUpdate: (u) => this.o.onRow(id, replicaFromUpdate(u)),
      });
      this.open.push({ id, handle });
    }
  }
}

/**
 * One subscription per replica id, for the machine-level page. The fleet stream carries no
 * per-replica rows (a subscription names exactly one entity), so the page asks for the rows it
 * shows and nothing more: the wire is bounded by what is on screen, which is the ui spec's promise.
 * Keyed on the id *set*, so re-sorting a page over the same replicas does not close and reopen
 * anything; paging to other ids does.
 */
export function useServerReplicas(
  runId: string | null,
  ids: number[],
  samplesPerSimSecond: number,
  smoothingWindowNs = 0n,
  client?: IngressClient
): ServerReplicas {
  const mode = useMemo(() => serverMode(), []);
  const c = useMemo(() => client ?? new IngressClient({ baseUrl: mode.baseUrl }), [client, mode.baseUrl]);
  const [state, setState] = useState<ServerReplicas>(EMPTY_REPLICAS);
  const key = JSON.stringify([...new Set(ids)].sort((a, b) => a - b));

  // A new window is a new set of streams and a new history: rows smoothed two ways must not sit in
  // one sparkline.
  useEffect(() => {
    setState(EMPTY_REPLICAS);
  }, [c, runId, smoothingWindowNs]);

  useEffect(() => {
    if (!runId) return;
    const ids = JSON.parse(key) as number[];
    if (ids.length === 0) return;
    const streams = new ReplicaStreams({
      client: c,
      runId,
      ids,
      samplesPerSimSecond,
      smoothingWindowNs,
      budget: streamBudget(),
      onRow: (id, row) =>
        setState((prev) => {
          const latest = new Map(prev.latest);
          latest.set(id, row);
          const history = new Map(prev.history);
          const had = prev.history.get(id) ?? [];
          const next = had.length >= REPLICA_HISTORY ? had.slice(had.length - REPLICA_HISTORY + 1) : had.slice();
          next.push(row);
          history.set(id, next);
          return { ...prev, latest, history };
        }),
    });
    const announce = () => setState((prev) => ({ ...prev, streaming: streams.streaming(), slots: streams.slots() }));
    announce();
    // One slot turns over per tick, so a stream lives a whole `REPLICA_ROTATE_MS` and the opens are
    // spread out rather than all landing on the server at once.
    const timer = streams.rotates()
      ? setInterval(() => {
          streams.tick();
          announce();
        }, REPLICA_ROTATE_MS / Math.max(1, streams.slots()))
      : null;
    return () => {
      if (timer !== null) clearInterval(timer);
      streams.close();
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [c, runId, key, samplesPerSimSecond, smoothingWindowNs]);

  return state;
}
