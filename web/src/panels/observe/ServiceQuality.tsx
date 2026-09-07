import { useMemo } from 'react';
import type { Frame } from '../../lib/engine';
import type { ScenarioConfig } from '../../lib/config';
import { attainment, goodput, percentileSeries, series, xs } from '../../lib/derive';
import { mergeWindow } from '../../lib/engine';
import { fractionBelow, quantile } from '../../lib/hist';
import { Panel, Tile } from '../../components/ui';
import { LineChart } from '../../components/charts/LineChart';
import { HistogramChart } from '../../components/charts/Histogram';
import { fmtMs, fmtNum, fmtPct } from '../../lib/format';
import { useSubscriptions } from '../../lib/useSubscriptions';
import { Metric } from '../../lib/types';
import { realness } from '../../lib/wired';

const PCTS = [50, 90, 99, 99.9];

// Fields this panel reads off Frame, directly and via attainment()/goodput() which only touch the
// histograms already listed here. Keep this list honest: it drives the mock tag on every Panel below.
const FRAME_READS: (keyof Frame)[] = ['outputTokensPerS', 'ttft', 'itl', 'e2e'];

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
  useSubscriptions(
    'service-quality',
    [{ scope: 'FLEET' }],
    [Metric.TTFT, Metric.ITL, Metric.E2E, Metric.SLO_ATTAINMENT, Metric.GOODPUT_TOKENS_PER_S, Metric.OUTPUT_TOKENS_PER_S],
    config.samplesPerSimSecond
  );

  const x = xs(frames);
  const data = realness(frame, FRAME_READS);
  const att = attainment(frame, config.slo);
  const gp = goodput(frame, config.slo);
  const tp = frame.outputTokensPerS;

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
        sub={`over the ${config.slo.ttftMs} ms / ${config.slo.itlMs} ms / ${config.slo.e2eS} s targets`}
        highlight={highlight === 'headline'}
        id="headline"
        data={data}
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

      <Panel title="Goodput against throughput" sub="same unit, same axis" highlight={highlight === 'goodput'} id="goodput" data={data}>
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

      <Panel title="Time to first token" sub="percentiles, requested [50, 90, 99, 99.9]" highlight={highlight === 'ttft'} id="ttft" data={data}>
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

      <Panel title="Inter-token latency" sub="percentiles; step time is the floor" highlight={highlight === 'itl'} id="itl" data={data}>
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
        data={data}
      >
        <p className="section-label" style={{ marginBottom: 2 }}>time to first token</p>
        <HistogramChart hist={merged.ttft} threshold={config.slo.ttftMs} thresholdLabel="ttft slo" marks={[50, 99]} height={86} />
        <p className="section-label" style={{ margin: '8px 0 2px' }}>inter-token latency</p>
        <HistogramChart hist={merged.itl} threshold={config.slo.itlMs} thresholdLabel="itl slo" marks={[50, 99]} height={86} />
      </Panel>

      <Panel title="Percentile table" sub="window merged" bodyClass="tight" highlight={highlight === 'table'} id="table" data={data}>
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
