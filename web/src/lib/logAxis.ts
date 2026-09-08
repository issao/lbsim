// Pure log-axis math for a chart whose interesting range hugs one end of a fraction (badput near
// 0, i.e. goodput near 100%, is exactly where a linear axis has no resolution). Kept out of
// LineChart.tsx and its JSX so this can be unit-tested with a bare `node
// --experimental-strip-types` run: type stripping does not compile JSX, so a .tsx module can't be
// imported by the selftest harness the way this plain .ts one can.
//
// Not `logScale.ts` (already taken by the Load tab's slider position/value mapping, an unrelated
// log scale) to keep the two apart by name as well as by purpose.

/**
 * The log axis's clamp: a value at or below the floor maps to the floor (0 -> floor), and a value
 * at or above the top of the domain passes through unchanged (1 -> 100%, i.e. `yMax`). This is
 * also how a real reading below the floor gets a position to draw at -- LineChart marks that
 * segment dashed rather than pretending the clamp did not happen.
 */
export function clampToLogFloor(v: number, floor: number): number {
  return Math.max(v, floor);
}

/** One gridline per decade from `lo` to `hi`, both assumed positive (`lo` a floor like 1e-4). */
export function logTicks(lo: number, hi: number): number[] {
  if (!isFinite(lo) || !isFinite(hi) || lo <= 0 || hi <= lo) return [lo];
  const startExp = Math.ceil(Math.log10(lo) - 1e-9);
  const endExp = Math.floor(Math.log10(hi) + 1e-9);
  const out: number[] = [];
  for (let e = startExp; e <= endExp; e++) out.push(Number(Math.pow(10, e).toPrecision(6)));
  return out;
}
