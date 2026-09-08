import { useMemo } from 'react';
import type { Frame } from '../../lib/frame';
import type { ReplayFrame } from '../../lib/adapter';
import type { ScenarioConfig } from '../../lib/config';
import { attainment, badput, goodput, percentileSeries, series, xs } from '../../lib/derive';
import { mergeWindow } from '../../lib/frame';
import { fractionBelow, quantile } from '../../lib/hist';
import { Panel, Tile } from '../../components/ui';
import { LineChart } from '../../components/charts/LineChart';
import { HistogramChart } from '../../components/charts/Histogram';
import { fmtMs, fmtNum, fmtPct } from '../../lib/format';
import { smoothingLabel, useSmoothing } from '../../lib/smoothing';

const PCTS = [50, 90, 99, 99.9];

// The floor of the badput log axis: 0.01%, below which a real reading is drawn dashed at the
// floor rather than off the bottom of the chart. Kept beside the panel that uses it rather than
// in LineChart, whose own default (also 1e-4) is a chart-drawing concern, not a badput one.
const BADPUT_LOG_FLOOR = 1e-4;

/** goodput and throughput are both frame-scoped `Frame`; only `ServiceQuality` casts to reach the
 * wire's measured goodput, since the rest of the panel deliberately reads the client-derived one. */
function frameBadput(f: Frame): number | null {
  const rf = f as ReplayFrame;
  return badput(rf.goodputTokensPerS, f.outputTokensPerS);
}

/** Y-axis / hover label for the badput chart's log ticks: 100, 10, 1, 0.1, 0.01, all with "%". */
function fmtLogPct(v: number): string {
  const pct = v * 100;
  if (pct >= 1) return `${pct.toFixed(0)}%`;
  if (pct >= 0.1) return `${pct.toFixed(1)}%`;
  return `${pct.toFixed(2)}%`;
}

/** The readout: badput to two significant figures, "-" when the frame has no throughput to divide by. */
function fmtSig2Pct(fraction: number | null): string {
  if (fraction === null || !isFinite(fraction)) return '-';
  const pct = Math.max(fraction, 0) * 100;
  if (pct <= 0) return '0%';
  const decimals = Math.max(0, 1 - Math.floor(Math.log10(pct)));
  return `${pct.toFixed(decimals)}%`;
}

export function ServiceQuality({
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
  const att = attainment(frame, config.slo);
  const gp = goodput(frame, config.slo);
  const tp = frame.outputTokensPerS;
  const curBadput = frameBadput(frame);

  const badputPoints = useMemo(() => frames.map(frameBadput), [frames]);

  const merged = useMemo(
    () => ({
      ttft: mergeWindow(frames, (f) => f.ttft),
      itl: mergeWindow(frames, (f) => f.itl),
      e2e: mergeWindow(frames, (f) => f.e2e),
    }),
    [frames]
  );

  return (
    <div className="grid c2">
      <Panel
        title="Headline"
        sub={`over the ${config.slo.ttftMs} ms / ${config.slo.itlMs} ms / ${config.slo.e2eS} s targets · ${window}`}
        highlight={highlight === 'headline'}
        id="headline"
      >
        <div className="grid c4" style={{ gap: 6 }}>
          <Tile
            label="slo attainment"
            value={fmtPct(att, 1)}
            status={att > 0.97 ? 'good' : att > 0.8 ? 'serious' : 'critical'}
            statusText={att > 0.97 ? 'within target' : att > 0.8 ? 'degraded' : 'failing'}
          />
          <Tile label="goodput" value={fmtNum(gp, 0)} unit=" tok/s" note="delivered within SLO" />
          <Tile label="throughput" value={fmtNum(tp, 0)} unit=" tok/s" note="delivered at all" />
          <Tile
            dataTile="wasted"
            label="wasted"
            value={fmtPct(1 - (tp > 0 ? gp / tp : 1), 1)}
            note="tokens produced that earn no goodput"
            status={tp > 0 && gp / tp < 0.9 ? 'serious' : undefined}
            statusText={tp > 0 && gp / tp < 0.9 ? 'capacity spent on late tokens' : undefined}
          />
        </div>
        <p className="note" style={{ margin: '7px 0 0' }}>
          A fleet can have excellent throughput and near-zero goodput by making everyone slightly too slow, which is
          why the two are drawn on the same axis below rather than in separate panels.
        </p>
      </Panel>

      <Panel title="Goodput against throughput" sub={`same unit, same axis · ${window}`} highlight={highlight === 'goodput'} id="goodput">
        <LineChart
          xs={x}
          series={[
            { key: 'tp', label: 'throughput', color: 'var(--series-2)', points: series(frames, (f) => f.outputTokensPerS) },
            { key: 'gp', label: 'goodput', color: 'var(--series-1)', points: series(frames, (f) => goodput(f, config.slo)) },
          ]}
          format={(v) => fmtNum(v, 0)}
          unit=" tok/s"
          height={116}
        />
      </Panel>

      <Panel
        title="Badput"
        sub={`1 − goodput / throughput, log scale so the region near 100% goodput has resolution · ${window}`}
        highlight={highlight === 'badput'}
        id="badput"
      >
        <div className="grid" style={{ gridTemplateColumns: 'minmax(0, 1fr) 130px', gap: 10, alignItems: 'center' }}>
          <LineChart
            xs={x}
            series={[{ key: 'bp', label: 'badput', color: 'var(--critical)', points: badputPoints }]}
            format={fmtLogPct}
            logY
            logFloor={BADPUT_LOG_FLOOR}
            yMax={1}
            height={116}
          />
          <Tile
            dataTile="badput"
            label="badput"
            value={fmtSig2Pct(curBadput)}
            note="of tokens delivered, this share missed their SLO"
          />
        </div>
        <p className="note" style={{ margin: '7px 0 0' }}>
          A window with no throughput has no fraction to report and leaves a gap rather than a false
          zero; a real reading below the {fmtLogPct(BADPUT_LOG_FLOOR)} floor draws dashed at it instead
          of running off the bottom of the chart.
        </p>
      </Panel>

      <Panel title="Time to first token" sub={`p50, p90, p99, p99.9 · ${window}`} highlight={highlight === 'ttft'} id="ttft">
        <LineChart
          xs={x}
          series={PCTS.map((p, i) => ({
            key: `p${p}`,
            label: `p${p}`,
            color: `var(--series-${(i % 3) + 1})`,
            dashed: i === 3,
            points: percentileSeries(frames, (f) => f.ttft, p),
          }))}
          format={(v) => fmtMs(v)}
          height={116}
          thresholds={[{ value: config.slo.ttftMs, label: 'slo' }]}
        />
      </Panel>

      <Panel title="Inter-token latency" sub={`percentiles, step time is the floor · ${window}`} highlight={highlight === 'itl'} id="itl">
        <LineChart
          xs={x}
          series={PCTS.map((p, i) => ({
            key: `p${p}`,
            label: `p${p}`,
            color: `var(--series-${(i % 3) + 1})`,
            dashed: i === 3,
            points: percentileSeries(frames, (f) => f.itl, p),
          }))}
          format={(v) => fmtMs(v)}
          height={116}
          thresholds={[{ value: config.slo.itlMs, label: 'slo' }]}
        />
      </Panel>

      <Panel
        title="Distribution over the window"
        sub="merged bucket-wise from the recorded histograms"
        highlight={highlight === 'dist'}
        id="dist"
      >
        <p className="section-label" style={{ marginBottom: 2 }}>time to first token</p>
        <HistogramChart hist={merged.ttft} threshold={config.slo.ttftMs} thresholdLabel="ttft slo" marks={[50, 99]} height={86} />
        <p className="section-label" style={{ margin: '8px 0 2px' }}>inter-token latency</p>
        <HistogramChart hist={merged.itl} threshold={config.slo.itlMs} thresholdLabel="itl slo" marks={[50, 99]} height={86} />
      </Panel>

      <Panel title="Percentile table" sub="window merged" bodyClass="tight" highlight={highlight === 'table'} id="table">
        <table className="data">
          <thead>
            <tr>
              <th className="nosort">metric</th>
              {PCTS.map((p) => (
                <th key={p} className="nosort">p{p}</th>
              ))}
              <th className="nosort">mean</th>
              <th className="nosort">within slo</th>
            </tr>
          </thead>
          <tbody>
            {(
              [
                ['ttft', merged.ttft, config.slo.ttftMs],
                ['itl', merged.itl, config.slo.itlMs],
                ['end to end', merged.e2e, config.slo.e2eS * 1000],
              ] as const
            ).map(([label, h, thr]) => (
              <tr key={label}>
                <td>{label}</td>
                {PCTS.map((p) => (
                  <td className="n" key={p}>{fmtMs(quantile(h, p))}</td>
                ))}
                <td className="n">{fmtMs(h.count > 0 ? h.sum / h.count : 0)}</td>
                <td className={`n${fractionBelow(h, thr) < 0.97 ? ' warn' : ''}`}>{fmtPct(fractionBelow(h, thr), 1)}</td>
              </tr>
            ))}
          </tbody>
          <caption>
            Percentiles are not mergeable, so a target spanning shards ships histograms and Ingress merges before
            computing. This table is the merged case: accurate at bucket resolution rather than exact, and it says so.
          </caption>
        </table>
      </Panel>
    </div>
  );
}
