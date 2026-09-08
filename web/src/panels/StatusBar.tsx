import type { RunHandle } from '../lib/useRun';
import { dataSourceGloss } from '../lib/mode';

/** Where the frames on screen came from, and how many: the run's own facts, nothing counted in the browser. */
export function StatusBar({ run }: { run: RunHandle }) {
  const live = run.source?.kind === 'server';
  const runId = run.source?.runId;
  const n = run.engine.frames.length;
  const rate = run.config.samplesPerSimSecond;
  const title = live
    ? `mode live (${dataSourceGloss('server')}) · frames streamed ${n} from the Ingress server${runId ? ` (run ${runId})` : ''} · sample rate ${rate}/sim s`
    : `mode replay (${dataSourceGloss('replay')}) · frames loaded ${n} from runs/${runId}/fleet.jsonl · sample rate ${rate}/sim s · the whole run is recorded, nothing is generated in this browser`;
  return (
    <div className="statusbar">
      <span title={title}>
        {live ? (
          <>
            mode <b>live</b> <span className="note">({dataSourceGloss('server')})</span> &middot; frames streamed{' '}
            <b className="num">{n}</b> from the Ingress server
            {runId ? <> (run <code>{runId}</code>)</> : null} &middot; sample rate <b className="num">{rate}/sim s</b>
          </>
        ) : (
          <>
            mode <b>replay</b> <span className="note">({dataSourceGloss('replay')})</span> &middot; frames loaded{' '}
            <b className="num">{n}</b> from <code>runs/{runId}/fleet.jsonl</code> &middot; sample rate{' '}
            <b className="num">{rate}/sim s</b> &middot; the whole run is recorded, nothing is generated in this browser
          </>
        )}
      </span>
    </div>
  );
}
