import type { RunHandle } from '../lib/useRun';
import { useRegistryStats } from '../lib/useSubscriptions';
import { LEASE_MS } from '../lib/subscriptions';

/**
 * The data-budget readout. Section 3 of the spec makes four promises about what the client costs;
 * this line is how you check them without opening a network tab, and it is the reason the
 * subscription lifecycle is modelled at all in a stand-in with no network.
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
        frames recorded <b>{run.engine.frames.length}</b> &middot; sample rate{' '}
        <b>{run.config.samplesPerSimSecond}/sim s</b> &middot; no network: every number above is generated in this
        browser
      </span>
    </div>
  );
}
