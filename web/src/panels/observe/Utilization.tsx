import type { Frame } from '../../lib/engine';
import type { ScenarioConfig } from '../../lib/config';
import { series, xs } from '../../lib/derive';
import { Panel, Tile } from '../../components/ui';
import { LineChart } from '../../components/charts/LineChart';
import { fmtNum, fmtPct } from '../../lib/format';
import { useSubscriptions } from '../../lib/useSubscriptions';
import { Metric } from '../../lib/types';
import { realness } from '../../lib/wired';

// Fields this panel reads off Frame. Keep this list honest: it drives the mock tag on every Panel below.
const FRAME_READS: (keyof Frame)[] = [
  'kvUtilization',
  'wastedGpuFraction',
  'preemptionsPerS',
  'prefixHitRate',
  'tierUtilization',
  'tierBandwidth',
  'rejectedRps',
];

export function Utilization({
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
  useSubscriptions(
    'utilization',
    [{ scope: 'FLEET' }, { scope: 'POOL', id: 1 }],
    [
      Metric.KV_UTILIZATION,
      Metric.TIER_UTILIZATION,
      Metric.TIER_BANDWIDTH_UTILIZATION,
      Metric.WASTED_GPU_FRACTION,
      Metric.PREEMPTIONS_PER_S,
      Metric.PREFIX_HIT_RATE,
    ],
    config.samplesPerSimSecond
  );
  const x = xs(frames);
  const data = realness(frame, FRAME_READS);

  return (
    <div className="grid c2">
      <Panel title="Where the capacity goes" highlight={highlight === 'capacity'} id="capacity" data={data}>
        <div className="grid c4" style={{ gap: 6 }}>
          <Tile
            label="kv utilization"
            value={fmtPct(frame.kvUtilization, 0)}
            status={frame.kvUtilization > 0.95 ? 'critical' : frame.kvUtilization > 0.85 ? 'serious' : 'good'}
            statusText={frame.kvUtilization > 0.95 ? 'preempting' : frame.kvUtilization > 0.85 ? 'tight' : 'headroom'}
          />
          <Tile
            label="wasted gpu"
            value={fmtPct(frame.wastedGpuFraction, 1)}
            note="on tokens never delivered"
            status={frame.wastedGpuFraction > 0.15 ? 'critical' : undefined}
            statusText={frame.wastedGpuFraction > 0.15 ? 'most of it under overload' : undefined}
          />
          <Tile label="preemptions" value={fmtNum(frame.preemptionsPerS, 1)} unit=" /s" />
          <Tile label="prefix hit rate" value={fmtPct(frame.prefixHitRate, 0)} note={config.routing.kind === 'prefix_affinity' ? 'affinity routing' : 'incidental only'} />
        </div>
        <p className="note" style={{ margin: '7px 0 0' }}>
          Wasted GPU fraction is invisible in a utilization metric: a decode-bound replica reports high utilization
          across a wide range of actual useful work, which is why autoscaling on GPU utilization does not work here.
        </p>
      </Panel>

      <Panel title="Key-value cache utilization" sub="fleet mean" highlight={highlight === 'kv'} id="kv" data={data}>
        <LineChart
          xs={x}
          series={[
            { key: 'kv', label: 'kv utilization', color: 'var(--series-1)', points: series(frames, (f) => f.kvUtilization) },
          ]}
          format={(v) => fmtPct(v, 0)}
          yMax={1.05}
          height={116}
          thresholds={[{ value: 0.96, label: 'preempt' }]}
        />
      </Panel>

      <Panel title="Memory-tier occupancy" sub="HBM on the replica, DRAM and SSD cluster-pooled" highlight={highlight === 'tiers'} id="tiers" data={data}>
        <LineChart
          xs={x}
          series={[
            { key: 'hbm', label: 'hbm', color: 'var(--series-1)', points: series(frames, (f) => f.tierUtilization.hbm) },
            { key: 'dram', label: 'dram', color: 'var(--series-2)', points: series(frames, (f) => f.tierUtilization.dram) },
            { key: 'ssd', label: 'ssd', color: 'var(--series-3)', points: series(frames, (f) => f.tierUtilization.ssd) },
          ]}
          format={(v) => fmtPct(v, 0)}
          yMax={1.05}
          height={116}
        />
      </Panel>

      <Panel title="Tier bandwidth" sub="the shared path that inverts the swap-vs-recompute tradeoff" highlight={highlight === 'bandwidth'} id="bandwidth" data={data}>
        <LineChart
          xs={x}
          series={[
            { key: 'dram', label: 'dram path', color: 'var(--series-2)', points: series(frames, (f) => f.tierBandwidth.dram) },
            { key: 'ssd', label: 'ssd path', color: 'var(--series-3)', points: series(frames, (f) => f.tierBandwidth.ssd) },
          ]}
          format={(v) => fmtPct(v, 0)}
          yMax={1.05}
          height={116}
        />
        <p className="note" style={{ margin: '5px 0 0' }}>
          Swapping to DRAM costs about 20 ms each way and recomputing 4k tokens of prefill costs about 270 ms, so swap
          wins &mdash; until this line saturates, at which point the advantage inverts.
        </p>
      </Panel>

      <Panel title="Preemptions and shed load" sub="the expensive failures" highlight={highlight === 'preempt'} id="preempt" data={data}>
        <LineChart
          xs={x}
          series={[
            { key: 'preempt', label: 'preemptions', color: 'var(--series-2)', points: series(frames, (f) => f.preemptionsPerS) },
            { key: 'shed', label: 'shed', color: 'var(--series-1)', points: series(frames, (f) => f.rejectedRps) },
          ]}
          format={(v) => fmtNum(v, 1)}
          unit=" /s"
          height={116}
        />
      </Panel>

      <Panel title="Wasted GPU fraction" sub="cumulative over the run" highlight={highlight === 'wasted'} id="wasted" data={data}>
        <LineChart
          xs={x}
          series={[{ key: 'w', label: 'wasted', color: 'var(--series-2)', points: series(frames, (f) => f.wastedGpuFraction) }]}
          format={(v) => fmtPct(v, 0)}
          // A replayed frame carries NaN here until the engine measures waste; a NaN axis draws nothing.
          yMax={Math.max(0.2, Math.max(...frames.map((f) => f.wastedGpuFraction).filter(Number.isFinite)) * 1.3)}
          height={116}
        />
      </Panel>
    </div>
  );
}
