import { useEffect, useMemo, useState } from 'react';
import type { Frame, ReplicaSample } from '../../lib/engine';
import type { ScenarioConfig } from '../../lib/config';
import { REPLICA_COLUMNS, type ReplicaColumn } from '../../lib/derive';
import { Panel, Unwired } from '../../components/ui';
import { Heatmap } from '../../components/charts/Heatmap';
import { Sparkline } from '../../components/charts/Sparkline';
import { fmtMs, fmtNum, fmtPct, fmtTokens } from '../../lib/format';
import { useRegistryStats, useSubscriptions } from '../../lib/useSubscriptions';
import { useServerReplicas } from '../../lib/useServerRun';
import { Metric, type Target } from '../../lib/types';
import { isWireFrame, realness, WIRED_REPLICA_FIELDS } from '../../lib/wired';

const PAGE_SIZES = [10, 20, 50];

// Fields this panel reads off Frame / ReplicaSample. Keep these lists honest: they drive the mock
// tag on every Panel below. `weight` is read only via the sort-value switch below, reachable when a
// viewer sorts by that column, but the tag is a static claim about the panel, not the current sort.
// U95b: on a wire frame the unwired replica fields (state, prefix hit rate, TTFT mean, speed
// multiplier, weight) render as `Unwired` rather than as the adapter's placeholders, and sorting
// by one of them falls back to id, so a live page never ranks rows by an invented value.
const FRAME_READS: (keyof Frame)[] = ['loadImbalanceCv', 'replicas'];
const REPLICA_READS: (keyof ReplicaSample)[] = [
  'id',
  'present',
  'state',
  'queuedSeqs',
  'batchSize',
  'kvTokensResident',
  'kvUtilization',
  'stepTimeMs',
  'prefixHitRate',
  'ttftMeanMs',
  'trueSpeedMultiplier',
  'weight',
];

/**
 * The machine-level view, paginated client-side over subscriptions, per docs/ui-spec.md section 2.
 *
 * The wire protocol has no pagination on purpose: a subscription names exactly one entity. So the
 * client asks which replicas exist from the cluster-scoped stream, sorts locally, and opens one
 * subscription per visible row. Payload is bounded by what is on screen rather than by fleet size.
 *
 * The cost is stated rather than hidden: sorting by a live value only sorts what the client is
 * subscribed to, so the header says it is showing a page rather than a global ranking.
 */
export function MachineLevel({
  frames,
  frame,
  config,
  highlight,
  live,
}: {
  frames: Frame[];
  frame: Frame;
  config: ScenarioConfig;
  highlight?: string | null;
  /** Set on a server-backed run: rows then come from one subscription per visible replica. */
  live?: { runId: string };
}) {
  const [sort, setSort] = useState<ReplicaColumn>('queuedSeqs');
  const [dir, setDir] = useState<'asc' | 'desc'>('desc');
  const [page, setPage] = useState(0);
  const [pageSize, setPageSize] = useState(20);
  const [heatMetric, setHeatMetric] = useState<'queuedSeqs' | 'kvUtilization' | 'batchSize'>('queuedSeqs');

  const data = realness(frame, FRAME_READS, REPLICA_READS);
  const wire = isWireFrame(frame);

  // Live frames carry no replica rows: the fleet stream is one entity, and the protos have no "all
  // replicas" call. What the fleet row does say is how many replicas are ready, and the server
  // numbers them densely from 0, so that count is the list of ids; the rows themselves arrive one
  // subscription each, only for the page on screen. Replay frames carry their rows from
  // replicas.jsonl and the mock invents them, so those two keep reading `frame.replicas`.
  const liveRunId = live?.runId ?? null;
  const [subIds, setSubIds] = useState<number[]>([]);
  const streamed = useServerReplicas(liveRunId, subIds, config.samplesPerSimSecond);
  const present = useMemo(() => {
    if (liveRunId === null) return frame.replicas.filter((r) => r.present);
    const n = Number.isFinite(frame.readyReplicas) ? Math.max(0, Math.floor(frame.readyReplicas)) : 0;
    return Array.from({ length: n }, (_, id) => streamed.latest.get(id) ?? pendingRow(id));
  }, [liveRunId, frame, streamed.latest]);
  const sorted = useMemo(() => {
    const rows = [...present];
    rows.sort((a, b) => {
      const av = sortValue(a, sort, wire);
      const bv = sortValue(b, sort, wire);
      // A value the wire has not carried (a live row not yet streamed, a NaN column on replay)
      // sorts after every measured one in either direction, then by id, so the order is stable.
      const an = Number.isNaN(av);
      const bn = Number.isNaN(bv);
      if (an || bn) return an && bn ? a.id - b.id : an ? 1 : -1;
      return dir === 'desc' ? bv - av : av - bv;
    });
    return rows;
  }, [present, sort, dir, wire]);

  const pages = Math.max(1, Math.ceil(sorted.length / pageSize));
  const p = Math.min(page, pages - 1);
  const visible = sorted.slice(p * pageSize, p * pageSize + pageSize);
  const visibleIds = visible.map((r) => r.id);
  const visibleKey = JSON.stringify([...visibleIds].sort((a, b) => a - b));
  // The page decides which rows to stream and the streamed rows decide the page's order, so the
  // request is committed one render behind the sort rather than computed inside it.
  useEffect(() => {
    if (liveRunId !== null) setSubIds(JSON.parse(visibleKey) as number[]);
  }, [liveRunId, visibleKey]);

  // One subscription per visible row. Changing page closes these and opens others.
  const targets: Target[] = visible.map((r) => ({ scope: 'REPLICA', id: r.id }));
  useSubscriptions(
    'machine-level',
    targets,
    [
      Metric.QUEUED_SEQS,
      Metric.KV_TOKENS_RESIDENT,
      Metric.BATCH_SIZE,
      Metric.STEP_TIME,
      Metric.PREFIX_HIT_RATE,
      Metric.TTFT,
    ],
    config.samplesPerSimSecond
  );
  const stats = useRegistryStats();

  const spark = useMemo(() => {
    const m = new Map<number, number[]>();
    if (liveRunId !== null) {
      for (const [id, h] of streamed.history) m.set(id, h.map((r) => r.queuedSeqs));
      return m;
    }
    for (const f of frames) {
      for (const r of f.replicas) {
        if (!r.present) continue;
        let a = m.get(r.id);
        if (!a) m.set(r.id, (a = []));
        a.push(r.queuedSeqs);
      }
    }
    return m;
  }, [frames, liveRunId, streamed.history]);
  const sparkMax = Math.max(1, ...[...spark.values()].flat().filter(Number.isFinite));

  // Live histories are per row and start when the row's subscription opened, so the heatmap's
  // columns are the last N fleet samples, N being the longest history on this page, and shorter
  // histories are right-aligned to them; replica and fleet streams sample at the same rate.
  const heatCols = useMemo(() => {
    if (liveRunId === null) return frames.map((f) => f.simS);
    const n = Math.min(frames.length, Math.max(0, ...visibleIds.map((id) => streamed.history.get(id)?.length ?? 0)));
    return frames.slice(frames.length - n).map((f) => f.simS);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [frames, liveRunId, streamed.history, visibleKey]);
  const heatRows = useMemo(() => {
    if (liveRunId !== null) {
      const n = heatCols.length;
      return visibleIds.map((id) => {
        const h = streamed.history.get(id) ?? [];
        const tail = h.slice(Math.max(0, h.length - n)).map((r) => pick(r, heatMetric));
        return { id, values: [...new Array<number>(n - tail.length).fill(0), ...tail], muted: false };
      });
    }
    const ids = present.map((r) => r.id);
    return ids.map((id) => ({
      id,
      values: frames.map((f) => {
        const r = f.replicas.find((x) => x.id === id);
        return r ? pick(r, heatMetric) : 0;
      }),
      muted: (frame.replicas.find((x) => x.id === id)?.state ?? 'READY') !== 'READY',
    }));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [frames, heatMetric, present.length, liveRunId, streamed.history, heatCols, visibleKey]);
  const heatMax = Math.max(1e-6, ...heatRows.flatMap((r) => r.values).filter(Number.isFinite));

  const header = (key: ReplicaColumn, label: string, metric?: string) => (
    <th
      key={key}
      aria-sort={sort === key ? (dir === 'desc' ? 'descending' : 'ascending') : undefined}
      title={metric ? `sorted locally over the subscribed page; wire metric ${metric}` : 'sorted locally'}
      onClick={() => {
        if (sort === key) setDir(dir === 'desc' ? 'asc' : 'desc');
        else {
          setSort(key);
          setDir('desc');
        }
      }}
    >
      {label}
      {sort === key ? (dir === 'desc' ? ' ↓' : ' ↑') : ''}
    </th>
  );

  return (
    <div className="grid" style={{ gridTemplateColumns: 'minmax(0, 1fr)' }}>
      <Panel
        title="Replicas"
        sub={`page ${p + 1} of ${pages} — a page, not a global ranking`}
        bodyClass="tight"
        highlight={highlight === 'replicas'}
        id="replicas"
        data={data}
        right={
          <span className="note">
            <b className="num">{stats.byOwner['machine-level'] ?? 0}</b> open subscriptions
          </span>
        }
      >
        <table className="data">
          <thead>
            <tr>
              {REPLICA_COLUMNS.map((c) => header(c.key, c.label, c.metric))}
              <th className="nosort">queue, {heatCols.length} pts</th>
            </tr>
          </thead>
          <tbody>
            {visible.map((r) => (
              <tr key={r.id} className={!wire && r.state !== 'READY' ? 'sel' : ''}>
                <td className="n">{r.id}</td>
                <td style={{ textAlign: 'left' }}>
                  {wire ? (
                    <Unwired what="state" />
                  ) : (
                    <>
                      <i
                        className={`dot ${r.state === 'READY' ? (r.trueSpeedMultiplier < 0.9 ? 'critical' : 'good') : r.state === 'EJECTED' ? 'warning' : 'info'}`}
                        style={{ marginRight: 5 }}
                      />
                      {r.state.toLowerCase()}
                      {r.trueSpeedMultiplier < 0.9 ? ` ${r.trueSpeedMultiplier.toFixed(2)}x` : ''}
                    </>
                  )}
                </td>
                <td className={`n bar-cell${r.queuedSeqs > config.fleet.maxQueue * 0.5 ? ' bad' : ''}`}>
                  <i style={{ width: `${Math.min((r.queuedSeqs / config.fleet.maxQueue) * 100, 100)}%` }} />
                  <span>{fmt1(r.queuedSeqs)}</span>
                </td>
                <td className={`n${r.kvUtilization > 0.95 ? ' bad' : r.kvUtilization > 0.85 ? ' warn' : ''}`}>
                  {fmtTokens(r.kvTokensResident)} <span style={{ color: 'var(--ink-3)' }}>{fmtPct(r.kvUtilization, 0)}</span>
                </td>
                <td className="n">{fmt1(r.batchSize)}</td>
                <td className="n">{fmtMs(r.stepTimeMs)}</td>
                <td className="n">{wire ? <Unwired what="prefixHitRate" /> : fmtPct(r.prefixHitRate, 0)}</td>
                <td className={`n${!wire && r.ttftMeanMs > config.slo.ttftMs ? ' bad' : ''}`}>
                  {wire ? <Unwired what="ttftMeanMs" /> : fmtMs(r.ttftMeanMs)}
                </td>
                <td>
                  <Sparkline points={spark.get(r.id) ?? []} max={sparkMax} />
                </td>
              </tr>
            ))}
          </tbody>
        </table>
        <div className="pager">
          {sorted.length > Math.min(...PAGE_SIZES) ? (
            <>
              <button className="btn" disabled={p === 0} onClick={() => setPage(p - 1)}>
                prev
              </button>
              <button className="btn" disabled={p >= pages - 1} onClick={() => setPage(p + 1)}>
                next
              </button>
            </>
          ) : null}
          {sorted.length > 0 ? (
            <span>
              rows{' '}
              <span className="seg">
                {PAGE_SIZES.map((n) => (
                  <button key={n} aria-pressed={pageSize === n} onClick={() => { setPageSize(n); setPage(0); }}>
                    {n}
                  </button>
                ))}
              </span>
            </span>
          ) : null}
          <span className="grow">
            {liveRunId !== null ? (
              <>
                {sorted.length} replicas exist. Sorting is local and instant, so it ranks only the rows this page has
                streamed; a replica never shown yet sorts last, by id. Only the {visible.length} rows on screen carry
                live per-replica subscriptions.
              </>
            ) : (
              <>
                {sorted.length} replicas exist. Sorting is local and instant, so it ranks the coarse cluster summary
                used to choose this page; only the {visible.length} rows on screen carry live per-replica subscriptions.
              </>
            )}
          </span>
        </div>
      </Panel>

      <Panel
        title="Per-replica over time"
        sub={liveRunId !== null ? 'from the rows on this page, since they opened' : 'from the cluster summary, not from the row subscriptions'}
        data={data}
        highlight={highlight === 'heatmap'}
        id="heatmap"
        right={
          <span className="seg">
            {(
              [
                ['queuedSeqs', 'queue'],
                ['kvUtilization', 'kv'],
                ['batchSize', 'batch'],
              ] as const
            ).map(([k, label]) => (
              <button key={k} aria-pressed={heatMetric === k} onClick={() => setHeatMetric(k)}>
                {label}
              </button>
            ))}
          </span>
        }
      >
        <Heatmap
          rows={heatRows}
          colTimes={heatCols}
          rowLabel="replica"
          valueLabel={heatMetric === 'queuedSeqs' ? 'queued requests' : heatMetric === 'kvUtilization' ? 'kv utilization' : 'batch size'}
          format={(v) => (heatMetric === 'kvUtilization' ? fmtPct(v, 0) : fmtNum(v, 1))}
          vmax={heatMax}
        />
        <p className="note" style={{ margin: '5px 0 0' }}>
          Under <b>{config.routing.kind.replace(/_/g, ' ')}</b> the imbalance coefficient of variation is{' '}
          <b className="num">{frame.loadImbalanceCv.toFixed(3)}</b>. Round robin paints a stripe that walks the fleet;
          power-of-two-choices paints flat noise. Switch policies in the Policies tab and watch this panel.
        </p>
      </Panel>
    </div>
  );
}

/** `wire`: the row is from a live or replay frame, so a column the engine does not produce sorts by id instead. */
function sortValue(r: ReplicaSample, key: ReplicaColumn, wire: boolean): number {
  if (wire && !WIRED_REPLICA_FIELDS.has(key)) return r.id;
  switch (key) {
    case 'id':
      return r.id;
    case 'state':
      return r.state === 'READY' ? 0 : r.state === 'DRAINING' ? 1 : r.state === 'WARMING' ? 2 : 3;
    case 'queuedSeqs':
      return r.queuedSeqs;
    case 'kvTokensResident':
      return r.kvTokensResident;
    case 'batchSize':
      return r.batchSize;
    case 'stepTimeMs':
      return r.stepTimeMs;
    case 'prefixHitRate':
      return r.prefixHitRate;
    case 'ttftMeanMs':
      return r.ttftMeanMs;
    case 'weight':
      return r.weight;
  }
}

/** A replica the fleet row counts but whose own row has not streamed yet: every column reads as missing. */
function pendingRow(id: number): ReplicaSample {
  return {
    id,
    present: true,
    state: 'READY',
    weight: 1,
    queuedSeqs: NaN,
    runningSeqs: NaN,
    batchSize: NaN,
    kvTokensResident: NaN,
    kvUtilization: NaN,
    gpuUtilization: NaN,
    gpuComputeBoundFraction: NaN,
    stepTimeMs: NaN,
    queueWaitMs: NaN,
    ttftMeanMs: NaN,
    itlMeanMs: NaN,
    prefixHitRate: NaN,
    admittedRps: NaN,
    completedRps: NaN,
    preemptionsPerS: NaN,
    trueSpeedMultiplier: NaN,
    telemetryStalenessMs: NaN,
  };
}

function fmt1(v: number): string {
  return Number.isFinite(v) ? v.toFixed(1) : '—';
}

function pick(r: ReplicaSample, key: 'queuedSeqs' | 'kvUtilization' | 'batchSize'): number {
  return key === 'queuedSeqs' ? r.queuedSeqs : key === 'kvUtilization' ? r.kvUtilization : r.batchSize;
}
