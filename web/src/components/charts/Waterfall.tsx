import { useState, type ReactNode } from 'react';
import type { WireRequestTrace, WireTraceSpan } from '../../lib/api';
import { fmtMs, fmtTokens } from '../../lib/format';

/** Milliseconds between two instants, as a plain number: only ever a span's width, never an epoch. */
function msBetween(a: bigint, b: bigint): number {
  return Number(b - a) / 1e6;
}

/** A span the replica recorded, as opposed to the gateway's and router's, which hold no cache and no batch. */
function onReplica(s: WireTraceSpan): boolean {
  return s.component.startsWith('replica:');
}

const ROW = 18;
const LABEL_W = 150;
const PLOT_W = 420;

/**
 * One request's journey, span by span: a row per span, x is time since the request arrived, the
 * label is the span's operation and where it ran. Every number is the engine's; a field the wire
 * did not carry for the span reads "—" with the reason on hover, never a stand-in.
 */
export function Waterfall({ trace }: { trace: WireRequestTrace }) {
  const [pick, setPick] = useState(0);
  const spans = trace.spans;
  if (spans.length === 0) {
    return <p className="note">this request has no spans</p>;
  }
  const t0 = spans.reduce((m, s) => (s.startUnixNs < m ? s.startUnixNs : m), trace.record.arrivedAtUnixNs);
  const t1 = spans.reduce((m, s) => (s.endUnixNs > m ? s.endUnixNs : m), t0);
  const totalMs = Math.max(msBetween(t0, t1), 1e-6);
  const x = (t: bigint) => LABEL_W + (msBetween(t0, t) / totalMs) * PLOT_W;
  const height = spans.length * ROW + 22;
  const sel = spans[Math.min(pick, spans.length - 1)];
  const ticks = [0, 0.25, 0.5, 0.75, 1];

  return (
    <div className="waterfall" style={{ overflowX: 'auto' }}>
      <svg width={LABEL_W + PLOT_W + 40} height={height} role="img" aria-label="request waterfall" style={{ display: 'block', font: '11px var(--mono, monospace)' }}>
        {ticks.map((f) => (
          <g key={f}>
            <line x1={LABEL_W + f * PLOT_W} x2={LABEL_W + f * PLOT_W} y1={0} y2={spans.length * ROW} stroke="var(--line, #ddd)" strokeDasharray="2 3" />
            <text x={LABEL_W + f * PLOT_W} y={spans.length * ROW + 14} textAnchor="middle" fill="var(--ink-3, #888)">
              {fmtMs(f * totalMs)}
            </text>
          </g>
        ))}
        {spans.map((s, i) => {
          const x0 = x(s.startUnixNs);
          const w = Math.max(x(s.endUnixNs) - x0, 1.5);
          const y = i * ROW;
          const replica = onReplica(s) ? String(s.replicaId) : null;
          return (
            <g key={i} onClick={() => setPick(i)} style={{ cursor: 'pointer' }}>
              <rect x={0} y={y} width={LABEL_W + PLOT_W + 40} height={ROW} fill={i === pick ? 'var(--accent-bg, rgba(0,0,0,0.06))' : 'transparent'} />
              <text x={4} y={y + 13} fill="var(--ink-1, #222)" data-replica={replica ?? undefined}>
                {s.operation} · {s.component}
              </text>
              <rect x={x0} y={y + 3} width={w} height={ROW - 6} fill={onReplica(s) ? 'var(--series-1, #4a7)' : 'var(--series-2, #79a)'} rx={1.5}>
                <title>
                  {s.operation} on {s.component}: {fmtMs(msBetween(s.startUnixNs, s.endUnixNs))}, from +{fmtMs(msBetween(t0, s.startUnixNs))}
                </title>
              </rect>
            </g>
          );
        })}
      </svg>
      <SpanDetail span={sel} t0={t0} />
    </div>
  );
}

/** "—", and why, for a field the engine did not carry for this span. */
function Dash() {
  return <span title="not simulated yet">—</span>;
}

function SpanDetail({ span: s, t0 }: { span: WireTraceSpan; t0: bigint }) {
  const rep = onReplica(s);
  const kvPct = s.kvCapacity > 0n ? `${((Number(s.kvTokensResident) / Number(s.kvCapacity)) * 100).toFixed(0)}%` : null;
  const cells: [string, ReactNode][] = [
    ['span', `${s.operation} · ${s.component}`],
    ['start', `+${fmtMs(msBetween(t0, s.startUnixNs))}`],
    ['duration', fmtMs(msBetween(s.startUnixNs, s.endUnixNs))],
    ['in flight', String(s.concurrentSeqs)],
    ['tokens', s.tokensProcessed > 0 ? fmtTokens(s.tokensProcessed) : <Dash />],
    ['batch', rep ? String(s.batchSize) : <Dash />],
    ['queued behind', rep ? String(s.queued) : <Dash />],
    ['kv resident', rep && s.kvCapacity > 0n ? `${fmtTokens(Number(s.kvTokensResident))} / ${fmtTokens(Number(s.kvCapacity))}${kvPct ? ` (${kvPct})` : ''}` : <Dash />],
    ['kv tier', s.kvTier === 'MEMORY_TIER_UNSPECIFIED' ? <Dash /> : s.kvTier.replace('MEMORY_TIER_', '').toLowerCase()],
    ['step', rep && s.stepNs > 0n ? fmtMs(Number(s.stepNs) / 1e6) : <Dash />],
    ['bound', rep && s.bound !== 'STEP_BOUND_UNSPECIFIED' ? s.bound.replace('STEP_BOUND_', '').toLowerCase() : <Dash />],
    ['candidates', s.candidates.length > 0 ? s.candidates.map(String).join(', ') : <Dash />],
    ['view age', s.candidates.length > 0 ? fmtMs(Number(s.staleViewAgeNs) / 1e6) : <Dash />],
  ];
  return (
    <dl className="span-detail" style={{ display: 'grid', gridTemplateColumns: 'repeat(auto-fill, minmax(150px, 1fr))', gap: '2px 12px', margin: '6px 0 0', fontSize: 11 }}>
      {cells.map(([k, v]) => (
        <div key={k} style={{ display: 'flex', justifyContent: 'space-between', gap: 6 }}>
          <dt style={{ color: 'var(--ink-3, #888)' }}>{k}</dt>
          <dd className="num" style={{ margin: 0 }}>
            {v}
          </dd>
        </div>
      ))}
    </dl>
  );
}
