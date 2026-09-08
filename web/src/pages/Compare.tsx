import { useEffect, useMemo, useState, type ReactNode } from 'react';
import { BASE, cloneConfig, comparability, FIELD_LABEL, PRESETS, ROUTING_LABEL, type ScenarioConfig } from '../lib/config';
import { DATA_SOURCE_LABEL, dataSourceGloss, setActiveMode, type DataMode } from '../lib/mode';
import type { RoutingKind } from '../lib/types';
import { useDataSource, useReplayRun, useRun, type ReplayRunHandle, type RunHandle } from '../lib/useRun';
import { speedLabel, useServerRun, type ServerRunHandle } from '../lib/useServerRun';
import { loadRun, runDurationS, type LoadedRun, type RunIndexEntry } from '../lib/replay';
import { loadIndex, loadScript } from '../lib/walkthrough';
import { windowFrames, attainment, goodput, percentileSeries, series, xs } from '../lib/derive';
import { estimatedFleetRps, type Frame } from '../lib/engine';
import { realness } from '../lib/wired';
import { PlaybackBar, UpdateBanner } from '../components/PlaybackBar';
import { MockTag, Panel, Select, Slider, Tile, type PanelData } from '../components/ui';
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
 *
 * Three sources, as on the dashboard: two runs started on the Ingress server from one scenario
 * text and one seed; two recordings a walkthrough declared as a pair; the browser's mock engine
 * otherwise. One body, `CompareBody`, draws all three, so what differs is only where the two
 * handles come from.
 */
export function Compare() {
  const src = useDataSource(true);
  useEffect(() => {
    if (src.state === 'probing') setActiveMode('connecting');
    if (src.state === 'mock') setActiveMode('mock');
  }, [src.state]);
  if (src.state === 'probing') return <div className="page-pad">looking for a server or recorded runs…</div>;
  if (src.state === 'server') return <ServerCompare />;
  if (src.state === 'replay') return <ReplayCompare runs={src.runs} />;
  return <MockCompare />;
}

// ---------------------------------------------------------------------------
// Live: two runs on the Ingress server, one scenario text, one seed, two policies
// ---------------------------------------------------------------------------

function ServerCompare() {
  // Two hooks, two engines: each is its own StartRun, its own subscription, its own SetSpeed and
  // UpdatePolicies. The page keeps them together; the server never learns they are a pair.
  const a = useServerRun(A0, { autoplay: true });
  const b = useServerRun(B0, { autoplay: true });
  useEffect(() => {
    const err = a.error ?? b.error;
    if (a.runId && b.runId) setActiveMode('server', `${a.runId} vs ${b.runId}`);
    else if (err) setActiveMode('refused', undefined, err);
    else setActiveMode('connecting');
  }, [a.runId, b.runId, a.error, b.error]);
  return <CompareBody a={a} b={b} mode="server" banner={<ServerPairBanner a={a} b={b} />} />;
}

/** Names both runs and what the server said no to, on either side. */
function ServerPairBanner({ a, b }: { a: ServerRunHandle; b: ServerRunHandle }) {
  return (
    <>
      <div className="banner">
        <span className="tagline">live</span>
        <span className="note">{dataSourceGloss('server')}</span>
        <span title={`streams ${a.connection} / ${b.connection}`}>
          run <code>{a.runId ?? '…'}</code> vs <code>{b.runId ?? '…'}</code> &middot; {a.engine.frames.length} and{' '}
          {b.engine.frames.length} samples &middot; speed {speedLabel(a.status, a.paused, a.achievedFactor)}
        </span>
        <span style={{ marginLeft: 'auto', color: 'var(--ink-3)' }} title={a.disabledReason}>
          rewind is off on a live run
        </span>
      </div>
      {[a, b].map((r, i) =>
        r.error ? (
          <div className="banner" key={`e${i}`}>
            <span className="tagline">server, run {i === 0 ? 'A' : 'B'}</span>
            <span style={{ color: 'var(--critical)' }}>{r.error}</span>
          </div>
        ) : null
      )}
      {[a, b].map((r, i) =>
        r.refused ? (
          <div className="banner" key={`r${i}`}>
            <span className="tagline">not applied to {i === 0 ? 'A' : 'B'}</span>
            <span style={{ color: 'var(--serious)' }}>{r.refused}</span>
            <button className="btn" onClick={r.dismissRefused}>
              dismiss
            </button>
          </div>
        ) : null
      )}
    </>
  );
}

// ---------------------------------------------------------------------------
// Replay: the two recordings a walkthrough declared as a pair, or the pair the URL names
// ---------------------------------------------------------------------------

type Pair = [string, string];

/** `?runs=<a>,<b>` before or after the hash, the way the dashboard takes `?run=`. */
function pairFromUrl(): Pair | null {
  const hash = window.location.hash;
  const q = hash.indexOf('?');
  const raw =
    new URLSearchParams(window.location.search).get('runs') ??
    (q === -1 ? null : new URLSearchParams(hash.slice(q + 1)).get('runs'));
  if (!raw) return null;
  const [x, y] = raw.split(',').map((s) => s.trim());
  return x && y ? [x, y] : null;
}

/** The first walkthrough, in showcase order, that exported both a `run` and a `compare`. */
async function walkthroughPair(): Promise<Pair | null> {
  const idx = await loadIndex();
  const scripts = await Promise.all(
    idx.cards.map((c) => (c.script ? loadScript(c.script).catch(() => null) : Promise.resolve(null)))
  );
  for (const s of scripts) if (s?.run && s.compare) return [s.run, s.compare];
  return null;
}

function ReplayCompare({ runs }: { runs: RunIndexEntry[] }) {
  // null: still looking; 'none': the index has no declared pair, so the mock stands in.
  const [pair, setPair] = useState<Pair | 'none' | null>(pairFromUrl);
  const [loaded, setLoaded] = useState<[LoadedRun, LoadedRun] | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (pair !== null) return;
    let alive = true;
    walkthroughPair()
      .then((p) => {
        if (alive) setPair(p ?? 'none');
      })
      .catch(() => {
        if (alive) setPair('none');
      });
    return () => {
      alive = false;
    };
  }, [pair]);

  useEffect(() => {
    if (pair === null || pair === 'none') return;
    const entries = pair.map((id) => runs.find((r) => r.runId === id));
    const missing = pair.filter((_, i) => !entries[i]);
    if (missing.length) {
      setError(`not in runs/index.json: ${missing.join(', ')}`);
      return;
    }
    let alive = true;
    setError(null);
    setActiveMode('replay', `${pair[0]} vs ${pair[1]}`);
    Promise.all(entries.map((e) => loadRun(e as RunIndexEntry)))
      .then(([la, lb]) => {
        if (alive) setLoaded([la, lb]);
      })
      .catch((e: unknown) => {
        if (alive) setError(e instanceof Error ? e.message : String(e));
      });
    return () => {
      alive = false;
    };
  }, [runs, pair]);

  if (pair === 'none') {
    return (
      <MockCompare
        note="no walkthrough declares a recorded pair (`run` and `compare`), and the URL names none, so this is the mock"
      />
    );
  }
  if (error) {
    return (
      <div className="dash">
        <div className="banner">
          <span className="tagline">replay</span>
          <span style={{ color: 'var(--critical)' }}>could not load the pair: {error}</span>
        </div>
      </div>
    );
  }
  if (pair === null || !loaded || loaded[0].entry.runId !== pair[0] || loaded[1].entry.runId !== pair[1]) {
    return (
      <div className="dash">
        <div className="banner">
          <span className="tagline">replay</span>
          <span>{pair === null ? 'looking for a recorded pair…' : `loading ${pair[0]} and ${pair[1]}…`}</span>
        </div>
      </div>
    );
  }
  // Keyed on the pair so a new pair mounts fresh handles rather than rewinding the old ones.
  return <ReplayPair key={pair.join('|')} la={loaded[0]} lb={loaded[1]} />;
}

function ReplayPair({ la, lb }: { la: LoadedRun; lb: LoadedRun }) {
  const a = useReplayRun(la, true);
  const b = useReplayRun(lb, true);
  return <CompareBody a={a} b={b} mode="replay" banner={<ReplayPairBanner a={a} b={b} />} />;
}

function ReplayPairBanner({ a, b }: { a: ReplayRunHandle; b: ReplayRunHandle }) {
  const ea = a.loaded.entry;
  const eb = b.loaded.entry;
  return (
    <div className="banner">
      <span className="tagline">replay</span>
      <span className="note">{dataSourceGloss('replay')}</span>
      <span title={`seeds ${a.loaded.result.seed.toString()} / ${b.loaded.result.seed.toString()}`}>
        <code>{ea.runId}</code> vs <code>{eb.runId}</code> &middot; {a.loaded.frames.length} and {b.loaded.frames.length}{' '}
        samples over {Math.min(runDurationS(ea), runDurationS(eb)).toFixed(0)} s
      </span>
      <span style={{ marginLeft: 'auto', color: 'var(--ink-3)' }} title={a.disabledReason}>
        rewind and changes are off on a replay
      </span>
    </div>
  );
}

// ---------------------------------------------------------------------------
// Mock: the browser's engine, twice
// ---------------------------------------------------------------------------

function MockCompare({ note }: { note?: string }) {
  const a = useRun(A0, true);
  const b = useRun(B0, true);
  useEffect(() => setActiveMode('mock'), []);
  const banner = note ? (
    <div className="banner">
      <span className="tagline">mock</span>
      <span className="note">{note}</span>
    </div>
  ) : null;
  return <CompareBody a={a} b={b} mode="mock" banner={banner} />;
}

// ---------------------------------------------------------------------------
// The surface, over any two handles
// ---------------------------------------------------------------------------

/** The Frame fields the panels below read, so each panel's tag can say what it is entitled to. */
const RUN_READS = ['ttft', 'itl', 'e2e', 'outputTokensPerS', 'loadImbalanceCv'] as const;
const DIFF_READS = [...RUN_READS, 'wastedGpuFraction', 'replicas'] as const;
const REPLICA_READS = ['id', 'present', 'queuedSeqs'] as const;

/**
 * The comparability gate. On live and mock it is `comparability()` as is. On a replay the pair is
 * a walkthrough author's declaration, and the policy it varies is usually an engine key under
 * `extra` (preemption, admission, tenants) rather than `routing`, so those differences are named
 * as the policy under comparison rather than refused; seed, workload and fleet still block.
 */
function gateFor(a: ScenarioConfig, b: ScenarioConfig, mode: DataMode) {
  const g = comparability(a, b);
  const blocking = mode === 'replay' ? g.blocking.filter((p) => !p.startsWith('extra.')) : g.blocking;
  const policy = g.differing.filter((p) => p.startsWith('routing.') || (mode === 'replay' && p.startsWith('extra.')));
  return { ok: blocking.length === 0, blocking, policy };
}

/** What a side's policy is, in the words the gate compared: the routing label, then any engine key that differs. */
function policyLabel(c: ScenarioConfig, policy: string[]): string {
  const parts = [ROUTING_LABEL[c.routing.kind]];
  for (const p of policy) {
    if (!p.startsWith('extra.')) continue;
    const k = p.slice('extra.'.length);
    parts.push(`${k} = ${String(c.extra[k] ?? '(unset)')}`);
  }
  return parts.join(', ');
}

function CompareBody({ a, b, mode, banner }: { a: RunHandle; b: RunHandle; mode: DataMode; banner?: ReactNode }) {
  const gate = gateFor(a.config, b.config, mode);
  // One cursor for both sides. Live, it is the earlier of the two live edges: each run streams at
  // its own pace and neither is rewound, so the later one waits for the other to catch up (the
  // lockstep of stepping both through StepForward is U97). Elsewhere it is the earlier of the two
  // cursors, which the linked controls keep equal.
  const cursorS = mode === 'server' ? Math.min(a.recordedToS, b.recordedToS) : Math.min(a.cursorS, b.cursorS);
  const linked = useLinked(a, b, cursorS);
  const locked = mode === 'replay';

  const fa = windowFrames(a.engine, cursorS);
  const fb = windowFrames(b.engine, cursorS);
  const la = fa[fa.length - 1];
  const lb = fb[fb.length - 1];
  // One colour scale across both heatmaps, or the panels would invite a comparison they do not
  // support: the same shade would mean different numbers on each side.
  const sharedHeatMax = Math.max(4, maxQueue(fa), maxQueue(fb));
  const seedsAgree = a.config.seed === b.config.seed;

  const setBoth = (mut: (d: ScenarioConfig) => void) => {
    const na = cloneConfig(a.config);
    const nb = cloneConfig(b.config);
    mut(na);
    mut(nb);
    a.update(na);
    b.update(nb);
  };

  const sliders = (
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
        note={mode === 'server' ? 'not live-tunable on the server: the change is refused, and named below' : undefined}
      />
    </div>
  );

  return (
    <div className="dash">
      <PlaybackBar run={linked} />
      {banner}
      <UpdateBanner run={a} />
      <UpdateBanner run={b} />
      <div className="banner">
        <span className="tagline">{seedsAgree ? `same seed ${a.config.seed}` : `seeds differ: ${a.config.seed} and ${b.config.seed}`}</span>
        <span>
          Both runs draw from the same named streams, so the arrival sequence and the prompt and output lengths are
          identical. Only the policy differs.
        </span>
        <MockTag what={DATA_SOURCE_LABEL[mode]} />
      </div>

      <div className="dash-body ab-body">
        <Panel
          title="Shared scenario"
          sub="applied to both runs at once, so they cannot drift apart by accident"
          data={{ kind: 'real', mockFields: [] }}
        >
          {locked ? (
            <fieldset className="dropped" disabled title={a.source?.disabledReason ?? undefined}>
              {sliders}
              <span className="dropped-why">recording</span>
            </fieldset>
          ) : (
            sliders
          )}
        </Panel>

        {!gate.ok ? (
          <Refusal
            a={a.config}
            b={b.config}
            blocking={gate.blocking}
            onFix={
              locked
                ? undefined
                : () => {
                    b.restart({ ...cloneConfig(a.config), name: b.config.name, routing: { ...b.config.routing } });
                    b.scrubTo(a.cursorS);
                  }
            }
          />
        ) : null}

        <div className="ab-grid">
          <Side run={a} label="A" other={b} cursorS={cursorS} enabled={gate.ok} heatMax={sharedHeatMax} policy={gate.policy} locked={locked} />
          <Side run={b} label="B" other={a} cursorS={cursorS} enabled={gate.ok} heatMax={sharedHeatMax} policy={gate.policy} locked={locked} />
        </div>

        {gate.ok && la && lb ? (
          <Panel
            title="Difference"
            sub={`${policyLabel(a.config, gate.policy)} against ${policyLabel(b.config, gate.policy)}, at ${cursorS.toFixed(0)} s`}
            bodyClass="tight"
            data={realness(la, DIFF_READS, REPLICA_READS)}
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
  /** Absent where nothing can be restarted: a recording is what it is. */
  onFix?: () => void;
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
        the panels below could be luck rather than policy.{' '}
        {onFix ? 'Fix the mismatch and the comparison comes back.' : 'Recordings cannot be changed; pick another pair.'}
      </p>
      <ul>
        {blocking.map((p) => (
          <li key={p}>
            <b>{FIELD_LABEL[p] ?? p}</b>: A = <span className="num">{val(a, p)}</span>, B ={' '}
            <span className="num">{val(b, p)}</span>
          </li>
        ))}
      </ul>
      {onFix ? (
        <div className="btn-row" style={{ marginTop: 8 }}>
          <button className="btn primary" onClick={onFix}>
            copy A's scenario to B, keeping B's policy
          </button>
        </div>
      ) : null}
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
  cursorS,
  enabled,
  heatMax,
  policy,
  locked,
}: {
  run: RunHandle;
  label: string;
  other: RunHandle;
  cursorS: number;
  enabled: boolean;
  heatMax: number;
  policy: string[];
  locked: boolean;
}) {
  const frames = windowFrames(run.engine, cursorS);
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
  const own = policyLabel(run.config, policy);
  const tag = (reads: readonly (keyof Frame)[]): PanelData => realness(last, reads, REPLICA_READS);

  const picker = (
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
        own === policyLabel(other.config, policy) ? 'both sides are running the same policy, so there is nothing to compare' : undefined
      }
    />
  );

  return (
    <div className="grid" style={{ gridTemplateColumns: 'minmax(0, 1fr)' }}>
      <Panel
        title={`Run ${label}`}
        sub={enabled ? own : `${own} — shown for reference, not comparable`}
        data={tag(RUN_READS)}
      >
        {locked ? (
          <fieldset className="dropped" disabled title={run.source?.disabledReason ?? undefined}>
            {picker}
          </fieldset>
        ) : (
          picker
        )}
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

      <Panel title="Queue depth per replica" sub="same colour scale on both sides" data={tag(['replicas'])}>
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

      <Panel title="Time to first token" sub="p50 and p99" data={tag(['ttft'])}>
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

      <Panel title="Goodput against throughput" data={tag(['outputTokensPerS', 'ttft', 'itl', 'e2e'])}>
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

function p99(frames: Frame[], which: 'ttft' | 'itl'): number {
  const s = percentileSeries(frames, (f) => (which === 'ttft' ? f.ttft : f.itl), 99);
  return s[s.length - 1] ?? 0;
}

function meanQueue(f: { replicas: { present: boolean; queuedSeqs: number }[] }): number {
  const live = f.replicas.filter((r) => r.present);
  return live.length ? live.reduce((s, r) => s + r.queuedSeqs, 0) / live.length : 0;
}

/**
 * One playback bar driving both runs, since the whole point is that they stay aligned in time.
 * Every control fans out to both handles: live, that is two SetSpeeds and two UpdatePolicies.
 */
function useLinked(a: RunHandle, b: RunHandle, cursorS: number): RunHandle {
  return {
    ...a,
    cursorS,
    setPaused: (p) => { a.setPaused(p); b.setPaused(p); },
    setSpeed: (f) => { a.setSpeed(f); b.setSpeed(f); },
    step: () => { a.step(); b.step(); },
    rewindTo: (s) => { a.rewindTo(s); b.rewindTo(s); },
    scrubTo: (s) => { a.scrubTo(s); b.scrubTo(s); },
    recordedToS: Math.min(a.recordedToS, b.recordedToS),
    paused: a.paused && b.paused,
  };
}
