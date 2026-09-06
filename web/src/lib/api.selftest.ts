// Self-test for the Ingress transport client. No network, no browser, no test framework.
//
//   cd web && node --experimental-strip-types src/lib/api.selftest.ts
//
// It prints one line per case and throws at the end if any failed, so the exit code is the answer.
// A framework would have to be added to package.json, and the point of this file is that the
// transport can be checked with what is already installed.
//
// The imports are dynamic and carry the `.ts` extension because Node's type stripping resolves the
// real file name, while the project's tsconfig does not enable `allowImportingTsExtensions` and this
// file does not own tsconfig. The `typeof import(...)` cast keeps the module fully type-checked
// despite the untyped dynamic specifier, so nothing here is checked more loosely than the app is.

import type { RunHandle } from './useRun';
import type { ServerRunHandle } from './useServerRun';

type Assert<T extends true> = T;
/** The server hook is drop-in for the mock's handle apart from `engine` and `update`. */
export type ServerHandleMatchesRunHandle = Assert<ServerRunHandle extends Omit<RunHandle, 'engine' | 'update'> ? true : false>;

async function load<T>(name: string): Promise<T> {
  return (await import(`./${name}.ts`)) as T;
}

const api = await load<typeof import('./api')>('api');
const fx = await load<typeof import('./apiFixtures')>('apiFixtures');
const mode = await load<typeof import('./mode')>('mode');
const { BASE, cloneConfig } = await load<typeof import('./config')>('config');

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

async function checkAsync(name: string, fn: () => Promise<string>): Promise<void> {
  try {
    pass(name, await fn());
  } catch (e) {
    report(name, e);
  }
}

function show(v: unknown): string {
  if (typeof v === 'bigint') return `${v}n`;
  if (Array.isArray(v)) return `[${v.map(show).join(',')}]`;
  if (v && typeof v === 'object') return `{${Object.entries(v).map(([k, x]) => `${k}:${show(x)}`).join(',')}}`;
  return JSON.stringify(v) ?? String(v);
}

function same(a: unknown, b: unknown): boolean {
  if (typeof a === 'bigint' || typeof b === 'bigint') return typeof a === typeof b && a === b;
  if (Array.isArray(a) || Array.isArray(b)) {
    if (!Array.isArray(a) || !Array.isArray(b) || a.length !== b.length) return false;
    return a.every((x, i) => same(x, b[i]));
  }
  if (a && b && typeof a === 'object' && typeof b === 'object') {
    const ka = Object.keys(a as object);
    const kb = Object.keys(b as object);
    if (ka.length !== kb.length) return false;
    return ka.every((k) => same((a as Record<string, unknown>)[k], (b as Record<string, unknown>)[k]));
  }
  if (typeof a === 'number' && typeof b === 'number' && Number.isNaN(a) && Number.isNaN(b)) return true;
  return a === b;
}

function eq(actual: unknown, expected: unknown, what: string): void {
  if (!same(actual, expected)) throw new Error(`${what}: expected ${show(expected)}, got ${show(actual)}`);
}

function ok(cond: boolean, what: string): void {
  if (!cond) throw new Error(what);
}

function throws(fn: () => unknown, what: string): string {
  try {
    fn();
  } catch (e) {
    return e instanceof Error ? e.message : String(e);
  }
  throw new Error(`${what}: expected a throw, got none`);
}

// ---------------------------------------------------------------------------
// 64-bit integers
// ---------------------------------------------------------------------------

check('uint64 survives the wire as bigint, and would not as a number', () => {
  const exact = BigInt(fx.EPOCH_NS_PLUS_123);
  eq(api.u64(fx.EPOCH_NS_PLUS_123), exact, 'u64 of the JSON string');
  const viaFloat = BigInt(Number(fx.EPOCH_NS_PLUS_123));
  ok(viaFloat !== exact, 'Number() was expected to lose the low nanoseconds and did not');
  const lost = viaFloat > exact ? viaFloat - exact : exact - viaFloat;
  // Decoding the same field out of a real message, not just the scalar helper.
  const status = api.decodeRunStatus(JSON.parse(fx.RUN_STATUS));
  eq(status.simTimeUnixNs, exact, 'RunStatus.simTimeUnixNs');
  return `${exact}n exact, Number() lands ${lost}n away`;
});

check('relSeconds is the only float form of time, and is exact at nanosecond scale', () => {
  const t0 = BigInt(fx.EPOCH_NS);
  const t = BigInt(fx.EPOCH_NS_PLUS_123);
  eq(api.relSeconds(t, t0), 1.23e-7, 'relSeconds');
  eq(api.relSeconds(t0 + 120_000_000_000n, t0), 120, 'relSeconds over two minutes');
  eq(api.secondsToNs(2), 2_000_000_000n, 'secondsToNs');
  return 'relSeconds(t0+123ns, t0) = 1.23e-7 s';
});

check('a uint64 arriving as an oversized JSON number is refused, not carried', () => {
  const msg = throws(() => api.u64(1767225600000000123), 'u64 of a float');
  ok(/already lost precision/.test(msg), `unexpected message: ${msg}`);
  eq(api.u64(42), 42n, 'a small integer is still accepted');
  return msg.slice(0, 60);
});

// ---------------------------------------------------------------------------
// message decoding
// ---------------------------------------------------------------------------

check('RunStatus decodes, and omitted fields get proto3 defaults', () => {
  const full = api.decodeRunStatus(JSON.parse(fx.RUN_STATUS));
  eq(full.runId, 'r-1', 'runId');
  eq(full.state, 'STATE_RUNNING', 'state');
  eq(full.realtimeFactor, 2.5, 'realtimeFactor');
  const sparse = api.decodeRunStatus(JSON.parse(fx.RUN_STATUS_SPARSE));
  eq(sparse, { runId: 'r-2', state: 'STATE_QUEUED', simTimeUnixNs: 0n, simEndUnixNs: 0n, realtimeFactor: 0, error: '' }, 'sparse RunStatus');
  return 'STATE_RUNNING, and a sparse status defaulted to 0n/0/""';
});

check('ListRunsResponse decodes both rows', () => {
  const l = api.decodeListRuns(JSON.parse(fx.LIST_RUNS_RESPONSE));
  eq(l.runs.length, 2, 'row count');
  eq(l.runs[1].error, 'replica pool exhausted', 'second row error');
  eq(l.runs[1].simTimeUnixNs, 0n, 'second row default time');
  eq(l.nextCursor, '', 'nextCursor');
  return '2 runs, nextCursor ""';
});

check('RewindResponse distinguishes log from re-simulation', () => {
  const fromLog = api.decodeRewind(JSON.parse(fx.REWIND_RESPONSE));
  eq(fromLog.fromLog, true, 'fromLog');
  eq(fromLog.restoredFromSnapshotUnixNs, 0n, 'no snapshot');
  const resim = api.decodeRewind(JSON.parse(fx.REWIND_RESPONSE_RESIMULATED));
  eq(resim.fromLog, false, 'fromLog defaults to false when omitted');
  eq(resim.restoredFromSnapshotUnixNs, 1767225620000000000n, 'snapshot instant');
  return 'fromLog true/false, snapshot 1767225620000000000n';
});

check('UpdateResponse decodes, including a rejection with everything else omitted', () => {
  const u = api.decodeUpdate(JSON.parse(fx.UPDATE_RESPONSE));
  eq(u, { accepted: true, requiredResimulation: true, rewoundToUnixNs: 1767225620000000000n, rejectedReason: '' }, 'accepted update');
  const r = api.decodeUpdate(JSON.parse(fx.UPDATE_RESPONSE_REJECTED));
  eq(r, { accepted: false, requiredResimulation: false, rewoundToUnixNs: 0n, rejectedReason: 'arrival_rps must be positive' }, 'rejected update');
  return 'rewoundTo 1767225620000000000n; rejection defaults accepted=false';
});

check('subscription lifecycle messages decode', () => {
  const open = api.decodeOpenSubscription(JSON.parse(fx.OPEN_SUBSCRIPTION_RESPONSE));
  eq(open.subscriptionId, 'sub-7', 'subscriptionId');
  eq(open.leaseExpiresAtUnixNs, 1767225660000000000n, 'lease expiry');
  eq(api.decodeRenew(JSON.parse(fx.RENEW_RESPONSE)), { leaseExpiresAtUnixNs: 1767225720000000000n, expired: false }, 'renew');
  eq(api.decodeRenew(JSON.parse(fx.RENEW_RESPONSE_EXPIRED)), { leaseExpiresAtUnixNs: 0n, expired: true }, 'expired renew');
  eq(JSON.parse(fx.CLOSE_SUBSCRIPTION_RESPONSE), {}, 'close is empty');
  return 'sub-7, lease 1767225660000000000n, expired renew flagged';
});

check('SubscriptionUpdate maps metric numbers to names and keeps unknown ones', () => {
  const u = api.decodeSubscriptionUpdate(JSON.parse(fx.SUBSCRIPTION_UPDATE_FLEET));
  eq(u.simTimeUnixNs, BigInt(fx.EPOCH_NS_PLUS_123), 'simTimeUnixNs');
  eq(u.row.target, { scope: 'SCOPE_FLEET' }, 'fleet target carries no id');
  eq(u.row.values.METRIC_QUEUED_SEQS, 4, 'METRIC_QUEUED_SEQS from key "23"');
  eq(u.row.values.METRIC_OFFERED_RPS, 70.5, 'METRIC_OFFERED_RPS from key "40"');
  eq(u.row.values.METRIC_LOAD_IMBALANCE_CV, 0.31, 'METRIC_LOAD_IMBALANCE_CV from key "64"');
  eq(u.row.unknownValues, { '999': 1.5 }, 'unknown metric number kept aside');
  const ttft = u.row.distributions.METRIC_TTFT;
  ok(ttft !== undefined, 'METRIC_TTFT distribution missing');
  eq(ttft?.count, 18422n, 'distribution count is a uint64');
  eq(ttft?.percentile, [50, 90, 99, 99.9], 'percentiles');
  eq(ttft?.value, [310, 780.5, 2400, 8800], 'percentile values');
  eq(ttft?.fromMergedHistogram, true, 'fromMergedHistogram');
  eq(u.final, false, 'final');
  return 'values keyed 23/40/45/64 named, 999 preserved, TTFT count 18422n';
});

check('a replica update carries exactly one id, and the final flag', () => {
  const u = api.decodeSubscriptionUpdate(JSON.parse(fx.SUBSCRIPTION_UPDATE_REPLICA));
  eq(u.row.target, { scope: 'SCOPE_REPLICA', replicaId: 3n }, 'replica target');
  eq(u.row.values.METRIC_KV_UTILIZATION, 0.82, 'kv utilization');
  eq(u.row.distributions, {}, 'no distributions');
  eq(u.final, true, 'final');
  eq(u.realtimeFactor, 0, 'omitted realtimeFactor defaults to 0');
  return 'SCOPE_REPLICA replicaId 3n, final true';
});

check('RunResult decodes scorecard maps keyed by enum number', () => {
  const r = api.decodeRunResult(JSON.parse(fx.RUN_RESULT));
  eq(r.seed, 20260906n, 'seed');
  eq(r.eventCount, 48211904n, 'eventCount');
  // Above 2^63: an unsigned checksum is exactly the value a signed or float path would corrupt.
  eq(r.stateChecksum, 12297829382473034410n, 'stateChecksum');
  eq(r.overall.values.METRIC_GOODPUT_TOKENS_PER_S, 18400.25, 'goodput');
  eq(r.overall.values.METRIC_SLO_ATTAINMENT, 0.972, 'attainment');
  eq(r.overall.outcomeCounts.OUTCOME_OK, 2038112n, 'OUTCOME_OK count');
  eq(r.overall.outcomeCounts.OUTCOME_TIMEOUT_RUNNING, 12n, 'OUTCOME_TIMEOUT_RUNNING count');
  eq(r.overall.distributions.METRIC_E2E?.value, [42000], 'e2e p99');
  eq(r.overall.recoveryTimeNs, 0n, 'recoveryTimeNs');
  eq(r.byScope[0].target, { scope: 'SCOPE_REPLICA', replicaId: 3n }, 'scoped target');
  eq(r.recorded[0].row.values.METRIC_OFFERED_RPS, 70, 'recorded frame');
  eq(r.refereeViolations.REFEREE_VERDICT_KV_OVERCOMMIT, 4n, 'referee violation by name');
  eq(r.realtimeFactor, 9.6, 'realtimeFactor');
  return 'checksum 12297829382473034410n exact, outcomes and violations named';
});

check('RequestTrace and its spans decode', () => {
  const traces = api.decodeGetTraces(JSON.parse(fx.GET_TRACES_RESPONSE));
  eq(traces.length, 1, 'trace count');
  const t = traces[0];
  eq(t.record.id, 8891234n, 'record id');
  eq(t.record.outcome, 'OUTCOME_OK_SLO_VIOLATED', 'outcome');
  eq(t.record.sloClass, 'SLO_CLASS_INTERACTIVE', 'slo class');
  eq(t.record.replicaPathId, [17n, 4n], 'replica path');
  eq(t.record.e2eNs, 11900000000n, 'e2e ns');
  eq(t.record.promptTokens, 24310, 'prompt tokens is a plain number');
  eq(t.spans.length, 2, 'span count');
  eq(t.spans[0].component, 'gateway', 'first span component');
  eq(t.spans[1].kvTier, 'MEMORY_TIER_HBM', 'kv tier name');
  eq(t.spans[1].replicaId, 17n, 'span replica id');
  eq(t.spans[0].kvTier, 'MEMORY_TIER_UNSPECIFIED', 'omitted enum defaults to zero value');
  return 'e2e 11900000000n, 2 spans, MEMORY_TIER_HBM';
});

check('Target encoding puts ids on the wire as strings, and none for the fleet', () => {
  eq(api.encodeTarget(api.fleetTarget()), { scope: 'SCOPE_FLEET' }, 'fleet');
  eq(api.encodeTarget(api.replicaTarget(3)), { scope: 'SCOPE_REPLICA', replicaId: '3' }, 'replica');
  eq(api.encodeTarget(api.poolTarget(9007199254740993n)), { scope: 'SCOPE_POOL', poolId: '9007199254740993' }, 'a pool id past 2^53');
  eq(api.uiTargetToWire({ scope: 'REPLICA', id: 12 }), { scope: 'SCOPE_REPLICA', replicaId: 12n }, 'ui target to wire');
  eq(api.uiTargetToWire({ scope: 'FLEET' }), { scope: 'SCOPE_FLEET' }, 'ui fleet target');
  eq(api.wireTargetToUi(api.replicaTarget(12)), { scope: 'REPLICA', id: 12 }, 'wire target back to ui');
  eq(api.decodeTarget(JSON.parse('{"scope":"SCOPE_TENANT","tenantId":"5"}')), { scope: 'SCOPE_TENANT', tenantId: 5n }, 'decode tenant target');
  return 'replicaId "3" as a string; pool id 9007199254740993 intact';
});

check('OpenSubscriptionRequest encodes metric names and a uint64 lease as a string', () => {
  const body = api.encodeOpenSubscription({
    runId: 'r-1',
    target: api.replicaTarget(3),
    metrics: ['METRIC_TTFT', 'METRIC_QUEUED_SEQS'],
    samplesPerSimSecond: 2,
    percentiles: [50, 99],
    leaseNs: api.DEFAULT_LEASE_NS,
  });
  eq(body, {
    runId: 'r-1',
    target: { scope: 'SCOPE_REPLICA', replicaId: '3' },
    metrics: ['METRIC_TTFT', 'METRIC_QUEUED_SEQS'],
    samplesPerSimSecond: 2,
    percentiles: [50, 99],
    leaseNs: '60000000000',
  }, 'request body');
  eq(JSON.stringify(body).includes('"leaseNs":"60000000000"'), true, 'lease serialises as a string');
  eq(api.METRIC_NUMBER.METRIC_QUEUED_SEQS, 23, 'METRIC_QUEUED_SEQS number');
  eq(api.METRIC_NAME[45], 'METRIC_GOODPUT_TOKENS_PER_S', '45 is goodput');
  return 'leaseNs "60000000000", metrics as names';
});

// ---------------------------------------------------------------------------
// the HTTP layer, against a stub fetch
// ---------------------------------------------------------------------------

interface StubCall {
  method: string;
  path: string;
  body: unknown;
}

function stub(handler: (c: StubCall) => { status?: number; body: string }): { client: import('./api').IngressClient; calls: StubCall[] } {
  const calls: StubCall[] = [];
  const fetchImpl: import('./api').FetchLike = async (url, init) => {
    const call: StubCall = {
      method: init?.method ?? 'GET',
      path: url.replace('http://ingress.test', ''),
      body: typeof init?.body === 'string' ? JSON.parse(init.body) : undefined,
    };
    calls.push(call);
    const r = handler(call);
    return new Response(r.body, { status: r.status ?? 200, headers: { 'content-type': 'application/json' } });
  };
  return { client: new api.IngressClient({ baseUrl: 'http://ingress.test/', fetchImpl }), calls };
}

await checkAsync('every endpoint hits its documented method and path', async () => {
  const bodies: Record<string, string> = {
    'POST /v1/runs': fx.START_RUN_RESPONSE,
    'POST /v1/runs/r-1:stop': fx.RUN_STATUS,
    'GET /v1/runs/r-1': fx.RUN_STATUS,
    'GET /v1/runs?limit=2': fx.LIST_RUNS_RESPONSE,
    'POST /v1/runs/r-1:setSpeed': fx.RUN_STATUS,
    'POST /v1/runs/r-1:stepForward': fx.RUN_STATUS,
    'POST /v1/runs/r-1:rewind': fx.REWIND_RESPONSE,
    'POST /v1/runs/r-1:updateWorkload': fx.UPDATE_RESPONSE,
    'POST /v1/runs/r-1:updatePolicies': fx.UPDATE_RESPONSE,
    'POST /v1/subscriptions': fx.OPEN_SUBSCRIPTION_RESPONSE,
    'POST /v1/subscriptions/sub-7:renew': fx.RENEW_RESPONSE,
    'POST /v1/subscriptions/sub-7:close': fx.CLOSE_SUBSCRIPTION_RESPONSE,
    'GET /v1/runs/r-1/result': fx.RUN_RESULT,
    'GET /v1/runs/r-1/traces?outcome=OUTCOME_FAILED&minE2eNs=5000000000&limit=10': fx.GET_TRACES_RESPONSE,
  };
  const { client, calls } = stub((c) => {
    const key = `${c.method} ${c.path}`;
    const body = bodies[key];
    if (body === undefined) throw new Error(`unexpected call ${key}`);
    return { body };
  });
  eq(await client.startRun({ scenario: { name: 'base' }, recordTraces: true }), 'r-1', 'startRun');
  await client.stopRun('r-1');
  await client.getRun('r-1');
  await client.listRuns({ limit: 2 });
  await client.setSpeed('r-1', 2, false);
  await client.stepForward('r-1', 2_000_000_000n);
  await client.rewind('r-1', BigInt(fx.EPOCH_NS));
  await client.updateWorkload('r-1', { arrival_rps: 120 });
  await client.updatePolicies('r-1', { routing: 'least_queue_tokens' });
  await client.openSubscription({ runId: 'r-1', target: api.fleetTarget(), metrics: ['METRIC_TTFT'], samplesPerSimSecond: 2 });
  await client.renewSubscription('sub-7', api.DEFAULT_LEASE_NS);
  await client.closeSubscription('sub-7');
  await client.getResult('r-1');
  await client.getTraces('r-1', { outcome: 'OUTCOME_FAILED', minE2eNs: 5_000_000_000n, limit: 10 });
  eq(calls.length, 14, 'call count');
  const step = calls.find((c) => c.path.endsWith(':stepForward'));
  eq(step?.body, { simDurationNs: '2000000000' }, 'StepForward sends its uint64 as a string');
  const rewind = calls.find((c) => c.path.endsWith(':rewind'));
  eq(rewind?.body, { toSimTimeUnixNs: fx.EPOCH_NS }, 'Rewind sends its uint64 as a string');
  eq(calls.find((c) => c.path.endsWith(':updateWorkload'))?.body, { workload: { arrival_rps: 120 } }, 'UpdateWorkload wraps the flat key set');
  eq(client.streamUrl('sub-7'), 'http://ingress.test/v1/subscriptions/sub-7/stream', 'stream URL');
  return `14 calls, all paths matched, uint64 request fields as strings`;
});

await checkAsync('a non-2xx body becomes an IngressError with its code', async () => {
  const { client } = stub(() => ({ status: 409, body: fx.ERROR_BODY }));
  let caught: unknown;
  try {
    await client.getRun('r-9');
  } catch (e) {
    caught = e;
  }
  ok(caught instanceof api.IngressError, 'expected an IngressError');
  const e = caught as import('./api').IngressError;
  eq(e.code, 9, 'error code');
  eq(e.httpStatus, 409, 'http status');
  eq(e.message, 'run r-9 is not running', 'error message');
  return 'HTTP 409, code 9, "run r-9 is not running"';
});

// ---------------------------------------------------------------------------
// server-sent events
// ---------------------------------------------------------------------------

check('the SSE parser reads whole frames and skips keepalives', () => {
  const { frames, rest } = api.parseSseFrames(fx.SSE_STREAM);
  eq(frames.length, 3, 'frame count');
  eq(rest, '', 'nothing left over');
  const first = api.decodeSubscriptionUpdate(JSON.parse(frames[0]));
  eq(first.simTimeUnixNs, BigInt(fx.EPOCH_NS), 'first frame instant');
  eq(api.decodeSubscriptionUpdate(JSON.parse(frames[2])).final, true, 'last frame is final');
  return '3 frames, 2 keepalives ignored';
});

check('the SSE parser survives chunks chopped mid-frame', () => {
  const whole = api.parseSseFrames(fx.SSE_STREAM).frames;
  const buf = new api.SseBuffer();
  const chunks = fx.chopStream(fx.SSE_STREAM, fx.SSE_CHUNK_SIZES);
  const got: string[] = [];
  for (const c of chunks) got.push(...buf.push(c));
  eq(got, whole, 'frames from chopped chunks');
  eq(buf.pending(), '', 'nothing pending at the end');
  const rps = got.map((f) => api.decodeSubscriptionUpdate(JSON.parse(f)).row.values.METRIC_OFFERED_RPS);
  eq(rps, [70, 71.5, 69], 'offered rps per frame');
  // A one-byte-at-a-time reader is the pathological case, so check it too.
  const byByte = new api.SseBuffer();
  const single: string[] = [];
  for (const ch of fx.SSE_STREAM) single.push(...byByte.push(ch));
  eq(single, whole, 'frames from one-byte chunks');
  return `${chunks.length} chunks of sizes ${fx.SSE_CHUNK_SIZES.join('/')} and ${fx.SSE_STREAM.length} single bytes both yield 3 identical frames`;
});

check('a stream truncated mid-frame yields no frame and keeps the tail', () => {
  const buf = new api.SseBuffer();
  eq(buf.push(fx.SSE_TRUNCATED), [], 'no frame from a partial one');
  eq(buf.pending(), fx.SSE_TRUNCATED, 'the partial frame is still buffered');
  eq(buf.push('}\n\n').length, 1, 'completing the frame emits it');
  return 'partial frame buffered, then emitted once terminated';
});

await checkAsync('readSseWithFetch decodes a chopped stream from a ReadableStream', async () => {
  const enc = new TextEncoder();
  const chunks = fx.chopStream(fx.SSE_STREAM, [11, 2, 53]);
  const fetchImpl: import('./api').FetchLike = async () =>
    new Response(
      new ReadableStream<Uint8Array>({
        start(c) {
          for (const ch of chunks) c.enqueue(enc.encode(ch));
          c.close();
        },
      }),
      { status: 200, headers: { 'content-type': 'text/event-stream' } }
    );
  const seen: bigint[] = [];
  await api.readSseWithFetch('http://ingress.test/v1/subscriptions/sub-7/stream', {
    fetchImpl,
    onFrame: (data) => seen.push(api.decodeSubscriptionUpdate(JSON.parse(data)).simTimeUnixNs),
  });
  eq(seen, [BigInt(fx.EPOCH_NS), 1767225600500000000n, 1767225601000000000n], 'instants in order');
  return `${chunks.length} network chunks, 3 updates, first instant ${seen[0]}n`;
});

check('backoff is full jitter, 250 ms base and 5 s cap', () => {
  eq(api.backoffDelayMs(0, () => 0), 0, 'floor at attempt 0');
  eq(api.backoffDelayMs(0, () => 0.999999), 249, 'ceiling at attempt 0');
  eq(api.backoffDelayMs(3, () => 0.999999), 1999, 'ceiling at attempt 3');
  eq(api.backoffDelayMs(20, () => 0.999999), 4999, 'capped at 5 s');
  eq(api.backoffDelayMs(-5, () => 0.5), 125, 'a negative attempt is treated as the first');
  return 'attempt 0 < 250 ms, attempt 3 < 2 s, attempt 20 capped below 5 s';
});

await checkAsync('a closed stream reconnects while the lease lives, and reopens when it does not', async () => {
  const bodies: Record<string, string> = {
    'POST /v1/subscriptions': fx.OPEN_SUBSCRIPTION_RESPONSE,
    'POST /v1/subscriptions/sub-7:renew': fx.RENEW_RESPONSE,
    'POST /v1/subscriptions/sub-7:close': fx.CLOSE_SUBSCRIPTION_RESPONSE,
  };
  const { client, calls } = stub((c) => {
    const body = bodies[`${c.method} ${c.path}`];
    if (body === undefined) throw new Error(`unexpected call ${c.method} ${c.path}`);
    return { body };
  });
  const phases: string[] = [];
  const delays: number[] = [];
  const updates: bigint[] = [];
  let streams = 0;
  // A clock the test drives: the lease is 60 s, so jumping 90 s makes the loop reopen rather than
  // reconnect, which is the branch a browser only reaches after being asleep.
  let clock = 0;
  let handle: import('./api').SubscriptionHandle | null = null;
  handle = api.subscribeToTarget(client, {
    runId: 'r-1',
    target: { scope: 'FLEET' },
    metrics: ['METRIC_OFFERED_RPS'],
    samplesPerSimSecond: 2,
    rnd: () => 0.5,
    now: () => clock,
    sleep: async (ms) => {
      delays.push(ms);
    },
    onPhase: (p) => phases.push(p),
    onUpdate: (u) => updates.push(u.simTimeUnixNs),
    openStreamImpl: (_url, o) => {
      streams++;
      const mine = streams;
      for (const f of api.parseSseFrames(fx.SSE_STREAM).frames) o.onFrame(f);
      if (mine === 2) clock += 90_000; // lease gone by the time this stream ends
      if (mine === 3) handle?.close();
      return { done: Promise.resolve(), close: () => undefined };
    },
  });
  await handle.done;
  eq(streams, 3, 'streams opened');
  eq(updates.length, 9, 'updates delivered across the three streams');
  eq(calls.filter((c) => c.path === '/v1/subscriptions').length, 2, 'OpenSubscription calls: one initial, one reopen');
  eq(delays, [125], 'one jittered reconnect delay, none before a reopen');
  eq(phases.filter((p) => p === 'reconnecting').length, 1, 'reconnecting phases');
  eq(phases.filter((p) => p === 'reopening').length, 1, 'reopening phases');
  eq(phases[phases.length - 1], 'closed', 'ends closed');
  eq(calls.some((c) => c.path === '/v1/subscriptions/sub-7:close'), true, 'CloseSubscription on close');
  return `3 streams, 9 updates, reconnect after ${delays[0]} ms, reopen after the lease expired`;
});

// ---------------------------------------------------------------------------
// scenario encoding
// ---------------------------------------------------------------------------

const EXPECTED_DROPPED = [
  'fleet.accelerator',
  'workload.perturbation',
  'workload.perturbAmplitude',
  'workload.perturbFrequencyHz',
  'routing.maxLoadRatio',
  'routing.fallbackChoices',
];

check('scenarioConfigToWire(BASE) sends only keys Scenario::parse accepts', () => {
  const { fields, dropped } = api.scenarioConfigToWire(BASE);
  eq(api.unacceptedKeys(fields), [], 'keys outside SCENARIO_KEYS');
  eq(dropped, EXPECTED_DROPPED, 'dropped fields');
  eq(fields.name, 'base', 'name');
  eq(fields.seed, 20260906, 'seed');
  eq(fields.duration_s, 120, 'duration_s');
  eq(fields.kv_capacity_tokens, 60000, 'fleet.kvTokensPerReplica becomes kv_capacity_tokens');
  eq(fields.arrival_rps, 100, 'arrival_rps');
  eq(fields.routing, 'round_robin', 'routing');
  eq(fields.p2c_choices, 2, 'p2c_choices');
  eq(fields.probe_live, false, 'probe_live');
  eq(fields.ttft_slo_ms, 2000, 'ttft_slo_ms');
  eq(fields.e2e_slo_s, 60, 'e2e_slo_s');
  eq(fields.sample_interval_ms, 500, '2 samples per simulated second is a 500 ms interval');
  eq(Object.keys(fields).length, 28, 'field count');
  return `${Object.keys(fields).length} keys, all accepted; dropped ${dropped.join(', ')}`;
});

check('the two engine name mismatches are translated, and prefix affinity is refused', () => {
  const leastKv = cloneConfig(BASE);
  leastKv.routing = { ...leastKv.routing, kind: 'least_kv_tokens' };
  eq(api.scenarioConfigToWire(leastKv).fields.routing, 'least_queue_tokens', 'least_kv_tokens maps to the engine name');
  const affinity = cloneConfig(BASE);
  affinity.routing = { ...affinity.routing, kind: 'prefix_affinity' };
  const enc = api.scenarioConfigToWire(affinity);
  eq('routing' in enc.fields, false, 'no routing key is sent for a policy the engine lacks');
  eq(enc.dropped.includes('routing.kind'), true, 'routing.kind is reported dropped');
  eq(api.unacceptedKeys(enc.fields), [], 'still only accepted keys');
  return 'least_kv_tokens -> least_queue_tokens; prefix_affinity dropped, not sent';
});

check('workload and policy updates carry only their own keys', () => {
  const w = api.workloadToWire(BASE);
  eq(api.unacceptedKeys(w.fields, api.WORKLOAD_KEYS), [], 'workload keys outside WORKLOAD_KEYS');
  eq(w.fields.arrival_rps, 100, 'arrival_rps');
  eq(w.dropped, ['workload.perturbation', 'workload.perturbAmplitude', 'workload.perturbFrequencyHz'], 'workload dropped');
  const p = api.policiesToWire(BASE);
  eq(api.unacceptedKeys(p.fields, api.POLICY_KEYS), [], 'policy keys outside POLICY_KEYS');
  eq(p.fields, { routing: 'round_robin', p2c_choices: 2, probe_live: false }, 'policy fields');
  return `workload ${Object.keys(w.fields).length} keys, policies ${Object.keys(p.fields).length} keys`;
});

check('the flat scenario form and the text form are both expressible', () => {
  eq(api.scenarioText('name = base\nseed = 1\n'), { text: 'name = base\nseed = 1\n' }, 'text form');
  const extra = api.scenarioConfigToWire(BASE, { step_token_budget: 1024, max_attempts: 3 });
  eq(extra.fields.step_token_budget, 1024, 'an engine-only key can be added');
  eq(api.unacceptedKeys(extra.fields), [], 'still only accepted keys');
  return 'text form and engine-only extras both accepted';
});

// ---------------------------------------------------------------------------
// mode
// ---------------------------------------------------------------------------

check('mock is the default, and the mock marker is unchanged', () => {
  eq(mode.MOCK_BANNER, 'mock data, no engine attached', 'the existing header marker');
  const m = mode.serverModeFrom(undefined, '', '');
  eq(m, { enabled: false, baseUrl: '', source: 'default' }, 'default mode');
  eq(mode.modeBanner(m), 'mock data, no engine attached', 'default banner');
  return 'default is mock, banner unchanged';
});

check('server mode turns on from the environment or from the URL', () => {
  eq(mode.serverModeFrom('1', '', ''), { enabled: true, baseUrl: '', source: 'env' }, 'VITE_LBSIM_SERVER=1');
  eq(mode.serverModeFrom('http://localhost:8099/', '', ''), { enabled: true, baseUrl: 'http://localhost:8099', source: 'env' }, 'explicit base from env');
  eq(mode.serverModeFrom(undefined, '?server=1', ''), { enabled: true, baseUrl: '', source: 'query' }, '?server=1');
  eq(mode.serverModeFrom(undefined, '', '#/dashboard?server=http://localhost:8099'), { enabled: true, baseUrl: 'http://localhost:8099', source: 'query' }, 'hash-route query');
  eq(mode.serverModeFrom('1', '?server=0', ''), { enabled: false, baseUrl: '', source: 'query' }, '?server=0 forces mock');
  eq(mode.modeBanner(mode.serverModeFrom(undefined, '?server=http://localhost:8099', '')), 'live data from the Ingress server at http://localhost:8099', 'server banner names the base');
  return 'env, ?server=1, ?server=<url>, hash query, ?server=0';
});

// ---------------------------------------------------------------------------

console.log(`${cases} cases, ${cases - failures} passed, ${failures} failed`);
if (failures > 0) throw new Error(`${failures} of ${cases} transport self-test cases failed`);
