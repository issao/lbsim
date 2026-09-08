import { useCallback, useMemo, useRef, useState } from 'react';
import { clampToLogFloor, logTicks } from '../../lib/logAxis';

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
  xFormat,
  thresholds = [],
  directLabels = true,
  unit,
  logY = false,
  logFloor = 1e-4,
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
  /**
   * Logarithmic y-axis, for a ratio whose interesting range hugs one end (badput near 0, i.e.
   * goodput near 100%, is exactly where linear scale has no resolution). The domain runs from
   * `logFloor` to `yMax ?? 1`. A point at or below the floor clamps to it and draws dashed there,
   * so "no real reading below the floor" stays visually distinct from an actual measurement.
   */
  logY?: boolean;
  logFloor?: number;
}) {
  const ref = useRef<HTMLDivElement>(null);
  const [hover, setHover] = useState<{ i: number; x: number } | null>(null);
  const W = 640; // viewBox width; the svg scales to its container
  const H = height;

  const { x0, x1, y0, y1 } = useMemo(() => {
    if (logY) {
      return { x0: xs[0] ?? 0, x1: xs[xs.length - 1] ?? 1, y0: logFloor, y1: yMax ?? 1 };
    }
    let hi = yMax ?? -Infinity;
    if (yMax === undefined) {
      for (const s of series) for (const p of s.points) if (p !== null && p > hi) hi = p;
      for (const t of thresholds) hi = Math.max(hi, t.value);
      if (!isFinite(hi) || hi <= 0) hi = 1;
      hi *= 1.12;
    }
    return { x0: xs[0] ?? 0, x1: xs[xs.length - 1] ?? 1, y0: yMin, y1: hi };
  }, [xs, series, yMax, yMin, thresholds, logY, logFloor]);

  const sx = useCallback(
    (v: number) => PAD.left + ((v - x0) / Math.max(x1 - x0, 1e-6)) * (W - PAD.left - PAD.right),
    [x0, x1]
  );
  const sy = useCallback(
    (v: number) => {
      if (logY) {
        const cv = clampToLogFloor(v, y0);
        const t = (Math.log10(cv) - Math.log10(y0)) / Math.max(Math.log10(y1) - Math.log10(y0), 1e-9);
        return H - PAD.bottom - t * (H - PAD.top - PAD.bottom);
      }
      return H - PAD.bottom - ((v - y0) / Math.max(y1 - y0, 1e-9)) * (H - PAD.top - PAD.bottom);
    },
    [y0, y1, H, logY]
  );

  const ticks = useMemo(() => (logY ? logTicks(y0, y1) : niceTicks(y0, y1, 3)), [y0, y1, logY]);
  const xticks = useMemo(() => niceTicks(x0, x1, 4), [x0, x1]);
  // Tick precision follows the span, so a four-second window does not print "1s 1s 2s 2s".
  const xfmt = useMemo(() => {
    if (xFormat) return xFormat;
    const span = Math.max(x1 - x0, 1e-6);
    const d = span >= 20 ? 0 : span >= 4 ? 1 : 2;
    return (v: number) => `${v.toFixed(d)}s`;
  }, [xFormat, x0, x1]);

  // Direct labels are pushed apart rather than allowed to overprint each other.
  const endLabels = useMemo(() => {
    if (!directLabels || series.length < 2 || series.length > 4) return [];
    const raw = series
      .map((s) => {
        const last = lastDefined(s.points);
        return last === null ? null : { key: s.key, color: s.color, v: last.v, y: sy(last.v) };
      })
      .filter((x): x is { key: string; color: string; v: number; y: number } => x !== null)
      .sort((a, b) => a.y - b.y);
    for (let i = 1; i < raw.length; i++) if (raw[i].y - raw[i - 1].y < 9) raw[i].y = raw[i - 1].y + 9;
    return raw;
  }, [series, sy, directLabels]);

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
            {xfmt(t)}
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
              x={PAD.left + 3}
              y={sy(t.value) - 3}
              fill={t.color ?? 'var(--critical)'}
              stroke="var(--surface-1)"
              strokeWidth={3}
              paintOrder="stroke"
            >
              {t.label}
            </text>
          </g>
        ))}

        {series.flatMap((s) =>
          logY
            ? logPathRuns(xs, s.points, sx, sy, y0).map((run, ri) => (
                <path
                  key={`${s.key}-${ri}`}
                  data-key={s.key}
                  // The drawn values, for a reader outside the page (tools/qa/qa.js checks that a
                  // smoothing window lowers a series' variance); pixels cannot say that, since the
                  // axis rescales.
                  data-values={s.points.map((p) => (p === null ? '' : p.toPrecision(5))).join(',')}
                  d={run.d}
                  fill="none"
                  stroke={s.color}
                  strokeWidth={2}
                  strokeLinejoin="round"
                  strokeLinecap="round"
                  strokeDasharray={run.dashed ? '4 3' : undefined}
                />
              ))
            : [
                <path
                  key={s.key}
                  data-key={s.key}
                  // The drawn values, for a reader outside the page (tools/qa/qa.js checks that a
                  // smoothing window lowers a series' variance); pixels cannot say that, since the
                  // axis rescales.
                  data-values={s.points.map((p) => (p === null ? '' : p.toPrecision(5))).join(',')}
                  d={pathOf(xs, s.points, sx, sy)}
                  fill="none"
                  stroke={s.color}
                  strokeWidth={2}
                  strokeLinejoin="round"
                  strokeLinecap="round"
                  strokeDasharray={s.dashed ? '4 3' : undefined}
                />,
              ]
        )}

        {endLabels.map((l) => (
          <text
            key={`l${l.key}`}
            className="axis-label"
            x={W - PAD.right + 3}
            y={l.y + 3}
            fill={l.color}
            stroke="var(--surface-1)"
            strokeWidth={3}
            paintOrder="stroke"
          >
            {format(l.v)}
          </text>
        ))}

        {hv >= 0 ? (
          <>
            <line className="zero-line" x1={sx(xs[hv])} x2={sx(xs[hv])} y1={PAD.top} y2={H - PAD.bottom} />
            {series.map((s) => {
              const v = s.points[hv];
              return v === null || v === undefined || !isFinite(v) ? null : (
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

/**
 * Path segments for a log-scaled series, split wherever a point crosses the floor: a run of real
 * (>= floor) values draws solid, a run of clamped (< floor, including <= 0) values draws dashed at
 * the floor. The boundary point is duplicated into both runs so the line stays visually
 * continuous across the style change.
 */
function logPathRuns(
  xs: number[],
  ps: (number | null)[],
  sx: (v: number) => number,
  sy: (v: number) => number,
  floor: number
): { d: string; dashed: boolean }[] {
  const runs: { d: string; dashed: boolean }[] = [];
  let d = '';
  let dashed = false;
  let started = false;
  for (let i = 0; i < xs.length; i++) {
    const v = ps[i];
    if (v === null || v === undefined || !isFinite(v)) {
      if (started) runs.push({ d: d.trim(), dashed });
      d = '';
      started = false;
      continue;
    }
    const isClamped = v < floor;
    const X = sx(xs[i]).toFixed(2);
    const Y = sy(v).toFixed(2);
    if (!started) {
      d = `M${X} ${Y} `;
      dashed = isClamped;
      started = true;
    } else if (isClamped !== dashed) {
      d += `L${X} ${Y} `;
      runs.push({ d: d.trim(), dashed });
      d = `M${X} ${Y} `;
      dashed = isClamped;
    } else {
      d += `L${X} ${Y} `;
    }
  }
  if (started) runs.push({ d: d.trim(), dashed });
  return runs;
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
