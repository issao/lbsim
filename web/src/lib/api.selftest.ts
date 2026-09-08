// Self-test for the Ingress transport client. No network, no browser, no test framework.
//
//   cd web && node --experimental-strip-types src/lib/api.selftest.ts
//
// It prints one line per case and throws at the end if any failed, so the exit code is the answer.
// A framework would have to be added to package.json, and the point of this file is that the
// transport can be checked with what is already installed. Everything runs on the fixtures in
// apiFixtures.ts plus two files read from the repo: the protos, to check every hand-written wire
// field name against the schema of record, and scenarios/base.txt, to check config.ts's BASE
// against the numbers the engine actually runs.
//
// The imports are dynamic and carry the `.ts` extension because Node's type stripping resolves the
// real file name, while the project's tsconfig does not enable `allowImportingTsExtensions` and this
// file does not own tsconfig. The `typeof import(...)` cast keeps the module fully type-checked
// despite the untyped dynamic specifier, so nothing here is checked more loosely than the app is.

import type { RunHandle } from './useRun';
import type { ServerRunHandle } from './useServerRun';

type Assert<T extends true> = T;
/** The server hook is drop-in for the replay handle apart from `engine` and `update`. */
export type ServerHandleMatchesRunHandle = Assert<ServerRunHandle extends Omit<RunHandle, 'engine' | 'update'> ? true : false>;

async function load<T>(name: string): Promise<T> {
  return (await import(`./${name}.ts`)) as T;
}

const api = await load<typeof import('./api')>('api');
const fx = await load<typeof import('./apiFixtures')>('apiFixtures');
const mode = await load<typeof import('./mode')>('mode');
const { BASE, cloneConfig } = await load<typeof import('./config')>('config');

// Node's fs, without @types/node: a variable specifier keeps tsc out of it, and this file is the
// only one that reads the disk.
const fsModule = 'node:fs';
const fs = (await import(fsModule)) as { readFileSync: (p: URL | string, enc: 'utf8') => string; readdirSync: (p: URL | string) => string[] };
const repoFile = (rel: string): string => fs.readFileSync(new URL(`../../../${rel}`, import.meta.url), 'utf8');

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
  eq(status.simTimeUnixNs, exact, 'RunStatus.sim_time_unix_ns');
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
// WIRE.md's own examples
// ---------------------------------------------------------------------------

check('every example message in WIRE.md decodes', () => {
  const status = api.decodeRunStatus(JSON.parse(fx.WIRE_RUN_STATUS));
  eq(status, { runId: 'r-1', state: 'STATE_RUNNING', simTimeUnixNs: 1767225615000000000n, simEndUnixNs: 1767225720000000000n, realtimeFactor: 2, error: '' }, 'RunStatus');

  const start = JSON.parse(fx.WIRE_START_RUN_REQUEST);
  eq(start.scenario, { text: 'name = p2c\nseed = 20260906\n...', overrides: { arrival_rps: '90' } }, 'StartRunRequest.scenario');
  eq(api.parseScenarioText('name = p2c\nseed = 20260906\n'), { name: 'p2c', seed: '20260906' }, 'the text is the flat key = value form');

  const openEv = api.parseSseFrames(fx.WIRE_OPEN_EVENT).events;
  eq(openEv.length, 1, 'one open event');
  eq(openEv[0].event, 'open', 'event name');
  const open = api.decodeOpenSubscription(JSON.parse(openEv[0].data));
  eq(open, { subscriptionId: 's-7', leaseExpiresAtWallNs: 1767225660000000000n, rejectedReason: '' }, 'OpenSubscriptionResponse');

  const updEv = api.parseSseFrames(fx.WIRE_UPDATE_EVENT).events;
  eq(updEv.length, 1, 'one update event');
  eq(updEv[0].event, 'update', 'event name');
  eq(updEv[0].id, '1', 'id');
  const u = api.decodeSubscriptionUpdate(JSON.parse(updEv[0].data));
  eq(u.subscriptionId, 's-7', 'subscription_id');
  eq(u.simTimeUnixNs, 1767225615000000000n, 'sim_time_unix_ns');
  eq(u.realtimeFactor, 2, 'realtime_factor');
  eq(u.row.target, { scope: 'SCOPE_FLEET' }, 'target');
  eq(u.row.values, { METRIC_OFFERED_RPS: 70, METRIC_QUEUED_SEQS: 12 }, 'values by metric number');
  eq(u.row.distributions.METRIC_TTFT, { count: 41n, mean: 812.5, min: 120000000, max: 2400000000, percentile: [50, 99], value: [700000000, 2100000000], fromMergedHistogram: false }, 'distribution 1');
  eq(u.final, false, 'final');

  const err = api.toIngressError(JSON.parse(fx.WIRE_ERROR_BODY), 409, 'POST /v1/ingress/SetSpeed');
  eq(err.message, 'run r-9 is not running', 'error message');
  eq(err.httpStatus, 409, 'error status');
  eq(JSON.parse(fx.WIRE_UPDATE_REQUEST), { run_id: 'r-1', overrides: { arrival_rps: '90' } }, 'UpdateWorkload body');
  return 'RunStatus, StartRunRequest, open, update, error, update request';
});

// ---------------------------------------------------------------------------
// message decoding
// ---------------------------------------------------------------------------

check('RunStatus decodes, and omitted fields get proto3 defaults', () => {
  const full = api.decodeRunStatus(JSON.parse(fx.RUN_STATUS));
  eq(full.runId, 'r-1', 'run_id');
  eq(full.state, 'STATE_RUNNING', 'state');
  eq(full.realtimeFactor, 2.5, 'realtime_factor');
  const sparse = api.decodeRunStatus(JSON.parse(fx.RUN_STATUS_SPARSE));
  eq(sparse, { runId: 'r-2', state: 'STATE_QUEUED', simTimeUnixNs: 0n, simEndUnixNs: 0n, realtimeFactor: 0, error: '' }, 'sparse RunStatus');
  return 'STATE_RUNNING, and a sparse status defaulted to 0n/0/""';
});

check('ListRunsResponse decodes both rows', () => {
  const l = api.decodeListRuns(JSON.parse(fx.LIST_RUNS_RESPONSE));
  eq(l.runs.length, 2, 'row count');
  eq(l.runs[1].error, 'replica pool exhausted', 'second row error');
  eq(l.runs[1].simTimeUnixNs, 0n, 'second row default time');
  eq(l.nextCursor, '', 'next_cursor');
  return '2 runs, next_cursor ""';
});

check('RewindResponse distinguishes log from re-simulation', () => {
  const fromLog = api.decodeRewind(JSON.parse(fx.REWIND_RESPONSE));
  eq(fromLog.fromLog, true, 'from_log');
  eq(fromLog.restoredFromSnapshotUnixNs, 0n, 'no snapshot');
  const resim = api.decodeRewind(JSON.parse(fx.REWIND_RESPONSE_RESIMULATED));
  eq(resim.fromLog, false, 'from_log defaults to false when omitted');
  eq(resim.restoredFromSnapshotUnixNs, 1767225620000000000n, 'snapshot instant');
  return 'from_log true/false, snapshot 1767225620000000000n';
});

check('UpdateResponse decodes, including a rejection with everything else omitted', () => {
  const u = api.decodeUpdate(JSON.parse(fx.UPDATE_RESPONSE));
  eq(u, { accepted: true, requiredResimulation: true, rewoundToUnixNs: 1767225620000000000n, rejectedReason: '' }, 'accepted update');
  const r = api.decodeUpdate(JSON.parse(fx.UPDATE_RESPONSE_REJECTED));
  eq(r, { accepted: false, requiredResimulation: false, rewoundToUnixNs: 0n, rejectedReason: 'arrival_rps must be positive' }, 'rejected update');
  return 'rewound_to 1767225620000000000n; rejection defaults accepted=false';
});

check('the lease expiry is lease_expires_at_wall_ns, and the old _unix_ns name is not read', () => {
  const open = api.decodeOpenSubscription(JSON.parse(fx.OPEN_SUBSCRIPTION_RESPONSE));
  eq(open.subscriptionId, 'sub-7', 'subscription_id');
  eq(open.leaseExpiresAtWallNs, 1767225660000000000n, 'lease_expires_at_wall_ns');
  eq(api.decodeRenew(JSON.parse(fx.RENEW_RESPONSE)), { leaseExpiresAtWallNs: 1767225720000000000n, expired: false }, 'renew');
  eq(api.decodeRenew(JSON.parse(fx.RENEW_RESPONSE_EXPIRED)), { leaseExpiresAtWallNs: 0n, expired: true }, 'expired renew');
  eq(JSON.parse(fx.CLOSE_SUBSCRIPTION_RESPONSE), {}, 'close is empty');
  // The superseded contract's name decodes to the default rather than being quietly accepted.
  const old = api.decodeOpenSubscription(JSON.parse(fx.OPEN_SUBSCRIPTION_RESPONSE_OLD_NAME));
  eq(old.leaseExpiresAtWallNs, 0n, 'lease_expires_at_unix_ns is ignored');
  ok(!('leaseExpiresAtUnixNs' in old), 'no field with the old name on the decoded type');
  // Nothing in api.ts compares the wall-clock lease to the browser's clock.
  const src = repoFile('web/src/lib/api.ts');
  ok(!/leaseExpiresAtWallNs\s*[<>]|[<>]\s*leaseExpiresAtWallNs/.test(src), 'lease_expires_at_wall_ns is never compared');
  ok(!/Date\.now\(\)\s*[*\/]\s*1e6|BigInt\(Date\.now/.test(src), 'no browser wall clock is converted to compare with a server instant');
  return 'sub-7, lease 1767225660000000000n wall, old name ignored, never compared';
});

check('Distribution count is uint64, and mean/min/max/value are doubles', () => {
  const u = api.decodeSubscriptionUpdate(JSON.parse(fx.SUBSCRIPTION_UPDATE_FLEET));
  const ttft = u.row.distributions.METRIC_TTFT;
  ok(ttft !== undefined, 'METRIC_TTFT distribution missing');
  eq(typeof ttft?.count, 'bigint', 'count is a bigint');
  eq(ttft?.count, 18422n, 'count');
  eq(typeof ttft?.min, 'number', 'min is a number');
  eq(typeof ttft?.max, 'number', 'max is a number');
  eq(ttft?.min, 88000000, 'min');
  eq(ttft?.max, 9120500000.5, 'max keeps its fraction, which a uint64 could not');
  eq(ttft?.mean, 412500000, 'mean');
  eq(ttft?.percentile, [50, 90, 99, 99.9], 'percentiles');
  eq(ttft?.value, [310000000, 780500000, 2400000000, 8800000000], 'percentile values');
  eq(ttft?.fromMergedHistogram, true, 'from_merged_histogram');
  // A distribution that is not a JSON number for `count` is refused: the proto says uint64.
  const msg = throws(() => api.decodeDistribution({ count: 1.5 }), 'fractional count');
  ok(/not an integer|beyond 2\^53|expected a string/.test(msg) || /1\.5/.test(msg), `unexpected: ${msg}`);
  return 'count 18422n, max 9120500000.5 as a double';
});

check('enums are proto names, and enum-keyed maps use the number as the key', () => {
  const u = api.decodeSubscriptionUpdate(JSON.parse(fx.SUBSCRIPTION_UPDATE_FLEET));
  eq(u.row.target.scope, 'SCOPE_FLEET', 'scope by name');
  eq(u.row.values.METRIC_QUEUED_SEQS, 4, 'METRIC_QUEUED_SEQS from key "23"');
  eq(u.row.values.METRIC_OFFERED_RPS, 70.5, 'METRIC_OFFERED_RPS from key "40"');
  eq(u.row.values.METRIC_LOAD_IMBALANCE_CV, 0.31, 'METRIC_LOAD_IMBALANCE_CV from key "64"');
  eq(u.row.unknownValues, { '999': 1.5 }, 'unknown metric number kept aside');
  eq(api.decodeRunStatus({ state: 'STATE_PAUSED' }).state, 'STATE_PAUSED', 'RunStatus.State by name');
  const bad = throws(() => api.decodeRunStatus({ state: 'RUNNING' }), 'a bare enum name');
  ok(/not a value of this enum/.test(bad), `unexpected: ${bad}`);
  const badKey = throws(() => api.decodeMetricRow({ values: { METRIC_TTFT: 1 } }), 'a name-keyed map');
  ok(/enum number as the key/.test(badKey), `unexpected: ${badKey}`);
  eq(api.encodeTarget(api.replicaTarget(3)), { scope: 'SCOPE_REPLICA', replica_id: '3' }, 'Target encodes the scope by name');
  eq(api.METRIC_NUMBER.METRIC_QUEUED_SEQS, 23, 'METRIC_QUEUED_SEQS number');
  eq(api.METRIC_NAME[45], 'METRIC_GOODPUT_TOKENS_PER_S', '45 is goodput');
  return 'SCOPE_FLEET, keys 23/40/64 named, 999 preserved, "RUNNING" and name-keyed maps refused';
});

check('a replica update carries exactly one id, and the final flag', () => {
  const u = api.decodeSubscriptionUpdate(JSON.parse(fx.SUBSCRIPTION_UPDATE_REPLICA));
  eq(u.row.target, { scope: 'SCOPE_REPLICA', replicaId: 3n }, 'replica target');
  eq(u.row.values.METRIC_KV_UTILIZATION, 0.82, 'kv utilization');
  eq(u.row.distributions, {}, 'no distributions');
  eq(u.final, true, 'final');
  eq(u.realtimeFactor, 0, 'omitted realtime_factor defaults to 0');
  return 'SCOPE_REPLICA replica_id 3n, final true';
});

check('RunResult decodes scorecard maps keyed by enum number', () => {
  const r = api.decodeRunResult(JSON.parse(fx.RUN_RESULT));
  eq(r.seed, 20260906n, 'seed');
  eq(r.eventCount, 48211904n, 'event_count');
  // Above 2^63: an unsigned checksum is exactly the value a signed or float path would corrupt.
  eq(r.stateChecksum, 12297829382473034410n, 'state_checksum');
  eq(r.overall.values.METRIC_GOODPUT_TOKENS_PER_S, 18400.25, 'goodput');
  eq(r.overall.values.METRIC_SLO_ATTAINMENT, 0.972, 'attainment');
  eq(r.overall.outcomeCounts.OUTCOME_OK, 2038112n, 'OUTCOME_OK count');
  eq(r.overall.outcomeCounts.OUTCOME_TIMEOUT_RUNNING, 12n, 'OUTCOME_TIMEOUT_RUNNING count');
  eq(r.overall.distributions.METRIC_E2E?.value, [42000000000], 'e2e p99');
  eq(r.overall.recoveryTimeNs, 0n, 'recovery_time_ns');
  eq(r.byScope[0].target, { scope: 'SCOPE_REPLICA', replicaId: 3n }, 'scoped target');
  eq(r.recorded[0].row.values.METRIC_OFFERED_RPS, 70, 'recorded frame');
  eq(r.refereeViolations.REFEREE_VERDICT_KV_OVERCOMMIT, 4n, 'referee violation by name');
  eq(r.realtimeFactor, 9.6, 'realtime_factor');
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
  eq(api.encodeTarget(api.poolTarget(9007199254740993n)), { scope: 'SCOPE_POOL', pool_id: '9007199254740993' }, 'a pool id past 2^53');
  eq(api.uiTargetToWire({ scope: 'REPLICA', id: 12 }), { scope: 'SCOPE_REPLICA', replicaId: 12n }, 'ui target to wire');
  eq(api.uiTargetToWire({ scope: 'FLEET' }), { scope: 'SCOPE_FLEET' }, 'ui fleet target');
  eq(api.wireTargetToUi(api.replicaTarget(12)), { scope: 'REPLICA', id: 12 }, 'wire target back to ui');
  eq(api.decodeTarget(JSON.parse('{"scope":"SCOPE_TENANT","tenant_id":"5"}')), { scope: 'SCOPE_TENANT', tenantId: 5n }, 'decode tenant target');
  return 'replica_id "3" as a string; pool id 9007199254740993 intact';
});

// ---------------------------------------------------------------------------
// the HTTP layer, against a stub fetch
// ---------------------------------------------------------------------------

interface StubCall {
  method: string;
  path: string;
  headers: Record<string, string>;
  body: unknown;
}

interface StubReply {
  status?: number;
  body: string;
}

function stub(handler: (c: StubCall) => StubReply): { client: import('./api').IngressClient; calls: StubCall[]; fetchImpl: import('./api').FetchLike } {
  const calls: StubCall[] = [];
  const fetchImpl: import('./api').FetchLike = async (url, init) => {
    const headers: Record<string, string> = {};
    for (const [k, v] of Object.entries((init?.headers as Record<string, string> | undefined) ?? {})) headers[k.toLowerCase()] = v;
    const call: StubCall = {
      method: init?.method ?? 'GET',
      path: url.replace('http://ingress.test', ''),
      headers,
      body: typeof init?.body === 'string' ? JSON.parse(init.body) : undefined,
    };
    calls.push(call);
    const r = handler(call);
    return new Response(r.body, { status: r.status ?? 200, headers: { 'content-type': r.body.startsWith('event:') || r.body.startsWith(':') || r.body.startsWith('id:') ? 'text/event-stream' : 'application/json' } });
  };
  return { client: new api.IngressClient({ baseUrl: 'http://ingress.test/', fetchImpl }), calls, fetchImpl };
}

await checkAsync('every unary RPC is POST /v1/ingress/<RpcName>, and the stream is GET OpenSubscription', async () => {
  const bodies: Record<string, string> = {
    StartRun: fx.START_RUN_RESPONSE,
    StopRun: fx.RUN_STATUS,
    GetRun: fx.RUN_STATUS,
    ListRuns: fx.LIST_RUNS_RESPONSE,
    SetSpeed: fx.RUN_STATUS,
    StepForward: fx.RUN_STATUS,
    Rewind: fx.REWIND_RESPONSE,
    UpdateWorkload: fx.UPDATE_RESPONSE,
    UpdatePolicies: fx.UPDATE_RESPONSE,
    RenewSubscription: fx.RENEW_RESPONSE,
    CloseSubscription: fx.CLOSE_SUBSCRIPTION_RESPONSE,
    GetResult: fx.RUN_RESULT,
    GetTraces: fx.GET_TRACES_RESPONSE,
  };
  const { client, calls } = stub((c) => {
    const m = /^\/v1\/ingress\/([A-Za-z]+)$/.exec(c.path);
    if (!m || c.method !== 'POST') throw new Error(`unexpected call ${c.method} ${c.path}`);
    const body = bodies[m[1]];
    if (body === undefined) throw new Error(`unexpected RPC ${m[1]}`);
    return { body };
  });
  const env = api.scenarioEnvelope({ name: 'base', seed: 1 }, { arrival_rps: '90' });
  eq(await client.startRun({ scenario: env, recordTraces: true, maxRealtimeFactor: 0 }), 'r-1', 'StartRun');
  await client.stopRun('r-1');
  await client.getRun('r-1');
  await client.listRuns({ limit: 2 });
  await client.setSpeed('r-1', 2, false);
  await client.stepForward('r-1', 2_000_000_000n);
  await client.rewind('r-1', BigInt(fx.EPOCH_NS));
  await client.updateWorkload('r-1', { arrival_rps: '120' });
  await client.updatePolicies('r-1', { routing: 'least_queue_tokens' });
  await client.renewSubscription('sub-7', api.DEFAULT_LEASE_NS);
  await client.closeSubscription('sub-7');
  await client.getResult('r-1');
  await client.getTraces('r-1', { outcome: 'OUTCOME_FAILED', minE2eNs: 5_000_000_000n, limit: 10 });
  eq(calls.length, 13, 'call count');
  eq(calls.map((c) => c.path), Object.keys(bodies).map((n) => `/v1/ingress/${n}`), 'paths in order');
  ok(calls.every((c) => c.method === 'POST' && c.headers['content-type'] === 'application/json'), 'all POST with a JSON body');
  const by = (n: string) => calls.find((c) => c.path === `/v1/ingress/${n}`)?.body;
  eq(by('StartRun'), { scenario: { text: 'name = base\nseed = 1\n', overrides: { arrival_rps: '90' } }, max_realtime_factor: 0, record_traces: true }, 'StartRun body');
  eq(by('StopRun'), { run_id: 'r-1' }, 'StopRun is a RunRef');
  eq(by('GetRun'), { run_id: 'r-1' }, 'GetRun is a RunRef');
  eq(by('ListRuns'), { limit: 2 }, 'ListRuns body');
  eq(by('SetSpeed'), { run_id: 'r-1', realtime_factor: 2, paused: false }, 'SetSpeed body');
  eq(by('StepForward'), { run_id: 'r-1', sim_duration_ns: '2000000000' }, 'StepForward sends its uint64 as a string');
  eq(by('Rewind'), { run_id: 'r-1', to_sim_time_unix_ns: fx.EPOCH_NS }, 'Rewind sends its uint64 as a string');
  eq(by('UpdateWorkload'), { run_id: 'r-1', overrides: { arrival_rps: '120' } }, 'UpdateWorkload carries overrides');
  eq(by('UpdatePolicies'), { run_id: 'r-1', overrides: { routing: 'least_queue_tokens' } }, 'UpdatePolicies carries overrides');
  eq(by('RenewSubscription'), { subscription_id: 'sub-7', lease_ns: '60000000000' }, 'RenewSubscription body');
  eq(by('CloseSubscription'), { subscription_id: 'sub-7' }, 'CloseSubscription body');
  eq(by('GetResult'), { run_id: 'r-1' }, 'GetResult is a RunRef');
  eq(by('GetTraces'), { run_id: 'r-1', outcome: 'OUTCOME_FAILED', min_e2e_ns: '5000000000', limit: 10 }, 'GetTraces body');

  const url = new URL(client.openSubscriptionUrl({
    runId: 'r-1',
    target: api.replicaTarget(3),
    metrics: ['METRIC_TTFT', 'METRIC_QUEUED_SEQS'],
    samplesPerSimSecond: 2,
    percentiles: [50, 99],
    leaseNs: api.DEFAULT_LEASE_NS,
  }));
  eq(url.pathname, '/v1/ingress/OpenSubscription', 'stream path');
  eq(Object.fromEntries(url.searchParams), {
    run_id: 'r-1', scope: 'SCOPE_REPLICA', replica_id: '3', metrics: 'METRIC_TTFT,METRIC_QUEUED_SEQS',
    samples_per_sim_second: '2', percentiles: '50,99', lease_ns: '60000000000',
  }, 'stream query mirrors OpenSubscriptionRequest');
  const fleet = new URL(client.openSubscriptionUrl({ runId: 'r-1', target: api.fleetTarget(), metrics: ['METRIC_OFFERED_RPS'], samplesPerSimSecond: 4 }));
  eq(Object.fromEntries(fleet.searchParams), { run_id: 'r-1', scope: 'SCOPE_FLEET', metrics: 'METRIC_OFFERED_RPS', samples_per_sim_second: '4' }, 'fleet query carries no id, no empty percentiles');
  return '13 unary POSTs on /v1/ingress/<RpcName>; GET /v1/ingress/OpenSubscription?run_id&scope&replica_id&metrics&samples_per_sim_second&percentiles&lease_ns';
});

await checkAsync('a non-2xx body {"error": "..."} becomes an IngressError with the HTTP status', async () => {
  const { client } = stub(() => ({ status: 409, body: fx.WIRE_ERROR_BODY }));
  let caught: unknown;
  try {
    await client.getRun('r-9');
  } catch (e) {
    caught = e;
  }
  ok(caught instanceof api.IngressError, 'expected an IngressError');
  const e = caught as import('./api').IngressError;
  eq(e.httpStatus, 409, 'http status');
  eq(e.message, 'run r-9 is not running', 'error message');
  eq(e.gone, false, 'not gone');
  const gone = api.toIngressError(undefined, 410, 'GET stream');
  eq(gone.gone, true, '410 is gone');
  return 'HTTP 409, "run r-9 is not running"';
});

// ---------------------------------------------------------------------------
// server-sent events
// ---------------------------------------------------------------------------

check('SSE framing: event: open first, event: update after, every update with an id', () => {
  const { events, rest } = api.parseSseFrames(fx.SSE_STREAM);
  eq(events.length, 4, 'event count');
  eq(rest, '', 'nothing left over');
  eq(events[0].event, 'open', 'first is open');
  eq(events[0].id, null, 'open carries no id');
  const open = api.decodeOpenSubscription(JSON.parse(events[0].data));
  eq(open.subscriptionId, 'sub-7', 'open decodes as OpenSubscriptionResponse');
  eq(events.slice(1).map((e) => e.event), ['update', 'update', 'update'], 'the rest are updates');
  eq(events.slice(1).map((e) => e.id), ['1', '2', '3'], 'ids from 1');
  const first = api.decodeSubscriptionUpdate(JSON.parse(events[1].data));
  eq(first.simTimeUnixNs, BigInt(fx.EPOCH_NS), 'first update instant');
  eq(api.decodeSubscriptionUpdate(JSON.parse(events[3].data)).final, true, 'last update is final');
  return 'open + 3 updates (ids 1..3), 2 keepalives ignored';
});

check('the SSE parser survives chunks chopped mid-frame', () => {
  const whole = api.parseSseFrames(fx.SSE_STREAM).events;
  const buf = new api.SseBuffer();
  const chunks = fx.chopStream(fx.SSE_STREAM, fx.SSE_CHUNK_SIZES);
  const got: import('./api').SseEvent[] = [];
  for (const c of chunks) got.push(...buf.push(c));
  eq(got, whole, 'events from chopped chunks');
  eq(buf.pending(), '', 'nothing pending at the end');
  const rps = got.filter((e) => e.event === 'update').map((e) => api.decodeSubscriptionUpdate(JSON.parse(e.data)).row.values.METRIC_OFFERED_RPS);
  eq(rps, [70, 71.5, 69], 'offered rps per update');
  // A one-byte-at-a-time reader is the pathological case, so check it too.
  const byByte = new api.SseBuffer();
  const single: import('./api').SseEvent[] = [];
  for (const ch of fx.SSE_STREAM) single.push(...byByte.push(ch));
  eq(single, whole, 'events from one-byte chunks');
  return `${chunks.length} chunks of sizes ${fx.SSE_CHUNK_SIZES.join('/')} and ${fx.SSE_STREAM.length} single bytes both yield 4 identical events`;
});

check('a stream truncated mid-frame yields no event and keeps the tail', () => {
  const buf = new api.SseBuffer();
  eq(buf.push(fx.SSE_TRUNCATED), [], 'no event from a partial one');
  eq(buf.pending(), fx.SSE_TRUNCATED, 'the partial event is still buffered');
  const done = buf.push('}\n\n');
  eq(done.length, 1, 'completing the frame emits it');
  eq(done[0].id, '4', 'with its id');
  return 'partial event buffered, then emitted once terminated';
});

await checkAsync('readSseStream decodes a chopped stream from a ReadableStream', async () => {
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
  const seen: string[] = [];
  await api.readSseStream('http://ingress.test/v1/ingress/OpenSubscription?run_id=r-1&scope=SCOPE_FLEET', {
    fetchImpl,
    onEvent: (ev) => seen.push(`${ev.event}${ev.id === null ? '' : `#${ev.id}`}`),
  });
  eq(seen, ['open', 'update#1', 'update#2', 'update#3'], 'events in order');
  return `${chunks.length} network chunks, open + 3 updates`;
});

check('backoff is full jitter, 250 ms base and 5 s cap', () => {
  eq(api.backoffDelayMs(0, () => 0), 0, 'floor at attempt 0');
  eq(api.backoffDelayMs(0, () => 0.999999), 249, 'ceiling at attempt 0');
  eq(api.backoffDelayMs(3, () => 0.999999), 1999, 'ceiling at attempt 3');
  eq(api.backoffDelayMs(20, () => 0.999999), 4999, 'capped at 5 s');
  eq(api.backoffDelayMs(-5, () => 0.5), 125, 'a negative attempt is treated as the first');
  return 'attempt 0 < 250 ms, attempt 3 < 2 s, attempt 20 capped below 5 s';
});

await checkAsync('reconnect sends Last-Event-ID, and a 410 resubscribes from scratch', async () => {
  // Four streams through the real fetch reader: a fresh open the server closes mid-run, a resume
  // that replays two updates, a resume the server answers 410, and a fresh open that runs to final.
  let streams = 0;
  const { client, calls } = stub((c) => {
    if (c.method === 'POST') return { body: fx.CLOSE_SUBSCRIPTION_RESPONSE };
    streams++;
    if (streams === 1) return { body: fx.SSE_STREAM_OPEN_ENDED };
    if (streams === 2) return { body: fx.sseReplay(3, 2) };
    if (streams === 3) return { status: 410, body: '{"error": "gap larger than the ring"}' };
    return { body: fx.SSE_STREAM };
  });
  const phases: string[] = [];
  const delays: number[] = [];
  const updates: string[] = [];
  const handle = api.subscribeToTarget(client, {
    runId: 'r-1',
    target: { scope: 'FLEET' },
    metrics: ['METRIC_OFFERED_RPS'],
    samplesPerSimSecond: 2,
    rnd: () => 0.5,
    now: () => 0,
    sleep: async (ms) => {
      delays.push(ms);
    },
    onPhase: (p) => phases.push(p),
    onUpdate: (u) => updates.push(u.simTimeUnixNs.toString()),
  });
  await handle.done;
  const gets = calls.filter((c) => c.method === 'GET');
  eq(gets.length, 4, 'streams opened');
  eq(gets.map((c) => c.headers['last-event-id']), [undefined, '3', '5', undefined], 'Last-Event-ID: none on open, the last id on resume, none after 410');
  eq(gets.map((c) => new URL(`http://ingress.test${c.path}`).searchParams.get('subscription_id')), [null, 'sub-7', 'sub-7', null], 'subscription_id named only on a resume');
  ok(gets.every((c) => new URL(`http://ingress.test${c.path}`).pathname === '/v1/ingress/OpenSubscription'), 'always the OpenSubscription path');
  eq(updates.length, 8, '3 + 2 replayed + 3 after the resubscribe');
  eq(handle.lastEventId(), '3', 'the last id of the final stream');
  eq(phases.filter((p) => p === 'reconnecting').length, 2, 'two resumes');
  eq(phases.filter((p) => p === 'reopening').length, 1, 'one resubscribe');
  eq(phases[phases.length - 1], 'complete', 'ends on the final update');
  eq(handle.phase(), 'complete', 'handle agrees');
  eq(delays, [125, 250, 500], 'jittered backoff before each resume and the resubscribe, attempts counted up');
  eq(calls.filter((c) => c.method === 'POST').length, 0, 'no unary calls: no renew was due, and completion is not a close');
  return '4 GETs, Last-Event-ID [-, 3, 5, -], 410 -> fresh open, 8 updates, complete';
});

await checkAsync('a lapsed lease reopens rather than resumes, and close sends CloseSubscription', async () => {
  const { client, calls } = stub((c) => {
    if (c.path === '/v1/ingress/CloseSubscription') return { body: fx.CLOSE_SUBSCRIPTION_RESPONSE };
    throw new Error(`unexpected call ${c.method} ${c.path}`);
  });
  const phases: string[] = [];
  const delays: number[] = [];
  const lastIds: (string | undefined)[] = [];
  let streams = 0;
  // A clock the test drives: the lease is 60 s, so jumping 90 s makes the loop reopen rather than
  // resume, which is the branch a browser only reaches after being asleep.
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
    onUpdate: () => undefined,
    openStreamImpl: (_url, o) => {
      streams++;
      lastIds.push(o.lastEventId);
      const mine = streams;
      for (const ev of api.parseSseFrames(fx.SSE_STREAM_OPEN_ENDED).events) o.onEvent(ev);
      if (mine === 2) clock += 90_000; // lease gone by the time this stream ends
      if (mine === 3) handle?.close();
      return { done: Promise.resolve(), close: () => undefined };
    },
  });
  await handle.done;
  eq(streams, 3, 'streams opened');
  eq(lastIds, [undefined, '3', undefined], 'resume carries the id; the reopen after a lapsed lease does not');
  eq(delays, [125], 'one jittered delay before the resume, none before a reopen');
  eq(phases.filter((p) => p === 'reconnecting').length, 1, 'reconnecting phases');
  eq(phases.filter((p) => p === 'reopening').length, 1, 'reopening phases');
  eq(phases[phases.length - 1], 'closed', 'ends closed');
  eq(calls.map((c) => c.path), ['/v1/ingress/CloseSubscription'], 'CloseSubscription on close, once');
  eq(calls[0].body, { subscription_id: 'sub-7' }, 'closing the current subscription');
  return '3 streams, resume with id 3, reopen after the lease lapsed, one CloseSubscription';
});

await checkAsync('a rejected open (HTTP 200, rejected_reason set) fails without reconnecting', async () => {
  const { client, calls } = stub(() => ({ body: fx.SSE_REJECTED }));
  const phases: string[] = [];
  const handle = api.subscribeToTarget(client, {
    runId: 'r-1',
    target: { scope: 'REPLICA' },
    metrics: ['METRIC_STEP_TIME'],
    samplesPerSimSecond: 2,
    sleep: async () => undefined,
    onPhase: (p, d) => phases.push(d ? `${p}:${d}` : p),
    onUpdate: () => undefined,
  });
  await handle.done;
  eq(calls.length, 1, 'one attempt');
  eq(phases, ['opening', 'failed:SCOPE_REPLICA needs replica_id'], 'phases');
  eq(handle.subscriptionId(), null, 'no subscription');
  return 'failed with the server\'s reason, one GET';
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
  eq(fields.kv_capacity_tokens, 1370000, 'fleet.kvTokensPerReplica becomes kv_capacity_tokens');
  eq(fields.arrival_rps, 70, 'arrival_rps');
  eq(fields.routing, 'round_robin', 'routing');
  eq(fields.p2c_choices, 2, 'p2c_choices');
  eq(fields.probe_live, false, 'probe_live');
  eq(fields.ttft_slo_ms, 2000, 'ttft_slo_ms');
  eq(fields.e2e_slo_s, 60, 'e2e_slo_s');
  eq(fields.sample_interval_ms, 250, '4 samples per simulated second is a 250 ms interval');
  eq(Object.keys(fields).length, 32, 'field count');
  return `${Object.keys(fields).length} keys, all accepted; dropped ${dropped.join(', ')}`;
});

check('config.ts BASE matches scenarios/base.txt key for key', () => {
  const file = api.parseScenarioText(repoFile('scenarios/base.txt'));
  const { fields } = api.scenarioConfigToWire(BASE);
  const missing = Object.keys(file).filter((k) => !(k in fields));
  eq(missing, [], 'keys in base.txt that BASE does not encode');
  const differing: string[] = [];
  for (const [k, v] of Object.entries(file)) {
    const ours = fields[k];
    const agree = typeof ours === 'number' ? Number(v) === ours : String(ours) === v;
    if (!agree) differing.push(`${k}: file ${v}, BASE ${String(ours)}`);
  }
  eq(differing, [], 'values that disagree');
  ok(Object.keys(file).length >= 30, `base.txt has only ${Object.keys(file).length} keys; was it read?`);
  return `${Object.keys(file).length} keys in base.txt, every one equal (replicas ${file.replicas}, max_batch ${file.max_batch}, kv ${file.kv_capacity_tokens}, rps ${file.arrival_rps})`;
});

check('the two engine name mismatches are translated, and prefix affinity is refused', () => {
  const leastKv = cloneConfig(BASE);
  leastKv.routing = { ...leastKv.routing, kind: 'least_kv_tokens' };
  eq(api.scenarioConfigToWire(leastKv).fields.routing, 'least_queue_tokens', 'least_kv_tokens maps to the engine name');
  const p2c = cloneConfig(BASE);
  p2c.routing = { ...p2c.routing, kind: 'power_of_two_choices' };
  eq(api.scenarioConfigToWire(p2c).fields.routing, 'p2c', 'power_of_two_choices is spelled as scenarios/route_p2c.txt spells it');
  const affinity = cloneConfig(BASE);
  affinity.routing = { ...affinity.routing, kind: 'prefix_affinity' };
  const enc = api.scenarioConfigToWire(affinity);
  eq('routing' in enc.fields, false, 'no routing key is sent for a policy the engine lacks');
  eq(enc.dropped.includes('routing.kind'), true, 'routing.kind is reported dropped');
  eq(api.unacceptedKeys(enc.fields), [], 'still only accepted keys');
  return 'least_kv_tokens -> least_queue_tokens; power_of_two_choices -> p2c; prefix_affinity dropped, not sent';
});

check('workload and policy updates carry only their own keys, as string overrides', () => {
  const w = api.workloadToWire(BASE);
  eq(api.unacceptedKeys(w.fields, api.WORKLOAD_KEYS), [], 'workload keys outside WORKLOAD_KEYS');
  eq(w.fields.arrival_rps, 70, 'arrival_rps');
  eq(w.dropped, ['workload.perturbation', 'workload.perturbAmplitude', 'workload.perturbFrequencyHz'], 'workload dropped');
  const p = api.policiesToWire(BASE);
  eq(api.unacceptedKeys(p.fields, api.POLICY_KEYS), [], 'policy keys outside POLICY_KEYS');
  eq(p.fields, { routing: 'round_robin', p2c_choices: 2, probe_live: false }, 'policy fields');
  eq(api.toOverrides(p.fields), { routing: 'round_robin', p2c_choices: '2', probe_live: 'false' }, 'overrides are strings, like --set');
  return `workload ${Object.keys(w.fields).length} keys, policies ${Object.keys(p.fields).length} keys`;
});

check('StartRunRequest.scenario is {text, overrides}, and the text round-trips through the parser', () => {
  const { fields } = api.scenarioConfigToWire(BASE, { max_attempts: 3 });
  const env = api.scenarioEnvelope(fields, { arrival_rps: '90' });
  eq(Object.keys(env), ['text', 'overrides'], 'envelope shape');
  eq(env.overrides, { arrival_rps: '90' }, 'overrides pass through');
  ok(env.text.startsWith('name = base\nseed = 20260906\nduration_s = 120\n'), `text starts with the header: ${JSON.stringify(env.text.slice(0, 50))}`);
  ok(/^probe_live = false$/m.test(env.text), 'probe_live is the literal Scenario::parse compares against');
  ok(/^step_per_seq_ms = 0$/m.test(env.text), 'a zero is a number the parser reads');
  eq(api.parseScenarioText(env.text), api.toOverrides(fields), 'text parses back to the same key set');
  eq(api.unacceptedKeys(api.parseScenarioText(env.text)), [], 'only accepted keys in the text');
  eq(api.scenarioEnvelope({ seed: 1, name: 'x' }).text, 'name = x\nseed = 1\n', 'lines follow SCENARIO_KEYS order regardless of insertion order');
  eq(api.scenarioEnvelope({}).overrides, {}, 'overrides default to empty');
  return `${env.text.split('\n').length - 1} lines of key = value, overrides {arrival_rps: "90"}`;
});

check('every wire field name this client reads or writes is a field in proto/lbsim/v1', () => {
  const protoDir = new URL('../../../proto/lbsim/v1/', import.meta.url);
  const names = new Set<string>();
  for (const f of fs.readdirSync(protoDir)) {
    if (!f.endsWith('.proto')) continue;
    const text = fs.readFileSync(new URL(f, protoDir), 'utf8');
    // Anchored on a line start, `{` or `;`, because several messages put two fields on one line.
    for (const m of text.matchAll(/(?<=^|[{;])\s*(?:repeated\s+|optional\s+)?(?:map<[^>]+>|[\w.]+)\s+(\w+)\s*=\s*\d+\s*;/gm)) names.add(m[1]);
  }
  ok(names.has('lease_expires_at_wall_ns') && names.has('sim_time_unix_ns') && names.size > 100, `proto scan looks wrong: ${names.size} names`);
  // WIRE.md's documented deviations: the scenario envelope and the error body.
  const documented = new Set(['text', 'overrides', 'error']);
  const used = [...api.WIRE_FIELDS].sort();
  const unknown = used.filter((n) => !names.has(n) && !documented.has(n));
  eq(unknown, [], 'wire names not in any proto');
  ok(used.length >= 80, `only ${used.length} wire names recorded; were the decoders exercised?`);
  return `${used.length} wire names checked against ${names.size} proto fields; deviations: ${[...documented].join(', ')}`;
});

// ---------------------------------------------------------------------------
// mode
// ---------------------------------------------------------------------------

check('no server is configured by default; the probe decides', () => {
  const m = mode.serverModeFrom(undefined, '', '');
  eq(m, { enabled: false, baseUrl: '', source: 'default' }, 'default mode');
  eq(mode.modeBanner(m), mode.SERVER_BANNER, 'a same-origin server carries no base in its banner');
  eq(mode.serverMode().enabled, false, 'serverMode() outside a browser configures nothing');
  return 'default configures no server; the probe decides';
});

check('server mode turns on from the URL, then localStorage, then the environment', () => {
  eq(mode.serverModeFrom(undefined, '?server=1', ''), { enabled: true, baseUrl: '', source: 'query' }, '?server=1');
  eq(mode.serverModeFrom(undefined, '', '#/dashboard?server=http://localhost:8099'), { enabled: true, baseUrl: 'http://localhost:8099', source: 'query' }, 'hash-route query');
  eq(mode.serverModeFrom(undefined, '', '', '1'), { enabled: true, baseUrl: '', source: 'storage' }, 'localStorage lbsim.server=1');
  eq(mode.serverModeFrom(undefined, '', '', 'http://localhost:8099/'), { enabled: true, baseUrl: 'http://localhost:8099', source: 'storage' }, 'localStorage base URL');
  eq(mode.serverModeFrom('1', '', ''), { enabled: true, baseUrl: '', source: 'env' }, 'VITE_LBSIM_SERVER=1');
  eq(mode.serverModeFrom('http://localhost:8099/', '', ''), { enabled: true, baseUrl: 'http://localhost:8099', source: 'env' }, 'explicit base from env');
  eq(mode.serverModeFrom('1', '?server=0', '1'), { enabled: false, baseUrl: '', source: 'query' }, '?server=0 turns the server off over everything');
  eq(mode.serverModeFrom('1', '', '', '0'), { enabled: false, baseUrl: '', source: 'storage' }, 'localStorage 0 turns the server off over the env');
  eq(mode.serverModeFrom('http://a', '?server=http://b', 'http://c'), { enabled: true, baseUrl: 'http://b', source: 'query' }, 'query beats storage beats env');
  eq(mode.modeBanner(mode.serverModeFrom(undefined, '?server=http://localhost:8099', '')), `${mode.SERVER_BANNER} at http://localhost:8099`, 'server banner names the base');
  eq(mode.STORAGE_KEY, 'lbsim.server', 'storage key');
  return '?server=1, ?server=<url>, hash query, localStorage, env, ?server=0';
});

// ---------------------------------------------------------------------------

console.log(`${cases} cases, ${cases - failures} passed, ${failures} failed`);
if (failures > 0) throw new Error(`${failures} of ${cases} transport self-test cases failed`);
