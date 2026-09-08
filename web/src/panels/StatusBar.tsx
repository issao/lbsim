import type { RunHandle } from '../lib/useRun';
import { useRegistryStats } from '../lib/useSubscriptions';
import { LEASE_MS } from '../lib/subscriptions';
import { dataSourceGloss } from '../lib/mode';

/**
 * The data-budget readout. Section 3 of the spec makes four promises about what the client costs;
 * this line is how you check them without opening a network tab, and it is the reason the
 * subscription lifecycle is modelled at all when the run has no network of its own.
 */
export function StatusBar({ run }: { run: RunHandle }) {
  const s = useRegistryStats();
  return (
    <div className="statusbar">
      <span>
        subscriptions open <b className="num">{s.open}</b> &middot; peak <b className="num">{s.peak}</b> &middot; opened{' '}
        <b className="num">{s.opened}</b> &middot; closed <b className="num">{s.closed}</b> &middot; expired{' '}
        <b className="num">{s.expired}</b>
      </span>
      <span>
        leases <b className="num">{LEASE_MS / 1000}</b> s, renewals <b className="num">{s.renewals}</b>,{' '}
        {s.renewing ? 'renewing' : <span className="warn">tab hidden, not renewing</span>}
      </span>
      {/* U99: the owner list grows and shrinks as subscriptions open and close; it stays on this
          one line (ellipsised, with a title) rather than wrapping the bar onto a second. */}
      <span title={Object.entries(s.byOwner).map(([k, v]) => `${k} ${v}`).join(' · ') || 'none'}>
        {Object.entries(s.byOwner)
          .map(([k, v]) => `${k} ${v}`)
          .join(' · ') || 'none'}
      </span>
      <span
        style={{ marginLeft: 'auto' }}
        title={
          run.source?.kind === 'server'
            ? `mode live (${dataSourceGloss('server')}) · frames streamed ${run.engine.frames.length} from the Ingress server${run.source.runId ? ` (run ${run.source.runId})` : ''} · sample rate ${run.config.samplesPerSimSecond}/sim s`
            : run.source?.kind === 'replay'
              ? `mode replay (${dataSourceGloss('replay')}) · frames loaded ${run.engine.frames.length} from runs/${run.source.runId}/fleet.jsonl · sample rate ${run.config.samplesPerSimSecond}/sim s · the whole run is recorded, nothing is generated in this browser`
              : `mode mock (${dataSourceGloss('mock')}) · frames recorded ${run.engine.frames.length} · sample rate ${run.config.samplesPerSimSecond}/sim s · no network: every number above is generated in this browser`
        }
      >
        {run.source?.kind === 'server' ? (
          <>
            mode{' '}
            <span title="nothing is generated in this browser; the subscription counts on the left are this page's own, not the server's">
              <b>live</b>
            </span>{' '}
            <span className="note">({dataSourceGloss('server')})</span> &middot; frames streamed{' '}
            <b className="num">{run.engine.frames.length}</b> from the Ingress server
            {run.source.runId ? <> (run <code>{run.source.runId}</code>)</> : null} &middot; sample rate{' '}
            <b className="num">{run.config.samplesPerSimSecond}/sim s</b>
          </>
        ) : run.source?.kind === 'replay' ? (
          <>
            mode <b>replay</b> <span className="note">({dataSourceGloss('replay')})</span> &middot; frames loaded{' '}
            <b className="num">{run.engine.frames.length}</b> from <code>runs/{run.source.runId}/fleet.jsonl</code>{' '}
            &middot; sample rate <b className="num">{run.config.samplesPerSimSecond}/sim s</b> &middot; the whole run
            is recorded, nothing is generated in this browser
          </>
        ) : (
          <>
            mode <b>mock</b> <span className="note">({dataSourceGloss('mock')})</span> &middot; frames recorded{' '}
            <b className="num">{run.engine.frames.length}</b> &middot; sample rate{' '}
            <b className="num">{run.config.samplesPerSimSecond}/sim s</b> &middot; no network: every number above is
            generated in this browser
          </>
        )}
      </span>
    </div>
  );
}
