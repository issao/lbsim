import type { RunHandle } from '../lib/useRun';
import { Tabs, type TabDef } from '../components/ui';
import { ClusterHealth } from './observe/ClusterHealth';
import { ServiceQuality } from './observe/ServiceQuality';
import { MachineLevel } from './observe/MachineLevel';
import { Utilization } from './observe/Utilization';
import { Traces } from './observe/Traces';
import type { Frame } from '../lib/frame';

export type ObserveTab = 'cluster' | 'quality' | 'machine' | 'utilization' | 'traces';

const TABS: TabDef<ObserveTab>[] = [
  { id: 'cluster', label: 'Cluster health' },
  { id: 'quality', label: 'Service quality' },
  { id: 'machine', label: 'Machine level' },
  { id: 'utilization', label: 'Utilization' },
  { id: 'traces', label: 'Traces' },
];

/**
 * Only the active tab is mounted. That is the data budget rule from docs/ui-spec.md section 3
 * expressed as component structure: switching tabs unmounts the previous panel, which closes the
 * per-replica subscriptions the machine-level view holds, and nothing has to remember to do it.
 */
export function ObservationPanel({
  run,
  frames,
  frame,
  tab,
  onTab,
  highlight,
}: {
  run: RunHandle;
  frames: Frame[];
  frame: Frame;
  tab: ObserveTab;
  onTab: (t: ObserveTab) => void;
  highlight?: string | null;
}) {
  return (
    <section className="panel observe-panel">
      <Tabs tabs={TABS} value={tab} onChange={onTab} scope="observe" />
      <div className="panel-body scroll">
        {tab === 'cluster' ? <ClusterHealth frames={frames} frame={frame} config={run.config} highlight={highlight} /> : null}
        {tab === 'quality' ? (
          <ServiceQuality frames={frames} frame={frame} config={run.config} highlight={highlight} />
        ) : null}
        {tab === 'machine' ? (
          <MachineLevel
            frames={frames}
            frame={frame}
            config={run.config}
            highlight={highlight}
            live={run.source?.kind === 'server' && run.source.runId ? { runId: run.source.runId } : undefined}
          />
        ) : null}
        {tab === 'utilization' ? (
          <Utilization frames={frames} frame={frame} config={run.config} highlight={highlight} />
        ) : null}
        {tab === 'traces' ? <Traces run={run} /> : null}
      </div>
    </section>
  );
}
