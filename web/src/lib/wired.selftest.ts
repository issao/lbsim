// Self-test for the "—" rule's source of truth in wired.ts. No network, no browser, no framework.
//
//   cd web && node --experimental-strip-types src/lib/wired.selftest.ts
//
// Same shape as api.selftest.ts / replay.selftest.ts: one line per case, a summary line, a throw
// when anything failed.
//
// Why the resolve hook below: this file resolves its own bare relative imports the way every module
// does, extensionless, which Vite resolves and Node's ESM loader does not. The hook tries
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

const wired = await load<typeof import('./wired')>('wired');
const mode = await load<typeof import('./mode')>('mode');
const useRun = await load<typeof import('./useRun')>('useRun');

// ---------------------------------------------------------------------------
// harness
// ---------------------------------------------------------------------------

let cases = 0;
let failures = 0;

function check(name: string, body: () => string): void {
  cases++;
  try {
    const detail = body();
    console.log(`ok ${cases} — ${name}${detail ? `: ${detail}` : ''}`);
  } catch (e) {
    failures++;
    console.log(`FAIL ${cases} — ${name}: ${e instanceof Error ? e.message : String(e)}`);
  }
}

function eq<T>(got: T, want: T, what: string): void {
  const g = JSON.stringify(got);
  const w = JSON.stringify(want);
  if (g !== w) throw new Error(`${what}: got ${g}, want ${w}`);
}

// ---------------------------------------------------------------------------
// cases
// ---------------------------------------------------------------------------

check('the fields the engine measures are wired; the ones it does not are not', () => {
  for (const f of ['offeredRps', 'completedRps', 'ttft', 'kvUtilization', 'gpuUtilization', 'gpuUsefulFraction', 'gpuUsefulFractionP', 'replicas', 'preemptionsPerS'] as const) {
    eq(wired.WIRED_FRAME_FIELDS.has(f), true, `frame ${f} wired`);
  }
  for (const f of ['wastedGpuFraction', 'prefixHitRate', 'tierUtilization', 'tierBandwidth', 'events', 'warmingReplicas'] as const) {
    eq(wired.WIRED_FRAME_FIELDS.has(f), false, `frame ${f} unwired`);
  }
  return `${wired.WIRED_FRAME_FIELDS.size} frame fields wired`;
});

check("U101's three replica fields are wired now; prefix hit rate and weight are not", () => {
  for (const f of ['id', 'queuedSeqs', 'stepTimeMs', 'state', 'trueSpeedMultiplier', 'ttftMeanMs', 'gpuUsefulFraction'] as const) {
    eq(wired.WIRED_REPLICA_FIELDS.has(f), true, `replica ${f} wired`);
  }
  for (const f of ['prefixHitRate', 'weight', 'itlMeanMs', 'telemetryStalenessMs'] as const) {
    eq(wired.WIRED_REPLICA_FIELDS.has(f), false, `replica ${f} unwired`);
  }
  return `${wired.WIRED_REPLICA_FIELDS.size} replica fields wired`;
});

check('fieldLabel names every wired field and splits an unlisted identifier into words', () => {
  for (const f of [...wired.WIRED_FRAME_FIELDS, ...wired.WIRED_REPLICA_FIELDS]) {
    if (!(f in wired.FIELD_LABEL)) throw new Error(`no label for ${f}`);
  }
  eq(wired.fieldLabel('ttftMeanMs'), 'TTFT mean', 'listed');
  eq(wired.fieldLabel('someNewThingMs'), 'some new thing ms', 'unlisted, split');
  return 'every wired field labelled';
});

check('no exported symbol of wired.ts, mode.ts or useRun.ts mentions mock', () => {
  const offenders: string[] = [];
  for (const [file, mod] of [['wired', wired], ['mode', mode], ['useRun', useRun]] as const) {
    for (const k of Object.keys(mod)) if (/mock/i.test(k)) offenders.push(`${file}.${k}`);
  }
  for (const [k, v] of Object.entries(mode.DATA_SOURCE_GLOSS)) if (/mock/i.test(k) || /mock|invented/i.test(v)) offenders.push(`gloss ${k}`);
  for (const k of Object.keys(mode.DATA_SOURCE_LABEL)) if (/mock/i.test(k)) offenders.push(`label ${k}`);
  eq(offenders, [], 'symbols mentioning mock');
  return `${Object.keys(wired).length + Object.keys(mode).length + Object.keys(useRun).length} exports checked`;
});

check('with no server and no recordings the mode is none, and the badge says nothing', () => {
  const off = mode.serverModeFrom(undefined, '', '');
  eq(mode.dataModeFrom(off, false, null, false), 'none', 'nothing reachable');
  eq(mode.dataModeFrom(off, true, null, false), 'replay', 'index served');
  eq(mode.dataModeFrom(off, true, null, true), 'server', 'server answers');
  eq(mode.dataModeFrom(off, false, null, true), 'server', 'server answers, no index');
  eq(mode.badgeText({ mode: 'none' }), '', 'badge empty');
  eq(mode.badgeTitle({ mode: 'none' }), '', 'title empty');
  eq(mode.badgeText({ mode: 'server', runId: 'r-1' }), `${mode.SERVER_BANNER} · run r-1`, 'live badge');
  eq(mode.badgeText({ mode: 'replay', runId: '1-routing/p2c' }), `${mode.REPLAY_BANNER}: 1-routing/p2c`, 'replay badge');
  eq(mode.badgeText({ mode: 'released', runId: 'r-5' }), `${mode.REPLAY_RELEASED_BANNER}: r-5`, 'released badge names the run');
  eq(mode.badgeTitle({ mode: 'released', runId: 'r-5' }).includes('r-5'), true, 'released title names the run too');
  return 'server > replay > none';
});

console.log(`${cases} cases, ${cases - failures} passed, ${failures} failed`);
if (failures > 0) throw new Error(`${failures} wired self-test case(s) failed`);
