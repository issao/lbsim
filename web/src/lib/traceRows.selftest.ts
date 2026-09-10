// Self-test for the Traces tab's row shaping and sort, per Issao: "for the traces view, show the
// table with sampled requests at the top." No network, no browser, no framework.
//
//   cd web && node --experimental-strip-types src/lib/traceRows.selftest.ts
//
// Same shape as replay.selftest.ts and api.selftest.ts. The resolve hook below is needed because
// traceRows.ts imports `./api` extensionless, the way every other module does.

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

const { shapeTraceRow, shapeTraceRows, sortTraceRows } = await load<typeof import('./traceRows')>('traceRows');
type WireRequestTrace = import('./api').WireRequestTrace;

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

// ---------------------------------------------------------------------------
// fixtures — plain WireRequestTrace values, not decoded off the wire; that path is api.selftest.ts's.
// ---------------------------------------------------------------------------

const ORIGIN = 1_000_000_000n;

function trace(over: Partial<WireRequestTrace['record']> & { spans?: number; bucket?: WireRequestTrace['bucket'] }): WireRequestTrace {
  const record: WireRequestTrace['record'] = {
    id: 1n,
    tenantId: 0n,
    sloClass: 'SLO_CLASS_UNSPECIFIED',
    outcome: 'OUTCOME_OK',
    arrivedAtUnixNs: ORIGIN,
    admittedAtUnixNs: ORIGIN,
    firstTokenAtUnixNs: 0n,
    finishedAtUnixNs: 0n,
    promptTokens: 0,
    outputTokens: 0,
    cachedPrefixTokens: 0,
    clusterId: 0n,
    replicaId: 0n,
    replicaPathId: [],
    attempts: 0,
    preemptions: 0,
    queueWaitNs: 0n,
    preemptedNs: 0n,
    ttftNs: 0n,
    e2eNs: 0n,
    meanItlNs: 0n,
    p99ItlNs: 0n,
    ...over,
  };
  return { record, spans: new Array(over.spans ?? 0).fill(0).map(() => ({} as WireRequestTrace['spans'][number])), bucket: over.bucket ?? 'TRACE_BUCKET_P50' };
}

// ---------------------------------------------------------------------------
// shaping
// ---------------------------------------------------------------------------

check('shapeTraceRow reads every column the table shows, and only the engine\'s own numbers', () => {
  const t = trace({
    id: 42n,
    outcome: 'OUTCOME_OK',
    bucket: 'TRACE_BUCKET_P99',
    arrivedAtUnixNs: ORIGIN + 500_000_000n,
    ttftNs: 12_000_000n,
    e2eNs: 340_000_000n,
    promptTokens: 900,
    outputTokens: 128,
    replicaId: 7n,
    spans: 5,
  });
  const row = shapeTraceRow(t, ORIGIN);
  eq(row.id, 42n, 'id');
  eq(row.bucket, 'TRACE_BUCKET_P99', 'bucket');
  eq(row.outcome, 'OUTCOME_OK', 'outcome');
  eq(row.arrivedS, 0.5, 'arrivedS');
  eq(row.ttftMs, 12, 'ttftMs');
  eq(row.e2eMs, 340, 'e2eMs');
  eq(row.promptTokens, 900, 'promptTokens');
  eq(row.outputTokens, 128, 'outputTokens');
  eq(row.replicaId, 7n, 'replicaId');
  eq(row.spanCount, 5, 'spanCount');
  return `row ${row.id} at +${row.arrivedS}s, ttft ${row.ttftMs}ms, e2e ${row.e2eMs}ms, prompt ${row.promptTokens}, output ${row.outputTokens}, ${row.spanCount} spans`;
});

check('a request with no ttft/e2e/origin yet reads null, never a zero or an invented number', () => {
  const t = trace({ ttftNs: 0n, e2eNs: 0n, outcome: 'OUTCOME_UNSPECIFIED', replicaId: 3n });
  const withOrigin = shapeTraceRow(t, ORIGIN);
  eq(withOrigin.ttftMs, null, 'ttftMs before the first token');
  eq(withOrigin.e2eMs, null, 'e2eMs before completion');
  eq(withOrigin.replicaId, null, 'replicaId of a request that never named a home');
  const noOrigin = shapeTraceRow(t, null);
  eq(noOrigin.arrivedS, null, 'arrivedS with no origin known yet');
  return 'ttftMs, e2eMs, replicaId and arrivedS all null rather than 0';
});

check('a finished-but-SLO-violated request still carries its replica', () => {
  const t = trace({ outcome: 'OUTCOME_OK_SLO_VIOLATED', replicaId: 9n });
  eq(shapeTraceRow(t, ORIGIN).replicaId, 9n, 'replicaId');
  return 'replica 9';
});

check('shapeTraceRows maps the whole page in order', () => {
  const rows = shapeTraceRows([trace({ id: 1n }), trace({ id: 2n }), trace({ id: 3n })], ORIGIN);
  eq(rows.map((r) => r.id).join(','), '1,2,3', 'ids in input order');
  return '3 rows, order preserved';
});

// ---------------------------------------------------------------------------
// sort
// ---------------------------------------------------------------------------

const rows = [
  shapeTraceRow(trace({ id: 1n, ttftNs: 30_000_000n, e2eNs: 900_000_000n, arrivedAtUnixNs: ORIGIN + 3_000_000_000n, promptTokens: 3000, outputTokens: 90 }), ORIGIN),
  shapeTraceRow(trace({ id: 2n, ttftNs: 10_000_000n, e2eNs: 100_000_000n, arrivedAtUnixNs: ORIGIN + 1_000_000_000n, promptTokens: 1000, outputTokens: 10 }), ORIGIN),
  shapeTraceRow(trace({ id: 3n, ttftNs: 20_000_000n, e2eNs: 500_000_000n, arrivedAtUnixNs: ORIGIN + 2_000_000_000n, promptTokens: 2000, outputTokens: 50 }), ORIGIN),
  // Not finished: no ttft, no e2e — must sort last on both columns regardless of direction. Also
  // carries no prompt/output count yet, which must sort last the same way.
  shapeTraceRow(trace({ id: 4n, outcome: 'OUTCOME_UNSPECIFIED', ttftNs: 0n, e2eNs: 0n, arrivedAtUnixNs: ORIGIN + 4_000_000_000n, promptTokens: 0, outputTokens: 0 }), ORIGIN),
];

check('sortTraceRows by ttft, descending, is highest first with the un-timestamped row last', () => {
  const ids = sortTraceRows(rows, 'ttft', 'desc').map((r) => r.id);
  eq(ids.join(','), '1,3,2,4', 'order');
  return ids.join(',');
});

check('sortTraceRows by ttft, ascending, is lowest first with the un-timestamped row still last', () => {
  const ids = sortTraceRows(rows, 'ttft', 'asc').map((r) => r.id);
  eq(ids.join(','), '2,3,1,4', 'order');
  return ids.join(',');
});

check('sortTraceRows by e2e agrees with the ttft ordering here but is computed independently', () => {
  const ids = sortTraceRows(rows, 'e2e', 'desc').map((r) => r.id);
  eq(ids.join(','), '1,3,2,4', 'order');
  return ids.join(',');
});

check('sortTraceRows by arrived time orders every row, since arrival is always known once the origin is', () => {
  const ids = sortTraceRows(rows, 'arrived', 'desc').map((r) => r.id);
  eq(ids.join(','), '4,1,3,2', 'order');
  return ids.join(',');
});

check('sortTraceRows by prompt size, descending, is largest first with the countless row last', () => {
  const ids = sortTraceRows(rows, 'prompt', 'desc').map((r) => r.id);
  eq(ids.join(','), '1,3,2,4', 'order');
  return ids.join(',');
});

check('sortTraceRows by output size, ascending, is smallest first with the countless row still last', () => {
  const ids = sortTraceRows(rows, 'output', 'asc').map((r) => r.id);
  eq(ids.join(','), '2,3,1,4', 'order');
  return ids.join(',');
});

check('sortTraceRows does not mutate its input', () => {
  const before = rows.map((r) => r.id).join(',');
  sortTraceRows(rows, 'ttft', 'asc');
  eq(rows.map((r) => r.id).join(','), before, 'rows unchanged after sorting a snapshot of it');
  return 'input array identical after sorting';
});

check('two rows with no value for the sorted column keep a stable order (by id)', () => {
  const a = shapeTraceRow(trace({ id: 20n, outcome: 'OUTCOME_UNSPECIFIED' }), ORIGIN);
  const b = shapeTraceRow(trace({ id: 10n, outcome: 'OUTCOME_UNSPECIFIED' }), ORIGIN);
  const asc = sortTraceRows([a, b], 'ttft', 'asc').map((r) => r.id);
  const desc = sortTraceRows([a, b], 'ttft', 'desc').map((r) => r.id);
  eq(asc.join(','), '10,20', 'ascending tie-break by id');
  eq(desc.join(','), '10,20', 'descending tie-break by id too, since neither has a value to reverse');
  return 'both directions agree: 10 before 20';
});

// ---------------------------------------------------------------------------

console.log(`${cases} cases, ${cases - failures} passed, ${failures} failed`);
if (failures > 0) throw new Error(`${failures} of ${cases} traceRows self-test cases failed`);
