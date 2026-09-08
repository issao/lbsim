import { createContext, useCallback, useContext, useRef, useState } from 'react';
import type { RunHandle } from '../lib/useRun';
import { SPEEDS, STEP_S } from '../lib/useRun';
import { fmtTime } from '../lib/format';
import { updateBannerText } from '../lib/updateBanner';
import { SMOOTHING_OPTIONS, setSmoothing, useSmoothing } from '../lib/smoothing';

/**
 * Playback lives across the top of the dashboard and the A/B view rather than inside a tab, per
 * docs/ui-spec.md section 1.2: it is the one control a user reaches for constantly.
 *
 * The scrubber draws the recorded extent explicitly. Dragging inside it is a log read and instant;
 * dragging past it forces re-simulation, and the hatched region plus the caption say so before the
 * click rather than after. A slider that behaves differently in two halves without saying so is
 * worse than one that is honest about it.
 */
/**
 * U102: while a walkthrough is open, its runner owns play and pause, so the bar's button does
 * exactly what the narration card's Play does (Issao: "Using the play button in the play bar
 * should have the same effect"). A context rather than a prop, because the dashboard renders this
 * bar and the walkthrough host renders the dashboard; absent a provider the bar drives the run
 * directly as before.
 */
export interface PlaybackOverride {
  onPlay: () => void;
  onPause: () => void;
}
export const PlaybackOverrideContext = createContext<PlaybackOverride | null>(null);

export function PlaybackBar({
  run,
  dense = false,
  endOfRun = true,
}: {
  run: RunHandle;
  dense?: boolean;
  /**
   * Swap play/pause for Restart (live) or "replay again" (replay) once the run is over. Off on
   * the A/B page (Compare.tsx): its bar drives two handles glued together, restart isn't fanned
   * out to both, and picking a policy apart is the whole point of that view, so the paired run's
   * own end-of-run handling is a separate unit rather than something this bar assumes.
   */
  endOfRun?: boolean;
}) {
  const trackRef = useRef<HTMLDivElement>(null);
  const override = useContext(PlaybackOverrideContext);
  const togglePlay = () => {
    if (!override) run.setPaused(!run.paused);
    else if (run.paused) override.onPlay();
    else override.onPause();
  };
  const [drag, setDrag] = useState<number | null>(null);
  const smooth = useSmoothing();
  const ended = endOfRun && run.ended;
  const restartAgain = () => {
    if (run.source?.kind === 'replay') {
      run.rewindTo(0);
      run.setPaused(false);
    } else {
      run.restart(run.config);
    }
  };

  const timeAt = useCallback(
    (clientX: number): number => {
      const el = trackRef.current;
      if (!el) return 0;
      const r = el.getBoundingClientRect();
      const f = Math.min(Math.max((clientX - r.left) / r.width, 0), 1);
      return f * run.durationS;
    },
    [run.durationS]
  );

  const onPointerDown = (e: React.PointerEvent) => {
    (e.target as HTMLElement).setPointerCapture?.(e.pointerId);
    const t = timeAt(e.clientX);
    setDrag(t);
    run.scrubTo(t);
  };
  const onPointerMove = (e: React.PointerEvent) => {
    if (drag === null) return;
    const t = timeAt(e.clientX);
    setDrag(t);
    if (t <= run.recordedToS) run.scrubTo(t);
  };
  const onPointerUp = (e: React.PointerEvent) => {
    if (drag === null) return;
    const t = timeAt(e.clientX);
    setDrag(null);
    run.scrubTo(t);
  };

  const pct = (s: number) => `${(Math.min(s, run.durationS) / run.durationS) * 100}%`;
  const events = run.engine.eventsUpTo(run.recordedToS);
  const beyond = drag !== null && drag > run.recordedToS;
  // Neither a live nor a replayed run accepts rewind-and-resimulate yet, and a button that only
  // ever says no is clutter rather than a control; the buttons return with the server's Rewind.
  const canRewind = (run.source?.disabledReason ?? null) === null;

  return (
    <div className="playback">
      <div className="btn-row" style={{ flex: '0 0 auto' }}>
        {canRewind ? (
          <>
            <button className="btn icon" onClick={() => run.rewindTo(0)} title="rewind to start" aria-label="rewind to start">
              |&lt;
            </button>
            <button
              className="btn icon"
              onClick={() => run.rewindTo(Math.max(0, run.cursorS - 10))}
              title="back 10 s"
              aria-label="back ten seconds"
            >
              &lt;&lt;
            </button>
          </>
        ) : null}
        {ended ? (
          <button
            className="btn primary"
            onClick={restartAgain}
            title={run.source?.kind === 'replay' ? 'replay again' : 'restart'}
            aria-label={run.source?.kind === 'replay' ? 'replay again' : 'restart'}
          >
            {run.source?.kind === 'replay' ? 'replay again' : 'Restart'}
          </button>
        ) : (
          <button
            className="btn icon primary"
            onClick={togglePlay}
            title={run.paused ? 'play' : 'pause'}
            aria-label={run.paused ? 'play' : 'pause'}
          >
            {run.paused ? '▶' : '‖'}
          </button>
        )}
        <button className="btn icon" onClick={run.step} title={`step ${STEP_S} s`} aria-label="step forward">
          &gt;|
        </button>
      </div>

      <div className="seg" title="speed: simulated seconds per wall second">
        {SPEEDS.map((s) => (
          <button key={s} aria-pressed={run.speed === s} onClick={() => run.setSpeed(s)}>
            {s}&times;
          </button>
        ))}
      </div>

      {/* Issao: "a global selector of a window average to be applied on them, live selectable".
          One selector for every time-series chart on every surface that shares this bar: live, the
          subscriptions are reopened with the window; on a replay the recorded frames are smoothed
          the same way. The heatmap's replica states and the trace list stay per sample. */}
      <div
        className="seg smooth"
        role="group"
        aria-label="smoothing window"
        title="smoothing: every time-series chart shows the trailing mean over this much simulated time; latency percentiles are over every request in the window"
      >
        <span className="seg-label">avg</span>
        {SMOOTHING_OPTIONS.map((o) => (
          <button key={o.id} data-smooth={o.id} aria-pressed={smooth === o.id} onClick={() => setSmoothing(o.id)}>
            {o.label}
          </button>
        ))}
      </div>

      <div className="scrubber">
        <div
          className="scrub-track"
          ref={trackRef}
          onPointerDown={onPointerDown}
          onPointerMove={onPointerMove}
          onPointerUp={onPointerUp}
          role="slider"
          tabIndex={0}
          aria-label="simulated time"
          aria-valuemin={0}
          aria-valuemax={run.durationS}
          aria-valuenow={Number(run.cursorS.toFixed(1))}
          onKeyDown={(e) => {
            if (e.key === 'ArrowLeft') run.scrubTo(Math.max(0, run.cursorS - 1));
            if (e.key === 'ArrowRight') run.scrubTo(Math.min(run.durationS, run.cursorS + 1));
          }}
        >
          <div className="scrub-recorded" style={{ width: pct(run.recordedToS) }} />
          <div className="scrub-beyond" style={{ left: pct(run.recordedToS), right: 0 }} />
          {events.map((e, i) => (
            <div
              key={i}
              className="scrub-event"
              style={{
                left: pct(e.simS),
                background:
                  e.severity === 'critical' ? 'var(--critical)' : e.severity === 'warning' ? 'var(--serious)' : 'var(--ink-3)',
              }}
              title={`${e.simS.toFixed(0)} s: ${e.text}`}
            />
          ))}
          <div className="scrub-head" style={{ left: pct(drag ?? run.cursorS) }} />
        </div>
        {!dense ? (
          <div className="scrub-legend">
            <span>
              <i className="rec" />
              <b>recorded</b> to {run.recordedToS.toFixed(0)} s &mdash; scrubbing here is instant
            </span>
            {run.recordedToS < run.durationS ? (
              <span>
                <i className="beyond" />
                <b>{canRewind ? 'not yet simulated' : 'not yet reached'}</b>
                {canRewind ? <> &mdash; dragging here re-simulates</> : null}
              </span>
            ) : null}
            {beyond ? <span style={{ color: 'var(--serious)' }}>release to re-simulate to {drag!.toFixed(1)} s</span> : null}
          </div>
        ) : null}
      </div>

      <div className="clock num">
        {ended ? (
          <>run complete &middot; {run.durationS} s simulated</>
        ) : (
          <>
            {fmtTime(run.cursorS)} <em>/ {fmtTime(run.durationS)}</em>
          </>
        )}
      </div>
    </div>
  );
}

/** The banner the spec insists on: whether the last change re-simulated, and where a rewind came from. */
export function UpdateBanner({ run }: { run: RunHandle }) {
  const u = run.lastUpdate;
  const r = run.lastRewind;
  if (run.resimulating) {
    return (
      <div className="banner resim">
        <span className="tagline">re-simulating</span>
        <span>restoring the snapshot and replaying forward</span>
      </div>
    );
  }
  if (u) {
    // U57 (found by U28): a rejected update carries `accepted: false` and a `rejectedReason`, and
    // used to fall through to the `requiredResimulation` wording, which reads as success. `kind`
    // is checked first so a rejection can never render as either success case.
    const t = updateBannerText(u);
    return (
      <div className={`banner${t.kind === 'rejected' ? ' rejected' : t.kind === 'resim' ? ' resim' : ''}`}>
        <span className="tagline">{t.headline}</span>
        <span>{t.detail}</span>
        <button className="btn" onClick={run.dismissUpdate}>
          dismiss
        </button>
      </div>
    );
  }
  if (r && !r.fromLog) {
    return (
      <div className="banner resim">
        <span className="tagline">from_log = false</span>
        <span>
          That scrub went past the recording, so it was simulated forward from {r.restoredFromSnapshotS.toFixed(1)} s
          rather than replayed.
        </span>
      </div>
    );
  }
  return null;
}
