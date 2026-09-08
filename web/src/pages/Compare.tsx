import { useEffect, useMemo } from 'react';
import { BASE, cloneConfig, comparability, FIELD_LABEL, PRESETS, ROUTING_LABEL, type ScenarioConfig } from '../lib/config';
import { setActiveMode } from '../lib/mode';
import type { RoutingKind } from '../lib/types';
import { useRun, type RunHandle } from '../lib/useRun';
import { windowFrames, attainment, goodput, percentileSeries, series, xs } from '../lib/derive';
import { estimatedFleetRps } from '../lib/engine';
import { PlaybackBar } from '../components/PlaybackBar';
import { MockTag, Panel, Select, Slider, Tile } from '../components/ui';
import { LineChart } from '../components/charts/LineChart';
import { Heatmap } from '../components/charts/Heatmap';
import { fmtMs, fmtNum, fmtPct } from '../lib/format';

const A0: ScenarioConfig = (() => {
  const c = cloneConfig(BASE);
  c.name = 'A round robin';
  c.routing.kind = 'round_robin';
  c.workload.arrivalRps = 130;
  return c;
})();
const B0: ScenarioConfig = (() => {
  const c = cloneConfig(A0);
  c.name = 'B power of two choices';
  c.routing.kind = 'power_of_two_choices';
  return c;
})();

/**
 * Two runs from the same scenario and the same seed, differing only in the policy.
 *
 * Seed equality is not a nicety: named independent random streams mean the workload does not shift
 * when the policy changes, so a visible difference is a difference in policy rather than in luck.
 * This view therefore refuses to place two runs side by side when anything but the policy differs,
 * and names the fields, rather than letting someone draw a conclusion from two unlike runs.
 */
export function Compare() {
  useEffect(() => setActiveMode('mock'), []);
  const a = useRun(A0, true);
  const b = useRun(B0, true);

  const gate = comparability(a.config, b.config);
  const linked = useLinked(a, b);

  const fa = windowFrames(a.engine, a.cursorS);
  const fb = windowFrames(b.engine, b.cursorS);
  const la = fa[fa.length - 1];
  const lb = fb[fb.length - 1];
  // One colour scale across both heatmaps, or the panels would invite a comparison they do not
  // support: the same shade would mean different numbers on each side.
  const sharedHeatMax = Math.max(4, maxQueue(fa), maxQueue(fb));

  const setBoth = (mut: (d: ScenarioConfig) => void) => {
    const na = cloneConfig(a.config);
    const nb = cloneConfig(b.config);
    mut(na);
    mut(nb);
    a.update(na);
    b.update(nb);
  };

  return (
    <div className="dash">
      <PlaybackBar run={linked} />
      <div className="banner">
        <span className="tagline">same seed {a.config.seed}</span>
        <span>
          Both runs draw from the same named streams, so the arrival sequence and the prompt and output lengths are
          identical. Only the routing policy differs.
        </span>
        <MockTag what="mock" />
      </div>

      <div className="dash-body" style={{ gridTemplateColumns: 'minmax(0, 1fr)' }}>
        <Panel title="Shared scenario" sub="applied to both runs at once, so they cannot drift apart by accident">
          <div style={{ display: 'grid', gridTemplateColumns: 'repeat(auto-fit, minmax(190px, 1fr))', gap: '0 14px' }}>
            <Slider
              label="arrival rate (both)"
              value={a.config.workload.arrivalRps}
              min={20}
              max={400}
              step={5}
              format={(v) => `${v} rps`}
              onChange={(v) => setBoth((d) => { d.workload.arrivalRps = v; })}
              note={`capacity about ${fmtNum(estimatedFleetRps(a.config), 0)} rps, identical for both runs`}
            />
            <Slider
              label="long-request probability (both)"
              value={a.config.workload.longProbability}
              min={0}
              max={0.4}
              step={0.01}
              format={(v) => `${(v * 100).toFixed(0)}%`}
              onChange={(v) => setBoth((d) => { d.workload.longProbability = v; })}
              note="at zero, the two policies converge: the imbalance was size heterogeneity"
            />
            <Slider
              label="telemetry delay (both)"
              value={a.config.telemetryDelayMs}
              min={0}
              max={2000}
              step={50}
              format={(v) => `${v} ms`}
              onChange={(v) => setBoth((d) => { d.telemetryDelayMs = v; })}
            />
          </div>
        </Panel>

        {!gate.ok ? (
          <Refusal a={a.config} b={b.config} blocking={gate.blocking} onFix={() => {
              b.restart({ ...cloneConfig(a.config), name: b.config.name, routing: { ...b.config.routing } });
              b.scrubTo(a.cursorS);
            }} />
        ) : null}

        <div className="ab-grid">
          <Side run={a} label="A" other={b} enabled={gate.ok} heatMax={sharedHeatMax} />
          <Side run={b} label="B" other={a} enabled={gate.ok} heatMax={sharedHeatMax} />
        </div>

        {gate.ok && la && lb ? (
          <Panel
            title="Difference"
            sub={`${ROUTING_LABEL[a.config.routing.kind]} against ${ROUTING_LABEL[b.config.routing.kind]}, at ${a.cursorS.toFixed(0)} s`}
            bodyClass="tight"
          >
            <table className="data diff-table">
              <thead>
                <tr>
                  <th className="nosort">measure</th>
                  <th className="nosort">A</th>
                  <th className="nosort">B</th>
                  <th className="nosort">B − A</th>
                </tr>
              </thead>
              <tbody>
                <Row label="slo attainment" a={attainment(la, a.config.slo)} b={attainment(lb, b.config.slo)} fmt={(v) => fmtPct(v, 1)} betterHigh />
                <Row label="goodput tok/s" a={goodput(la, a.config.slo)} b={goodput(lb, b.config.slo)} fmt={(v) => fmtNum(v, 0)} betterHigh />
                <Row label="ttft p99" a={p99(fa, 'ttft')} b={p99(fb, 'ttft')} fmt={fmtMs} />
                <Row label="itl p99" a={p99(fa, 'itl')} b={p99(fb, 'itl')} fmt={fmtMs} />
                <Row label="imbalance cv" a={la.loadImbalanceCv} b={lb.loadImbalanceCv} fmt={(v) => v.toFixed(3)} />
                <Row label="queue, fleet mean" a={meanQueue(la)} b={meanQueue(lb)} fmt={(v) => v.toFixed(1)} />
                <Row label="wasted gpu" a={la.wastedGpuFraction} b={lb.wastedGpuFraction} fmt={(v) => fmtPct(v, 1)} />
              </tbody>
              <caption>
                The two runs saw the same arrivals, so a difference here is the policy. That is the only claim this view
                is entitled to make, and it is only entitled to make it while the seeds and the workload match.
              </caption>
            </table>
          </Panel>
        ) : null}
      </div>
    </div>
  );
}

function Refusal({
  a,
  b,
  blocking,
  onFix,
}: {
  a: ScenarioConfig;
  b: ScenarioConfig;
  blocking: string[];
  onFix: () => void;
}) {
  const val = (c: ScenarioConfig, path: string): string => {
    const [head, tail] = path.split('.');
    const root = c as unknown as Record<string, unknown>;
    const v = tail ? (root[head] as Record<string, unknown>)[tail] : root[head];
    return String(v);
  };
  return (
    <div className="refusal">
      <h3>
        <i className="dot critical" /> Refusing to compare these two runs
      </h3>
      <p className="why">
        Two runs are only comparable when they differ in the policy alone. These differ in{' '}
        {blocking.length === 1 ? 'one other field' : `${blocking.length} other fields`}, so any difference you saw in
        the panels below could be luck rather than policy. Fix the mismatch and the comparison comes back.
      </p>
      <ul>
        {blocking.map((p) => (
          <li key={p}>
            <b>{FIELD_LABEL[p] ?? p}</b>: A = <span className="num">{val(a, p)}</span>, B ={' '}
            <span className="num">{val(b, p)}</span>
          </li>
        ))}
      </ul>
      <div className="btn-row" style={{ marginTop: 8 }}>
        <button className="btn primary" onClick={onFix}>
          copy A's scenario to B, keeping B's policy
        </button>
      </div>
    </div>
  );
}

function maxQueue(frames: { replicas: { queuedSeqs: number }[] }[]): number {
  let m = 0;
  for (const f of frames) for (const r of f.replicas) if (r.queuedSeqs > m) m = r.queuedSeqs;
  return m;
}

function Side({
  run,
  label,
  other,
  enabled,
  heatMax,
}: {
  run: RunHandle;
  label: string;
  other: RunHandle;
  enabled: boolean;
  heatMax: number;
}) {
  const frames = windowFrames(run.engine, run.cursorS);
  const last = frames[frames.length - 1];
  const kinds = Object.keys(ROUTING_LABEL) as RoutingKind[];
  const preset = PRESETS.find((p) => p.id === 'p2c');
  const heatRows = useMemo(() => {
    const ids = last ? last.replicas.filter((r) => r.present).map((r) => r.id) : [];
    return ids.map((id) => ({
      id,
      values: frames.map((f) => f.replicas.find((x) => x.id === id)?.queuedSeqs ?? 0),
    }));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [frames.length, run.revision, last?.tick]);

  if (!last) return <Panel title={label} sub="warming up">{null}</Panel>;
  const att = attainment(last, run.config.slo);

  return (
    <div className="grid" style={{ gridTemplateColumns: 'minmax(0, 1fr)' }}>
      <Panel
        title={`Run ${label}`}
        sub={enabled ? ROUTING_LABEL[run.config.routing.kind] : `${ROUTING_LABEL[run.config.routing.kind]} — shown for reference, not comparable`}
      >
        <Select
          label="routing policy"
          value={run.config.routing.kind}
          options={kinds.map((k) => ({ value: k, label: ROUTING_LABEL[k] }))}
          onChange={(v) => {
            const next = cloneConfig(run.config);
            next.routing.kind = v;
            run.update(next);
          }}
          note={
            v(run) === v(other) ? 'both sides are running the same policy, so there is nothing to compare' : undefined
          }
        />
        <div className="grid c4" style={{ gap: 6 }}>
          <Tile
            label="attainment"
            value={enabled ? fmtPct(att, 1) : '—'}
            status={enabled ? (att > 0.97 ? 'good' : att > 0.8 ? 'serious' : 'critical') : undefined}
            statusText={enabled ? (att > 0.97 ? 'within target' : att > 0.8 ? 'degraded' : 'failing') : undefined}
          />
          <Tile label="goodput" value={enabled ? fmtNum(goodput(last, run.config.slo), 0) : '—'} unit=" tok/s" />
          <Tile label="ttft p99" value={enabled ? fmtMs(p99(frames, 'ttft')) : '—'} />
          <Tile label="imbalance cv" value={enabled ? last.loadImbalanceCv.toFixed(3) : '—'} />
        </div>
        {preset && run.config.routing.kind === 'round_robin' ? (
          <p className="note" style={{ margin: '6px 0 0' }}>
            Equal request counts, unequal work. Watch the heatmap below rather than the tiles.
          </p>
        ) : null}
      </Panel>

      <Panel title="Queue depth per replica" sub="same colour scale on both sides">
        <Heatmap
          rows={heatRows}
          colTimes={frames.map((f) => f.simS)}
          rowLabel="replica"
          valueLabel="queued requests"
          format={(x) => x.toFixed(0)}
          vmax={heatMax}
          cellH={6}
        />
      </Panel>

      <Panel title="Time to first token" sub="p50 and p99">
        <LineChart
          xs={xs(frames)}
          series={[
            { key: 'p50', label: 'p50', color: 'var(--series-1)', points: percentileSeries(frames, (f) => f.ttft, 50) },
            { key: 'p99', label: 'p99', color: 'var(--series-2)', points: percentileSeries(frames, (f) => f.ttft, 99) },
          ]}
          format={fmtMs}
          height={100}
          thresholds={[{ value: run.config.slo.ttftMs, label: 'slo' }]}
        />
      </Panel>

      <Panel title="Goodput against throughput">
        <LineChart
          xs={xs(frames)}
          series={[
            { key: 'tp', label: 'throughput', color: 'var(--series-2)', points: series(frames, (f) => f.outputTokensPerS) },
            { key: 'gp', label: 'goodput', color: 'var(--series-1)', points: series(frames, (f) => goodput(f, run.config.slo)) },
          ]}
          format={(x) => fmtNum(x, 0)}
          height={100}
        />
      </Panel>
    </div>
  );
}

function v(r: RunHandle): string {
  return r.config.routing.kind;
}

function Row({
  label,
  a,
  b,
  fmt,
  betterHigh = false,
}: {
  label: string;
  a: number;
  b: number;
  fmt: (v: number) => string;
  betterHigh?: boolean;
}) {
  const d = b - a;
  const good = betterHigh ? d > 0 : d < 0;
  const material = Math.abs(d) > Math.abs(a) * 0.02;
  return (
    <tr>
      <td>{label}</td>
      <td className="n a">{fmt(a)}</td>
      <td className="n b">{fmt(b)}</td>
      <td className="n" style={{ color: material ? (good ? 'var(--good)' : 'var(--critical)') : 'var(--ink-3)' }}>
        {d >= 0 ? '+' : '−'}
        {fmt(Math.abs(d))}
      </td>
    </tr>
  );
}

function p99(frames: { ttft: never; itl: never }[] | Parameters<typeof percentileSeries>[0], which: 'ttft' | 'itl'): number {
  const s = percentileSeries(frames as Parameters<typeof percentileSeries>[0], (f) => (which === 'ttft' ? f.ttft : f.itl), 99);
  return s[s.length - 1] ?? 0;
}

function meanQueue(f: { replicas: { present: boolean; queuedSeqs: number }[] }): number {
  const live = f.replicas.filter((r) => r.present);
  return live.length ? live.reduce((s, r) => s + r.queuedSeqs, 0) / live.length : 0;
}

/** One playback bar driving both runs, since the whole point is that they stay aligned in time. */
function useLinked(a: RunHandle, b: RunHandle): RunHandle {
  return {
    ...a,
    setPaused: (p) => { a.setPaused(p); b.setPaused(p); },
    setSpeed: (f) => { a.setSpeed(f); b.setSpeed(f); },
    step: () => { a.step(); b.step(); },
    rewindTo: (s) => { a.rewindTo(s); b.rewindTo(s); },
    scrubTo: (s) => { a.scrubTo(s); b.scrubTo(s); },
    recordedToS: Math.min(a.recordedToS, b.recordedToS),
  };
}
