import { useRef, useState, type ReactNode } from 'react';
import type { WireRequestTrace, WireTraceSpan } from '../../lib/api';
import { fmtMs, fmtTokens, fmtAbsNs } from '../../lib/format';
import { axisOf, layoutSpans, msBetween, type Axis } from '../../lib/spanLayout';

/** A span the replica recorded, as opposed to the gateway's and router's, which hold no cache and no batch. */
function onReplica(s: WireTraceSpan): boolean {
  return s.component.startsWith('replica:');
}

const ROW = 18;
const LABEL_W = 150;
const PLOT_W = 420;

/**
 * One request's journey, span by span: a row per span, x is time since the request arrived, width
 * is the span's own duration — a queue span that lasted 3s renders visibly 3s wide, not a point.
 * Issao: "The traces seems to show bars with a point in time. They should show width proportional
 * of each trace duration... Also a mouse over to a bar in the trace should show machine id, start
 * and stop time." The geometry (and the proof that width tracks duration) lives in `spanLayout.ts`,
 * with its own self-test; this file is presentation only. Every field the wire did not carry for
 * the span reads "—" with the reason on hover, never a stand-in.
 */
export function Waterfall({ trace }: { trace: WireRequestTrace }) {
  const [pick, setPick] = useState(0);
  const [hover, setHover] = useState<{ i: number; x: number; y: number } | null>(null);
  const wrapRef = useRef<HTMLDivElement | null>(null);
  const spans = trace.spans;
  if (spans.length === 0) {
    return <p className="note">this request has no spans</p>;
  }
  const arrivedAt = trace.record.arrivedAtUnixNs;
  const finishedAt = trace.record.finishedAtUnixNs;
  // Arrival to finish, per spec — not the spans' own min/max, which silently drops any untraced
  // gap (before the first span, or between the last one and the recorded finish) from the axis.
  const axis: Axis = axisOf(arrivedAt, finishedAt > 0n ? finishedAt : null, spans);
  const layout = layoutSpans(spans, axis, PLOT_W);
  const x = (ms: number) => LABEL_W + (ms / axis.totalMs) * PLOT_W;
  const height = spans.length * ROW + 22;
  const sel = spans[Math.min(pick, spans.length - 1)];
  const ticks = [0, 0.25, 0.5, 0.75, 1];

  function moveTo(i: number, e: { clientX: number; clientY: number }) {
    const el = wrapRef.current;
    if (!el) return;
    const r = el.getBoundingClientRect();
    setHover({ i, x: e.clientX - r.left, y: e.clientY - r.top });
  }

  return (
    <div className="waterfall" ref={wrapRef} style={{ position: 'relative', overflowX: 'auto' }} onMouseLeave={() => setHover(null)}>
      <svg width={LABEL_W + PLOT_W + 40} height={height} role="img" aria-label="request waterfall" style={{ display: 'block', font: '11px var(--mono, monospace)' }}>
        {ticks.map((f) => (
          <g key={f}>
            <line x1={LABEL_W + f * PLOT_W} x2={LABEL_W + f * PLOT_W} y1={0} y2={spans.length * ROW} stroke="var(--line, #ddd)" strokeDasharray="2 3" />
            <text x={LABEL_W + f * PLOT_W} y={spans.length * ROW + 14} textAnchor="middle" fill="var(--ink-3, #888)">
              {fmtMs(f * axis.totalMs)}
            </text>
          </g>
        ))}
        {spans.map((s, i) => {
          const g = layout[i];
          const y = i * ROW;
          const replica = onReplica(s) ? String(s.replicaId) : null;
          const startMs = msBetween(arrivedAt, s.startUnixNs);
          const endMs = msBetween(arrivedAt, s.endUnixNs);
          return (
            <g
              key={i}
              onClick={() => setPick(i)}
              onMouseEnter={(e) => moveTo(i, e)}
              onMouseMove={(e) => moveTo(i, e)}
              style={{ cursor: 'pointer' }}
            >
              <rect x={0} y={y} width={LABEL_W + PLOT_W + 40} height={ROW} fill={i === pick ? 'var(--accent-bg, rgba(0,0,0,0.06))' : 'transparent'} />
              <text x={4} y={y + 13} fill="var(--ink-1, #222)" data-replica={replica ?? undefined}>
                {s.operation} · {s.component}
              </text>
              <rect
                className="wf-bar"
                x={x(g.startMs)}
                y={y + 3}
                width={g.width}
                height={ROW - 6}
                rx={1.5}
                fill={onReplica(s) ? 'var(--series-1, #4a7)' : 'var(--series-2, #79a)'}
                data-op={s.operation}
                data-component={s.component}
                data-replica={replica ?? undefined}
                data-duration-ms={g.durationMs.toFixed(6)}
                data-width-px={g.width.toFixed(3)}
              >
                <title>
                  {s.operation} on {s.component}: {fmtMs(g.durationMs)}, from +{fmtMs(startMs)} to +{fmtMs(endMs)} ({fmtAbsNs(s.startUnixNs)}–
                  {fmtAbsNs(s.endUnixNs)})
                </title>
              </rect>
            </g>
          );
        })}
      </svg>
      {hover ? <SpanTooltip s={spans[hover.i]} arrivedAt={arrivedAt} x={hover.x} y={hover.y} /> : null}
      <SpanDetail span={sel} arrivedAt={arrivedAt} />
    </div>
  );
}

/** "—", and why, for a field the engine did not carry for this span. */
function Dash() {
  return <span title="not simulated yet">—</span>;
}

/** The small tooltip that follows the pointer while it is over a bar, per Issao: "a mouse over to a
 * bar in the trace should show machine id, start and stop time." Positioned from the container's
 * own corner (not the page), so it tracks the cursor as it moves without re-measuring on scroll. */
function SpanTooltip({ s, arrivedAt, x, y }: { s: WireTraceSpan; arrivedAt: bigint; x: number; y: number }) {
  const rep = onReplica(s);
  const startMs = msBetween(arrivedAt, s.startUnixNs);
  const endMs = msBetween(arrivedAt, s.endUnixNs);
  const kvPct = s.kvCapacity > 0n ? ` (${((Number(s.kvTokensResident) / Number(s.kvCapacity)) * 100).toFixed(0)}%)` : '';
  const rows: [string, ReactNode][] = [
    ['machine', s.component],
    ['start', `+${fmtMs(startMs)} · ${fmtAbsNs(s.startUnixNs)}`],
    ['stop', `+${fmtMs(endMs)} · ${fmtAbsNs(s.endUnixNs)}`],
    ['duration', fmtMs(endMs - startMs)],
    ['batch', rep ? String(s.batchSize) : <Dash />],
    ['queued', rep ? String(s.queued) : <Dash />],
    ['running', String(s.concurrentSeqs)],
    ['kv', rep && s.kvCapacity > 0n ? `${fmtTokens(Number(s.kvTokensResident))}/${fmtTokens(Number(s.kvCapacity))}${kvPct}` : <Dash />],
    ['step', rep && s.stepNs > 0n ? fmtMs(Number(s.stepNs) / 1e6) : <Dash />],
    ['bound', rep && s.bound !== 'STEP_BOUND_UNSPECIFIED' ? s.bound.replace('STEP_BOUND_', '').toLowerCase() : <Dash />],
  ];
  return (
    <div className="tooltip" style={{ left: x + 12, top: y + 12, minWidth: 150 }}>
      <div className="tt-time">{s.operation}</div>
      {rows.map(([k, v]) => (
        <div className="tt-row" key={k}>
          <span style={{ color: 'var(--ink-2)' }}>{k}</span>
          <b>{v}</b>
        </div>
      ))}
    </div>
  );
}

function SpanDetail({ span: s, arrivedAt }: { span: WireTraceSpan; arrivedAt: bigint }) {
  const rep = onReplica(s);
  const kvPct = s.kvCapacity > 0n ? `${((Number(s.kvTokensResident) / Number(s.kvCapacity)) * 100).toFixed(0)}%` : null;
  const cells: [string, ReactNode][] = [
    ['span', `${s.operation} · ${s.component}`],
    ['start', `+${fmtMs(msBetween(arrivedAt, s.startUnixNs))}`],
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
