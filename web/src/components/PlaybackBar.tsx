import { useCallback, useRef, useState } from 'react';
import type { RunHandle } from '../lib/useRun';
import { SPEEDS, STEP_S } from '../lib/useRun';
import { SNAPSHOT_S } from '../lib/engine';
import { fmtTime } from '../lib/format';
import { MockTag } from './ui';

/**
 * Playback lives across the top of the dashboard and the A/B view rather than inside a tab, per
 * docs/ui-spec.md section 1.2: it is the one control a user reaches for constantly.
 *
 * The scrubber draws the recorded extent explicitly. Dragging inside it is a log read and instant;
 * dragging past it forces re-simulation, and the hatched region plus the caption say so before the
 * click rather than after. A slider that behaves differently in two halves without saying so is
 * worse than one that is honest about it.
 */
export function PlaybackBar({ run, dense = false }: { run: RunHandle; dense?: boolean }) {
  const trackRef = useRef<HTMLDivElement>(null);
  const [drag, setDrag] = useState<number | null>(null);

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

  return (
    <div className="playback">
      <div className="btn-row" style={{ flex: '0 0 auto' }}>
        <button
          className="btn icon"
          onClick={() => run.rewindTo(0)}
          title="Rewind(to_sim_time = 0)"
          aria-label="rewind to start"
        >
          |&lt;
        </button>
        <button
          className="btn icon"
          onClick={() => run.rewindTo(Math.max(0, run.cursorS - 10))}
          title="Rewind(to_sim_time = now - 10 s)"
          aria-label="back ten seconds"
        >
          &lt;&lt;
        </button>
        <button
          className="btn icon primary"
          onClick={() => run.setPaused(!run.paused)}
          title="SetSpeed(paused). Pausing does not close subscriptions, so the charts hold their last values."
        >
          {run.paused ? '▶' : '‖'}
        </button>
        <button
          className="btn icon"
          onClick={run.step}
          title={`StepForward(sim_duration_ns = ${STEP_S} s), bounded; the response says where it stopped`}
          aria-label="step forward"
        >
          &gt;|
        </button>
      </div>

      <div className="seg" title="SetSpeed(realtime_factor): simulated seconds per wall-clock second">
        {SPEEDS.map((s) => (
          <button key={s} aria-pressed={run.speed === s} onClick={() => run.setSpeed(s)}>
            {s}&times;
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
          {Array.from({ length: Math.floor(run.durationS / SNAPSHOT_S) }, (_, i) => (i + 1) * SNAPSHOT_S)
            .filter((t) => t < run.recordedToS)
            .map((t) => (
              <div key={t} className="scrub-snap" style={{ left: pct(t) }} title={`snapshot at ${t} s`} />
            ))}
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
            <span>
              <i className="beyond" />
              <b>not yet simulated</b> &mdash; dragging here re-simulates
            </span>
            <span>| snapshot every {SNAPSHOT_S} s</span>
            {beyond ? <span style={{ color: 'var(--serious)' }}>release to re-simulate to {drag!.toFixed(1)} s</span> : null}
          </div>
        ) : null}
      </div>

      <div className="clock num">
        {fmtTime(run.cursorS)} <em>/ {fmtTime(run.durationS)}</em>
      </div>
      <MockTag what="mock run" />
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
    return (
      <div className={`banner${u.requiredResimulation ? ' resim' : ''}`}>
        <span className="tagline">
          {u.requiredResimulation ? 'required_resimulation = true' : 'required_resimulation = false'}
        </span>
        <span>
          {u.changed.join(', ')} changed.{' '}
          {u.requiredResimulation
            ? `Physics changed, so the run rewound to the ${u.rewoundToS.toFixed(0)} s snapshot and re-simulated from there. History after that point is new.`
            : 'View only: nothing was re-simulated, the panels re-derived from the recording.'}
        </span>
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
