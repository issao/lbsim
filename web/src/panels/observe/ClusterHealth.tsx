import { useMemo } from 'react';
import type { Frame } from '../../lib/engine';
import type { ScenarioConfig } from '../../lib/config';
import type { MockEngine } from '../../lib/engine';
import { healthCounts, series, xs } from '../../lib/derive';
import { Panel, Tile } from '../../components/ui';
import { LineChart } from '../../components/charts/LineChart';
import { fmtNum } from '../../lib/format';
import { useSubscriptions } from '../../lib/useSubscriptions';
import { Metric } from '../../lib/types';

export function ClusterHealth({
  engine,
  frames,
  frame,
  config,
  cursorS,
  highlight,
}: {
  engine: MockEngine;
  frames: Frame[];
  frame: Frame;
  config: ScenarioConfig;
  cursorS: number;
  highlight?: string | null;
}) {
  // One cluster-scoped subscription. Switching away from this tab closes it.
  useSubscriptions(
    'cluster-health',
    [{ scope: 'CLUSTER', id: 1 }],
    [Metric.READY_REPLICAS, Metric.WARMING_REPLICAS, Metric.DRAINING_REPLICAS, Metric.LOAD_IMBALANCE_CV],
    config.samplesPerSimSecond
  );

  const h = healthCounts(frame);
  const x = xs(frames);
  const events = useMemo(() => engine.eventsUpTo(cursorS).slice(-12).reverse(), [engine, cursorS, frames.length]);

  return (
    <div className="grid c2">
      <Panel title="Fleet state" sub={`${config.fleet.accelerator}, 1 cluster / 1 pool`} highlight={highlight === 'fleet-state'} id="fleet-state">
        <div className="grid c4" style={{ gap: 6 }}>
          <Tile label="ready" value={String(h.ready)} />
          <Tile label="warming" value={String(h.warming)} note="cold start" />
          <Tile label="draining" value={String(h.draining)} />
          <Tile
            label="ejected"
            value={String(h.ejected)}
            status={h.ejected > 0 ? 'warning' : undefined}
            statusText={h.ejected > 0 ? 'removed from routing' : undefined}
          />
        </div>
        <div style={{ marginTop: 8 }}>
          <LineChart
            xs={x}
            series={[
              { key: 'ready', label: 'ready', color: 'var(--series-1)', points: series(frames, (f) => f.readyReplicas) },
              { key: 'warming', label: 'warming', color: 'var(--series-2)', points: series(frames, (f) => f.warmingReplicas) },
              { key: 'draining', label: 'draining', color: 'var(--series-3)', points: series(frames, (f) => f.drainingReplicas) },
            ]}
            format={(v) => v.toFixed(0)}
            height={92}
          />
        </div>
      </Panel>

      <Panel
        title="Health is a vector, not a boolean"
        sub="announced state against true speed"
        highlight={highlight === 'gray'}
        id="gray"
      >
        <div className="grid c2" style={{ gap: 6 }}>
          <Tile
            label="announcing healthy, degraded"
            value={String(h.gray)}
            status={h.gray > 0 ? 'critical' : 'good'}
            statusText={h.gray > 0 ? 'gray failure in progress' : 'none detected'}
            note="ground truth; a policy can only infer it"
          />
          <Tile
            label="load imbalance cv"
            value={frame.loadImbalanceCv.toFixed(3)}
            status={frame.loadImbalanceCv > 0.4 ? 'critical' : frame.loadImbalanceCv > 0.18 ? 'serious' : 'good'}
            statusText={frame.loadImbalanceCv > 0.4 ? 'badly unbalanced' : frame.loadImbalanceCv > 0.18 ? 'unbalanced' : 'balanced'}
            note="coefficient of variation of per-replica resident work"
          />
        </div>
        <div style={{ marginTop: 8 }}>
          <LineChart
            xs={x}
            series={[{ key: 'cv', label: 'imbalance cv', color: 'var(--series-1)', points: series(frames, (f) => f.loadImbalanceCv) }]}
            format={(v) => v.toFixed(2)}
            height={92}
            thresholds={[{ value: 0.18, label: 'ok', color: 'var(--serious)' }]}
          />
        </div>
      </Panel>

      <Panel title="Failure events" sub="on the simulated timeline" bodyClass="scroll" highlight={highlight === 'events'} id="events">
        {events.length === 0 ? (
          <p className="note" style={{ margin: 0 }}>nothing yet. The scripted schedule starts at 52 s.</p>
        ) : (
          <ul className="events">
            {events.map((e, i) => (
              <li key={i}>
                <span className="t num">{e.simS.toFixed(0)}s</span>
                <span className="d">
                  <i className={`dot ${e.severity}`} />
                </span>
                <span className="x">{e.text}</span>
              </li>
            ))}
          </ul>
        )}
      </Panel>

      <Panel title="Throughput against arrivals" sub="offered, admitted, shed" highlight={highlight === 'arrivals'} id="arrivals">
        <LineChart
          xs={x}
          series={[
            { key: 'offered', label: 'offered', color: 'var(--series-1)', points: series(frames, (f) => f.offeredRps) },
            { key: 'completed', label: 'completed', color: 'var(--series-3)', points: series(frames, (f) => f.completedRps) },
            { key: 'rejected', label: 'shed', color: 'var(--series-2)', points: series(frames, (f) => f.rejectedRps) },
          ]}
          format={(v) => fmtNum(v, 0)}
          unit=" rps"
          height={112}
        />
        <p className="note" style={{ marginBottom: 0 }}>
          Shedding early and shedding late are very different outcomes; this line is the early kind, at{' '}
          <code>max_queue</code>.
        </p>
      </Panel>
    </div>
  );
}
