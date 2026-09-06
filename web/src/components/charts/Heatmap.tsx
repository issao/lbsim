import { useState } from 'react';

const RAMP = ['--seq-0', '--seq-1', '--seq-2', '--seq-3', '--seq-4', '--seq-5', '--seq-6', '--seq-7'];

/** Sequential magnitude: one hue, light to dark. Never a rainbow. */
export function rampColor(v: number): string {
  const i = Math.max(0, Math.min(RAMP.length - 1, Math.floor(v * RAMP.length)));
  return `var(${RAMP[i]})`;
}

/**
 * Replica by time. This is the panel that makes an imbalance policy difference visible at a glance:
 * round robin paints a diagonal stripe walking the fleet, power-of-two-choices paints flat noise.
 */
export function Heatmap({
  rows,
  colTimes,
  rowLabel,
  valueLabel,
  format,
  vmax,
  cellH = 7,
}: {
  rows: { id: number; values: number[]; muted?: boolean }[];
  colTimes: number[];
  rowLabel: string;
  valueLabel: string;
  format: (v: number) => string;
  vmax: number;
  cellH?: number;
}) {
  const [hover, setHover] = useState<{ r: number; c: number; x: number; y: number } | null>(null);
  const cols = colTimes.length;
  const cellW = Math.max(2, Math.min(9, Math.floor(760 / Math.max(cols, 1))));
  const W = cellW * cols;
  const LABEL_W = 26;

  return (
    <div className="chart">
      <div className="heat-wrap">
        <svg
          viewBox={`0 0 ${LABEL_W + W + 4} ${rows.length * cellH + 14}`}
          height={rows.length * cellH + 14}
          role="img"
          aria-label={`${valueLabel} per ${rowLabel} over time`}
          onMouseLeave={() => setHover(null)}
        >
          {rows.map((row, ri) => (
            <g key={row.id}>
              {ri % 4 === 0 ? (
                <text className="axis-label" x={LABEL_W - 4} y={ri * cellH + cellH} textAnchor="end">
                  {row.id}
                </text>
              ) : null}
              {row.values.map((v, ci) => (
                <rect
                  key={ci}
                  x={LABEL_W + ci * cellW}
                  y={ri * cellH}
                  width={Math.max(cellW - 0.5, 1)}
                  height={cellH - 0.5}
                  fill={row.muted ? 'var(--surface-3)' : rampColor(Math.min(v / vmax, 1))}
                  onMouseEnter={() =>
                    setHover({ r: ri, c: ci, x: (LABEL_W + ci * cellW) / (LABEL_W + W), y: ri * cellH })
                  }
                />
              ))}
            </g>
          ))}
          <text className="axis-label" x={LABEL_W} y={rows.length * cellH + 10}>
            {colTimes[0]?.toFixed(0) ?? 0}s
          </text>
          <text className="axis-label" x={LABEL_W + W} y={rows.length * cellH + 10} textAnchor="end">
            {colTimes[cols - 1]?.toFixed(0) ?? 0}s
          </text>
        </svg>
      </div>
      <div className="heat-legend">
        <span>0</span>
        <span className="ramp">
          {RAMP.map((c) => (
            <i key={c} style={{ background: `var(${c})` }} />
          ))}
        </span>
        <span>{format(vmax)}</span>
        <span style={{ marginLeft: 6 }}>{valueLabel}</span>
        <span style={{ marginLeft: 'auto' }}>
          <i style={{ background: 'var(--surface-3)', width: 12, height: 8, display: 'inline-block', marginRight: 4 }} />
          not receiving
        </span>
      </div>
      {hover ? (
        <div className="tooltip" style={{ left: `min(${hover.x * 100}%, calc(100% - 120px))`, top: hover.y + 12 }}>
          <div className="tt-time">
            {rowLabel} {rows[hover.r].id} &middot; t = {colTimes[hover.c]?.toFixed(1)} s
          </div>
          <div className="tt-row">
            <span style={{ color: 'var(--ink-2)' }}>{valueLabel}</span>
            <b>{format(rows[hover.r].values[hover.c])}</b>
          </div>
        </div>
      ) : null}
    </div>
  );
}
