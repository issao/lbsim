import type { FractionPercentiles, Frame } from '../../lib/frame';
import type { ScenarioConfig } from '../../lib/config';
import { fractionPercentileSeries, percentilesOver, series, xs } from '../../lib/derive';
import { Panel, Tile, Unwired } from '../../components/ui';
import { LineChart } from '../../components/charts/LineChart';
import { fmtNum, fmtPct } from '../../lib/format';

/** The percentiles the wire reports across replicas; the same three the fallback computes. */
const UTIL_PCTS = [50, 90, 99];

/**
 * The spread across replicas: the wire's distribution when the fleet row carried one, else recomputed
 * from the per-replica rows when the frame has them (an older recording), else a gap.
 */
function spread(f: Frame, fleet: FractionPercentiles | null, perReplica: (r: Frame['replicas'][number]) => number): FractionPercentiles | null {
  if (fleet) return fleet;
  if (f.replicas.length === 0) return null;
  return percentilesOver(f.replicas.filter((r) => r.present).map(perReplica), UTIL_PCTS);
}

/** Mean plus p50/p90/p99 bands, the latency panels' idiom on a fraction. */
function bands(frames: Frame[], mean: (f: Frame) => number, pick: (f: Frame) => FractionPercentiles | null) {
  return [
    { key: 'mean', label: 'mean', color: 'var(--series-1)', points: series(frames, mean) },
    ...UTIL_PCTS.map((p, i) => ({
      key: `p${p}`,
      label: `p${p}`,
      color: `var(--series-${(i % 2) + 2})`,
      dashed: p === 99,
      points: fractionPercentileSeries(frames, pick, p),
    })),
  ];
}

/** The engine's `preemption` key as the KV panel states it; `never` is the engine default. */
const PREEMPTION_MODE: Record<string, string> = {
  never: 'never',
  recompute: 'recompute',
  swap_to_dram: 'swap',
  swap_else_recompute: 'swap, else recompute',
};

// Wasted GPU, prefix hits and the memory tiers are not on the wire yet: the tiles say so, and there
// is no chart for a quantity the engine does not measure. Preemptions are (U115).
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
  const x = xs(frames);
  const gpuSpread = (f: Frame) => spread(f, f.gpuUtilizationP, (r) => r.gpuUtilization);
  const kvSpread = (f: Frame) => spread(f, f.kvUtilizationP, (r) => r.kvUtilization);
  const preemption = String(config.extra.preemption ?? 'never');
  const victim = String(config.extra.preemption_victim ?? 'newest');

  return (
    <div className="grid c2">
      <Panel title="GPU utilization" sub="fleet mean and percentiles across replicas" highlight={highlight === 'gpu'} id="gpu">
        <LineChart
          xs={x}
          series={bands(frames, (f) => f.gpuUtilization, gpuSpread)}
          format={(v) => fmtPct(v, 0)}
          yMax={1.05}
          height={116}
        />
        <p className="note" style={{ margin: '7px 0 0' }}>
          Wasted GPU fraction is invisible in a utilization metric: a decode-bound replica reports high utilization
          across a wide range of actual useful work, which is why autoscaling on GPU utilization does not work here.
          Compute-bound is the share of busy time under the compute roofline, {fmtPct(frame.gpuComputeBoundFraction, 0)} across
          the fleet now; the rest is bandwidth-bound decode at small batch.
        </p>
      </Panel>

      <Panel title="Where the capacity goes" highlight={highlight === 'capacity'} id="capacity">
        <div className="grid" style={{ gap: 6, gridTemplateColumns: 'repeat(auto-fit, minmax(120px, 1fr))' }}>
          <Tile
            label="gpu utilization"
            value={fmtPct(frame.gpuUtilization, 0)}
            note="busy share of the window"
            status={frame.gpuUtilization > 0.95 ? 'serious' : undefined}
            statusText={frame.gpuUtilization > 0.95 ? 'saturated' : undefined}
          />
          <Tile
            label="kv utilization"
            value={fmtPct(frame.kvUtilization, 0)}
            status={frame.kvUtilization > 0.95 ? 'critical' : frame.kvUtilization > 0.85 ? 'serious' : 'good'}
            statusText={frame.kvUtilization > 0.95 ? 'preempting' : frame.kvUtilization > 0.85 ? 'tight' : 'headroom'}
          />
          <Tile label="wasted gpu" value={<Unwired what="wastedGpuFraction" />} note="on tokens never delivered" />
          <Tile label="preemptions" value={`${fmtNum(frame.preemptionsPerS, 1)} /s`} note="contexts evicted" />
          <Tile
            label="prefix hit rate"
            value={<Unwired what="prefixHitRate" />}
            note={config.routing.kind === 'prefix_affinity' ? 'affinity routing' : 'incidental only'}
          />
        </div>
      </Panel>

      <Panel title="Key-value cache utilization" sub="fleet mean and percentiles across replicas" highlight={highlight === 'kv'} id="kv">
        <LineChart
          xs={x}
          series={bands(frames, (f) => f.kvUtilization, kvSpread)}
          format={(v) => fmtPct(v, 0)}
          yMax={1.05}
          height={116}
          thresholds={[{ value: 0.96, label: 'preempt' }]}
        />
        {/* Issao (U115): a full cache showed no preemptions. Under `never` that is the engine's
            behaviour, not a missing wire, so the panel says which it is. */}
        <p className="note" style={{ margin: '7px 0 0' }}>
          {preemption === 'never' ? (
            <>preemption: never &mdash; arrivals wait for space instead of evicting</>
          ) : (
            <>
              preemption: {PREEMPTION_MODE[preemption] ?? preemption} ({victim.replace(/_/g, ' ')} victim) &middot;{' '}
              {fmtNum(frame.preemptionsPerS, 1)}/s
            </>
          )}
        </p>
      </Panel>

      <Panel title="Shed load and preemptions" sub="the expensive failures" highlight={highlight === 'preempt'} id="preempt">
        <LineChart
          xs={x}
          series={[
            { key: 'shed', label: 'shed', color: 'var(--series-1)', points: series(frames, (f) => f.rejectedRps) },
            { key: 'preempt', label: 'preempted', color: 'var(--series-2)', points: series(frames, (f) => f.preemptionsPerS) },
          ]}
          format={(v) => v.toFixed(1)}
          unit=" /s"
          height={116}
        />
      </Panel>
    </div>
  );
}
