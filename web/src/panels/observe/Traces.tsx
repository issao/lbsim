import { useEffect, useMemo, useState } from 'react';
import type { ReplayRunHandle, RunHandle } from '../../lib/useRun';
import { useServerTraces } from '../../lib/useServerRun';
import { loadTraces } from '../../lib/replay';
import { relSeconds, type OutcomeName, type TraceBucketName, type WireRequestTrace } from '../../lib/api';
import { OUTCOMES, TRACE_BUCKETS, type Outcome, type TraceBucket } from '../../lib/types';
import { Waterfall } from '../../components/charts/Waterfall';
import { fmtMs } from '../../lib/format';

const BUCKET_LABEL: Record<TraceBucketName, TraceBucket | null> = {
  TRACE_BUCKET_UNSPECIFIED: null,
  TRACE_BUCKET_P50: 'p50',
  TRACE_BUCKET_P90: 'p90',
  TRACE_BUCKET_P99: 'p99',
  TRACE_BUCKET_P999: 'p99.9',
};

function outcomeLabel(o: OutcomeName): Outcome | null {
  return o === 'OUTCOME_UNSPECIFIED' ? null : (o.slice('OUTCOME_'.length) as Outcome);
}

/** The instant the panel counts seconds from: where the engine's clock started, warm-up included. */
function originOf(run: RunHandle): bigint | null {
  if (run.source?.kind === 'replay') return (run as ReplayRunHandle).loaded.entry.simStartUnixNs;
  const origin = (run as { originUnixNs?: bigint | null }).originUnixNs;
  return origin ?? null;
}

/**
 * Sampled request journeys, as the engine recorded them: a table of the newest, filtered by outcome
 * and latency bucket, and the selected one span by span. Live, the server is polled; on a
 * recording, `traces.jsonl` is read once. Nothing here is computed in the browser beyond units.
 */
export function Traces({ run }: { run: RunHandle }) {
  const [outcome, setOutcome] = useState<Outcome | ''>('');
  const [bucket, setBucket] = useState<TraceBucket | ''>('');
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
  const rows = useMemo(() => {
    let all: WireRequestTrace[];
    if (liveRunId) all = live.traces;
    else if (replayRunId && replay?.runId === replayRunId && replay.traces) {
      // The file is in the engine's completion order; the table reads newest first like the server.
      all = replay.traces.slice().sort((a, b) => (a.record.finishedAtUnixNs < b.record.finishedAtUnixNs ? 1 : a.record.finishedAtUnixNs > b.record.finishedAtUnixNs ? -1 : 0));
    } else all = [];
    return all.filter((t) => (outcome === '' || outcomeLabel(t.record.outcome) === outcome) && (bucket === '' || BUCKET_LABEL[t.bucket] === bucket));
  }, [liveRunId, live.traces, replayRunId, replay, outcome, bucket]);

  const current = rows.find((t) => t.record.id === selected) ?? rows[0] ?? null;

  let note: string | null = null;
  if (!liveRunId && !replayRunId) note = 'no run';
  else if (liveRunId && live.error) note = `traces: ${live.error}`;
  else if (liveRunId && live.answered === 0) note = 'traces: asking the server';
  else if (replayRunId && replay?.runId !== replayRunId) note = 'traces: loading traces.jsonl';
  else if (replayRunId && replay?.error) note = `traces: ${replay.error}`;
  else if (replayRunId && replay?.traces === null) note = 'traces: this recording carries no traces.jsonl';
  else if (rows.length === 0) note = liveRunId ? 'traces: none sampled yet (1 in 20 requests is kept; the first appear with the first completions)' : 'traces: none match';

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
          {liveRunId ? ', newest first, polled every 2 s' : ''}
        </span>
      </div>
      {note ? (
        <p className="note" style={{ margin: 0 }}>
          {note}
        </p>
      ) : null}
      {current ? <Waterfall key={current.record.id.toString()} trace={current} /> : null}
      {rows.length > 0 ? (
        <table className="data" style={{ marginTop: 8 }}>
          <thead>
            <tr>
              <th>id</th>
              <th>outcome</th>
              <th>bucket</th>
              <th>tenant</th>
              <th>arrived</th>
              <th>queue wait</th>
              <th>ttft</th>
              <th>e2e</th>
              <th>replica</th>
              <th>spans</th>
            </tr>
          </thead>
          <tbody>
            {rows.map((t) => {
              const r = t.record;
              const b = BUCKET_LABEL[t.bucket];
              const o = outcomeLabel(r.outcome);
              const finished = r.outcome === 'OUTCOME_OK' || r.outcome === 'OUTCOME_OK_SLO_VIOLATED';
              return (
                <tr key={r.id.toString()} className={current && current.record.id === r.id ? 'sel' : ''} onClick={() => setSelected(r.id)} style={{ cursor: 'pointer' }}>
                  <td className="n">{r.id.toString()}</td>
                  <td>{o ? o.toLowerCase() : '—'}</td>
                  <td>{b ?? '—'}</td>
                  <td className="n">{r.tenantId.toString()}</td>
                  <td className="n">{origin === null ? '—' : `${relSeconds(r.arrivedAtUnixNs, origin).toFixed(3)} s`}</td>
                  <td className="n">{fmtMs(Number(r.queueWaitNs) / 1e6)}</td>
                  <td className="n">{r.ttftNs > 0n ? fmtMs(Number(r.ttftNs) / 1e6) : '—'}</td>
                  <td className="n">{r.e2eNs > 0n ? fmtMs(Number(r.e2eNs) / 1e6) : '—'}</td>
                  <td className="n" data-replica={finished ? r.replicaId.toString() : undefined}>
                    {finished ? r.replicaId.toString() : '—'}
                  </td>
                  <td className="n">{t.spans.length}</td>
                </tr>
              );
            })}
          </tbody>
        </table>
      ) : null}
    </div>
  );
}
