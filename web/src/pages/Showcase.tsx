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
import { probeDataSource, type RunHandle } from '../lib/useRun';
import type { ScenarioConfig } from '../lib/config';
import { type RunnerHandle, type StepState, WalkthroughRunner } from '../lib/walkthroughRunner';
import { type PlaybackOverride, PlaybackOverrideContext } from '../components/PlaybackBar';
import { useDraggable } from '../lib/useDraggable';
import { Dashboard, type TabHint } from './Dashboard';

/**
 * The open walkthrough lives in the hash, `#/showcase?script=<card id>`, so the nav link, the back
 * button, a reload and a pasted URL all agree on what is open. State kept in the component did not:
 * a click on the "Showcase" link is a same-route hash change, which re-renders the same instance.
 */
function scriptIdFromHash(): string | null {
  const q = window.location.hash.indexOf('?');
  return q < 0 ? null : new URLSearchParams(window.location.hash.slice(q + 1)).get('script');
}

export function Showcase() {
  const [index, setIndex] = useState<ShowcaseIndex | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [wanted, setWanted] = useState<string | null>(scriptIdFromHash);
  const [script, setScript] = useState<WalkthroughScript | null>(null);

  useEffect(() => {
    loadIndex().then(setIndex).catch((e) => setError(String(e)));
  }, []);

  useEffect(() => {
    const onHash = () => setWanted(scriptIdFromHash());
    window.addEventListener('hashchange', onHash);
    return () => window.removeEventListener('hashchange', onHash);
  }, []);

  // An id the index does not know shows the cards, the same as no id.
  useEffect(() => {
    const file = wanted && index ? index.cards.find((c) => c.id === wanted)?.script : undefined;
    if (!file) {
      setScript(null);
      return;
    }
    let alive = true;
    loadScript(file)
      .then((s) => alive && setScript(s))
      .catch((e) => alive && setError(String(e)));
    return () => {
      alive = false;
    };
  }, [wanted, index]);

  const open = (card: ShowcaseCard) => {
    if (card.script) window.location.hash = `#/showcase?script=${card.id}`;
  };

  if (script) return <Walkthrough script={script} onExit={() => (window.location.hash = '#/showcase')} />;

  return (
    <div className="page-pad">
      <h1 style={{ fontSize: 15, margin: '0 0 4px', fontWeight: 600 }}>Showcase</h1>
      <p className="note" style={{ maxWidth: '80ch', marginTop: 0 }}>
        One card per dynamic, stack ranked. A card with a walkthrough steps through a run: it advances, pauses at the
        moments that matter, and says what to look at. With a server the walkthrough drives a live run; otherwise it
        plays a recording, and says so when none is served.
      </p>
      {error ? <p style={{ color: 'var(--critical)' }}>{error}</p> : null}
      {!index ? (
        <p className="note">loading…</p>
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
                      title={c.script ? 'run the walkthrough' : 'no walkthrough yet'}
                    >
                      <span className="card-num">dynamic {c.dynamic}</span>
                      <span className="card-title">{c.title}</span>
                      <p className="card-body">{c.summary}</p>
                      <span className="card-foot">
                        <span className="phase">{c.script ? 'walkthrough' : 'coming'}</span>
                        <span className="card-needs">needs: {c.requires}</span>
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
  | { kind: 'probing' }
  /** Nothing to drive: no server, and no recording of the run the script names. The page says which. */
  | { kind: 'none'; note: string }
  | { kind: 'server' }
  | { kind: 'replay'; run: string };

/**
 * The runner's view of whatever handle the dashboard hands back. Read lazily, because the handle's
 * identity changes on every render and the runner outlives all of them. A synchronous refusal
 * (the replay returns an `UpdateResponse`) becomes a rejection here; the server's `update`
 * returns nothing and reports through `lastUpdate`, which the panel reads separately.
 */
function adapt(get: () => RunHandle): RunnerHandle {
  return {
    get mode() {
      return get().source?.kind ?? 'server';
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
  const [source, setSource] = useState<Source>({ kind: 'probing' });

  // The same probe the dashboard decides from, so the overlay never says "replay" over a live run.
  useEffect(() => {
    const wanted = script.run;
    let alive = true;
    void probeDataSource().then((d) => {
      if (!alive) return;
      if (d.state === 'server') setSource({ kind: 'server' });
      else if (d.state === 'replay' && wanted && d.runs.some((r) => r.runId === wanted)) setSource({ kind: 'replay', run: wanted });
      else if (d.state === 'replay' && wanted) setSource({ kind: 'none', note: `no server, and no recording of ${wanted} is served` });
      else if (d.state === 'replay') setSource({ kind: 'none', note: 'no server, and this walkthrough names no recording' });
      else setSource({ kind: 'none', note: 'no server and no recordings served' });
    });
    return () => {
      alive = false;
    };
  }, [script.run]);

  if (source.kind === 'probing') return <div className="page-pad">looking for a server{script.run ? ` or ${script.run}` : ''}…</div>;
  if (source.kind === 'none') {
    return (
      <div className="page-pad">
        <p style={{ margin: '0 0 8px' }}>{source.note}</p>
        <button className="btn" onClick={onExit}>
          back to the cards
        </button>
      </div>
    );
  }
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
  source: Exclude<Source, { kind: 'probing' } | { kind: 'none' }>;
  compare?: string;
  onExit: () => void;
}) {
  const runRef = useRef<RunHandle | null>(null);
  const runnerRef = useRef<WalkthroughRunner | null>(null);
  const [st, setSt] = useState<StepState | null>(null);
  const [sourceLabel, setSourceLabel] = useState('');
  // A refusal the handle reports after the fact (the server's update is fire-and-forget).
  const [lateReason, setLateReason] = useState<string | null>(null);
  // Issao asked, verbatim, "can you make the showcase card draggable?" The card itself is not remounted between
  // steps -- the runner swaps `current` and the JSX below re-renders in place -- so the offset
  // lives on this one element across the whole walkthrough. One storage key for every script: the
  // position is a browser habit ("I keep it over there"), not a per-walkthrough setting.
  const cardRef = useRef<HTMLDivElement | null>(null);
  const drag = useDraggable(cardRef, { storageKey: 'lbsim.showcase.wt-pos', handleSelector: '.wt-head' });

  // Called on every dashboard render, which is often enough to catch the moment a step is reached.
  const onRun = useCallback(
    (run: RunHandle) => {
      runRef.current = run;
      const label = run.source?.label ?? 'live run';
      setSourceLabel((l) => (l === label ? l : label));
      const late = run.lastUpdate && !run.lastUpdate.accepted ? run.lastUpdate.rejectedReason : null;
      setLateReason((r) => (r === late ? r : late));
      // A server handle renders before StartRun has answered, and its controls drop on the floor
      // until then; a runner built that early would call play() into nothing and wait forever.
      if (run.source?.kind === 'server' && !run.source.runId) return;
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

  // U102: one action behind the card's Play and the playback bar's play, so pressing either
  // resumes at the step's predetermined speed; the bar's pause shows on the card as paused.
  const playback = useMemo<PlaybackOverride>(
    () => ({
      onPlay: () => runnerRef.current && void runnerRef.current.resume().then(setSt),
      onPause: () => runnerRef.current && setSt(runnerRef.current.pauseHere()),
    }),
    []
  );

  const current = st ?? { index: 0, step: script.steps[0], advancing: true, paused: false, done: false };
  const hint = useMemo<TabHint>(
    () => ({ control: current.step.control_tab, observe: current.step.observe_tab, nonce: current.index + 1 }),
    [current.step, current.index]
  );
  const step = current.step;
  const notes = compare ? [`compare ${compare}: this walkthrough shows one run; the comparison run is not opened`] : [];
  const refusal = current.reason ?? lateReason;
  // U70: the word for what this walkthrough is driving, and, when a step's conditions could not be
  // applied, which of the two explains why -- replay never accepts a `set`, live can refuse one.
  const narration = source.kind === 'server' ? 'driving a live run' : 'stepping through a replay';
  const refusalPrefix = source.kind === 'replay' ? 'not applied (replay):' : 'not applied (live, refused):';

  return (
    <PlaybackOverrideContext.Provider value={playback}>
    <Dashboard
      key={script.id}
      initial={initial}
      // The decision was made above. `server` skips the dashboard's own probe so it cannot decide
      // differently; replay keeps `auto` because the replay branch takes its run list from that
      // (cached) probe, and `run` names the recording to open from it.
      data={source.kind === 'server' ? 'server' : 'auto'}
      run={source.kind === 'replay' ? source.run : undefined}
      autoplay={false}
      onRun={onRun}
      highlight={!current.advancing ? step.highlight ?? null : null}
      tabHint={hint}
      overlay={
        <div className="walkthrough" role="dialog" aria-label="walkthrough step" ref={cardRef}>
          <div className="wt-progress">
            <i style={{ width: `${((current.index + (current.advancing ? 0 : 1)) / script.steps.length) * 100}%` }} />
          </div>
          <div className="wt-head wt-drag-handle">
            <span className="wt-step">
              {current.index + 1}/{script.steps.length} &middot; {step.at_sim_s}s
            </span>
            <span className="wt-mode">{narration}</span>
            <span className="wt-title">{current.advancing ? 'advancing…' : step.title}</span>
            <button
              className="btn wt-play"
              aria-label="play"
              title="play to the next stop"
              disabled={current.advancing && !current.paused}
              onClick={playback.onPlay}
            >
              play
            </button>
            <button className="btn icon wt-reset" onClick={drag.reset} title="reset position" aria-label="reset position">
              &#8962;
            </button>
          </div>
          {current.advancing ? (
            <div className="wt-body">
              <p className="note" style={{ margin: 0 }}>
                {current.paused
                  ? `paused at ${runRef.current?.cursorS ?? 0} s; play to continue to ${step.at_sim_s} s`
                  : `advancing scenario… to ${step.at_sim_s} s at ${runnerRef.current?.speedOf(step) ?? step.speed ?? 1}×`}
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
              {refusalPrefix} {refusal}
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
            ) : null}
          </div>
        </div>
      }
    />
    </PlaybackOverrideContext.Provider>
  );
}
