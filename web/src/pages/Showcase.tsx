import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import {
  applyPatch,
  loadIndex,
  loadScript,
  scenarioFor,
  type ShowcaseCard,
  type ShowcaseIndex,
  type WalkthroughScript,
} from '../lib/walkthrough';
import type { RunHandle } from '../lib/useRun';
import type { ScenarioConfig } from '../lib/config';
import { dataModeFrom, replayOverride, serverMode } from '../lib/mode';
import { probeRunIndex } from '../lib/replay';
import { type RunnerHandle, type StepState, WalkthroughRunner } from '../lib/walkthroughRunner';
import { Dashboard, type TabHint } from './Dashboard';
import { MockTag } from '../components/ui';

export function Showcase() {
  const [index, setIndex] = useState<ShowcaseIndex | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [script, setScript] = useState<WalkthroughScript | null>(null);

  useEffect(() => {
    loadIndex().then(setIndex).catch((e) => setError(String(e)));
  }, []);

  const open = async (card: ShowcaseCard) => {
    if (!card.script) return;
    try {
      setScript(await loadScript(card.script));
    } catch (e) {
      setError(String(e));
    }
  };

  if (script) return <Walkthrough script={script} onExit={() => setScript(null)} />;

  return (
    <div className="page-pad">
      <div style={{ display: 'flex', alignItems: 'baseline', gap: 10, marginBottom: 4 }}>
        <h1 style={{ fontSize: 15, margin: 0, fontWeight: 600 }}>Showcase</h1>
        <MockTag what="mock, replay or live" />
      </div>
      <p className="note" style={{ maxWidth: '80ch', marginTop: 0 }}>
        One card per dynamic in <code>docs/ARCHITECTURE.md</code> section 12, stack ranked as it is there. Clicking a
        card with a script starts a scripted walkthrough: the run advances, pauses at the moments that matter, says what
        is interesting, and offers resume. When an Ingress server is on, the script runs live on it. Otherwise a script
        that names a recorded run plays that recording when the runs index is served, and drives the mock engine when
        it does not. The scripts are JSON files in{' '}
        <code>web/public/walkthroughs/</code>, loaded at runtime; the format is in{' '}
        <a href={`${import.meta.env.BASE_URL}walkthroughs/schema.md`}>schema.md</a>.
      </p>
      {error ? <p style={{ color: 'var(--critical)' }}>{error}</p> : null}
      {!index ? (
        <p className="note">loading the card index…</p>
      ) : (
        <>
          {[1, 2, 3].map((phase) => (
            <div key={phase} style={{ marginTop: 14 }}>
              <p className="section-label">
                Phase {phase}
                {phase === 1 ? ' — the foundation, and most of the lessons' : phase === 2 ? ' — locality and geography' : ' — advanced'}
              </p>
              <div className="cards">
                {index.cards
                  .filter((c) => c.phase === phase)
                  .map((c) => (
                    <button
                      key={c.id}
                      className="card"
                      disabled={!c.script}
                      onClick={() => open(c)}
                      title={c.script ? 'run the scripted walkthrough' : 'no script yet'}
                    >
                      <span className="card-num">dynamic {c.dynamic}</span>
                      <span className="card-title">{c.title}</span>
                      <p className="card-body">{c.summary}</p>
                      <span className="card-foot">
                        <span className="phase">{c.script ? 'walkthrough' : 'not scripted yet'}</span>
                        <span style={{ overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>
                          needs: {c.requires}
                        </span>
                      </span>
                    </button>
                  ))}
              </div>
            </div>
          ))}
        </>
      )}
    </div>
  );
}

/**
 * Where a walkthrough gets its run from. Decided once per script, before the dashboard mounts, and
 * handed to the dashboard as props (`data`, `run`) so the two never disagree about what is open.
 */
type Source =
  | { kind: 'probing'; note?: undefined }
  | { kind: 'mock'; note?: string }
  | { kind: 'server'; note?: undefined }
  | { kind: 'replay'; run: string; note?: undefined };

/**
 * What can be decided without a round trip: a server that is switched on takes every script,
 * live; a script with no recording drives the mock. Only a named recording needs the index.
 */
function initialSource(script: WalkthroughScript): Source {
  const override = replayOverride(window.location.search, window.location.hash);
  if (dataModeFrom(serverMode(), false, override) === 'server') return { kind: 'server' };
  return script.run ? { kind: 'probing' } : { kind: 'mock' };
}

/**
 * The runner's view of whatever handle the dashboard hands back. Read lazily, because the handle's
 * identity changes on every render and the runner outlives all of them. A synchronous refusal
 * (the mock and the replay return an `UpdateResponse`) becomes a rejection here; the server's
 * `update` returns nothing and reports through `lastUpdate`, which the panel reads separately.
 */
function adapt(get: () => RunHandle): RunnerHandle {
  return {
    get mode() {
      const r = get();
      return r.source?.kind ?? ((r.engine as unknown) === null ? 'server' : 'mock');
    },
    cursorS: () => get().cursorS,
    scrubTo: (s) => get().scrubTo(s),
    setSpeed: (x) => get().setSpeed(x),
    pause: () => get().setPaused(true),
    play: () => get().setPaused(false),
    update: (patch) => {
      const r = get();
      const resp = r.update(applyPatch(r.config, patch)) as ReturnType<RunHandle['update']> | undefined;
      if (resp && !resp.accepted) throw new Error(resp.rejectedReason || 'the run refused the change');
    },
  };
}

/**
 * The walkthrough surface. It drives the same load-test dashboard a user would drive by hand: set
 * the tabs, apply the step's conditions, run to the step's simulated timestamp, pause, narrate,
 * offer resume. Nothing here is a second interface; the state machine is walkthroughRunner.ts.
 */
function Walkthrough({ script, onExit }: { script: WalkthroughScript; onExit: () => void }) {
  const initial = useRef<ScenarioConfig>(scenarioFor(script)).current;
  const [source, setSource] = useState<Source>(() => initialSource(script));
  const probing = source.kind === 'probing';

  useEffect(() => {
    const wanted = script.run;
    if (!probing || !wanted) return;
    let alive = true;
    void probeRunIndex().then((runs) => {
      if (!alive) return;
      const served = runs?.some((r) => r.runId === wanted) ?? false;
      const mode = dataModeFrom(serverMode(), runs !== null, replayOverride(window.location.search, window.location.hash));
      if (served && mode === 'replay') setSource({ kind: 'replay', run: wanted });
      else setSource({ kind: 'mock', note: `recording ${wanted} is not in the served runs index; playing the mock instead` });
    });
    return () => {
      alive = false;
    };
  }, [script.run, probing]);

  if (source.kind === 'probing') return <div className="page-pad">looking for {script.run}…</div>;
  return <WalkthroughOver key={source.kind} script={script} initial={initial} source={source} compare={script.compare} onExit={onExit} />;
}

function WalkthroughOver({
  script,
  initial,
  source,
  compare,
  onExit,
}: {
  script: WalkthroughScript;
  initial: ScenarioConfig;
  source: Source;
  compare?: string;
  onExit: () => void;
}) {
  const runRef = useRef<RunHandle | null>(null);
  const runnerRef = useRef<WalkthroughRunner | null>(null);
  const [st, setSt] = useState<StepState | null>(null);
  const [sourceLabel, setSourceLabel] = useState('');
  // A refusal the handle reports after the fact (the server's update is fire-and-forget).
  const [lateReason, setLateReason] = useState<string | null>(null);

  // Called on every dashboard render, which is often enough to catch the moment a step is reached.
  const onRun = useCallback(
    (run: RunHandle) => {
      runRef.current = run;
      const label = run.source?.label ?? 'live run';
      setSourceLabel((l) => (l === label ? l : label));
      const late = run.lastUpdate && !run.lastUpdate.accepted ? run.lastUpdate.rejectedReason : null;
      setLateReason((r) => (r === late ? r : late));
      if (!runnerRef.current) {
        const runner = new WalkthroughRunner(script, adapt(() => runRef.current as RunHandle));
        runnerRef.current = runner;
        void runner.next().then(setSt);
        return;
      }
      // Same object when nothing changed, so this set is a no-op between steps.
      setSt(runnerRef.current.tick());
    },
    [script]
  );

  const current = st ?? { index: 0, step: script.steps[0], advancing: true, done: false };
  const hint = useMemo<TabHint>(
    () => ({ control: current.step.control_tab, observe: current.step.observe_tab, nonce: current.index + 1 }),
    [current.step, current.index]
  );
  const step = current.step;
  const notes = [source.note, compare ? `compare ${compare}: this page has no A/B view yet, so the second run is not opened` : null].filter(
    (n): n is string => n !== null && n !== undefined
  );
  const refusal = current.reason ?? lateReason;

  return (
    <Dashboard
      key={script.id}
      initial={initial}
      // The decision was made above. `server` and `mock` skip the dashboard's own probe so it
      // cannot decide differently; replay keeps `auto` because the replay branch takes its run
      // list from that (cached) probe, and `run` names the recording to open from it.
      data={source.kind === 'server' ? 'server' : source.kind === 'replay' ? 'auto' : 'mock'}
      run={source.kind === 'replay' ? source.run : undefined}
      autoplay={false}
      onRun={onRun}
      highlight={!current.advancing ? step.highlight ?? null : null}
      tabHint={hint}
      overlay={
        <div className="walkthrough" role="dialog" aria-label="walkthrough step">
          <div className="wt-progress">
            <i style={{ width: `${((current.index + (current.advancing ? 0 : 1)) / script.steps.length) * 100}%` }} />
          </div>
          <div className="wt-head">
            <span className="wt-step">
              {current.index + 1}/{script.steps.length} &middot; {step.at_sim_s}s
            </span>
            <span className="wt-title">{current.advancing ? 'advancing…' : step.title}</span>
          </div>
          {current.advancing ? (
            <div className="wt-body">
              <p className="note" style={{ margin: 0 }}>
                Running to {step.at_sim_s} s. It will pause there.
              </p>
            </div>
          ) : (
            <div className="wt-body">
              {step.body.map((p, i) => (
                <p key={i}>{p}</p>
              ))}
              {step.look_for ? <div className="wt-look">Look for: {step.look_for}</div> : null}
            </div>
          )}
          {refusal ? (
            <div className="wt-body" style={{ color: 'var(--critical)' }}>
              conditions not applied: {refusal}
            </div>
          ) : null}
          {notes.length ? (
            <div className="wt-body">
              {notes.map((n) => (
                <p key={n} className="note" style={{ margin: 0 }}>
                  {n}
                </p>
              ))}
            </div>
          ) : null}
          <div className="wt-foot">
            <span className="wt-step" style={{ flex: '0 0 auto' }}>
              {script.title}
              {sourceLabel ? ` · ${sourceLabel}` : ''}
            </span>
            <span className="grow" />
            <button className="btn" onClick={onExit}>
              exit
            </button>
            {current.advancing ? (
              <button className="btn" onClick={() => runnerRef.current && setSt(runnerRef.current.skip())}>
                skip ahead
              </button>
            ) : current.done ? (
              <button className="btn primary" onClick={onExit}>
                done
              </button>
            ) : (
              <button className="btn primary" onClick={() => runnerRef.current && void runnerRef.current.next().then(setSt)}>
                resume
              </button>
            )}
          </div>
        </div>
      }
    />
  );
}
