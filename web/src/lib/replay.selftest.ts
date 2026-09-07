// Self-test for the replay source and the Frame adapter. No network, no browser, no framework.
//
//   cd web && node --experimental-strip-types src/lib/replay.selftest.ts
//
// Same shape as api.selftest.ts: one line per case, a summary line, a throw when anything failed.
// The fixtures are two lines of a real `fleet.jsonl` from `sim-run export --demos` (the p2c run,
// at 0.25 s and at 25 s), the matching index entries and the run's `scenario.txt`, so the numbers
// checked here are the engine's, not invented ones.
//
// Why the resolve hook below: adapter.ts and replay.ts import `./api` and `./hist` the way every
// other module does, extensionless, which Vite resolves and Node's ESM loader does not. The hook
// tries `<specifier>.ts` for a bare relative specifier and otherwise defers, so the modules under
// test stay written like the rest of the app.

import type { RunHandle } from './useRun';
import type { ReplayRunHandle } from './useRun';

type Assert<T extends true> = T;
/** The replay handle is a `RunHandle`: every panel that takes one takes it. */
export type ReplayHandleIsRunHandle = Assert<ReplayRunHandle extends RunHandle ? true : false>;

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

const api = await load<typeof import('./api')>('api');
const adapter = await load<typeof import('./adapter')>('adapter');
const replay = await load<typeof import('./replay')>('replay');
const hist = await load<typeof import('./hist')>('hist');
const engine = await load<typeof import('./engine')>('engine');
const mode = await load<typeof import('./mode')>('mode');

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

function eq(actual: unknown, expected: unknown, what: string): void {
  const same =
    typeof actual === 'number' && typeof expected === 'number' && Number.isNaN(actual) && Number.isNaN(expected)
      ? true
      : Array.isArray(actual) && Array.isArray(expected)
        ? actual.length === expected.length && actual.every((x, i) => x === expected[i])
        : actual === expected;
  if (!same) throw new Error(`${what}: expected ${show(expected)}, got ${show(actual)}`);
}

function ok(cond: boolean, what: string): void {
  if (!cond) throw new Error(what);
}

function near(actual: number, expected: number, tol: number, what: string): void {
  if (!(Math.abs(actual - expected) <= tol)) throw new Error(`${what}: expected ${expected} ± ${tol}, got ${actual}`);
}

/** Within one hist.ts bucket: 4 per octave, so a ratio of 2^(1/4). */
const BUCKET_RATIO = Math.pow(2, 1 / 4);
function withinBucket(actual: number, expected: number, what: string): void {
  const r = actual / expected;
  if (!(r >= 1 / BUCKET_RATIO && r <= BUCKET_RATIO)) throw new Error(`${what}: ${actual} is not within one bucket of ${expected}`);
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
// fixtures: the p2c demo run as exported
// ---------------------------------------------------------------------------

const ORIGIN = '1767225600000000000';

/** fleet.jsonl line 1: the first sample window, before any request finished. */
const ROW_EMPTY = `{"subscription_id":"export","sim_time_unix_ns":"1767225600250000000","realtime_factor":0,"row":{"target":{"scope":"SCOPE_FLEET"},"values":{"20":0.0004613138686131387,"22":17,"23":0,"40":70,"41":0,"42":0,"43":0,"44":0,"45":0,"60":32,"64":1.2635690579259151},"distributions":{}},"final":false}`;

/** fleet.jsonl line 100: 25 s in, sixteen completions in the window. */
const ROW_25S = `{"subscription_id":"export","sim_time_unix_ns":"1767225625000000000","realtime_factor":0,"row":{"target":{"scope":"SCOPE_FLEET"},"values":{"20":0.04451532846715328,"22":315,"23":1,"40":70,"41":64,"42":64,"43":0,"44":12940,"45":12940,"60":32,"64":0.29298860493540535,"66":1},"distributions":{"1":{"count":"16","mean":236793156.3125,"min":28028807,"max":1628442058,"percentile":[50,90,99,99.9],"value":[51747847,568413189,1628442058,1628442058],"from_merged_histogram":false},"2":{"count":"16","mean":47268471.875,"min":43820167,"max":50123641,"percentile":[50,90,99,99.9],"value":[47063609,49206676,50123641,50123641],"from_merged_histogram":false},"3":{"count":"16","mean":3193849112.0625,"min":254467178,"max":8653270206,"percentile":[50,90,99,99.9],"value":[1958531795,6334394106,8653270206,8653270206],"from_merged_histogram":false},"4":{"count":"16","mean":8374596,"min":231221,"max":29873566,"percentile":[50,90,99,99.9],"value":[6294777,19754572,29873566,29873566],"from_merged_histogram":false}}},"final":false}`;

const INDEX = `[
{"run_id":"1-routing/p2c","name":"p2c","routing":"p2c(d=2)","scenario_file":"scenarios/route_p2c.txt","sim_start_unix_ns":"${ORIGIN}","sim_end_unix_ns":"1767225720000000000","sample_interval_ms":250,"replicas":32},
{"run_id":"1-routing/round-robin","name":"round_robin","routing":"round_robin","scenario_file":"scenarios/route_round_robin.txt","sim_start_unix_ns":"${ORIGIN}","sim_end_unix_ns":"1767225720000000000","sample_interval_ms":250,"replicas":32},
{"run_id":"6-retry/no-retries","name":"no_retries","routing":"p2c(d=2)","scenario_file":"scenarios/retry_none.txt","sim_start_unix_ns":"${ORIGIN}","sim_end_unix_ns":"1767225840000000000","sample_interval_ms":250,"replicas":32}
]
`;

const STATUS = `{"run_id":"1-routing/p2c","state":"STATE_COMPLETE","sim_time_unix_ns":"1767225720000000000","sim_end_unix_ns":"1767225720000000000","realtime_factor":0,"error":""}`;

const RESULT = `{"run_id":"1-routing/p2c","seed":"20260906","event_count":"296643","state_checksum":"13155901060006219336","overall":{"values":{"40":70,"42":67.08571428571429,"44":18743.609523809522,"45":18126.619047619046,"60":32,"64":0.3291215724658393,"66":0.9696410838416797},"distributions":{"1":{"count":"7044","mean":294145053.42490065,"min":12418401,"max":13690102998,"percentile":[50,90,99,99.9],"value":[71303168,721420288,3858759680,7247757312],"from_merged_histogram":true}},"outcome_counts":{"1":"6835","2":"209","3":"0","4":"0","5":"5"},"declared_rated_capacity_rps":234.76020039976,"metastable_collapse":false}}`;

const SCENARIO = `name = p2c
seed = 20260906
duration_s = 120
warmup_s = 15
replicas = 32
max_batch = 256
step_base_ms = 10.2
step_per_seq_ms = 0
step_per_kv_ktoken_ms = 0.0175
kv_capacity_tokens = 1370000
prefill_tokens_per_s = 28286
step_token_budget = 1024
max_queue = 400
arrival_rps = 70
prompt_mean = 1200
prompt_cv = 1.2
output_mean = 300
output_cv = 1.5
long_probability = 0.08
long_prompt_mean = 24000
long_output_mean = 400
load_step_at_s = -1
load_step_factor = 1
load_step_until_s = -1
routing = p2c
p2c_choices = 2
probe_live = false
admission = accept_all
admission_headroom = 0.5
fair_share_burst = 2
tenants = 1
tenant_weights =
tenant_demand =
telemetry_interval_ms = 1000
telemetry_delay_ms = 200
client_timeout_s = 60
max_attempts = 1
retry_budget_fraction = 1
retry_backoff_s = 0.5
ttft_slo_ms = 2000
itl_slo_ms = 80
e2e_slo_s = 60
sample_interval_ms = 250
`;

const rowAt = (line: string, t: bigint) => line.replace(/"sim_time_unix_ns":"\d+"/, `"sim_time_unix_ns":"${t}"`);
const origin = BigInt(ORIGIN);
const update = (line: string) => api.decodeSubscriptionUpdate(JSON.parse(line));

/** A stub fetch over a URL -> body table; anything else is a 404 with the dev server's HTML shell. */
function stubFetch(files: Record<string, string>, shellOn404 = false) {
  const calls: string[] = [];
  const f: typeof fetch = async (input) => {
    const url = String(input);
    calls.push(url);
    const body = files[url];
    if (body === undefined) {
      return new Response(shellOn404 ? '<!doctype html><html></html>' : '{"error":"not found"}', { status: shellOn404 ? 200 : 404 });
    }
    return new Response(body, { status: 200 });
  };
  return { f, calls };
}

// ---------------------------------------------------------------------------
// the adapter
// ---------------------------------------------------------------------------

check('a fleet row 25 s in becomes a Frame with the engine numbers', () => {
  const f = adapter.frameFromUpdate(update(ROW_25S), origin, 99);
  eq(f.tick, 99, 'tick');
  eq(f.simS, 25, 'simS relative to the index origin');
  eq(f.simTimeUnixNs, 1767225625000000000n, 'absolute instant kept as bigint');
  eq(f.offeredRps, 70, 'offered');
  eq(f.completedRps, 64, 'completed');
  eq(f.admittedRps, 64, 'admitted (= completed until the engine records admissions)');
  eq(f.rejectedRps, 0, 'rejected');
  eq(f.outputTokensPerS, 12940, 'output tokens/s');
  eq(f.goodputTokensPerS, 12940, 'goodput tokens/s');
  eq(f.sloAttainment, 1, 'slo attainment');
  eq(f.kvUtilization, 0.04451532846715328, 'kv utilization is the wire fraction, untouched');
  eq(f.tierUtilization.hbm, 0.04451532846715328, 'hbm is the kv utilization');
  eq(f.loadImbalanceCv, 0.29298860493540535, 'imbalance cv');
  eq(f.readyReplicas, 32, 'ready replicas');
  eq(f.queuedSeqs, 1, 'queued');
  eq(f.runningSeqs, 315, 'running');
  return 'offered 70, completed 64, kv 4.45 %, cv 0.293, 32 ready, 12 940 tok/s';
});

check('fields the engine does not simulate are NaN or empty, never a number that looks measured', () => {
  const f = adapter.frameFromUpdate(update(ROW_25S), origin, 0);
  eq(f.preemptionsPerS, NaN, 'preemptions');
  eq(f.wastedGpuFraction, NaN, 'wasted gpu');
  eq(f.prefixHitRate, NaN, 'prefix hit rate');
  eq(f.tierUtilization.dram, NaN, 'dram tier');
  eq(f.tierBandwidth.ssd, NaN, 'ssd bandwidth');
  eq(f.warmingReplicas, 0, 'warming: the fleet is static, so zero is true');
  eq(f.drainingReplicas, 0, 'draining');
  eq(f.ejectedReplicas, 0, 'ejected');
  eq(f.replicas.length, 0, 'no replica rows given, none invented');
  eq(f.events.length, 0, 'no failure injection, no events');
  return 'preemptions, wasted, prefix, tiers NaN; lifecycle counts 0; replicas []';
});

check('the wire p99 is kept exactly and the rebuilt histogram lands within one bucket of it', () => {
  const f = adapter.frameFromUpdate(update(ROW_25S), origin, 0);
  eq(adapter.latencyMs(f, 'ttft', 99), 1628.442058, 'exact ttft p99, ns -> ms');
  eq(adapter.latencyMs(f, 'ttft', 50), 51.747847, 'exact ttft p50');
  eq(adapter.latencyMs(f, 'ttft', 75), NaN, 'a percentile the wire did not carry is NaN, not interpolated');
  eq(f.exact.ttft?.count, 16, 'exact count');
  eq(f.ttft.count, 16, 'histogram count');
  eq(f.ttft.min, 28.028807, 'histogram min');
  eq(f.ttft.max, 1628.442058, 'histogram max');
  near(hist.histMean(f.ttft), 236.7931563125, 1e-9, 'histogram mean is the wire mean');
  withinBucket(hist.quantile(f.ttft, 99), 1628.442058, 'quantile(99)');
  withinBucket(hist.quantile(f.ttft, 90), 568.413189, 'quantile(90)');
  withinBucket(hist.quantile(f.ttft, 50), 51.747847, 'quantile(50)');
  withinBucket(hist.quantile(f.itl, 50), 47.063609, 'itl quantile(50)');
  withinBucket(hist.quantile(f.e2e, 99), 8653.270206, 'e2e quantile(99)');
  withinBucket(hist.quantile(f.queueWait, 90), 19.754572, 'queue-wait quantile(90)');
  return `exact p99 1628.44 ms; bucketed ${hist.quantile(f.ttft, 99).toFixed(0)} ms`;
});

check('a window with no completions has absent latency, NaN rather than zero', () => {
  const f = adapter.frameFromUpdate(update(ROW_EMPTY), origin, 0);
  eq(f.simS, 0.25, 'simS');
  eq(f.exact.ttft, undefined, 'no exact ttft');
  eq(f.exact.e2e, undefined, 'no exact e2e');
  eq(f.ttft.count, 0, 'empty histogram');
  eq(f.queueWait.count, 0, 'empty queue-wait histogram');
  eq(adapter.latencyMs(f, 'ttft', 99), NaN, 'latencyMs is NaN');
  eq(adapter.latencyMs(f, 'itl', 50), NaN, 'latencyMs itl is NaN');
  eq(f.sloAttainment, NaN, 'attainment omitted on the wire is NaN');
  eq(f.completedRps, 0, 'completed 0 is real: nothing finished');
  eq(f.offeredRps, 70, 'offered is still known');
  return 'ttft/itl/e2e/queue-wait absent, attainment NaN, completed 0';
});

check("the adapter's bucket edges are hist.ts's", () => {
  // 1e5 ms is inside the top bucket (2^17 ms); anything above is clamped into it by both sides.
  for (const v of [0.7, 1, 20, 1628.442058, 1e5]) {
    const h = hist.newHistogram();
    hist.record(h, v);
    const [b] = hist.bins(h);
    ok(b.lo <= v && v < b.hi, `${v} inside its bin`);
    const k = Math.round((Math.log2(b.lo) + 1) * 4);
    near(adapter.bucketLow(k), b.lo, 1e-9, `bucketLow(${k}) for ${v}`);
    near(adapter.bucketLow(k + 1), b.hi, 1e-9, `bucketLow(${k + 1}) for ${v}`);
  }
  return '4 per octave from 0.5 ms, edges agree';
});

check('a synthetic distribution round-trips: quantiles at every knot, CDF at the knots, moments exact', () => {
  const ms = (x: number) => x * 1e6;
  const d: import('./api').WireDistribution = {
    count: 1000n,
    mean: ms(220),
    min: ms(10),
    max: ms(5000),
    percentile: [50, 90, 99, 99.9],
    value: [ms(100), ms(400), ms(1500), ms(3000)],
    fromMergedHistogram: false,
  };
  const h = adapter.histogramFromDistribution(d);
  eq(h.count, 1000, 'count');
  near(hist.histMean(h), 220, 1e-9, 'mean');
  eq(h.min, 10, 'min');
  eq(h.max, 5000, 'max');
  for (const [p, v] of [[50, 100], [90, 400], [99, 1500], [99.9, 3000]] as const) withinBucket(hist.quantile(h, p), v, `quantile(${p})`);
  near(hist.fractionBelow(h, 100), 0.5, 0.05, 'fractionBelow at p50');
  near(hist.fractionBelow(h, 400), 0.9, 0.05, 'fractionBelow at p90');
  near(hist.fractionBelow(h, 1500), 0.99, 0.01, 'fractionBelow at p99');
  near(hist.fractionBelow(h, 6000), 1, 1e-6, 'everything below max');
  eq(hist.fractionBelow(h, 5), 0, 'nothing below min');
  let mass = 0;
  for (const b of hist.bins(h)) mass += b.count;
  // hist.ts keeps counts in a Float32Array, hence the tolerance.
  near(mass, 1000, 1e-3, 'bucket masses sum to the count');
  return 'quantiles within a bucket, CDF within 0.05, moments exact, mass conserved';
});

check('a single-point distribution is one bucket, and out-of-order percentiles are sorted', () => {
  const one = adapter.histogramFromDistribution({ count: 3n, mean: 20e6, min: 20e6, max: 20e6, percentile: [50, 99], value: [20e6, 20e6], fromMergedHistogram: false });
  eq(one.count, 3, 'count');
  eq(hist.bins(one).length, 1, 'one bucket');
  withinBucket(hist.quantile(one, 50), 20, 'quantile(50)');
  eq(hist.fractionBelow(one, 30), 1, 'all below 30 ms');
  const shuffled = adapter.histogramFromDistribution({ count: 100n, mean: 50e6, min: 10e6, max: 200e6, percentile: [99, 50], value: [150e6, 40e6], fromMergedHistogram: false });
  withinBucket(hist.quantile(shuffled, 50), 40, 'quantile(50) after sorting');
  withinBucket(hist.quantile(shuffled, 99), 150, 'quantile(99) after sorting');
  const empty = adapter.histogramFromDistribution({ count: 0n, mean: 0, min: 0, max: 0, percentile: [], value: [], fromMergedHistogram: false });
  eq(empty.count, 0, 'count 0 stays empty');
  return 'one bucket for one point; percentiles sorted; count 0 empty';
});

check('frames merge over a window the way the panels merge them', () => {
  const a = adapter.frameFromUpdate(update(ROW_25S), origin, 0);
  const b = adapter.frameFromUpdate(update(rowAt(ROW_25S, 1767225625250000000n)), origin, 1);
  const e = adapter.frameFromUpdate(update(ROW_EMPTY), origin, 2);
  const merged = engine.mergeWindow([a, b, e], (f) => f.ttft);
  eq(merged.count, 32, 'two windows of 16, plus an empty one');
  withinBucket(hist.quantile(merged, 99), 1628.442058, 'merged quantile(99)');
  return '16 + 16 + 0 = 32, p99 held';
});

// ---------------------------------------------------------------------------
// the index and the loader
// ---------------------------------------------------------------------------

check('runs/index.json decodes: uint64 instants as bigint, groups from the run id', () => {
  const runs = replay.decodeRunIndex(JSON.parse(INDEX));
  eq(runs.length, 3, 'entries');
  eq(runs[0].runId, '1-routing/p2c', 'run id');
  eq(runs[0].group, '1-routing', 'group');
  eq(runs[0].label, 'p2c', 'label');
  eq(runs[0].routing, 'p2c(d=2)', 'routing label');
  eq(runs[0].scenarioFile, 'scenarios/route_p2c.txt', 'scenario file');
  eq(runs[0].simStartUnixNs, origin, 'start');
  eq(runs[0].simEndUnixNs, 1767225720000000000n, 'end');
  eq(runs[0].sampleIntervalMs, 250, 'sample interval');
  eq(runs[0].replicas, 32, 'replicas');
  eq(replay.runDurationS(runs[0]), 120, 'duration, warm-up included');
  eq(replay.runDurationS(runs[2]), 240, 'the retry runs are 240 s');
  const groups = replay.groupRuns(runs);
  eq(groups.map((g) => g.group), ['1-routing', '6-retry'], 'groups in index order');
  eq(groups[0].runs.length, 2, 'two routing runs');
  ok(/expected an array/.test(throws(() => replay.decodeRunIndex({}), 'object index')), 'non-array refused');
  ok(/run_id is empty/.test(throws(() => replay.decodeRunIndex([{ name: 'x' }]), 'missing run_id')), 'missing run_id refused');
  return '3 runs, 2 groups, 120 s and 240 s';
});

await checkAsync('loadRun fetches the four documents relative to runs/ and decodes them with the transport', async () => {
  const base = '/runs/';
  const fleet = [ROW_EMPTY, rowAt(ROW_25S, 1767225625000000000n)].join('\n') + '\n';
  const { f, calls } = stubFetch({
    [`${base}1-routing/p2c/status.json`]: STATUS,
    [`${base}1-routing/p2c/fleet.jsonl`]: fleet,
    [`${base}1-routing/p2c/result.json`]: RESULT,
    [`${base}1-routing/p2c/scenario.txt`]: SCENARIO,
  });
  const entry = replay.decodeRunIndex(JSON.parse(INDEX))[0];
  const run = await replay.loadRun(entry, f, base);
  eq(calls.length, 5, 'five GETs: the four documents and the optional replicas.jsonl');
  eq(run.frames[1].replicas.length, 0, 'an export without replicas.jsonl has no replica breakdown');
  eq(run.status.state, 'STATE_COMPLETE', 'status');
  eq(run.status.simTimeUnixNs, 1767225720000000000n, 'status instant');
  eq(run.result.seed, 20260906n, 'result seed');
  eq(run.result.stateChecksum, 13155901060006219336n, 'checksum survives as bigint');
  eq(run.result.overall.values.METRIC_OFFERED_RPS, 70, 'scorecard offered');
  eq(run.result.overall.distributions.METRIC_TTFT?.count, 7044n, 'scorecard ttft count');
  eq(run.frames.length, 2, 'frames');
  eq(run.frames[0].simS, 0.25, 'first frame');
  eq(run.frames[1].simS, 25, 'second frame');
  eq(run.frames[1].tick, 1, 'tick is the line index');
  eq(run.config.name, 'p2c', 'config name');
  eq(run.config.routing.kind, 'power_of_two_choices', 'routing kind mapped back');
  eq(run.config.workload.arrivalRps, 70, 'arrival');
  eq(run.config.slo.ttftMs, 2000, 'ttft slo');
  eq(run.config.samplesPerSimSecond, 4, 'sample rate from 250 ms');
  eq(run.config.durationS, 120, 'duration');
  eq(run.config.warmupS, 15, 'warm-up');
  eq(run.config.extra.admission, 'accept_all', 'admission rides in extra rather than being reported');
  eq(run.unmapped, [], `nothing the engine accepts is unmapped: ${run.unmapped.join(', ')}`);
  ok(!run.unmapped.some((u) => u.startsWith('routing')), 'routing was mapped');
  return `4 documents, 2 frames, config ${run.config.routing.kind} at ${run.config.workload.arrivalRps} rps`;
});

check('replicas.jsonl fills Frame.replicas: ids and values from the wire, NaN where it is silent', () => {
  const rep = (t: bigint, id: number, queued: number, running: number, kv: number, util: number, step: number, final = false) =>
    `{"subscription_id":"export","sim_time_unix_ns":"${t}","realtime_factor":0,"row":{"target":{"scope":"SCOPE_REPLICA","replica_id":"${id}"},"values":{"8":${step},"20":${util},"21":${kv},"22":${running},"23":${queued}},"distributions":{}},"final":${final}}`;
  const t0 = 1767225600250000000n;
  const t1 = 1767225625000000000n;
  const text = [
    rep(t0, 0, 0, 5, 1200, 0.01, 0.031),
    rep(t0, 1, 0, 6, 1500, 0.0125, 0.032),
    rep(t0, 2, 0, 6, 1400, 0.0117, 0.03),
    rep(t1, 0, 1, 100, 60000, 0.5, 0.045),
    rep(t1, 1, 0, 105, 66000, 0.55, 0.046),
    rep(t1, 2, 0, 110, 72000, 0.6, 0.047, true),
  ].join('\n') + '\n';
  const groups = replay.parseReplicasJsonl(text);
  eq(groups.size, 2, 'two instants');
  eq(groups.get(t1)?.length, 3, 'three replicas at 25 s');
  const fleet = [ROW_EMPTY, rowAt(ROW_25S, t1)].join('\n') + '\n';
  const frames = replay.parseFleetJsonl(fleet, origin, groups);
  eq(frames.length, 2, 'frames');
  eq(frames[0].replicas.map((r) => r.id), [0, 1, 2], 'ids at 0.25 s');
  eq(frames[1].replicas.map((r) => r.id), [0, 1, 2], 'ids at 25 s');
  const r1 = frames[1].replicas[1];
  eq(r1.present, true, 'present');
  eq(r1.state, 'READY', 'ready: no lifecycle in the engine');
  eq(r1.weight, 1, 'weight');
  eq(r1.queuedSeqs, 0, 'queued');
  eq(r1.runningSeqs, 105, 'running');
  eq(r1.batchSize, 105, 'batch is the running count');
  eq(r1.kvTokensResident, 66000, 'kv tokens');
  eq(r1.kvUtilization, 0.55, 'kv utilization is the wire fraction, untouched');
  near(r1.stepTimeMs, 46, 1e-9, 'step time, s -> ms');
  eq(frames[1].replicas[0].queuedSeqs, 1, 'replica 0 queued');
  eq(frames[0].replicas[2].stepTimeMs, 30, 'replica 2 step at 0.25 s');
  for (const k of ['queueWaitMs', 'ttftMeanMs', 'itlMeanMs', 'prefixHitRate', 'admittedRps', 'completedRps', 'preemptionsPerS', 'trueSpeedMultiplier', 'telemetryStalenessMs'] as const) {
    eq(r1[k], NaN, `${k} is NaN, never a zero that looks measured`);
  }
  const lone = replay.parseFleetJsonl(fleet, origin, new Map([[t0, groups.get(t0) ?? []]]));
  eq(lone[1].replicas.length, 0, 'an instant with no replica rows gets none, not the previous instant\'s');
  return '3 replicas x 2 samples; ids 0..2; wire values through, the rest NaN';
});

await checkAsync('loadRun takes replicas.jsonl when it is there, and a dev server\'s HTML shell as absent', async () => {
  const base = '/runs/';
  const t1 = 1767225625000000000n;
  const rep = (id: number) =>
    `{"subscription_id":"export","sim_time_unix_ns":"${t1}","realtime_factor":0,"row":{"target":{"scope":"SCOPE_REPLICA","replica_id":"${id}"},"values":{"8":0.04,"20":0.5,"21":60000,"22":100,"23":0},"distributions":{}},"final":false}`;
  const docs = {
    [`${base}1-routing/p2c/status.json`]: STATUS,
    [`${base}1-routing/p2c/fleet.jsonl`]: rowAt(ROW_25S, t1) + '\n',
    [`${base}1-routing/p2c/result.json`]: RESULT,
    [`${base}1-routing/p2c/scenario.txt`]: SCENARIO,
  };
  const entry = replay.decodeRunIndex(JSON.parse(INDEX))[0];
  const withRows = await replay.loadRun(entry, stubFetch({ ...docs, [`${base}1-routing/p2c/replicas.jsonl`]: [rep(0), rep(1)].join('\n') }).f, base);
  eq(withRows.frames[0].replicas.map((r) => r.id), [0, 1], 'replica ids from replicas.jsonl');
  eq(withRows.frames[0].replicas[1].kvTokensResident, 60000, 'replica value');
  const shell = await replay.loadRun(entry, stubFetch(docs, true).f, base);
  eq(shell.frames[0].replicas.length, 0, 'the HTML shell is an absent file');
  return 'present: 2 replicas; shell: none';
});

await checkAsync('probeRunIndex answers null for a 404, for a dev server HTML shell, and for an empty index', async () => {
  eq(await replay.probeRunIndex(stubFetch({}).f, '/runs/'), null, '404');
  eq(await replay.probeRunIndex(stubFetch({}, true).f, '/runs/'), null, 'HTML shell with HTTP 200');
  eq(await replay.probeRunIndex(stubFetch({ '/runs/index.json': '[]' }).f, '/runs/'), null, 'empty index');
  const runs = await replay.probeRunIndex(stubFetch({ '/runs/index.json': INDEX }).f, '/runs/');
  eq(runs?.length, 3, 'a real index');
  const failing: typeof fetch = async () => {
    throw new TypeError('network down');
  };
  eq(await replay.probeRunIndex(failing, '/runs/'), null, 'network error');
  return 'null, null, null, 3 runs, null';
});

check('scenario.txt -> ScenarioConfig is the inverse of scenarioConfigToWire, key for key', () => {
  const { config, unmapped } = replay.configFromScenarioText(SCENARIO);
  const text = api.parseScenarioText(SCENARIO);
  const back = api.scenarioConfigToWire(config).fields;
  let checked = 0;
  for (const [k, v] of Object.entries(back)) {
    if (text[k] === undefined) continue;
    // Numbers compare as numbers (`0` against `0.0`); `name`, `routing` and `probe_live` as text.
    const numeric = Number.isFinite(Number(text[k])) && typeof v === 'number';
    const same = numeric ? Number(text[k]) === v : String(v) === text[k];
    if (!same) throw new Error(`${k}: text ${text[k]}, round trip ${String(v)}`);
    checked++;
  }
  const unmappedKeys = new Set(unmapped.map((u) => u.split(' = ')[0]));
  for (const k of Object.keys(text)) ok(back[k] !== undefined || unmappedKeys.has(k), `${k} neither mapped nor reported`);
  ok(checked >= 30, `only ${checked} keys round-tripped`);
  ok(/routing = deadline_aware/.test(replay.configFromScenarioText('routing = deadline_aware\n').unmapped.join()), 'an unknown routing is reported, not guessed');
  return `${checked} keys round-trip; ${unmappedKeys.size} reported unmapped`;
});

// ---------------------------------------------------------------------------
// the frame source and the cursor
// ---------------------------------------------------------------------------

check('ReplayEngine answers frameAt and window like the mock, over frames that already exist', () => {
  const lines = Array.from({ length: 8 }, (_, i) => rowAt(ROW_25S, origin + BigInt((i + 1) * 250_000_000)));
  const frames = replay.parseFleetJsonl(lines.join('\n') + '\n', origin);
  const { config } = replay.configFromScenarioText(SCENARIO);
  const e = new replay.ReplayEngine(frames, config);
  eq(e.frames.length, 8, 'frames');
  near(e.dtS, 0.25, 1e-12, 'sample interval recovered from the frames');
  eq(e.recordedToS, 120, 'the whole run is recorded');
  eq(e.frameAt(0.3)?.tick, 0, 'frameAt(0.3) is the 0.25 s frame');
  eq(e.frameAt(1.0)?.tick, 3, 'frameAt(1.0) is the 1.0 s frame');
  eq(e.frameAt(-1)?.tick, 0, 'before the first sample: the first frame');
  eq(e.frameAt(1e9)?.tick, 7, 'past the end: the last frame');
  eq(e.window(0, 2).length, 8, 'window over everything at the native rate');
  eq(e.window(0.4, 1.1).map((f) => f.tick), [1, 2, 3], 'window is [0.5, 0.75, 1.0]: inclusive, 1.25 is out');
  eq(e.window(0.5, 1.0).map((f) => f.tick), [1, 2, 3], 'inclusive at both ends exactly');
  eq(e.window(100, 200).length, 0, 'window past the recording is empty');
  e.config = { ...config, samplesPerSimSecond: 2 };
  eq(e.window(0, 2).map((f) => f.tick), [0, 2, 4, 6, 7], 'half rate: every other frame plus the last');
  eq(e.eventsUpTo(120).length, 0, 'no events');
  return '8 frames at 4/s; window and frameAt clamp; decimation matches the mock';
});

check('the replay cursor clamps to the recording: step stops at the end, scrub never leaves it', () => {
  eq(replay.clampCursor(-5, 120), 0, 'below zero');
  eq(replay.clampCursor(130, 120), 120, 'past the end');
  eq(replay.clampCursor(NaN, 120), 0, 'not a number');
  eq(replay.clampCursor(42.5, 120), 42.5, 'inside');
  eq(replay.stepCursor(10, 2, 120), 12, 'a step');
  eq(replay.stepCursor(119, 2, 120), 120, 'a step that stops short at the end');
  eq(replay.stepCursor(120, 2, 120), 120, 'a step at the end goes nowhere');
  return 'clamped to [0, 120]';
});

// ---------------------------------------------------------------------------
// mode
// ---------------------------------------------------------------------------

check('replay is the mode when no server is configured and the index is served; mock stays the fallback', () => {
  const off = mode.serverModeFrom(undefined, '', '');
  const on = mode.serverModeFrom(undefined, '?server=1', '');
  eq(mode.dataModeFrom(off, true, null), 'replay', 'index served, no server');
  eq(mode.dataModeFrom(off, false, null), 'mock', 'no index');
  eq(mode.dataModeFrom(off, false, true), 'mock', '?replay=1 cannot conjure an index');
  eq(mode.dataModeFrom(off, true, false), 'mock', '?replay=0 forces mock');
  eq(mode.dataModeFrom(on, true, null), 'server', 'a configured server wins');
  eq(mode.replayOverride('?replay=0', ''), false, '?replay=0');
  eq(mode.replayOverride('', '#/dashboard?replay=1'), true, 'hash-route query');
  eq(mode.replayOverride('', ''), null, 'absent');
  eq(mode.dataModeBanner('mock'), 'mock data, no engine attached', 'the mock marker is unchanged');
  eq(mode.dataModeBanner('replay', undefined, '1-routing/p2c'), 'replay of a recorded run: 1-routing/p2c', 'replay banner names the run');
  eq(mode.dataModeBanner('server', on), 'live data from the Ingress server', 'server banner unchanged');
  eq(replay.REPLAY_DISABLED_REASON, 'replay of a recorded run; changing load or policy needs a live engine', 'the disabled reason');
  let notified = 0;
  const unsub = mode.subscribeActiveMode(() => notified++);
  mode.setActiveMode('replay', 'x');
  mode.setActiveMode('replay', 'x');
  unsub();
  eq(notified, 1, 'the header store notifies once per change');
  eq(mode.activeMode().mode, 'replay', 'and holds the mode');
  mode.setActiveMode('mock');
  return 'server > replay > mock; ?replay=0 forces mock; banners as specified';
});

// ---------------------------------------------------------------------------
// engine keys the panel has no field for: `ScenarioConfig.extra`
// ---------------------------------------------------------------------------

const fsModule = 'node:fs';
const fs = (await import(fsModule)) as { readFileSync: (p: URL | string, enc: 'utf8') => string };
const repoFile = (rel: string): string => fs.readFileSync(new URL(`../../../${rel}`, import.meta.url), 'utf8');

check('scenarios/kv_spiral_never.txt: session and preemption keys land in `extra`, nothing is unmapped, every key round-trips', () => {
  const src = repoFile('scenarios/kv_spiral_never.txt');
  const { config, unmapped } = replay.configFromScenarioText(src);
  eq(unmapped, [], 'unmapped');
  eq(Object.keys(config.extra).sort(), ['preemption', 'session_think_s', 'session_turns_mean'], 'extra keys');
  eq(config.extra.session_turns_mean, 8, 'session_turns_mean is a number');
  eq(config.extra.session_think_s, 8, 'session_think_s is a number');
  eq(config.extra.preemption, 'never', 'preemption stays text');
  eq(config.fleet.replicas, 4, 'replicas');
  eq(config.fleet.kvTokensPerReplica, 30000, 'kv_capacity_tokens');
  const text = api.parseScenarioText(src);
  const back = api.scenarioConfigToWire(config).fields;
  for (const [k, raw] of Object.entries(text)) {
    const v = back[k];
    if (v === undefined) throw new Error(`${k} = ${raw} did not come back`);
    const numeric = Number.isFinite(Number(raw)) && typeof v === 'number';
    const same = numeric ? Number(raw) === v : String(v) === raw;
    if (!same) throw new Error(`${k}: text ${raw}, round trip ${String(v)}`);
  }
  return `${Object.keys(text).length} keys round-trip, 3 through extra`;
});

check('an extra key the engine does not accept throws at encode time, not at the server', () => {
  const { config } = replay.configFromScenarioText('name = x\n');
  config.extra.preemptoin = 'never';
  let threw = '';
  try {
    api.scenarioConfigToWire(config);
  } catch (e) {
    threw = e instanceof Error ? e.message : String(e);
  }
  ok(/preemptoin/.test(threw), `expected a throw naming the key, got ${JSON.stringify(threw)}`);
  return threw;
});

// ---------------------------------------------------------------------------

console.log(`${cases} cases, ${cases - failures} passed, ${failures} failed`);
if (failures > 0) throw new Error(`${failures} of ${cases} replay self-test cases failed`);
