// The run controller: playback plus the two update calls, over one MockEngine.
//
// The wire calls this stands in for are SetSpeed, StepForward, Rewind, UpdateWorkload and
// UpdatePolicies. Their responses carry information the UI is obliged to show -- where a step
// stopped, whether a rewind came from the log, whether an update forced re-simulation -- so they
// are modelled as return values here rather than being swallowed.

import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { MockEngine } from './engine';
import type { ScenarioConfig } from './config';
import { cloneConfig } from './config';
import type { RewindResponse, UpdateResponse } from './types';
import { clamp } from './rng';

export const SPEEDS = [0.25, 0.5, 1, 2, 5, 10];
/** StepForward is bounded server-side; this is the stand-in's bound. */
export const STEP_S = 2;

export interface RunHandle {
  engine: MockEngine;
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
  update: (next: ScenarioConfig) => UpdateResponse;
  restart: (next: ScenarioConfig) => void;
  dismissUpdate: () => void;
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
  };
}
