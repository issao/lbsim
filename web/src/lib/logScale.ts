// Position <-> value mapping for Slider's `log` mode.
//
// A linear <input type="range"> is unusable across a range that must serve both a 10 rps trickle
// and a 100,000 rps ceiling (U-arrival-ceiling): a step fine enough at the low end takes millions of
// steps to cross the high end, and vice-versa. The fix is a slider whose *position* is linear (an
// integer 0..LOG_POS_MAX) but whose *value* is exponential in that position, so every position
// covers the same ratio of the range rather than the same absolute span.

/** The range-input position domain: 0 is `min`, LOG_POS_MAX is `max`. */
export const LOG_POS_MAX = 1000;

/** position (0..LOG_POS_MAX) -> value, rounded to 3 significant figures. */
export function logPosToValue(min: number, max: number, pos: number): number {
  const clampedPos = Math.min(LOG_POS_MAX, Math.max(0, pos));
  const raw = min * Math.pow(max / min, clampedPos / LOG_POS_MAX);
  return roundSig(raw, 3);
}

/** value -> position (0..LOG_POS_MAX), the inverse of logPosToValue up to its rounding. */
export function valueToLogPos(min: number, max: number, value: number): number {
  const clampedValue = Math.min(max, Math.max(min, value));
  const pos = (Math.log(clampedValue / min) / Math.log(max / min)) * LOG_POS_MAX;
  return Math.round(Math.min(LOG_POS_MAX, Math.max(0, pos)));
}

/** Round `v` to `sig` significant figures. Exported only for the self-test. */
export function roundSig(v: number, sig: number): number {
  if (v === 0 || !isFinite(v)) return v;
  const sign = v < 0 ? -1 : 1;
  const abs = Math.abs(v);
  const magnitude = Math.floor(Math.log10(abs));
  const factor = Math.pow(10, sig - 1 - magnitude);
  return (sign * Math.round(abs * factor)) / factor;
}
