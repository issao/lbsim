/** A 60-wide trend, for a table cell or a tile. No axes: the number beside it carries the value. */
export function Sparkline({
  points,
  color = 'var(--accent)',
  width = 58,
  height = 13,
  max,
}: {
  points: number[];
  color?: string;
  width?: number;
  height?: number;
  max?: number;
}) {
  // Block in both branches: an inline svg sits on the text baseline and makes a table row a
  // fraction of a pixel taller than the same row once its trend has drawn (U99).
  if (points.length < 2) return <svg width={width} height={height} style={{ display: 'block' }} aria-hidden="true" />;
  const hi = max ?? Math.max(...points, 1e-9);
  const d = points
    .map((p, i) => {
      const x = (i / (points.length - 1)) * (width - 1) + 0.5;
      const y = height - 1.5 - (Math.min(p / hi, 1) * (height - 3));
      return `${i === 0 ? 'M' : 'L'}${x.toFixed(1)} ${y.toFixed(1)}`;
    })
    .join(' ');
  return (
    <svg width={width} height={height} style={{ display: 'block', overflow: 'visible' }} aria-hidden="true">
      <path d={d} fill="none" stroke={color} strokeWidth={1.5} strokeLinejoin="round" />
    </svg>
  );
}
