import { useCallback, useEffect, useRef, useState } from 'react';
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
        <MockTag what="mock walkthroughs" />
      </div>
      <p className="note" style={{ maxWidth: '80ch', marginTop: 0 }}>
        One card per dynamic in <code>docs/ARCHITECTURE.md</code> section 12, stack ranked as it is there. Clicking a
        card with a script starts a scripted walkthrough: the run advances, pauses at the moments that matter, says what
        is interesting, and offers resume. The scripts are JSON files in{' '}
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

type Phase = 'advancing' | 'paused' | 'done';

/**
 * The runner. It drives the same load-test dashboard a user would drive by hand: set the tabs,
 * apply the step's conditions, play until the step's simulated timestamp, pause, narrate, offer
 * resume. Nothing here is a second interface.
 */
function Walkthrough({ script, onExit }: { script: WalkthroughScript; onExit: () => void }) {
  const initial = useRef<ScenarioConfig>(scenarioFor(script)).current;
  const [step, setStep] = useState(0);
  const [phase, setPhase] = useState<Phase>('advancing');
  const [hint, setHint] = useState<TabHint>({
    control: script.steps[0].control_tab,
    observe: script.steps[0].observe_tab,
    nonce: 0,
  });

  const runRef = useRef<RunHandle | null>(null);
  const phaseRef = useRef<Phase>('advancing');
  const stepRef = useRef(0);
  phaseRef.current = phase;
  stepRef.current = step;

  const current = script.steps[Math.min(step, script.steps.length - 1)];

  // Called on every dashboard render, which is often enough to catch the moment a step is reached.
  const onRun = useCallback(
    (run: RunHandle) => {
      runRef.current = run;
      const st = script.steps[stepRef.current];
      if (!st || phaseRef.current !== 'advancing') return;
      if (run.speed !== (st.speed ?? 2)) run.setSpeed(st.speed ?? 2);
      if (run.cursorS >= st.at_sim_s) {
        run.setPaused(true);
        setPhase('paused');
      } else if (run.paused) {
        run.setPaused(false);
      }
    },
    [script]
  );

  const advanceTo = (next: number) => {
    const run = runRef.current;
    const st = script.steps[next];
    if (!run || !st) return;
    if (st.set) run.update(applyPatch(run.config, st.set));
    setHint({ control: st.control_tab, observe: st.observe_tab, nonce: next + 1 });
    setStep(next);
    setPhase('advancing');
    run.setSpeed(st.speed ?? 2);
    run.setPaused(false);
  };

  useEffect(() => {
    const st = script.steps[0];
    if (st.set && runRef.current) runRef.current.update(applyPatch(runRef.current.config, st.set));
  }, [script]);

  const atEnd = step >= script.steps.length - 1 && phase !== 'advancing';

  return (
    <Dashboard
      key={script.id}
      initial={initial}
      autoplay
      onRun={onRun}
      highlight={phase === 'paused' ? current.highlight ?? null : null}
      tabHint={hint}
      overlay={
        <div className="walkthrough" role="dialog" aria-label="walkthrough step">
          <div className="wt-progress">
            <i style={{ width: `${((step + (phase === 'paused' ? 1 : 0)) / script.steps.length) * 100}%` }} />
          </div>
          <div className="wt-head">
            <span className="wt-step">
              {step + 1}/{script.steps.length} &middot; {current.at_sim_s}s
            </span>
            <span className="wt-title">{phase === 'advancing' ? 'advancing…' : current.title}</span>
          </div>
          {phase === 'advancing' ? (
            <div className="wt-body">
              <p className="note" style={{ margin: 0 }}>
                Running to {current.at_sim_s} s at {current.speed ?? 2}&times;. It will pause there.
              </p>
            </div>
          ) : (
            <div className="wt-body">
              {current.body.map((p, i) => (
                <p key={i}>{p}</p>
              ))}
              {current.look_for ? <div className="wt-look">Look for: {current.look_for}</div> : null}
            </div>
          )}
          <div className="wt-foot">
            <span className="wt-step" style={{ flex: '0 0 auto' }}>
              {script.title}
            </span>
            <span className="grow" />
            <button className="btn" onClick={onExit}>
              exit
            </button>
            {phase === 'advancing' ? (
              <button
                className="btn"
                onClick={() => {
                  const run = runRef.current;
                  if (run) {
                    run.scrubTo(current.at_sim_s);
                    run.setPaused(true);
                  }
                  setPhase('paused');
                }}
              >
                skip ahead
              </button>
            ) : atEnd ? (
              <button className="btn primary" onClick={onExit}>
                done
              </button>
            ) : (
              <button className="btn primary" onClick={() => advanceTo(step + 1)}>
                resume
              </button>
            )}
          </div>
        </div>
      }
    />
  );
}
