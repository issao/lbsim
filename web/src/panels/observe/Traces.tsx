import { useEffect, useMemo, useState } from 'react';
import type { ReplayRunHandle, RunHandle } from '../../lib/useRun';
import { useServerTraces } from '../../lib/useServerRun';
import { loadTraces } from '../../lib/replay';
import type { OutcomeName, WireRequestTrace } from '../../lib/api';
import { OUTCOMES, TRACE_BUCKETS, type Outcome, type TraceBucket } from '../../lib/types';
import { shapeTraceRows, sortTraceRows, type TraceRow, type TraceSortKey } from '../../lib/traceRows';
import { Waterfall } from '../../components/charts/Waterfall';
import { fmtMs, fmtCount } from '../../lib/format';

/** The table's own row height, so the wrapper's fixed height (`TABLE_VISIBLE_ROWS` of them, plus
 * the header) is a CSS constant rather than something measured after the fact. */
const ROW_H = 20;
const TABLE_VISIBLE_ROWS = 10;

const BUCKET_LABEL: Record<TraceRow['bucket'], TraceBucket | null> = {
  TRACE_BUCKET_UNSPECIFIED: null,
  TRACE_BUCKET_P50: 'p50',
  TRACE_BUCKET_P90: 'p90',
  TRACE_BUCKET_P99: 'p99',
  TRACE_BUCKET_P999: 'p99.9',
};

function outcomeLabel(o: TraceRow['outcome']): Outcome | null {
  return o === 'OUTCOME_UNSPECIFIED' ? null : (o.slice('OUTCOME_'.length) as Outcome);
}

/** The instant the panel counts seconds from: where the engine's clock started, warm-up included. */
function originOf(run: RunHandle): bigint | null {
  if (run.source?.kind === 'replay') return (run as ReplayRunHandle).loaded.entry.simStartUnixNs;
  const origin = (run as { originUnixNs?: bigint | null }).originUnixNs;
  return origin ?? null;
}

const SORT_LABEL: Record<TraceSortKey, string> = { arrived: 'arrived', ttft: 'ttft', e2e: 'e2e', prompt: 'prompt', output: 'output' };

/**
 * Sampled request journeys, as the engine recorded them. Per Issao: "for the traces view, show
 * the table with sampled requests at the top" — one row per sample, sortable by the latency
 * columns, fixed at `TABLE_VISIBLE_ROWS` rows tall with its own scroll (U99/U107's rule: the page
 * and the panel never jump as rows stream in). The selected row's span timeline and resource-state
 * detail render below the table, never above or beside it. Live, the server is polled; on a
 * recording, `traces.jsonl` is read once. Nothing here is computed in the browser beyond units,
 * sorting and filtering.
 */
export function Traces({ run }: { run: RunHandle }) {
  const [outcome, setOutcome] = useState<Outcome | ''>('');
  const [bucket, setBucket] = useState<TraceBucket | ''>('');
  const [sort, setSort] = useState<TraceSortKey | null>(null);
  const [dir, setDir] = useState<'asc' | 'desc'>('desc');
  const [selected, setSelected] = useState<bigint | null>(null);

  const liveRunId = run.source?.kind === 'server' && run.source.runId ? run.source.runId : null;
  const replayRunId = run.source?.kind === 'replay' && run.source.runId ? run.source.runId : null;
  const live = useServerTraces(liveRunId, { outcome: outcome === '' ? undefined : (`OUTCOME_${outcome}` as OutcomeName) });

  const [replay, setReplay] = useState<{ runId: string; traces: WireRequestTrace[] | null; error: string | null } | null>(null);
  useEffect(() => {
    if (!replayRunId) return;
    let cancelled = false;
    loadTraces(replayRunId).then(
      (traces) => !cancelled && setReplay({ runId: replayRunId, traces, error: null }),
      (e: unknown) => !cancelled && setReplay({ runId: replayRunId, traces: null, error: e instanceof Error ? e.message : String(e) })
    );
    return () => {
      cancelled = true;
    };
  }, [replayRunId]);

  const origin = originOf(run);
  const traces = useMemo(() => {
    if (liveRunId) return live.traces;
    if (replayRunId && replay?.runId === replayRunId && replay.traces) {
      // The file is in the engine's completion order; the table reads newest first like the server.
      return replay.traces
        .slice()
        .sort((a, b) => (a.record.finishedAtUnixNs < b.record.finishedAtUnixNs ? 1 : a.record.finishedAtUnixNs > b.record.finishedAtUnixNs ? -1 : 0));
    }
    return [];
  }, [liveRunId, live.traces, replayRunId, replay]);

  const rows = useMemo(() => {
    const shaped = shapeTraceRows(traces, origin);
    const filtered = shaped.filter(
      (r) => (outcome === '' || outcomeLabel(r.outcome) === outcome) && (bucket === '' || BUCKET_LABEL[r.bucket] === bucket)
    );
    return sort === null ? filtered : sortTraceRows(filtered, sort, dir);
  }, [traces, origin, outcome, bucket, sort, dir]);

  // The selection is an id, not a row reference, so it survives a new page of samples arriving:
  // as long as the id is still in the (bounded) set the source carries, the same row stays picked.
  const current = rows.find((r) => r.id === selected) ?? rows[0] ?? null;
  const currentTrace = current ? traces.find((t) => t.record.id === current.id) ?? null : null;

  const rate = '1 in 20 requests (5%)';
  let note: string | null = null;
  if (!liveRunId && !replayRunId) note = 'no run';
  else if (liveRunId && live.error) note = `traces: ${live.error}`;
  else if (liveRunId && live.answered === 0) note = 'traces: asking the server';
  else if (replayRunId && replay?.runId !== replayRunId) note = 'traces: loading traces.jsonl';
  else if (replayRunId && replay?.error) note = `traces: ${replay.error}`;
  else if (replayRunId && replay?.traces === null) note = 'traces: this recording carries no traces.jsonl';
  else if (traces.length === 0) note = `no sampled requests yet — the engine keeps ${rate}${liveRunId ? '; the first appear with the first completions' : ''}`;
  else if (rows.length === 0) note = 'traces: none match the filter';

  const header = (key: TraceSortKey, label: string) => (
    <th
      key={key}
      aria-sort={sort === key ? (dir === 'desc' ? 'descending' : 'ascending') : undefined}
      title="sortable"
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
    <div className="traces" id="trace-list">
      <div className="row" style={{ display: 'flex', gap: 10, alignItems: 'center', marginBottom: 6, fontSize: 12 }}>
        <label>
          outcome{' '}
          <select value={outcome} onChange={(e) => setOutcome(e.target.value as Outcome | '')}>
            <option value="">any</option>
            {OUTCOMES.map((o) => (
              <option key={o} value={o}>
                {o.toLowerCase()}
              </option>
            ))}
          </select>
        </label>
        <label>
          bucket{' '}
          <select value={bucket} onChange={(e) => setBucket(e.target.value as TraceBucket | '')}>
            <option value="">any</option>
            {TRACE_BUCKETS.map((b) => (
              <option key={b} value={b}>
                {b}
              </option>
            ))}
          </select>
        </label>
        <span className="note" style={{ margin: 0 }}>
          {rows.length} sampled request{rows.length === 1 ? '' : 's'}
          {liveRunId ? ', polled every 2 s' : ''}
          {sort ? `, sorted by ${SORT_LABEL[sort]} ${dir === 'desc' ? 'high to low' : 'low to high'}` : ', newest first'}
        </span>
      </div>
      {note ? (
        <p className="note" style={{ margin: 0 }}>
          {note}
        </p>
      ) : null}
      {rows.length > 0 ? (
        <div className="trace-table-wrap" style={{ maxHeight: (TABLE_VISIBLE_ROWS + 1) * ROW_H + 2, overflowY: 'auto' }}>
          <table className="data">
            <thead>
              <tr>
                <th>id</th>
                <th>bucket</th>
                {header('arrived', 'arrived')}
                {header('ttft', 'ttft')}
                {header('e2e', 'e2e')}
                {header('prompt', 'prompt')}
                {header('output', 'output')}
                <th>outcome</th>
                <th>replica</th>
                <th>spans</th>
              </tr>
            </thead>
            <tbody>
              {rows.map((r) => {
                const b = BUCKET_LABEL[r.bucket];
                const o = outcomeLabel(r.outcome);
                return (
                  <tr
                    key={r.id.toString()}
                    className={current && current.id === r.id ? 'sel' : ''}
                    onClick={() => setSelected(r.id)}
                    style={{ cursor: 'pointer', height: ROW_H }}
                  >
                    <td className="n">{r.id.toString()}</td>
                    <td>{b ?? '—'}</td>
                    <td className="n">{r.arrivedS === null ? '—' : `${r.arrivedS.toFixed(3)} s`}</td>
                    <td className="n">{r.ttftMs === null ? '—' : fmtMs(r.ttftMs)}</td>
                    <td className="n">{r.e2eMs === null ? '—' : fmtMs(r.e2eMs)}</td>
                    <td className="n">{r.promptTokens > 0 ? fmtCount(r.promptTokens) : '—'}</td>
                    <td className="n">{r.outputTokens > 0 ? fmtCount(r.outputTokens) : '—'}</td>
                    <td>{o ? o.toLowerCase() : '—'}</td>
                    <td className="n" data-replica={r.replicaId !== null ? r.replicaId.toString() : undefined}>
                      {r.replicaId !== null ? r.replicaId.toString() : '—'}
                    </td>
                    <td className="n">{r.spanCount}</td>
                  </tr>
                );
              })}
            </tbody>
          </table>
        </div>
      ) : null}
      {currentTrace ? (
        <div className="trace-detail" style={{ marginTop: 8 }}>
          <p className="note" style={{ margin: '0 0 4px' }}>
            prompt {current!.promptTokens > 0 ? `${fmtCount(current!.promptTokens)} tokens` : '—'} · output{' '}
            {current!.outputTokens > 0 ? `${fmtCount(current!.outputTokens)} tokens` : '—'}
          </p>
          <Waterfall key={current!.id.toString()} trace={currentTrace} />
        </div>
      ) : null}
    </div>
  );
}
