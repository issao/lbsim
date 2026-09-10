// Self-test for the Traces tab's span-timeline geometry, per Issao: "The traces seems to show bars
// with a point in time. They should show width proportional of each trace duration... a mouse over
// to a bar in the trace should show machine id, start and stop time." This file proves the
// proportionality half — the exact property `tools/qa/qa.js` checks live (width ratio tracking
// duration ratio) — under plain node, no browser.
//
//   cd web && node --experimental-strip-types src/lib/spanLayout.selftest.ts
//
// Same shape as traceRows.selftest.ts.

const moduleModule = 'node:module';
const { register } = (await import(moduleModule)) as { register: (specifier: string, parentUrl: string) => void };
const hook = `
export async function resolve(specifier, context, next) {
  if (/^\\.\\.?\\//.test(specifier) && !/\\.[a-z]+$/.test(specifier)) {
    try { return await next(specifier + '.ts', context); }
    catch (e) { if (!e || e.code !== 'ERR_MODULE_NOT_FOUND') throw e; }
  }
  return next(specifier, context);
}`;
register(`data:text/javascript,${encodeURIComponent(hook)}`, import.meta.url);

async function load<T>(name: string): Promise<T> {
  return (await import(`./${name}.ts`)) as T;
}

const { axisOf, layoutSpan, layoutSpans, MIN_WIDTH_PX } = await load<typeof import('./spanLayout')>('spanLayout');
type SpanLike = import('./spanLayout').SpanLike;

let cases = 0;
let failures = 0;

function pass(name: string, detail: string): void {
  cases++;
  console.log(`ok ${cases} — ${name}${detail ? `: ${detail}` : ''}`);
}

function report(name: string, e: unknown): void {
  cases++;
  failures++;
  console.log(`FAIL ${cases} — ${name}: ${e instanceof Error ? e.message : String(e)}`);
}

function check(name: string, fn: () => string): void {
  try {
    pass(name, fn());
  } catch (e) {
    report(name, e);
  }
}

function close(actual: number, expected: number, tol: number, what: string): void {
  if (Math.abs(actual - expected) > tol) throw new Error(`${what}: expected ${expected} ± ${tol}, got ${actual}`);
}

const NS = (ms: number) => BigInt(Math.round(ms * 1e6));
const ORIGIN = 1_700_000_000_000_000_000n; // a real-looking unix-ns instant, per api.ts's warning about float precision

function span(startMs: number, durMs: number): SpanLike {
  return { startUnixNs: ORIGIN + NS(startMs), endUnixNs: ORIGIN + NS(startMs + durMs) };
}

// ---------------------------------------------------------------------------

check('a span that lasted 3s among ones that lasted ms is visibly wide, not a point', () => {
  // gateway (~0.1ms), route (~0.2ms), a 3s queue, prefill (200ms), decode (50ms) — Issao's own example.
  const spans = [span(0, 0.1), span(0.1, 0.2), span(0.3, 3000), span(3000.3, 200), span(3200.3, 50)];
  const axis = axisOf(ORIGIN, ORIGIN + NS(3250.3), spans);
  const layout = layoutSpans(spans, axis, 420);
  const queue = layout[2];
  const gateway = layout[0];
  close(queue.durationMs, 3000, 1e-6, 'queue duration');
  if (queue.width < 300) throw new Error(`queue bar only ${queue.width.toFixed(1)}px wide on a 420px plot for a span covering 92% of the axis`);
  if (queue.width <= gateway.width) throw new Error('a 3000ms span must render wider than a 0.1ms one');
  return `queue ${queue.width.toFixed(1)}px vs gateway ${gateway.width.toFixed(1)}px`;
});

check('width ratio tracks duration ratio — the exact check qa.js runs live', () => {
  const spans = [span(0, 10), span(10, 40)]; // durations differ by 4x
  const axis = axisOf(ORIGIN, ORIGIN + NS(50), spans);
  const [short, long] = layoutSpans(spans, axis, 420);
  const durationRatio = long.durationMs / short.durationMs;
  const widthRatio = long.width / short.width;
  close(durationRatio, 4, 1e-9, 'duration ratio');
  close(widthRatio, 4, 1e-9, 'width ratio (unfloored, both spans well above MIN_WIDTH_PX)');
  return `duration 4x → width ${widthRatio.toFixed(3)}x`;
});

check('a zero-duration marker span still renders, floored at MIN_WIDTH_PX, and says so in durationMs', () => {
  const spans = [span(0, 0), span(0, 100)];
  const axis = axisOf(ORIGIN, ORIGIN + NS(100), spans);
  const [marker, real] = layoutSpans(spans, axis, 420);
  close(marker.durationMs, 0, 1e-9, 'marker duration is exactly 0, not invented');
  close(marker.width, MIN_WIDTH_PX, 1e-9, 'marker floors to the minimum visible width');
  if (real.width <= marker.width) throw new Error('a 100ms span must be wider than a 0ms marker');
  return `marker ${marker.width}px (duration 0ms, honestly), real span ${real.width.toFixed(1)}px`;
});

check('the axis runs from arrival to finish, not from the spans\' own min/max', () => {
  // A gap after the last span (e.g. between the last span and the recorded finish) must still
  // count against the axis, or every span would silently render wider than its true share.
  const spans = [span(0, 10)];
  const finishNs = ORIGIN + NS(110); // 100ms of untraced tail after the one 10ms span
  const axis = axisOf(ORIGIN, finishNs, spans);
  close(axis.totalMs, 110, 1e-9, 'axis spans arrival→finish, including the untraced tail');
  const [only] = layoutSpans(spans, axis, 420);
  const naiveAxisWidth = (10 / 10) * 420; // what it would wrongly render as if anchored to the span's own extent
  if (Math.abs(only.width - naiveAxisWidth) < 1) {
    throw new Error('span width did not shrink to account for the untraced tail before finish');
  }
  close(only.width, (10 / 110) * 420, 1e-6, 'width proportional to the true arrival→finish axis');
  return `10ms span on a 110ms axis → ${only.width.toFixed(1)}px of 420`;
});

check('with no finish known yet, the axis falls back to the latest span\'s own end', () => {
  const spans = [span(0, 5), span(5, 15)];
  const axis = axisOf(ORIGIN, null, spans);
  close(axis.totalMs, 20, 1e-9, 'axis is exactly the spans\' own range when finish is unknown');
  return `axis ${axis.totalMs}ms`;
});

check('layoutSpan startMs/endMs are relative to the axis origin (arrival), for the hover to show "+Nms"', () => {
  const s = span(123.4, 56.7);
  const axis = axisOf(ORIGIN, ORIGIN + NS(500), [s]);
  const g = layoutSpan(s, axis, 420);
  close(g.startMs, 123.4, 1e-6, 'startMs');
  close(g.endMs, 123.4 + 56.7, 1e-6, 'endMs');
  return `+${g.startMs}ms to +${g.endMs}ms`;
});

// ---------------------------------------------------------------------------

console.log(`${cases} cases, ${cases - failures} passed, ${failures} failed`);
if (failures > 0) throw new Error(`${failures} of ${cases} spanLayout self-test cases failed`);
