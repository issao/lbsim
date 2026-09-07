import { useEffect, useState, type ReactNode } from 'react';
import type { ScenarioConfig } from '../lib/config';
import { useDataSource, useReplayRun, useRun, type ReplayRunHandle, type RunHandle } from '../lib/useRun';
import { speedLabel, useServerRun, type ServerRunHandle } from '../lib/useServerRun';
import { windowFrames } from '../lib/derive';
import { dataSourceGloss, setActiveMode } from '../lib/mode';
import { groupRuns, loadRun, runDurationS, type LoadedRun, type RunIndexEntry } from '../lib/replay';
import { PlaybackBar, UpdateBanner } from '../components/PlaybackBar';
import { ControlPanel, type ControlTab } from '../panels/ControlPanel';
import { ObservationPanel, type ObserveTab } from '../panels/ObservationPanel';
import { StatusBar } from '../panels/StatusBar';

export interface TabHint {
  control?: ControlTab;
  observe?: ObserveTab;
  /** Bumped by the caller to re-apply the same hint. */
  nonce: number;
}

interface DashboardProps {
  initial: ScenarioConfig;
  autoplay?: boolean;
  onRun?: (run: RunHandle) => void;
  highlight?: string | null;
  tabHint?: TabHint;
  overlay?: ReactNode;
  /**
   * `auto` runs on the Ingress server when one answers, plays recorded runs when `runs/index.json`
   * is served, and the mock otherwise; `mock` and `server` never probe. The showcase asks for one
   * of those two when it has already decided where a walkthrough runs, so the decision is made
   * once rather than twice.
   */
  data?: 'auto' | 'mock' | 'server';
  /**
   * The recorded run to open when the replay branch is taken; `?run=` on the URL otherwise. A
   * caller that knows which recording it wants (the showcase) says so here rather than by
   * rewriting the URL under the dashboard's feet.
   */
  run?: string;
}

/**
 * The load-test dashboard: playback across the top, control panel left, observation suite right.
 * Reused by the showcase, which drives the same surface from a walkthrough script rather than by
 * hand -- the walkthrough is content, not a second interface.
 *
 * The surface is one component, `DashboardBody`, over one `RunHandle`. What differs is only where
 * the handle comes from: the mock engine, a recorded run picked from the served index, or a run
 * started on the Ingress server.
 */
export function Dashboard(props: DashboardProps) {
  const data = props.data ?? 'auto';
  const src = useDataSource(data === 'auto');
  const state = data === 'server' ? 'server' : src.state;
  useEffect(() => {
    if (state === 'mock') setActiveMode('mock');
  }, [state]);
  if (state === 'probing') return <div className="page-pad">looking for a server or recorded runs…</div>;
  if (state === 'server') return <ServerDashboard {...props} />;
  if (src.state === 'replay') return <ReplayDashboard {...props} runs={src.runs} />;
  return <MockDashboard {...props} />;
}

// ---------------------------------------------------------------------------
// Live: a run started on the Ingress server, streamed as it advances
// ---------------------------------------------------------------------------

function ServerDashboard({ initial, autoplay = true, run: _recording, ...rest }: DashboardProps) {
  const run = useServerRun(initial, { autoplay });
  useEffect(() => {
    setActiveMode('server', run.runId ?? undefined);
  }, [run.runId]);
  return <DashboardBody run={run} banner={<ServerBanner run={run} />} {...rest} />;
}

/** Says where the numbers come from, what the server cannot do yet, and names what it refused. */
function ServerBanner({ run }: { run: ServerRunHandle }) {
  return (
    <>
      <div className="banner">
        <span className="tagline">live</span>
        <span className="note">{dataSourceGloss('server')}</span>
        <span>
          run <code>{run.runId ?? '…'}</code> &middot; stream {run.connection}
          {run.subscriptionId ? <> ({run.subscriptionId})</> : null} &middot; {run.engine.frames.length} samples at{' '}
          {run.config.samplesPerSimSecond}/sim s &middot; speed {speedLabel(run.status, run.paused)}
          {run.dropped.length ? <> &middot; inert controls: {run.dropped.join(', ')}</> : null}
          {run.unserved.length ? <> &middot; not served by this server, panels stay mock: {run.unserved.join(', ')}</> : null}
        </span>
        <span style={{ marginLeft: 'auto', color: 'var(--ink-3)' }} title={run.disabledReason}>
          load and policy changes go to the server; scrub is a local read; rewind is off: {run.disabledReason}
        </span>
      </div>
      {run.error ? (
        <div className="banner">
          <span className="tagline">server</span>
          <span style={{ color: 'var(--critical)' }}>{run.error}</span>
        </div>
      ) : null}
      {run.refused ? (
        <div className="banner">
          <span className="tagline">not applied</span>
          <span style={{ color: 'var(--serious)' }}>{run.refused}</span>
          <button className="btn" onClick={run.dismissRefused}>
            dismiss
          </button>
        </div>
      ) : null}
    </>
  );
}

function MockDashboard({ initial, autoplay = true, run: _recording, ...rest }: DashboardProps) {
  const run = useRun(initial, autoplay);
  return <DashboardBody run={run} {...rest} />;
}

// ---------------------------------------------------------------------------
// Replay: pick a run from the index, load it, drive the same body
// ---------------------------------------------------------------------------

/**
 * The `run` prop picks the run when the caller has one; `?run=<run_id>` on the URL picks it for
 * a tab opened by hand; otherwise the first one in the index.
 */
function initialRunId(runs: RunIndexEntry[], wanted: string | undefined): string {
  const asked = wanted ?? new URLSearchParams(window.location.search).get('run');
  return runs.find((r) => r.runId === asked)?.runId ?? runs[0].runId;
}

function ReplayDashboard({ runs, run: wanted, ...rest }: DashboardProps & { runs: RunIndexEntry[] }) {
  const [runId, setRunId] = useState(() => initialRunId(runs, wanted));
  useEffect(() => {
    // A caller that changes its mind is obeyed; a name not in the index leaves the picker alone.
    if (wanted && runs.some((r) => r.runId === wanted)) setRunId(wanted);
  }, [runs, wanted]);
  const [loaded, setLoaded] = useState<LoadedRun | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    const entry = runs.find((r) => r.runId === runId) ?? runs[0];
    let alive = true;
    setError(null);
    setActiveMode('replay', entry.runId);
    loadRun(entry)
      .then((l) => {
        if (alive) setLoaded(l);
      })
      .catch((e: unknown) => {
        if (alive) setError(e instanceof Error ? e.message : String(e));
      });
    return () => {
      alive = false;
    };
  }, [runs, runId]);

  const picker = <RunPicker runs={runs} value={runId} onChange={setRunId} />;
  if (error) {
    return (
      <div className="dash">
        <div className="banner">
          <span className="tagline">replay</span>
          {picker}
          <span style={{ color: 'var(--critical)' }}>could not load {runId}: {error}</span>
        </div>
      </div>
    );
  }
  if (!loaded || loaded.entry.runId !== runId) {
    return (
      <div className="dash">
        <div className="banner">
          <span className="tagline">replay</span>
          {picker}
          <span>loading {runId}…</span>
        </div>
      </div>
    );
  }
  // Keyed on the run so a new pick mounts a fresh handle rather than rewinding the old one.
  return <ReplayBody key={loaded.entry.runId} loaded={loaded} picker={picker} {...rest} />;
}

function ReplayBody({
  loaded,
  picker,
  autoplay = true,
  ...rest
}: Omit<DashboardProps, 'initial' | 'run'> & { loaded: LoadedRun; picker: ReactNode }) {
  const run = useReplayRun(loaded, autoplay);
  return <DashboardBody run={run} banner={<ReplayBanner run={run} picker={picker} />} {...rest} />;
}

function RunPicker({ runs, value, onChange }: { runs: RunIndexEntry[]; value: string; onChange: (id: string) => void }) {
  return (
    <label className="field" style={{ display: 'inline-flex', gap: 6, alignItems: 'center', margin: 0 }}>
      <span className="field-label">run</span>
      <select value={value} onChange={(e) => onChange(e.target.value)} aria-label="recorded run">
        {groupRuns(runs).map((g) => (
          <optgroup key={g.group || '(runs)'} label={g.group || 'runs'}>
            {g.runs.map((r) => (
              <option key={r.runId} value={r.runId}>
                {r.label} — {r.routing}
              </option>
            ))}
          </optgroup>
        ))}
      </select>
    </label>
  );
}

/** Says what this is, what it cannot do and why, and names the last thing it refused. */
function ReplayBanner({ run, picker }: { run: ReplayRunHandle; picker: ReactNode }) {
  const e = run.loaded.entry;
  const r = run.loaded.result;
  return (
    <>
      <div className="banner">
        <span className="tagline">replay</span>
        <span className="note">{dataSourceGloss('replay')}</span>
        {picker}
        <span>
          <code>runs/{e.runId}</code> &middot; routing {e.routing} &middot; {e.replicas} replicas &middot;{' '}
          {run.loaded.frames.length} samples at {1000 / e.sampleIntervalMs}/sim s over {runDurationS(e).toFixed(0)} s
          &middot; seed {r.seed.toString()} &middot; checksum {r.stateChecksum.toString()}
        </span>
        <span style={{ marginLeft: 'auto', color: 'var(--ink-3)' }} title={run.disabledReason}>
          play, pause, speed, step and scrub are local; rewind-and-resimulate, load and policy changes are off:{' '}
          {run.disabledReason}
        </span>
      </div>
      {run.refused ? (
        <div className="banner">
          <span className="tagline">not applied</span>
          <span style={{ color: 'var(--serious)' }}>{run.refused}</span>
          <button className="btn" onClick={run.dismissRefused}>
            dismiss
          </button>
        </div>
      ) : null}
      {run.loaded.unmapped.length ? (
        <details className="banner" style={{ display: 'block' }}>
          <summary style={{ cursor: 'pointer' }}>
            <span className="tagline">scenario</span> {run.loaded.unmapped.length} keys of this run's{' '}
            <code>scenario.txt</code> have no control-panel field
          </summary>
          <span style={{ color: 'var(--ink-3)' }}>{run.loaded.unmapped.join(' · ')}</span>
        </details>
      ) : null}
    </>
  );
}

// ---------------------------------------------------------------------------
// The surface itself, over any handle
// ---------------------------------------------------------------------------

function DashboardBody({
  run,
  onRun,
  highlight,
  tabHint,
  overlay,
  banner,
}: Omit<DashboardProps, 'initial' | 'autoplay' | 'data' | 'run'> & { run: RunHandle; banner?: ReactNode }) {
  const [controlTab, setControlTab] = useState<ControlTab>('load');
  const [observeTab, setObserveTab] = useState<ObserveTab>('quality');

  useEffect(() => {
    onRun?.(run);
    // the handle identity changes each render; the parent only needs the latest
  });

  useEffect(() => {
    if (!tabHint) return;
    if (tabHint.control) setControlTab(tabHint.control);
    if (tabHint.observe) setObserveTab(tabHint.observe);
  }, [tabHint?.nonce, tabHint?.control, tabHint?.observe]);

  const frames = windowFrames(run.engine, run.cursorS);
  const frame = frames[frames.length - 1] ?? run.engine.frames[0];
  if (!frame) {
    // No sample yet. The banner still renders, because a live run that was refused or a recording
    // that failed to load says so there; hiding it left the page on this line forever.
    return (
      <div className="dash">
        {banner}
        <div className="page-pad">waiting for the first sample…</div>
      </div>
    );
  }

  return (
    <div className="dash">
      {banner}
      <PlaybackBar run={run} />
      <UpdateBanner run={run} />
      <div className="dash-body">
        <div className="control-panel">
          <ControlPanel run={run} tab={controlTab} onTab={setControlTab} highlight={highlight} />
        </div>
        <ObservationPanel
          run={run}
          frames={frames}
          frame={frame}
          tab={observeTab}
          onTab={setObserveTab}
          highlight={highlight}
        />
      </div>
      <StatusBar run={run} />
      {overlay}
    </div>
  );
}
