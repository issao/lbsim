// Self-test for the mock/real classification in wired.ts. No network, no browser, no framework.
//
//   cd web && node --experimental-strip-types src/lib/wired.selftest.ts
//
// Same shape as api.selftest.ts / replay.selftest.ts: one line per case, a summary line, a throw
// when anything failed.
//
// Why the resolve hook below: wired.ts imports only types from ./engine (erased at strip-types time),
// but this file still resolves itself and its own bare relative imports the way every module does,
// extensionless, which Vite resolves and Node's ESM loader does not. The hook tries `<specifier>.ts`
// for a bare relative specifier and otherwise defers, so wired.ts stays written like the rest of the app.

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

const wired = await load<typeof import('./wired')>('wired');
const mode = await load<typeof import('./mode')>('mode');
type Frame = import('./engine').Frame;
type ReplicaSample = import('./engine').ReplicaSample;
type DataMode = import('./mode').DataMode;

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

function show(v: unknown): string {
  if (Array.isArray(v)) return `[${v.map(show).join(',')}]`;
  return JSON.stringify(v) ?? String(v);
}

function eq(actual: unknown, expected: unknown, what: string): void {
  const same = Array.isArray(actual) && Array.isArray(expected)
    ? actual.length === expected.length && actual.every((x, i) => x === expected[i])
    : actual === expected;
  if (!same) throw new Error(`${what}: expected ${show(expected)}, got ${show(actual)}`);
}

// ---------------------------------------------------------------------------
// fixtures: a plain mock-shaped frame (no simTimeUnixNs) and a wire-shaped one (has it). Field
// values themselves don't matter to realness(); only presence of `simTimeUnixNs` and the field
// names a panel declares it reads do.
// ---------------------------------------------------------------------------

const mockFrame = {
  tick: 1,
  simS: 1,
  offeredRps: 1,
  ttft: {},
  prefixHitRate: 0.5,
} as unknown as Frame;

const wireFrame = {
  ...mockFrame,
  simTimeUnixNs: 1_000_000_000n,
} as unknown as Frame;

// ---------------------------------------------------------------------------
// cases
// ---------------------------------------------------------------------------

check('mock frame is always mock', () => {
  const r = wired.realness(mockFrame, ['offeredRps', 'ttft']);
  eq(r.kind, 'mock', 'kind');
  eq(r.mockFields, [], 'mockFields');
  return 'no simTimeUnixNs outranks the fields read';
});

check('wire frame reading only wired fields is real', () => {
  const r = wired.realness(wireFrame, ['offeredRps', 'ttft']);
  eq(r.kind, 'real', 'kind');
  eq(r.mockFields, [], 'mockFields');
  return 'offeredRps, ttft are both wired';
});

check('wire frame reading one unwired field is partial', () => {
  const r = wired.realness(wireFrame, ['offeredRps', 'ttft', 'prefixHitRate']);
  eq(r.kind, 'partial', 'kind');
  eq(r.mockFields, ['prefixHitRate'], 'mockFields');
  return 'prefixHitRate is not wired, named alone';
});

check('replica read of a wired field is real', () => {
  const r = wired.realness(wireFrame, [], ['stepTimeMs' as keyof ReplicaSample]);
  eq(r.kind, 'real', 'kind');
  eq(r.mockFields, [], 'mockFields');
  return 'stepTimeMs is wired on ReplicaSample';
});

check('replica read of an unwired field is partial', () => {
  const r = wired.realness(wireFrame, [], ['queueWaitMs' as keyof ReplicaSample]);
  eq(r.kind, 'partial', 'kind');
  eq(r.mockFields, ['queueWaitMs'], 'mockFields');
  return 'queueWaitMs is not wired on ReplicaSample';
});

check('replica state is still mock even on a wire frame', () => {
  const r = wired.realness(wireFrame, [], ['state' as keyof ReplicaSample]);
  eq(r.kind, 'partial', 'kind');
  eq(r.mockFields, ['state'], 'mockFields');
  return 'announced state is invented until the engine wires it, per dashboard-plan section 3';
});

check('isWireFrame distinguishes the two fixtures', () => {
  eq(wired.isWireFrame(mockFrame), false, 'mock');
  eq(wired.isWireFrame(wireFrame), true, 'wire');
  return 'presence of simTimeUnixNs alone';
});

// U70: one vocabulary, the same three words everywhere, each with its own gloss.

check('every_data_mode_has_a_label_and_a_gloss', () => {
  const expectedWord: Record<DataMode, string> = { mock: 'mock', server: 'live', replay: 'replay' };
  for (const m of Object.keys(expectedWord) as DataMode[]) {
    eq(mode.DATA_SOURCE_LABEL[m], expectedWord[m], `label for ${m}`);
    const gloss = mode.dataSourceGloss(m);
    if (typeof gloss !== 'string' || gloss.length === 0) throw new Error(`${m} has no gloss: ${show(gloss)}`);
  }
  return 'mock/server/replay -> mock/live/replay, each with a non-empty gloss';
});

// U95: a partial panel on a live or replay run borrows the run's own word -- it is not lying
// about the run, only about a handful of columns on it. Only mock mode, and a genuinely mock
// frame, still say the bare word `mock`.
check('a_partial_panel_borrows_the_runs_word_on_live_or_replay', () => {
  const partial = wired.realness(wireFrame, ['offeredRps', 'prefixHitRate']);
  eq(partial.kind, 'partial', 'fixture is actually partial');
  eq(wired.panelTagWord(partial, 'server'), 'live', "partial panel's word in server mode");
  eq(wired.panelTagWord(partial, 'replay'), 'replay', "partial panel's word in replay mode");
  eq(wired.panelTagWord(partial, 'mock'), 'mock', "partial panel's word in mock mode");
  eq(wired.panelTagWord(undefined, 'server'), 'mock', 'unknown realness is treated as mock too');
  const real = wired.realness(wireFrame, ['offeredRps']);
  eq(wired.panelTagWord(real, 'server'), 'live', 'a fully-wired panel borrows the active mode\'s own word');
  eq(wired.panelTagWord(real, 'replay'), 'replay', 'same, in replay mode');
  return 'a partial panel says the run\'s own word except in mock mode, where the word is mock either way';
});

check('partialNote reads as one plain-English sentence', () => {
  const note = wired.partialNote('live', ['state', 'prefixHitRate', 'ttftMeanMs', 'trueSpeedMultiplier', 'weight']);
  eq(
    note,
    'live — 5 values not simulated yet, shown as placeholders: replica state, prefix hit rate, TTFT mean, speed multiplier, weight',
    'partialNote(live, [...5 fields])',
  );
  return note;
});

check('fieldLabel falls back to splitting camelCase for an unlisted identifier', () => {
  eq(wired.fieldLabel('someNewField'), 'some new field', 'fieldLabel(someNewField)');
  return 'unlisted identifiers still get a human label rather than the raw camelCase';
});

// ---------------------------------------------------------------------------

console.log(`${cases} cases, ${cases - failures} passed, ${failures} failed`);
if (failures > 0) throw new Error(`${failures} of ${cases} wired self-test cases failed`);
