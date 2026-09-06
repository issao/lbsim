import type { RequestTrace } from '../../lib/types';
import { fmtMs, fmtPct, fmtTokens } from '../../lib/format';

function spanClass(op: string): string {
  if (op.startsWith('preempted')) return 'preempted';
  if (op === 'queue') return 'queue';
  if (op === 'prefill') return 'prefill';
  if (op === 'decode') return 'decode';
  return 'rpc';
}

/**
 * One row per span, with the conditions alongside each: batch size, KV utilization, tokens
 * processed. A slow span with no surrounding conditions is a mystery rather than an explanation,
 * so the conditions are columns rather than a hover.
 */
export function Waterfall({ trace }: { trace: RequestTrace }) {
  const total = trace.spans[trace.spans.length - 1]?.endMs ?? 1;
  return (
    <div className="waterfall">
      <div className="wf-legend">
        <span><i style={{ background: 'var(--ink-3)' }} />queue</span>
        <span><i style={{ background: 'var(--series-1)' }} />prefill</span>
        <span><i style={{ background: 'var(--series-3)' }} />decode</span>
        <span><i style={{ background: 'var(--critical)' }} />preempted</span>
        <span><i style={{ background: 'var(--line-strong)' }} />gateway / router</span>
      </div>
      <div className="wf-row head">
        <span>span</span>
        <span>0 &ndash; {fmtMs(total)}</span>
        <span className="wf-num">dur</span>
        <span className="wf-num">batch</span>
        <span className="wf-num">kv</span>
        <span className="wf-num">tokens</span>
      </div>
      {trace.spans.map((s, i) => {
        const dur = s.endMs - s.startMs;
        return (
          <div className="wf-row" key={i} title={`${s.component} ${s.operation}`}>
            <span className="wf-op">
              {s.operation}
              {s.kvTier !== 'HBM' ? <span style={{ color: 'var(--ink-3)' }}> &rarr;{s.kvTier.toLowerCase()}</span> : null}
            </span>
            <span className="wf-track">
              <i
                className={`wf-span ${spanClass(s.operation)}`}
                style={{
                  left: `${(s.startMs / total) * 100}%`,
                  width: `${Math.max((dur / total) * 100, 0.4)}%`,
                }}
              />
            </span>
            <span className="wf-num">{fmtMs(dur)}</span>
            <span className="wf-num">{s.concurrentSeqs || '-'}</span>
            <span className="wf-num">{s.kvUtilization > 0 ? fmtPct(s.kvUtilization, 0) : '-'}</span>
            <span className="wf-num">{s.tokensProcessed > 0 ? fmtTokens(s.tokensProcessed) : '-'}</span>
          </div>
        );
      })}
    </div>
  );
}
