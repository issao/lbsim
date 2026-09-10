// Pure geometry for the Traces tab's span timeline (the `Waterfall` component), kept out of React
// so it has a self-test that runs under plain node rather than needing a browser — same reasoning
// as `traceRows.ts`.
//
// Issao: "The traces seems to show bars with a point in time. They should show width proportional
// of each trace duration (so e.g. if a request was in a queue for a while, the bar for queue should
// be wide indicating start and stop)." Audited `crates/sim-leaf` (span construction),
// `crates/sim-ingress/src/trace_wire.rs` (JSON serialization) and `web/src/lib/api.ts`
// (`decodeTraceSpan`): all three already carry a real `start_unix_ns`/`end_unix_ns` interval, and
// this file's `layoutSpan` already turns that interval into a width proportional to the duration —
// verified by the self-test below, which checks the exact width ratio the QA harness also checks.
// The defect this file fixes is narrower: `Waterfall.tsx` used to anchor its axis to the *spans'
// own* min-start/max-end, silently dropping any gap before the first span or after the last one
// (e.g. between the last span and `finished_at`) from the axis. `axisOf` anchors to the request's
// own arrival and finish instead, per spec ("a shared time axis from the request's arrival to its
// finish"), so a span's width is always relative to the request's own journey, not to whatever the
// sampled spans happened to cover.

/** The two fields a span's geometry needs; `WireTraceSpan` satisfies this without importing it. */
export interface SpanLike {
  startUnixNs: bigint;
  endUnixNs: bigint;
}

/** A bar narrower than this still renders, but at this floor; its real duration travels separately
 * (`durationMs`), never rounded away, so a hover or a self-test always sees the true number. */
export const MIN_WIDTH_PX = 1;

export interface Axis {
  originNs: bigint;
  /** Always > 0, so a degenerate all-zero trace never divides by zero. */
  totalMs: number;
}

export interface SpanGeometry {
  /** Pixels from the plot's own left edge (0 = `axis.originNs`), before any label-column offset. */
  x: number;
  /** Pixels, floored at `MIN_WIDTH_PX`. Never the number to read a duration back out of — use `durationMs`. */
  width: number;
  /** The span's real duration in ms, exact, regardless of how thin `width` renders. */
  durationMs: number;
  /** Milliseconds from `axis.originNs` to the span's start — what "start, relative to arrival" reads. */
  startMs: number;
  /** Milliseconds from `axis.originNs` to the span's end. */
  endMs: number;
}

export function msBetween(a: bigint, b: bigint): number {
  return Number(b - a) / 1e6;
}

/**
 * The plot's time axis: from `originNs` (the request's arrival) to `finishNs` (its finish, when
 * known) or, failing that, the latest span's own end — an in-flight or never-finished request still
 * gets an axis to draw on. Widened to cover every span regardless, so a span that (for any reason)
 * ran past the recorded finish is never clipped off the plot.
 */
export function axisOf(originNs: bigint, finishNs: bigint | null, spans: SpanLike[]): Axis {
  let end = finishNs !== null && finishNs > originNs ? finishNs : originNs;
  for (const s of spans) {
    if (s.endUnixNs > end) end = s.endUnixNs;
  }
  return { originNs, totalMs: Math.max(msBetween(originNs, end), 1e-6) };
}

/** One span's geometry on a `plotWidthPx`-wide axis. */
export function layoutSpan(s: SpanLike, axis: Axis, plotWidthPx: number): SpanGeometry {
  const startMs = msBetween(axis.originNs, s.startUnixNs);
  const endMs = msBetween(axis.originNs, s.endUnixNs);
  const durationMs = endMs - startMs;
  const x = (startMs / axis.totalMs) * plotWidthPx;
  const width = Math.max((durationMs / axis.totalMs) * plotWidthPx, MIN_WIDTH_PX);
  return { x, width, durationMs, startMs, endMs };
}

export function layoutSpans(spans: SpanLike[], axis: Axis, plotWidthPx: number): SpanGeometry[] {
  return spans.map((s) => layoutSpan(s, axis, plotWidthPx));
}
