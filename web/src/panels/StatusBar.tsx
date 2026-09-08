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
        subscriptions open <b>{s.open}</b> &middot; peak <b>{s.peak}</b> &middot; opened <b>{s.opened}</b> &middot;
        closed <b>{s.closed}</b> &middot; expired <b>{s.expired}</b>
      </span>
      <span>
        leases {LEASE_MS / 1000} s, renewals <b>{s.renewals}</b>,{' '}
        {s.renewing ? 'renewing' : <span className="warn">tab hidden, not renewing</span>}
      </span>
      <span>
        {Object.entries(s.byOwner)
          .map(([k, v]) => `${k} ${v}`)
          .join(' · ') || 'none'}
      </span>
      <span style={{ marginLeft: 'auto' }}>
        {run.source?.kind === 'server' ? (
          <>
            mode{' '}
            <span title="nothing is generated in this browser; the subscription counts on the left are this page's own, not the server's">
              <b>live</b>
            </span>{' '}
            <span className="note">({dataSourceGloss('server')})</span> &middot; frames streamed{' '}
            <b>{run.engine.frames.length}</b> from the Ingress server
            {run.source.runId ? <> (run <code>{run.source.runId}</code>)</> : null} &middot; sample rate{' '}
            <b>{run.config.samplesPerSimSecond}/sim s</b>
          </>
        ) : run.source?.kind === 'replay' ? (
          <>
            mode <b>replay</b> <span className="note">({dataSourceGloss('replay')})</span> &middot; frames loaded{' '}
            <b>{run.engine.frames.length}</b> from <code>runs/{run.source.runId}/fleet.jsonl</code> &middot; sample rate{' '}
            <b>{run.config.samplesPerSimSecond}/sim s</b> &middot; the whole run is recorded, nothing is generated in
            this browser
          </>
        ) : (
          <>
            mode <b>mock</b> <span className="note">({dataSourceGloss('mock')})</span> &middot; frames recorded{' '}
            <b>{run.engine.frames.length}</b> &middot; sample rate <b>{run.config.samplesPerSimSecond}/sim s</b> &middot;
            no network: every number above is generated in this browser
          </>
        )}
      </span>
    </div>
  );
}
