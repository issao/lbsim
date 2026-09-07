// The run controller: playback plus the two update calls, over one MockEngine.
//
// The wire calls this stands in for are SetSpeed, StepForward, Rewind, UpdateWorkload and
// UpdatePolicies. Their responses carry information the UI is obliged to show -- where a step
// stopped, whether a rewind came from the log, whether an update forced re-simulation -- so they
// are modelled as return values here rather than being swallowed.
//
// Two handles share the shape. `useRun` drives the in-browser mock engine, exactly as it always
// has. `useReplayRun` drives a recorded run loaded from static files (replay.ts): play, pause,
// speed, step and scrub work locally over frames that already exist, and the two things only a
// live engine can do -- rewind-and-resimulate and a workload or policy change -- are refused with
// a visible reason rather than pretended. Panels take a `RunHandle` and cannot tell which they got,
// apart from reading `source`.

import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { type FleetEvent, type Frame, MockEngine } from './engine';
import type { ScenarioConfig } from './config';
import { cloneConfig, diffConfig, FIELD_LABEL } from './config';
import type { RewindResponse, UpdateResponse } from './types';
import { clamp } from './rng';
import { type DataMode, type ServerMode, MOCK_BANNER, REPLAY_BANNER, dataModeFrom, probeServer, replayOverride, serverMode } from './mode';
import { IngressClient } from './api';
import {
  type LoadedRun,
  REPLAY_DISABLED_REASON,
  ReplayEngine,
  type RunIndexEntry,
  clampCursor,
  probeRunIndex,
  stepCursor,
} from './replay';

export const SPEEDS = [0.25, 0.5, 1, 2, 5, 10];
/** StepForward is bounded server-side; this is the stand-in's bound. */
export const STEP_S = 2;

/**
 * What the panels read frames through. The mock engine satisfies it as it is; a replay satisfies
 * it over decoded frames. Nothing a panel needs is outside this interface, and if a panel comes to
 * need more, that is a design question rather than a cast.
 */
export interface FrameSource {
  config: ScenarioConfig;
  frames: Frame[];
  /** Simulated seconds recorded so far. Scrubbing inside this is a log read. */
  recordedToS: number;
  frameAt(simS: number): Frame | undefined;
  /** Frames covering [fromS, toS], decimated to the configured chart sample rate. */
  window(fromS: number, toS: number): Frame[];
  eventsUpTo(simS: number): FleetEvent[];
}

/** Where a handle's numbers come from, so the status bar and the header can say so. */
export interface RunSourceInfo {
  kind: DataMode;
  /** The banner text for this source; the mock's is the unchanged marker. */
  label: string;
  /** Why rewind-and-resimulate and updates are refused, or null where they work. */
  disabledReason: string | null;
  runId?: string;
}

export const MOCK_SOURCE: RunSourceInfo = { kind: 'mock', label: MOCK_BANNER, disabledReason: null };

export interface RunHandle {
  engine: FrameSource;
  config: ScenarioConfig;
  cursorS: number;
  recordedToS: number;
  durationS: number;
  paused: boolean;
  speed: number;
  /** Set while the stand-in is re-simulating, so panels can say so rather than lying. */
  resimulating: boolean;
  lastRewind: RewindResponse | null;
  lastUpdate: UpdateResponse | null;
  revision: number;
  setPaused: (p: boolean) => void;
  setSpeed: (f: number) => void;
  step: () => void;
  rewindTo: (s: number) => void;
  scrubTo: (s: number) => void;
  /** Null when the answer is decided by a round trip and lands on `lastUpdate` instead. */
  update: (next: ScenarioConfig) => UpdateResponse | null;
  restart: (next: ScenarioConfig) => void;
  dismissUpdate: () => void;
  /** Optional only so the server handle, which is shaped by `Omit`, keeps compiling unchanged. */
  source?: RunSourceInfo;
}

export function useRun(initial: ScenarioConfig, autoplay = true): RunHandle {
  // Open at the end of warm-up with the window already populated: a dashboard whose panels are
  // empty for the first minute cannot be judged.
  const startS = Math.min(initial.warmupS, initial.durationS);
  const engine = useMemo(() => {
    const e = new MockEngine(initial);
    e.simulateTo(Math.min(startS + 6, initial.durationS));
    return e;
    // one engine per mount, seeded from the initial config
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const [config, setConfig] = useState<ScenarioConfig>(() => cloneConfig(initial));
  const [paused, setPaused] = useState(!autoplay);
  const [speed, setSpeed] = useState(2);
  const [cursorS, setCursorS] = useState(startS);
  const [recordedToS, setRecordedToS] = useState(engine.recordedToS);
  const [lastRewind, setLastRewind] = useState<RewindResponse | null>(null);
  const [lastUpdate, setLastUpdate] = useState<UpdateResponse | null>(null);
  const [resimulating, setResimulating] = useState(false);
  const [revision, setRevision] = useState(0);

  const cursorRef = useRef(startS);
  const lastPaint = useRef(0);

  useEffect(() => {
    let raf = 0;
    let prev = performance.now();
    const tick = (now: number) => {
      const dt = Math.min((now - prev) / 1000, 0.25);
      prev = now;
      if (!paused) {
        const next = Math.min(cursorRef.current + dt * speed, engine.config.durationS);
        cursorRef.current = next;
        if (next > engine.recordedToS - 1) {
          if (engine.simulateTo(next + 4)) setRecordedToS(engine.recordedToS);
        }
        if (next >= engine.config.durationS) setPaused(true);
      }
      if (now - lastPaint.current > 80) {
        lastPaint.current = now;
        setCursorS(cursorRef.current);
      }
      raf = requestAnimationFrame(tick);
    };
    raf = requestAnimationFrame(tick);
    return () => cancelAnimationFrame(raf);
  }, [paused, speed, engine]);

  const commit = useCallback((s: number) => {
    cursorRef.current = s;
    setCursorS(s);
  }, []);

  const step = useCallback(() => {
    setPaused(true);
    const to = clamp(cursorRef.current + STEP_S, 0, engine.config.durationS);
    if (engine.simulateTo(to)) setRecordedToS(engine.recordedToS);
    // StepForward's response says where it stopped, which may be short of what was asked.
    commit(Math.min(to, engine.recordedToS));
  }, [engine, commit]);

  const rewindTo = useCallback(
    (s: number) => {
      const target = clamp(s, 0, engine.config.durationS);
      const beyond = target > engine.recordedToS;
      if (beyond) setResimulating(true);
      const resp = engine.rewind(target);
      setRecordedToS(engine.recordedToS);
      setLastRewind(resp);
      commit(resp.simTimeS);
      if (beyond) window.setTimeout(() => setResimulating(false), 220);
    },
    [engine, commit]
  );

  const scrubTo = rewindTo;

  const update = useCallback(
    (next: ScenarioConfig): UpdateResponse => {
      const resp = engine.applyConfig(next, cursorRef.current);
      setConfig(cloneConfig(next));
      setRecordedToS(engine.recordedToS);
      setLastUpdate(resp.changed.length ? resp : null);
      setRevision((r) => r + 1);
      if (resp.requiredResimulation) {
        setResimulating(true);
        window.setTimeout(() => setResimulating(false), 260);
        if (cursorRef.current > engine.recordedToS) commit(engine.recordedToS);
      }
      return resp;
    },
    [engine, commit]
  );

  const restart = useCallback(
    (next: ScenarioConfig) => {
      const s0 = Math.min(next.warmupS, next.durationS);
      engine.config = cloneConfig(next);
      engine.reset();
      engine.simulateTo(Math.min(s0 + 6, next.durationS));
      setConfig(cloneConfig(next));
      setRecordedToS(engine.recordedToS);
      setLastUpdate(null);
      setLastRewind(null);
      setRevision((r) => r + 1);
      commit(s0);
    },
    [engine, commit]
  );

  return {
    engine,
    config,
    cursorS,
    recordedToS,
    durationS: config.durationS,
    paused,
    speed,
    resimulating,
    lastRewind,
    lastUpdate,
    revision,
    setPaused,
    setSpeed,
    step,
    rewindTo,
    scrubTo,
    update,
    restart,
    dismissUpdate: () => setLastUpdate(null),
    source: MOCK_SOURCE,
  };
}

// ---------------------------------------------------------------------------
// Replay
// ---------------------------------------------------------------------------

export interface ReplayRunHandle extends RunHandle {
  engine: ReplayEngine;
  source: RunSourceInfo & { kind: 'replay'; runId: string; disabledReason: string };
  loaded: LoadedRun;
  /** The reason every refused control gives. Constant; here so a panel need not import replay.ts. */
  disabledReason: string;
  /** The last change that was refused, named, so the banner can say what did not happen. */
  refused: string | null;
  dismissRefused: () => void;
}

/**
 * The replay branch. Same handle, same units, no engine behind it: the cursor moves over frames
 * that are all already loaded, which is what makes scrubbing a log read everywhere (M8 met
 * trivially). `update` still applies a view-only change -- an SLO threshold or the sample rate --
 * because attainment is derived from the recorded distributions at render time; a physics change
 * is refused and named. `restart` is refused too: the way to see a different run is to pick one.
 */
export function useReplayRun(loaded: LoadedRun, autoplay = true): ReplayRunHandle {
  const engine = useMemo(() => new ReplayEngine(loaded.frames, cloneConfig(loaded.config)), [loaded]);
  const durationS = engine.durationS;
  // Open where the recording has its first completion. The exporter buckets only the measured
  // records (`RunResult.records` excludes warm-up), so every window before warm-up ends has zero
  // completions and no latency; opening there would show a dashboard of zeros and dashes.
  const startS = clampCursor(loaded.frames.find((f) => f.completedRps > 0)?.simS ?? loaded.config.warmupS, durationS);

  const [config, setConfig] = useState<ScenarioConfig>(() => cloneConfig(loaded.config));
  const [paused, setPaused] = useState(!autoplay);
  const [speed, setSpeed] = useState(2);
  const [cursorS, setCursorS] = useState(startS);
  const [lastRewind, setLastRewind] = useState<RewindResponse | null>(null);
  const [lastUpdate, setLastUpdate] = useState<UpdateResponse | null>(null);
  const [refused, setRefused] = useState<string | null>(null);
  const [revision, setRevision] = useState(0);

  const cursorRef = useRef(startS);
  const lastPaint = useRef(0);

  // The chart's decimation reads the engine's config; keep it in step with view-only edits.
  engine.config = config;

  useEffect(() => {
    let raf = 0;
    let prev = performance.now();
    const tick = (now: number) => {
      // Wall-clock time paces playback only: it advances the cursor by `speed` simulated seconds
      // per real second and is never compared with a simulated instant.
      const dt = Math.min((now - prev) / 1000, 0.25);
      prev = now;
      if (!paused) {
        const next = clampCursor(cursorRef.current + dt * speed, durationS);
        cursorRef.current = next;
        if (next >= durationS) setPaused(true);
      }
      if (now - lastPaint.current > 80) {
        lastPaint.current = now;
        setCursorS(cursorRef.current);
      }
      raf = requestAnimationFrame(tick);
    };
    raf = requestAnimationFrame(tick);
    return () => cancelAnimationFrame(raf);
  }, [paused, speed, durationS]);

  const commit = useCallback((s: number) => {
    cursorRef.current = s;
    setCursorS(s);
  }, []);

  const step = useCallback(() => {
    setPaused(true);
    commit(stepCursor(cursorRef.current, STEP_S, durationS));
  }, [commit, durationS]);

  // Every scrub is a log read: the whole run is recorded, so `fromLog` is always true and there is
  // no snapshot to restore. A target past the end stops at the end and says so through simTimeS.
  const scrubTo = useCallback(
    (s: number) => {
      const target = clampCursor(s, durationS);
      setLastRewind({ simTimeS: target, fromLog: true, restoredFromSnapshotS: target });
      commit(target);
    },
    [commit, durationS]
  );

  const update = useCallback(
    (next: ScenarioConfig): UpdateResponse => {
      const d = diffConfig(config, next);
      const changed = d.paths.map((p) => FIELD_LABEL[p] ?? p);
      if (d.paths.length === 0) {
        return { accepted: true, requiredResimulation: false, rewoundToS: cursorRef.current, rejectedReason: '', changed };
      }
      if (d.physicsPaths.length > 0) {
        const names = d.physicsPaths.map((p) => FIELD_LABEL[p] ?? p).join(', ');
        setRefused(`${names}: not applied. ${REPLAY_DISABLED_REASON}.`);
        return { accepted: false, requiredResimulation: false, rewoundToS: cursorRef.current, rejectedReason: REPLAY_DISABLED_REASON, changed };
      }
      setConfig(cloneConfig(next));
      setRevision((r) => r + 1);
      const resp: UpdateResponse = { accepted: true, requiredResimulation: false, rewoundToS: cursorRef.current, rejectedReason: '', changed };
      setLastUpdate(resp);
      return resp;
    },
    [config]
  );

  const restart = useCallback((next: ScenarioConfig) => {
    const d = diffConfig(config, next);
    const names = d.paths.map((p) => FIELD_LABEL[p] ?? p).join(', ') || 'restart';
    setRefused(`${names}: not applied. ${REPLAY_DISABLED_REASON}; pick another recorded run instead.`);
  }, [config]);

  return {
    engine,
    config,
    cursorS,
    recordedToS: durationS,
    durationS,
    paused,
    speed,
    resimulating: false,
    lastRewind,
    lastUpdate,
    revision,
    setPaused,
    setSpeed,
    step,
    rewindTo: scrubTo,
    scrubTo,
    update,
    restart,
    dismissUpdate: () => setLastUpdate(null),
    source: { kind: 'replay', label: `${REPLAY_BANNER}: ${loaded.entry.runId}`, disabledReason: REPLAY_DISABLED_REASON, runId: loaded.entry.runId },
    loaded,
    disabledReason: REPLAY_DISABLED_REASON,
    refused,
    dismissRefused: () => setRefused(null),
  };
}

// ---------------------------------------------------------------------------
// Which source a dashboard should use
// ---------------------------------------------------------------------------

export type DataSource =
  | { state: 'probing' }
  | { state: 'mock' }
  | { state: 'replay'; runs: RunIndexEntry[] }
  | { state: 'server'; server: ServerMode };

// One probe per page load: the answer does not change while the page lives, and re-probing on
// every dashboard mount would add two round trips to each navigation.
let probe: Promise<DataSource> | null = null;

async function probeAll(): Promise<DataSource> {
  const override = typeof window === 'undefined' ? null : replayOverride(window.location.search, window.location.hash);
  const server = serverMode();
  // Both at once: the server probe has a 1.5 s budget, and a page that waited it out before asking
  // for the index would feel broken when there is no server, which is the common case.
  const [reachable, runs] = await Promise.all([
    probeServer(new IngressClient({ baseUrl: server.baseUrl }), server),
    override === false ? Promise.resolve(null) : probeRunIndex(),
  ]);
  const m = dataModeFrom(server, runs !== null, override, reachable);
  if (m === 'server') return { state: 'server', server };
  if (m === 'replay' && runs !== null) return { state: 'replay', runs };
  return { state: 'mock' };
}

/**
 * The one probe every surface decides from. The walkthrough page calls it too, so what a script
 * drives (live, replay or mock) is the same answer the dashboard under it acts on.
 */
export function probeDataSource(): Promise<DataSource> {
  if (probe === null) probe = probeAll();
  return probe;
}

/**
 * Server when `ListRuns` answers, replay when `runs/index.json` is served and non-empty, mock
 * otherwise. `enabled = false` skips the probes and answers mock at once, for a surface that is
 * mock by design (the walkthroughs).
 */
export function useDataSource(enabled = true): DataSource {
  const [src, setSrc] = useState<DataSource>(enabled ? { state: 'probing' } : { state: 'mock' });
  useEffect(() => {
    if (!enabled) return;
    let alive = true;
    void probeDataSource().then((s) => {
      if (alive) setSrc(s);
    });
    return () => {
      alive = false;
    };
  }, [enabled]);
  return src;
}
