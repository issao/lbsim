import { useEffect, useMemo, useState } from 'react';
import type { Frame, ReplicaSample } from '../../lib/frame';
import type { ScenarioConfig } from '../../lib/config';
import { REPLICA_COLUMNS, type ReplicaColumn } from '../../lib/derive';
import { Panel, Unwired } from '../../components/ui';
import { Heatmap } from '../../components/charts/Heatmap';
import { Sparkline } from '../../components/charts/Sparkline';
import { fmtMs, fmtNum, fmtPct, fmtTokens } from '../../lib/format';
import { useServerReplicas } from '../../lib/useServerRun';
import { WIRED_REPLICA_FIELDS } from '../../lib/wired';

const PAGE_SIZES = [10, 20, 50];

/**
 * The machine-level view, paginated client-side over subscriptions, per docs/ui-spec.md section 2.
 *
 * The wire protocol has no pagination on purpose: a subscription names exactly one entity. So the
 * client asks which replicas exist from the cluster-scoped stream, sorts locally, and opens one
 * subscription per visible row. Payload is bounded by what is on screen rather than by fleet size.
 *
 * The cost is stated rather than hidden: sorting by a live value only sorts what the client is
 * subscribed to, so the header says it is showing a page rather than a global ranking.
 *
 * U95b: a replica field the engine does not produce (prefix hit rate) renders as `Unwired`, and a
 * value a row has not carried yet (a live row not yet streamed, an older recording without the
 * state metrics) as "—"; sorting by an unwired column falls back to id, so the page never ranks
 * rows by a value nobody measured.
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

  // Live frames carry no replica rows: the fleet stream is one entity, and the protos have no "all
  // replicas" call. What the fleet row does say is how many replicas are ready, and the server
  // numbers them densely from 0, so that count is the list of ids; the rows themselves arrive one
  // subscription each, only for the page on screen. Replay frames carry their rows from
  // replicas.jsonl, so replay keeps reading `frame.replicas`.
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
      const av = sortValue(a, sort);
      const bv = sortValue(b, sort);
      // A value the wire has not carried (a live row not yet streamed, a NaN column on replay)
      // sorts after every measured one in either direction, then by id, so the order is stable.
      const an = Number.isNaN(av);
      const bn = Number.isNaN(bv);
      if (an || bn) return an && bn ? a.id - b.id : an ? 1 : -1;
      return dir === 'desc' ? bv - av : av - bv;
    });
    return rows;
  }, [present, sort, dir]);

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
      muted: ['DEGRADED', 'EJECTED', 'DRAINING'].includes(frame.replicas.find((x) => x.id === id)?.state ?? 'UNKNOWN'),
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
              <tr key={r.id} className={r.state === 'DEGRADED' || r.state === 'EJECTED' ? 'sel' : ''}>
                <td className="n">{r.id}</td>
                <td style={{ textAlign: 'left' }}>
                  {r.state === 'UNKNOWN' ? (
                    '—'
                  ) : (
                    <>
                      <i className={`dot ${stateDot(r)}`} style={{ marginRight: 5 }} />
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
                <td className="n">
                  <Unwired what="prefixHitRate" />
                </td>
                <td className={`n${r.ttftMeanMs > config.slo.ttftMs ? ' bad' : ''}`}>
                  {Number.isFinite(r.ttftMeanMs) ? fmtMs(r.ttftMeanMs) : '—'}
                </td>
                <td>
                  <Sparkline points={spark.get(r.id) ?? []} max={sparkMax} />
                </td>
              </tr>
            ))}
            {/* U99: the page is always `pageSize` rows tall. Rows the fleet has not named yet (before the
                first sample) or that a short last page lacks are drawn empty, so the panel's box never
                changes height as ids arrive. */}
            {Array.from({ length: Math.max(0, pageSize - visible.length) }, (_, i) => (
              <tr key={`filler-${i}`} aria-hidden="true" className="filler">
                {REPLICA_COLUMNS.map((c) => (
                  <td key={c.key} className="n">
                    —
                  </td>
                ))}
                <td>
                  <Sparkline points={[]} />
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

/** A column the engine does not produce sorts by id instead, so the order is never an invented ranking. */
function sortValue(r: ReplicaSample, key: ReplicaColumn): number {
  if (!WIRED_REPLICA_FIELDS.has(key)) return r.id;
  switch (key) {
    case 'id':
      return r.id;
    case 'state':
      return r.state === 'READY' ? 0 : r.state === 'DEGRADED' ? 1 : r.state === 'EJECTED' ? 2 : r.state === 'UNKNOWN' ? NaN : 3;
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

/** The dot beside a state: healthy is good, a degraded replica (true speed below 1) is the gray failure this view exists to show. */
function stateDot(r: ReplicaSample): string {
  if (r.state === 'READY') return r.trueSpeedMultiplier < 0.9 ? 'critical' : 'good';
  if (r.state === 'DEGRADED') return 'critical';
  if (r.state === 'EJECTED') return 'warning';
  return 'info';
}

/** A replica the fleet row counts but whose own row has not streamed yet: every column reads as missing. */
function pendingRow(id: number): ReplicaSample {
  return {
    id,
    present: true,
    state: 'UNKNOWN',
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
