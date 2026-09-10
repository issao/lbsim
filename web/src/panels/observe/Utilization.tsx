import type { FractionPercentiles, Frame } from '../../lib/frame';
import type { ScenarioConfig } from '../../lib/config';
import { fractionPercentileSeries, percentilesOver, series, xs } from '../../lib/derive';
import { Panel, Tile, Unwired } from '../../components/ui';
import { LineChart } from '../../components/charts/LineChart';
import { fmtNum, fmtPct } from '../../lib/format';
import { smoothingLabel, useSmoothing } from '../../lib/smoothing';

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
function bands(frames: Frame[], mean: (f: Frame) => number, pick: (f: Frame) => FractionPercentiles | null, label = 'mean') {
  return [
    { key: 'mean', label, color: 'var(--series-1)', points: series(frames, mean) },
    ...UTIL_PCTS.map((p, i) => ({
      key: `p${p}`,
      label: `p${p}`,
      color: `var(--series-${(i % 2) + 2})`,
      dashed: p === 99,
      points: fractionPercentileSeries(frames, pick, p),
    })),
  ];
}

/**
 * The readout's one sentence on batching: utilization over useful is the mean batch fill, so
 * "55% busy, 0.5% useful: batches average 1/100 of the limit". A gap when either is unmeasured.
 */
function batchFill(f: Frame): string {
  const busy = f.gpuUtilization;
  const useful = f.gpuUsefulFraction;
  if (!isFinite(busy) || !isFinite(useful) || busy <= 0) return 'no steps in this window yet';
  if (useful <= 0) return `${fmtPct(busy, 0)} busy, nothing useful yet`;
  const fill = useful / busy;
  const denominator = Math.max(1, Math.round(1 / fill));
  return `${fmtPct(busy, 0)} busy, ${fmtPct(useful, 1)} useful: batches average 1/${fmtNum(denominator, 0)} of the limit`;
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
  const window = smoothingLabel(useSmoothing());
  const gpuSpread = (f: Frame) => spread(f, f.gpuUtilizationP, (r) => r.gpuUtilization);
  const kvSpread = (f: Frame) => spread(f, f.kvUtilizationP, (r) => r.kvUtilization);
  const preemption = String(config.extra.preemption ?? 'never');
  const victim = String(config.extra.preemption_victim ?? 'newest');

  return (
    <div className="grid c2">
      <Panel title="GPU utilization" sub={`time in step, mean and percentiles across replicas; useful work in grey · ${window}`} highlight={highlight === 'gpu'} id="gpu">
        <LineChart
          xs={x}
          series={[
            ...bands(frames, (f) => f.gpuUtilization, gpuSpread, 'utilization'),
            // Useful work over the maximum possible, on the same axes in a lighter line. Issao, on
            // seeing 53% utilization for a fleet doing 3% of its rated work: "Keep the old gpu
            // utilization, but add a new metric with useful GPU work / max possible, so we can get
            // a sense of how small the batches are."
            { key: 'useful', label: 'useful work', color: 'var(--ink-2)', points: series(frames, (f) => f.gpuUsefulFraction) },
          ]}
          format={(v) => fmtPct(v, 0)}
          yMax={1.05}
          height={116}
        />
        <p className="note" style={{ margin: '7px 0 0' }}>
          Utilization is time inside a step, what a GPU counter reports; useful work is the share of the maximum the
          replica could do, and the ratio of the two is how full the batches are: {batchFill(frame)}. At batch 1 a
          replica is busy but 1/{fmtNum(config.fleet.maxBatch, 0)} useful, because every step re-reads the weights
          whether one sequence or a full batch rides on the read, which is why autoscaling on GPU-Util does not work
          here. Wasted GPU fraction is invisible in either: a decode-bound replica reports high utilization across a
          wide range of actual useful work. Compute-bound is the share of busy time under the compute roofline,{' '}
          {fmtPct(frame.gpuComputeBoundFraction, 0)} across the fleet now; the rest is bandwidth-bound decode at small
          batch.
        </p>
      </Panel>

      <Panel title="Where the capacity goes" sub={window} highlight={highlight === 'capacity'} id="capacity">
        <div className="grid" style={{ gap: 6, gridTemplateColumns: 'repeat(auto-fit, minmax(120px, 1fr))' }}>
          <Tile
            label="gpu utilization"
            value={fmtPct(frame.gpuUtilization, 0)}
            note="time in step (GPU-Util)"
            status={frame.gpuUtilization > 0.95 ? 'serious' : undefined}
            statusText={frame.gpuUtilization > 0.95 ? 'saturated' : undefined}
            dataTile="gpu-utilization"
          />
          <Tile label="gpu useful" value={fmtPct(frame.gpuUsefulFraction, 1)} note="useful work of max possible" dataTile="gpu-useful" />
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

      <Panel title="Key-value cache utilization" sub={`fleet mean and percentiles across replicas · ${window}`} highlight={highlight === 'kv'} id="kv">
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

      <Panel title="Shed load and preemptions" sub={`the expensive failures · ${window}`} highlight={highlight === 'preempt'} id="preempt">
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
