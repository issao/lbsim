import { useCallback, useMemo, useRef, useState } from 'react';

export interface Series {
  key: string;
  label: string;
  color: string;
  points: (number | null)[];
  dashed?: boolean;
}

export interface Threshold {
  value: number;
  label: string;
  color?: string;
}

const PAD = { top: 6, right: 46, bottom: 15, left: 40 };

/**
 * Hand-written SVG line chart. One y-axis, always: two measures of different scale get two charts.
 * Crosshair and tooltip ship by default, because an SVG chart in a browser is interactive whether
 * or not anyone designed it to be.
 */
export function LineChart({
  xs,
  series,
  height = 110,
  yMax,
  yMin = 0,
  format,
  xFormat = (v: number) => `${v.toFixed(0)}s`,
  thresholds = [],
  directLabels = true,
  unit,
}: {
  xs: number[];
  series: Series[];
  height?: number;
  yMax?: number;
  yMin?: number;
  format: (v: number) => string;
  xFormat?: (v: number) => string;
  thresholds?: Threshold[];
  directLabels?: boolean;
  unit?: string;
}) {
  const ref = useRef<HTMLDivElement>(null);
  const [hover, setHover] = useState<{ i: number; x: number } | null>(null);
  const W = 640; // viewBox width; the svg scales to its container
  const H = height;

  const { x0, x1, y0, y1 } = useMemo(() => {
    let hi = yMax ?? -Infinity;
    if (yMax === undefined) {
      for (const s of series) for (const p of s.points) if (p !== null && p > hi) hi = p;
      for (const t of thresholds) hi = Math.max(hi, t.value);
      if (!isFinite(hi) || hi <= 0) hi = 1;
      hi *= 1.12;
    }
    return { x0: xs[0] ?? 0, x1: xs[xs.length - 1] ?? 1, y0: yMin, y1: hi };
  }, [xs, series, yMax, yMin, thresholds]);

  const sx = useCallback(
    (v: number) => PAD.left + ((v - x0) / Math.max(x1 - x0, 1e-6)) * (W - PAD.left - PAD.right),
    [x0, x1]
  );
  const sy = useCallback(
    (v: number) => H - PAD.bottom - ((v - y0) / Math.max(y1 - y0, 1e-9)) * (H - PAD.top - PAD.bottom),
    [y0, y1, H]
  );

  const ticks = useMemo(() => niceTicks(y0, y1, 3), [y0, y1]);
  const xticks = useMemo(() => niceTicks(x0, x1, 4), [x0, x1]);

  const onMove = (e: React.MouseEvent) => {
    const el = ref.current;
    if (!el || xs.length === 0) return;
    const r = el.getBoundingClientRect();
    const px = ((e.clientX - r.left) / r.width) * W;
    const frac = (px - PAD.left) / Math.max(W - PAD.left - PAD.right, 1);
    const i = Math.round(frac * (xs.length - 1));
    if (i < 0 || i >= xs.length) {
      setHover(null);
      return;
    }
    setHover({ i, x: (e.clientX - r.left) / r.width });
  };

  const hv = hover ? hover.i : -1;

  return (
    <div className="chart" ref={ref} onMouseMove={onMove} onMouseLeave={() => setHover(null)}>
      {series.length > 1 ? (
        <div className="chart-legend">
          {series.map((s) => (
            <span key={s.key} style={{ color: s.color }}>
              <i className={s.dashed ? 'dashed' : ''} style={{ background: s.dashed ? undefined : s.color }} />
              <span style={{ color: 'var(--ink-2)' }}>{s.label}</span>
            </span>
          ))}
        </div>
      ) : null}
      <svg viewBox={`0 0 ${W} ${H}`} height={H} role="img" aria-label={series.map((s) => s.label).join(', ')}>
        {ticks.map((t) => (
          <g key={`y${t}`}>
            <line className="grid-line" x1={PAD.left} x2={W - PAD.right} y1={sy(t)} y2={sy(t)} />
            <text className="axis-label" x={PAD.left - 4} y={sy(t) + 3} textAnchor="end">
              {format(t)}
            </text>
          </g>
        ))}
        {xticks.map((t) => (
          <text key={`x${t}`} className="axis-label" x={sx(t)} y={H - 4} textAnchor="middle">
            {xFormat(t)}
          </text>
        ))}
        <line className="zero-line" x1={PAD.left} x2={W - PAD.right} y1={sy(y0)} y2={sy(y0)} />

        {thresholds.map((t) => (
          <g key={t.label}>
            <line
              x1={PAD.left}
              x2={W - PAD.right}
              y1={sy(t.value)}
              y2={sy(t.value)}
              stroke={t.color ?? 'var(--critical)'}
              strokeWidth={1}
              strokeDasharray="3 3"
            />
            <text
              className="axis-label"
              x={W - PAD.right + 3}
              y={sy(t.value) + 3}
              fill={t.color ?? 'var(--critical)'}
            >
              {t.label}
            </text>
          </g>
        ))}

        {series.map((s) => (
          <path
            key={s.key}
            d={pathOf(xs, s.points, sx, sy)}
            fill="none"
            stroke={s.color}
            strokeWidth={2}
            strokeLinejoin="round"
            strokeLinecap="round"
            strokeDasharray={s.dashed ? '4 3' : undefined}
          />
        ))}

        {directLabels && series.length > 1 && series.length <= 4
          ? series.map((s) => {
              const last = lastDefined(s.points);
              if (last === null) return null;
              return (
                <text
                  key={`l${s.key}`}
                  className="axis-label"
                  x={W - PAD.right + 3}
                  y={sy(last.v) + 3}
                  fill={s.color}
                >
                  {format(last.v)}
                </text>
              );
            })
          : null}

        {hv >= 0 ? (
          <>
            <line className="zero-line" x1={sx(xs[hv])} x2={sx(xs[hv])} y1={PAD.top} y2={H - PAD.bottom} />
            {series.map((s) => {
              const v = s.points[hv];
              return v === null || v === undefined ? null : (
                <circle
                  key={`h${s.key}`}
                  cx={sx(xs[hv])}
                  cy={sy(v)}
                  r={3}
                  fill={s.color}
                  stroke="var(--surface-1)"
                  strokeWidth={2}
                />
              );
            })}
          </>
        ) : null}
      </svg>
      {hover && hv >= 0 ? (
        <div
          className="tooltip"
          style={{
            left: `min(max(${hover.x * 100}% - 50px, 0px), calc(100% - 108px))`,
            top: 0,
          }}
        >
          <div className="tt-time">t = {xs[hv].toFixed(1)} s</div>
          {series.map((s) => (
            <div className="tt-row" key={`t${s.key}`}>
              <span style={{ color: 'var(--ink-2)' }}>{s.label}</span>
              <b style={{ color: s.color }}>
                {s.points[hv] === null || s.points[hv] === undefined ? '-' : format(s.points[hv] as number)}
                {unit ?? ''}
              </b>
            </div>
          ))}
        </div>
      ) : null}
    </div>
  );
}

function pathOf(
  xs: number[],
  ps: (number | null)[],
  sx: (v: number) => number,
  sy: (v: number) => number
): string {
  let d = '';
  let pen = false;
  for (let i = 0; i < xs.length; i++) {
    const v = ps[i];
    if (v === null || v === undefined || !isFinite(v)) {
      pen = false;
      continue;
    }
    d += `${pen ? 'L' : 'M'}${sx(xs[i]).toFixed(2)} ${sy(v).toFixed(2)} `;
    pen = true;
  }
  return d.trim();
}

function lastDefined(ps: (number | null)[]): { v: number } | null {
  for (let i = ps.length - 1; i >= 0; i--) {
    const v = ps[i];
    if (v !== null && v !== undefined && isFinite(v)) return { v };
  }
  return null;
}

export function niceTicks(lo: number, hi: number, n: number): number[] {
  if (!isFinite(lo) || !isFinite(hi) || hi <= lo) return [lo];
  const raw = (hi - lo) / n;
  const mag = Math.pow(10, Math.floor(Math.log10(raw)));
  const norm = raw / mag;
  const step = (norm >= 5 ? 5 : norm >= 2 ? 2 : 1) * mag;
  const out: number[] = [];
  for (let v = Math.ceil(lo / step) * step; v <= hi + 1e-9; v += step) out.push(Number(v.toFixed(6)));
  return out;
}
