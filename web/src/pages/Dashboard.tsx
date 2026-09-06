import { useEffect, useState, type ReactNode } from 'react';
import type { ScenarioConfig } from '../lib/config';
import { useRun, type RunHandle } from '../lib/useRun';
import { windowFrames } from '../lib/derive';
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

/**
 * The load-test dashboard: playback across the top, control panel left, observation suite right.
 * Reused by the showcase, which drives the same surface from a walkthrough script rather than by
 * hand -- the walkthrough is content, not a second interface.
 */
export function Dashboard({
  initial,
  autoplay = true,
  onRun,
  highlight,
  tabHint,
  overlay,
}: {
  initial: ScenarioConfig;
  autoplay?: boolean;
  onRun?: (run: RunHandle) => void;
  highlight?: string | null;
  tabHint?: TabHint;
  overlay?: ReactNode;
}) {
  const run = useRun(initial, autoplay);
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
  if (!frame) return <div className="page-pad">initialising the mock run…</div>;

  return (
    <div className="dash">
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
