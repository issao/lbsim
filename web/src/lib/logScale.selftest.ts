// Self-test for the Slider `log` mode's position <-> value mapping. No network, no browser, no
// framework.
//
//   cd web && node --experimental-strip-types src/lib/logScale.selftest.ts
//
// Same shape as updateBanner.selftest.ts: one line per case, a summary line, a throw when
// anything failed.
//
// Why the `load` indirection below rather than a plain `import './logScale.ts'`: `tsc` (this file
// is type-checked as part of `npm run build`) refuses a *literal* import specifier ending in `.ts`
// unless `allowImportingTsExtensions` is set. A dynamic `import()` built from a template with an
// interpolated name is not a literal, so it passes tsc, and under plain node ESM (no resolve hook
// needed here — logScale.ts has no relative imports of its own) the `.ts` extension resolves fine
// at runtime. Same trick as wired.selftest.ts's `load`.

export {}; // top-level await below requires this file to be a module

async function load<T>(name: string): Promise<T> {
  return (await import(`./${name}.ts`)) as T;
}

const { LOG_POS_MAX, logPosToValue, roundSig, valueToLogPos } = await load<typeof import('./logScale')>('logScale');

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

function eq(actual: unknown, expected: unknown, what: string): void {
  if (actual !== expected) throw new Error(`${what}: expected ${JSON.stringify(expected)}, got ${JSON.stringify(actual)}`);
}

function ok(cond: boolean, what: string): void {
  if (!cond) throw new Error(what);
}

function close(actual: number, expected: number, tolerance: number, what: string): void {
  if (Math.abs(actual - expected) > tolerance) {
    throw new Error(`${what}: expected ${expected} +/- ${tolerance}, got ${actual}`);
  }
}

// ---------------------------------------------------------------------------

check('position 0 maps to exactly min', () => {
  eq(logPosToValue(10, 100000, 0), 10, 'value at pos 0');
  return '10 rps';
});

check(`position ${LOG_POS_MAX} maps to exactly max`, () => {
  eq(logPosToValue(10, 100000, LOG_POS_MAX), 100000, 'value at max pos');
  return '100000 rps';
});

check('the midpoint position is the geometric mean of min and max', () => {
  const v = logPosToValue(10, 100000, LOG_POS_MAX / 2);
  close(v, Math.sqrt(10 * 100000), 5, 'geometric mean');
  return `${v} rps at the midpoint`;
});

check('the mapping is monotonic across the position domain', () => {
  let prev = -Infinity;
  for (let pos = 0; pos <= LOG_POS_MAX; pos += 10) {
    const v = logPosToValue(10, 100000, pos);
    ok(v >= prev, `value must not decrease: pos ${pos} gave ${v}, previous was ${prev}`);
    prev = v;
  }
  return 'non-decreasing across 101 sampled positions';
});

check('value -> position is the exact inverse at both endpoints', () => {
  eq(valueToLogPos(10, 100000, 10), 0, 'pos of min');
  eq(valueToLogPos(10, 100000, 100000), LOG_POS_MAX, 'pos of max');
  return `0 and ${LOG_POS_MAX}`;
});

check('round trip pos -> value -> pos recovers the original position, within rounding', () => {
  for (const pos of [0, 1, 50, 137, 250, 500, 731, 900, 999, LOG_POS_MAX]) {
    const v = logPosToValue(10, 100000, pos);
    const back = valueToLogPos(10, 100000, v);
    close(back, pos, 1, `pos ${pos} via value ${v}`);
  }
  return 'agrees within 1 position unit for 10 sampled positions';
});

check('round trip value -> pos -> value recovers the original value to 3 significant figures', () => {
  for (const v of [10, 50, 680, 1878, 12400, 100000]) {
    const pos = valueToLogPos(10, 100000, v);
    const back = logPosToValue(10, 100000, pos);
    // one position step is a ratio of (max/min)^(1/LOG_POS_MAX), so the round trip can miss by
    // slightly more than the 3-sig-fig rounding alone; half a percent covers both.
    close(back, v, Math.max(1, v * 0.005), `value ${v} via pos ${pos}`);
  }
  return 'agrees within 0.5% for 6 sampled values';
});

check('every mapped value carries at most 3 significant figures', () => {
  for (let pos = 0; pos <= LOG_POS_MAX; pos += 7) {
    const v = logPosToValue(10, 100000, pos);
    const sig = v.toPrecision(3);
    ok(Math.abs(Number(sig) - v) < 1e-9, `${v} (pos ${pos}) is not already 3 significant figures`);
  }
  return 'checked 143 sampled positions';
});

check('roundSig matches toPrecision(3) for representative magnitudes', () => {
  for (const v of [9.999, 10, 99.95, 100.05, 1234, 99999, 100001]) {
    eq(roundSig(v, 3), Number(v.toPrecision(3)), `roundSig(${v}, 3)`);
  }
  return 'agrees with toPrecision(3) on 7 values';
});

check('out-of-range positions and values clamp instead of extrapolating', () => {
  eq(logPosToValue(10, 100000, -50), 10, 'negative position clamps to min');
  eq(logPosToValue(10, 100000, 5000), 100000, 'over-range position clamps to max');
  eq(valueToLogPos(10, 100000, 1), 0, 'below-range value clamps to position 0');
  eq(valueToLogPos(10, 100000, 1e9), LOG_POS_MAX, 'above-range value clamps to max position');
  return 'clamped at both ends of both directions';
});

check('replicas range (10 -> 10,000, U91) is well-formed at its endpoints', () => {
  eq(logPosToValue(10, 10000, 0), 10, 'min replicas');
  eq(logPosToValue(10, 10000, LOG_POS_MAX), 10000, 'max replicas');
  return '10 and 10000';
});

// ---------------------------------------------------------------------------

console.log(`${cases} cases, ${cases - failures} passed, ${failures} failed`);
if (failures > 0) throw new Error(`${failures} of ${cases} log-scale self-test cases failed`);
