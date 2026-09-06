import { useState } from 'react';
import { bins, type Histogram as Hist, quantile } from '../../lib/hist';
import { fmtMs } from '../../lib/format';

const PAD = { top: 6, right: 8, bottom: 16, left: 8 };

/**
 * Log-x histogram of a latency distribution, drawn from the same bucketed shape the wire carries.
 * Percentile markers are drawn on top, because a histogram of a long-tailed latency is unreadable
 * without them.
 */
export function HistogramChart({
  hist,
  height = 96,
  marks = [50, 99],
  threshold,
  thresholdLabel,
}: {
  hist: Hist;
  height?: number;
  marks?: number[];
  threshold?: number;
  thresholdLabel?: string;
}) {
  const [hover, setHover] = useState<number | null>(null);
  const bs = bins(hist);
  const W = 640;
  const H = height;
  if (bs.length === 0) return <div className="note">no samples in this window</div>;

  const lo = Math.log2(bs[0].lo);
  const hi = Math.log2(bs[bs.length - 1].hi);
  const maxC = Math.max(...bs.map((b) => b.count));
  const sx = (v: number) => PAD.left + ((Math.log2(v) - lo) / Math.max(hi - lo, 1e-6)) * (W - PAD.left - PAD.right);
  const sy = (c: number) => H - PAD.bottom - (c / maxC) * (H - PAD.top - PAD.bottom);

  return (
    <div className="chart" onMouseLeave={() => setHover(null)}>
      <svg viewBox={`0 0 ${W} ${H}`} height={H} role="img" aria-label="latency distribution">
        {bs.map((b, i) => {
          const x = sx(b.lo);
          const w = Math.max(sx(b.hi) - x - 2, 1);
          return (
            <rect
              key={i}
              x={x}
              y={sy(b.count)}
              width={w}
              height={H - PAD.bottom - sy(b.count)}
              fill={hover === i ? 'var(--accent-ink)' : 'var(--accent)'}
              rx={1}
              onMouseEnter={() => setHover(i)}
            />
          );
        })}
        <line className="zero-line" x1={PAD.left} x2={W - PAD.right} y1={H - PAD.bottom} y2={H - PAD.bottom} />
        {marks.map((p) => {
          const v = quantile(hist, p);
          if (v < bs[0].lo || v > bs[bs.length - 1].hi) return null;
          return (
            <g key={p}>
              <line x1={sx(v)} x2={sx(v)} y1={PAD.top} y2={H - PAD.bottom} stroke="var(--ink-1)" strokeWidth={1} />
              <text
                className="axis-label"
                x={sx(v) + 3}
                y={PAD.top + 8}
                fill="var(--ink-1)"
                stroke="var(--surface-1)"
                strokeWidth={3}
                paintOrder="stroke"
              >
                p{p} {fmtMs(v)}
              </text>
            </g>
          );
        })}
        {threshold !== undefined && threshold >= bs[0].lo && threshold <= bs[bs.length - 1].hi ? (
          <g>
            <line
              x1={sx(threshold)}
              x2={sx(threshold)}
              y1={PAD.top}
              y2={H - PAD.bottom}
              stroke="var(--critical)"
              strokeWidth={1}
              strokeDasharray="3 3"
            />
            <text
              className="axis-label"
              x={sx(threshold) + 3}
              y={H - PAD.bottom - 4}
              fill="var(--critical)"
              stroke="var(--surface-1)"
              strokeWidth={3}
              paintOrder="stroke"
            >
              {thresholdLabel ?? 'SLO'}
            </text>
          </g>
        ) : null}
        <text className="axis-label" x={PAD.left} y={H - 4}>
          {fmtMs(bs[0].lo)}
        </text>
        <text className="axis-label" x={W - PAD.right} y={H - 4} textAnchor="end">
          {fmtMs(bs[bs.length - 1].hi)}
        </text>
        <text className="axis-label" x={W / 2} y={H - 4} textAnchor="middle" fill="var(--ink-3)">
          log scale
        </text>
      </svg>
      {hover !== null ? (
        <div className="tooltip" style={{ left: `min(${(sx(bs[hover].lo) / W) * 100}%, calc(100% - 130px))`, top: 0 }}>
          <div className="tt-time">
            {fmtMs(bs[hover].lo)} &ndash; {fmtMs(bs[hover].hi)}
          </div>
          <div className="tt-row">
            <span style={{ color: 'var(--ink-2)' }}>weight</span>
            <b>{bs[hover].count.toFixed(1)}</b>
          </div>
        </div>
      ) : null}
    </div>
  );
}
