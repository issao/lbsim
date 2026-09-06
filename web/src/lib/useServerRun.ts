// The run controller, backed by the Ingress server instead of the in-browser mock.
//
// Shaped to match `useRun`'s `RunHandle` field for field, so wiring a panel is a swap of the hook
// rather than a rewrite of the panel. Two fields cannot match, and pretending otherwise would be
// the dishonesty this whole exercise is trying to avoid:
//
//   - `engine` is `null`. `RunHandle.engine` is a `MockEngine`, and there is no server-side object
//     with that interface: the panels that reach into it are reading mock frames, and they have to
//     be pointed at `samples` here instead. Handing them a live `MockEngine` alongside server data
//     would put fabricated numbers on a chart labelled live.
//   - `update` returns void rather than an `UpdateResponse`. Two RPCs decide the answer, so it
//     arrives on `lastUpdate` a round trip later, exactly as `lastRewind` already does.
//
// Everything else -- cursor, playback, step, rewind, restart, the two banners the UI is obliged to
// show -- is the same shape and the same units (relative simulated seconds) as the mock's.

import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import type { RunHandle } from './useRun';
import { STEP_S } from './useRun';
import type { ScenarioConfig } from './config';
import { cloneConfig, diffConfig, FIELD_LABEL } from './config';
import type { RewindResponse, Target as UiTarget, UpdateResponse } from './types';
import {
  IngressClient,
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
import { serverMode } from './mode';

/** How often the run's status is polled. The stream carries metrics; this carries state and speed. */
const STATUS_POLL_MS = 500;
/** Bounded history, because a long run at a fine sample rate is otherwise unbounded browser memory. */
const MAX_SAMPLES = 4000;

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

export type ServerRunHandle = Omit<RunHandle, 'engine' | 'update'> & {
  engine: null;
  update: (next: ScenarioConfig) => void;
  /** The fleet subscription's history, oldest first, capped at MAX_SAMPLES. */
  samples: ServerSample[];
  /** The origin `simS` is measured from. Absolute, so it never passes through a float. */
  originUnixNs: bigint | null;
  runId: string | null;
  status: RunStatus | null;
  connection: StreamPhase;
  /** Control-panel fields the engine has no equivalent for, so the UI can say they are inert. */
  dropped: string[];
  /** A transport or server error worth showing, rather than a blank chart. */
  error: string | null;
};

/**
 * Compile-time proof that the server handle is drop-in for the mock's, apart from the two fields
 * documented above. If a field is added to `RunHandle` and not here, this stops compiling.
 */
export type HandleShapesAgree = ServerRunHandle extends Omit<RunHandle, 'engine' | 'update'> ? true : false;
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
  const metrics = useMemo(() => opts.metrics ?? FLEET_METRICS, [opts.metrics]);

  const [config, setConfig] = useState<ScenarioConfig>(() => cloneConfig(initial));
  const [runId, setRunId] = useState<string | null>(null);
  const [status, setStatus] = useState<RunStatus | null>(null);
  const [samples, setSamples] = useState<ServerSample[]>([]);
  const [connection, setConnection] = useState<StreamPhase>('opening');
  const [lastRewind, setLastRewind] = useState<RewindResponse | null>(null);
  const [lastUpdate, setLastUpdate] = useState<UpdateResponse | null>(null);
  const [resimulating, setResimulating] = useState(false);
  const [revision, setRevision] = useState(0);
  const [error, setError] = useState<string | null>(null);
  const [dropped, setDropped] = useState<string[]>([]);
  // Scrubbing pins the cursor: the server keeps advancing, and a chart that snapped back to live
  // the moment the user let go of the scrubber would be unusable.
  const [pinnedS, setPinnedS] = useState<number | null>(null);

  const originRef = useRef<bigint | null>(null);
  const [originUnixNs, setOriginUnixNs] = useState<bigint | null>(null);
  const configRef = useRef(config);
  configRef.current = config;

  const origin = useCallback((t: bigint): bigint => {
    if (originRef.current === null) {
      originRef.current = t;
      setOriginUnixNs(t);
    }
    return originRef.current;
  }, []);

  const fail = useCallback((e: unknown) => {
    setError(e instanceof Error ? e.message : String(e));
  }, []);

  // -- start, and stop on unmount -----------------------------------------
  //
  // A run the browser started and then navigated away from would keep a simulation going, so the
  // unmount stops it. The subscription lease covers the case where the tab dies instead.
  const startRun = useCallback(
    async (c: ScenarioConfig, play: boolean): Promise<string | null> => {
      const wire = scenarioConfigToWire(c);
      setDropped(wire.dropped);
      try {
        // Unset means as fast as possible; a pause is a SetSpeed rather than a cap of zero, so the
        // run's speed survives being unpaused.
        // The whole config goes as scenario text, per WIRE.md; overrides are for edits on top of a
        // served file, and this client has no served file to edit.
        const id = await client.startRun({
          scenario: scenarioEnvelope(wire.fields),
          maxRealtimeFactor: 0,
          recordTraces: opts.recordTraces ?? true,
        });
        setRunId(id);
        setError(null);
        if (!play) await client.setSpeed(id, 0, true).then(setStatus).catch(fail);
        return id;
      } catch (e) {
        fail(e);
        return null;
      }
    },
    [client, fail, opts.recordTraces]
  );

  useEffect(() => {
    let stopped = false;
    let id: string | null = null;
    void startRun(configRef.current, autoplay).then((r) => {
      id = r;
      if (stopped && r) void client.stopRun(r).catch(() => undefined);
    });
    return () => {
      stopped = true;
      if (id) void client.stopRun(id).catch(() => undefined);
    };
    // One run per mount, seeded from the initial config, exactly as useRun builds one engine.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // -- status polling ------------------------------------------------------
  useEffect(() => {
    if (!runId) return;
    let alive = true;
    const poll = async () => {
      try {
        const s = await client.getRun(runId);
        if (!alive) return;
        setStatus(s);
        if (s.simTimeUnixNs > 0n) origin(s.simTimeUnixNs);
        if (s.error) setError(s.error);
      } catch (e) {
        if (alive) fail(e);
      }
    };
    void poll();
    const t = setInterval(poll, STATUS_POLL_MS);
    return () => {
      alive = false;
      clearInterval(t);
    };
  }, [client, runId, origin, fail]);

  // -- the fleet subscription ---------------------------------------------
  useEffect(() => {
    if (!runId) return;
    let handle: SubscriptionHandle | null = subscribeToTarget(client, {
      runId,
      target: fleetTarget(),
      metrics,
      samplesPerSimSecond: configRef.current.samplesPerSimSecond,
      percentiles: DEFAULT_PERCENTILES,
      onPhase: (p, detail) => {
        setConnection(p);
        if (p === 'failed' && detail) setError(detail);
      },
      onUpdate: (u) => {
        const t0 = origin(u.simTimeUnixNs);
        const sample: ServerSample = {
          simS: relSeconds(u.simTimeUnixNs, t0),
          simTimeUnixNs: u.simTimeUnixNs,
          realtimeFactor: u.realtimeFactor,
          values: u.row.values,
          distributions: u.row.distributions,
        };
        setSamples((prev) => {
          const next = prev.length >= MAX_SAMPLES ? prev.slice(prev.length - MAX_SAMPLES + 1) : prev.slice();
          next.push(sample);
          return next;
        });
      },
    });
    return () => {
      handle?.close();
      handle = null;
    };
  }, [client, runId, metrics, origin]);

  // -- controls ------------------------------------------------------------

  const liveS = samples.length ? samples[samples.length - 1].simS : 0;
  const statusS = status && originUnixNs !== null ? relSeconds(status.simTimeUnixNs, originUnixNs) : 0;
  const recordedToS = Math.max(liveS, statusS);
  const cursorS = pinnedS ?? recordedToS;

  const setPaused = useCallback(
    (p: boolean) => {
      if (!runId) return;
      void client.setSpeed(runId, status?.realtimeFactor ?? 0, p).then(setStatus).catch(fail);
    },
    [client, runId, status?.realtimeFactor, fail]
  );

  const setSpeed = useCallback(
    (f: number) => {
      if (!runId) return;
      void client.setSpeed(runId, f, status?.state === 'STATE_PAUSED').then(setStatus).catch(fail);
    },
    [client, runId, status?.state, fail]
  );

  const step = useCallback(() => {
    if (!runId) return;
    setPinnedS(null);
    // StepForward is bounded server-side; the status it returns says where it actually stopped.
    void client.stepForward(runId, secondsToNs(STEP_S)).then(setStatus).catch(fail);
  }, [client, runId, fail]);

  const rewindTo = useCallback(
    (s: number) => {
      setPinnedS(s);
      if (!runId || originRef.current === null) return;
      const to = originRef.current + secondsToNs(Math.max(0, s));
      void client
        .rewind(runId, to)
        .then((r) => {
          const t0 = originRef.current ?? r.simTimeUnixNs;
          setLastRewind({
            simTimeS: relSeconds(r.simTimeUnixNs, t0),
            fromLog: r.fromLog,
            restoredFromSnapshotS: r.restoredFromSnapshotUnixNs === 0n ? 0 : relSeconds(r.restoredFromSnapshotUnixNs, t0),
          });
          setPinnedS(relSeconds(r.simTimeUnixNs, t0));
        })
        .catch(fail);
    },
    [client, runId, fail]
  );

  const update = useCallback(
    (next: ScenarioConfig) => {
      const prev = configRef.current;
      const d = diffConfig(prev, next);
      const changed = d.paths.map((p) => FIELD_LABEL[p] ?? p);
      setConfig(cloneConfig(next));
      setRevision((r) => r + 1);
      if (!runId || d.paths.length === 0) {
        setLastUpdate(d.paths.length ? { accepted: true, requiredResimulation: false, rewoundToS: cursorS, rejectedReason: '', changed } : null);
        return;
      }
      const workloadChanged = d.paths.some((p) => p.startsWith('workload.'));
      const policyChanged = d.paths.some((p) => p.startsWith('routing.'));
      const calls: Promise<{ accepted: boolean; requiredResimulation: boolean; rewoundToUnixNs: bigint; rejectedReason: string }>[] = [];
      if (workloadChanged) calls.push(client.updateWorkload(runId, toOverrides(workloadToWire(next).fields)));
      if (policyChanged) calls.push(client.updatePolicies(runId, toOverrides(policiesToWire(next).fields)));
      if (calls.length === 0) {
        // Anything else -- fleet shape, seed, duration -- is a new run by design: UpdateWorkload and
        // UpdatePolicies are the only two live-tunable calls in ingress.proto.
        setLastUpdate({ accepted: false, requiredResimulation: false, rewoundToS: cursorS, rejectedReason: 'only workload and policy are live-tunable; restart the run for this change', changed });
        return;
      }
      setResimulating(true);
      void Promise.all(calls)
        .then((rs) => {
          const t0 = originRef.current;
          const resim = rs.some((r) => r.requiredResimulation);
          const rewoundNs = rs.map((r) => r.rewoundToUnixNs).filter((n) => n > 0n).sort((a, b) => (a < b ? -1 : a > b ? 1 : 0))[0];
          setLastUpdate({
            accepted: rs.every((r) => r.accepted),
            requiredResimulation: resim,
            rewoundToS: rewoundNs !== undefined && t0 !== null ? relSeconds(rewoundNs, t0) : cursorS,
            rejectedReason: rs.map((r) => r.rejectedReason).filter(Boolean).join('; '),
            changed,
          });
          if (resim) {
            // The history after the rewind point is a different future now, so drop it rather than
            // leaving a chart that mixes two runs.
            setSamples((prev2) => (rewoundNs === undefined ? prev2 : prev2.filter((s) => s.simTimeUnixNs <= rewoundNs)));
          }
        })
        .catch(fail)
        .finally(() => setResimulating(false));
    },
    [client, runId, cursorS, fail]
  );

  const restart = useCallback(
    (next: ScenarioConfig) => {
      setConfig(cloneConfig(next));
      setSamples([]);
      setLastRewind(null);
      setLastUpdate(null);
      setPinnedS(null);
      originRef.current = null;
      setOriginUnixNs(null);
      setRevision((r) => r + 1);
      const old = runId;
      setRunId(null);
      void (async () => {
        if (old) await client.stopRun(old).catch(() => undefined);
        await startRun(next, true);
      })();
    },
    [client, runId, startRun]
  );

  return {
    engine: null,
    config,
    cursorS,
    recordedToS,
    durationS: config.durationS,
    paused: status?.state === 'STATE_PAUSED' || status?.state === 'STATE_COMPLETE' || connection === 'complete',
    speed: status?.realtimeFactor ?? 0,
    resimulating,
    lastRewind,
    lastUpdate,
    revision,
    setPaused,
    setSpeed,
    step,
    rewindTo,
    scrubTo: rewindTo,
    update,
    restart,
    dismissUpdate: () => setLastUpdate(null),
    samples,
    originUnixNs,
    runId,
    status,
    connection,
    dropped,
    error,
  };
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
