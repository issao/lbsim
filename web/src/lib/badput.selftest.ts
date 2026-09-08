// Self-test for the badput function (derive.ts) and the log-axis mapping it is drawn with
// (logAxis.ts). No network, no browser, no framework, and no JSX: the log mapping's math was
// deliberately split out of LineChart.tsx into logAxis.ts so this could import it directly, since
// type stripping does not compile JSX and a .tsx module cannot run under this harness.
//
//   cd web && node --experimental-strip-types src/lib/badput.selftest.ts
//
// Same shape as updateBanner.selftest.ts / replay.selftest.ts: one line per case, a summary line,
// a throw when anything failed.
//
// Why the resolve hook below: this file resolves its own bare relative imports the way every
// module does, extensionless, which Vite resolves and Node's ESM loader does not. The hook tries
// `<specifier>.ts` for a bare relative specifier and otherwise defers.

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

const derive = await load<typeof import('./derive')>('derive');
const logAxis = await load<typeof import('./logAxis')>('logAxis');

// ---------------------------------------------------------------------------
// harness
// ---------------------------------------------------------------------------

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

function close(actual: number, expected: number, what: string): void {
  if (!isFinite(actual) || Math.abs(actual - expected) > 1e-9) {
    throw new Error(`${what}: expected ~${expected}, got ${actual}`);
  }
}

// ---------------------------------------------------------------------------
// badput(): 1 - goodput/throughput, null (not 0) when throughput is absent
// ---------------------------------------------------------------------------

check('all throughput missed its SLO is 100% badput', () => {
  close(derive.badput(0, 100) as number, 1, 'badput(0, 100)');
  return '1';
});

check('goodput equal to throughput is 0% badput', () => {
  close(derive.badput(100, 100) as number, 0, 'badput(100, 100)');
  return '0';
});

check('a quarter missed is 25% badput', () => {
  close(derive.badput(75, 100) as number, 0.25, 'badput(75, 100)');
  return '0.25';
});

check('zero throughput is absent, not zero badput', () => {
  eq(derive.badput(0, 0), null, 'badput(0, 0)');
  eq(derive.badput(50, 0), null, 'badput(50, 0)');
  eq(derive.badput(0, -3), null, 'badput(0, -3) — a negative throughput is not a real reading either');
  return 'null in all three cases';
});

check('a NaN throughput (metric absent on the frame) is also absent', () => {
  eq(derive.badput(10, NaN), null, 'badput(10, NaN)');
  return 'null';
});

// ---------------------------------------------------------------------------
// clampToLogFloor(): the log axis's own mapping — 0 -> floor, 1 -> 100% (unchanged)
// ---------------------------------------------------------------------------

check('0 maps to the floor', () => {
  eq(logAxis.clampToLogFloor(0, 1e-4), 1e-4, 'clampToLogFloor(0, 1e-4)');
  return '1e-4';
});

check('1 (100%) passes through unchanged', () => {
  eq(logAxis.clampToLogFloor(1, 1e-4), 1, 'clampToLogFloor(1, 1e-4)');
  return '1';
});

check('a negative value also maps to the floor', () => {
  eq(logAxis.clampToLogFloor(-0.5, 1e-4), 1e-4, 'clampToLogFloor(-0.5, 1e-4)');
  return '1e-4';
});

check('a value already above the floor is untouched', () => {
  eq(logAxis.clampToLogFloor(0.02, 1e-4), 0.02, 'clampToLogFloor(0.02, 1e-4)');
  return '0.02';
});

check('logTicks(1e-4, 1) is one gridline per decade: 0.01%, 0.1%, 1%, 10%, 100%', () => {
  const t = logAxis.logTicks(1e-4, 1);
  eq(t.length, 5, 'tick count');
  eq(t[0], 0.0001, 't[0]');
  eq(t[1], 0.001, 't[1]');
  eq(t[2], 0.01, 't[2]');
  eq(t[3], 0.1, 't[3]');
  eq(t[4], 1, 't[4]');
  return JSON.stringify(t);
});

// ---------------------------------------------------------------------------

console.log(`${cases} cases, ${cases - failures} passed, ${failures} failed`);
if (failures > 0) throw new Error(`${failures} of ${cases} badput self-test cases failed`);
