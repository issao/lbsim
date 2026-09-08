import type { Frame } from '../../lib/frame';
import type { ScenarioConfig } from '../../lib/config';
import { healthCounts, series, xs } from '../../lib/derive';
import { Panel, Tile, Unwired } from '../../components/ui';
import { LineChart } from '../../components/charts/LineChart';
import { fmtNum } from '../../lib/format';
import { smoothingLabel, useSmoothing } from '../../lib/smoothing';

// The warming, draining and ejected counts and the failure events are not on the wire yet: the
// tiles say so, and the fleet-state chart draws the one series the engine measures.
export function ClusterHealth({
  frames,
  frame,
  config,
  highlight,
}: {
  frames: Frame[];
  frame: Frame;
  config: ScenarioConfig;
  highlight?: string | null;
}) {
  const h = healthCounts(frame);
  const x = xs(frames);
  const window = smoothingLabel(useSmoothing());

  return (
    <div className="grid c2">
      <Panel title="Fleet state" sub={`${config.fleet.accelerator}, 1 cluster / 1 pool`} highlight={highlight === 'fleet-state'} id="fleet-state">
        <div className="grid c4" style={{ gap: 6 }}>
          <Tile label="ready" value={String(h.ready)} />
          <Tile label="warming" value={<Unwired what="warmingReplicas" />} note="cold start" />
          <Tile label="draining" value={<Unwired what="drainingReplicas" />} />
          <Tile label="ejected" value={<Unwired what="ejectedReplicas" />} />
        </div>
        <div style={{ marginTop: 8 }}>
          <LineChart
            xs={x}
            series={[{ key: 'ready', label: 'ready', color: 'var(--series-1)', points: series(frames, (f) => f.readyReplicas) }]}
            format={(v) => v.toFixed(0)}
            height={92}
          />
        </div>
      </Panel>

      <Panel title="Health is a vector, not a boolean" sub={`announced state against true speed · ${window}`} highlight={highlight === 'gray'} id="gray">
        <div className="grid c2" style={{ gap: 6 }}>
          <Tile
            label="announcing healthy, degraded"
            value={Number.isFinite(h.gray) ? String(h.gray) : <Unwired what="trueSpeedMultiplier" />}
            status={Number.isFinite(h.gray) ? (h.gray > 0 ? 'critical' : 'good') : undefined}
            statusText={Number.isFinite(h.gray) ? (h.gray > 0 ? 'gray failure in progress' : 'none detected') : undefined}
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

      <Panel title="Throughput against arrivals" sub={`offered, completed, shed · ${window}`} highlight={highlight === 'arrivals'} id="arrivals">
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
