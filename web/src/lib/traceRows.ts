// Pure shaping and sorting for the Traces tab's table of sampled requests, kept out of the React
// component so it has a self-test that runs under plain node (see traceRows.selftest.ts) rather
// than needing a browser. Every field here is a plain number/bigint/string the engine actually
// reported; nothing is invented, and a value the wire never carried for a row is `null`.

import { relSeconds, type OutcomeName, type TraceBucketName, type WireRequestTrace } from './api';

export interface TraceRow {
  id: bigint;
  bucket: TraceBucketName;
  outcome: OutcomeName;
  /** Seconds since the run's origin, or `null` when the origin is not known yet. */
  arrivedS: number | null;
  /** Milliseconds, or `null` before the first token streamed. */
  ttftMs: number | null;
  /** Milliseconds, or `null` before the request finished. */
  e2eMs: number | null;
  outputTokens: number;
  /** `null` until the request finished on a replica: a failed or in-flight request named no home. */
  replicaId: bigint | null;
  spanCount: number;
}

function finished(outcome: OutcomeName): boolean {
  return outcome === 'OUTCOME_OK' || outcome === 'OUTCOME_OK_SLO_VIOLATED';
}

/** One sampled trace, as the table shows it. `origin` is the run's clock start, or `null` before it is known. */
export function shapeTraceRow(t: WireRequestTrace, origin: bigint | null): TraceRow {
  const r = t.record;
  return {
    id: r.id,
    bucket: t.bucket,
    outcome: r.outcome,
    arrivedS: origin === null ? null : relSeconds(r.arrivedAtUnixNs, origin),
    ttftMs: r.ttftNs > 0n ? Number(r.ttftNs) / 1e6 : null,
    e2eMs: r.e2eNs > 0n ? Number(r.e2eNs) / 1e6 : null,
    outputTokens: r.outputTokens,
    replicaId: finished(r.outcome) ? r.replicaId : null,
    spanCount: t.spans.length,
  };
}

export function shapeTraceRows(traces: WireRequestTrace[], origin: bigint | null): TraceRow[] {
  return traces.map((t) => shapeTraceRow(t, origin));
}

/** The table's three sortable latency columns; the id, bucket, outcome, replica and span-count
 * columns sort only implicitly, by leaving the natural (newest-first) order alone. */
export type TraceSortKey = 'arrived' | 'ttft' | 'e2e';
export type SortDir = 'asc' | 'desc';

function sortValue(row: TraceRow, key: TraceSortKey): number | null {
  if (key === 'arrived') return row.arrivedS;
  if (key === 'ttft') return row.ttftMs;
  return row.e2eMs;
}

/**
 * `rows` sorted by one latency column, without mutating the input. A row with no value for that
 * column (not timestamped yet, or the origin not known) sorts after every row that has one,
 * in either direction, and ties break by id ascending so the order is stable across re-renders.
 */
export function sortTraceRows(rows: TraceRow[], key: TraceSortKey, dir: SortDir): TraceRow[] {
  const out = rows.slice();
  out.sort((a, b) => {
    const av = sortValue(a, key);
    const bv = sortValue(b, key);
    if (av === null || bv === null) {
      if (av === null && bv === null) return a.id < b.id ? -1 : a.id > b.id ? 1 : 0;
      return av === null ? 1 : -1;
    }
    return dir === 'desc' ? bv - av : av - bv;
  });
  return out;
}
