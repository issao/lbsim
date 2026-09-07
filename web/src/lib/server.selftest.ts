// Self-test for the live path: the dashboard's run controller against a fake Ingress server.
//
//   cd web && node --experimental-strip-types src/lib/server.selftest.ts
//
// Same shape as api.selftest.ts and replay.selftest.ts: one line per case, a summary, a throw when
// anything failed. No React: `ServerRunEngine` is driven directly, over the real `IngressClient`,
// the real SSE reader and the real subscription lifecycle, against `fakeIngress`, which answers
// WIRE.md's RPCs from twenty rows of a recorded run. The comparison in case (b) is the point of
// the whole file: a frame decoded from the stream equals the frame replay decodes from the file,
// because both go through `frameFromUpdate`.
//
// The resolve hook is replay.selftest.ts's: the modules under test import extensionless, which
// Vite resolves and Node does not.

import type { RunHandle } from './useRun';
import type { ServerRunHandle } from './useServerRun';

type Assert<T extends true> = T;
/** The server handle is a `RunHandle`: every panel that takes one takes it. */
export type ServerHandleIsRunHandle = Assert<ServerRunHandle extends RunHandle ? true : false>;

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
const mode = await load<typeof import('./mode')>('mode');
const fx = await load<typeof import('./apiFixtures')>('apiFixtures');
const { fakeIngress } = await load<typeof import('./fakeIngress')>('fakeIngress');
const { ServerRunEngine, SERVER_DISABLED_REASON, SERVER_NOT_YET } = await load<typeof import('./useServerRun')>('useServerRun');
const { STEP_S } = await load<typeof import('./useRun')>('useRun');
const { BASE, cloneConfig, FIELD_LABEL } = await load<typeof import('./config')>('config');

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

async function checkAsync(name: string, fn: () => Promise<string>): Promise<void> {
  try {
    pass(name, await fn());
  } catch (e) {
    report(name, e);
  }
}

function show(v: unknown): string {
  if (typeof v === 'bigint') return `${v}n`;
  return JSON.stringify(v) ?? String(v);
}

function eq(actual: unknown, expected: unknown, what: string): void {
  const same = typeof actual === 'bigint' || typeof expected === 'bigint' ? actual === expected : Object.is(actual, expected) || (Number.isNaN(actual as number) && Number.isNaN(expected as number));
  if (!same) throw new Error(`${what}: expected ${show(expected)}, got ${show(actual)}`);
}

function ok(cond: boolean, what: string): void {
  if (!cond) throw new Error(what);
}

/** Wait for a condition with a hard bound: a hung stream fails the case rather than the run. */
async function until(cond: () => boolean, what: string, ms = 2000): Promise<void> {
  const deadline = Date.now() + ms;
  while (!cond()) {
    if (Date.now() > deadline) throw new Error(`timed out waiting for ${what}`);
    await new Promise<void>((r) => setTimeout(r, 2));
  }
}

// ---------------------------------------------------------------------------
// fixtures and seams
// ---------------------------------------------------------------------------

const FIXTURE = { fleetJsonl: fx.FLEET_JSONL_EXCERPT, scenarioText: fx.FLEET_EXCERPT_SCENARIO };
const ROWS = fx.FLEET_EXCERPT_ROWS;
/** The frames replay would decode from the same rows. */
const EXPECTED = replay.parseFleetJsonl(fx.FLEET_JSONL_EXCERPT, BigInt(fx.FLEET_EXCERPT_ORIGIN));
/** No server is configured: the probe alone decides. */
const UNCONFIGURED = mode.serverModeFrom(undefined, '', '', null);

function rig(opts: Parameters<typeof fakeIngress>[1] = {}) {
  const fake = fakeIngress(FIXTURE, opts);
  const client = new api.IngressClient({ baseUrl: '', fetchImpl: fake });
  let changes = 0;
  const engine = new ServerRunEngine(cloneConfig(BASE), {
    client,
    statusPollMs: 0,
    onChange: () => changes++,
    // No backoff and no jitter: a reconnect in the test is immediate and deterministic.
    sleep: async () => undefined,
    rnd: () => 0,
  });
  return { fake, client, engine, changes: () => changes };
}

const calls = (fake: ReturnType<typeof fakeIngress>, rpc: string) => fake.calls.filter((c) => c.rpc === rpc);

// ---------------------------------------------------------------------------
// (a) the fake answers ListRuns and the mode resolves to server
// ---------------------------------------------------------------------------

await checkAsync('(a) ListRuns answers and dataModeFrom picks server', async () => {
  const { fake, client } = rig();
  const listed = await client.listRuns();
  eq(listed.runs.length, 0, 'no runs before any StartRun');
  eq(listed.nextCursor, '', 'next_cursor');
  ok(await mode.probeServer(client, UNCONFIGURED), 'the probe sees the fake');
  eq(mode.dataModeFrom(UNCONFIGURED, false, null, true), 'server', 'reachable, no index, no override');
  eq(mode.dataModeFrom(UNCONFIGURED, true, null, true), 'server', 'reachable beats a served index');
  eq(mode.dataModeFrom(UNCONFIGURED, true, true, true), 'replay', '?replay=1 asks for the recording instead');
  eq(mode.dataModeFrom(UNCONFIGURED, true, false, true), 'mock', '?replay=0 forces mock');
  eq(mode.dataModeFrom(UNCONFIGURED, true, null, false), 'replay', 'unreachable falls back to the index');
  eq(mode.dataModeFrom(UNCONFIGURED, false, null, false), 'mock', 'unreachable and no index is mock');

  const refusing = new api.IngressClient({ baseUrl: '', fetchImpl: async () => new Response('<!doctype html>', { status: 200 }) });
  eq(await mode.probeServer(refusing, UNCONFIGURED), false, 'a dev server answering HTML is not a server');
  const silent = new api.IngressClient({ baseUrl: '', fetchImpl: () => new Promise<Response>(() => undefined) });
  eq(await mode.probeServer(silent, UNCONFIGURED, 20), false, 'no answer inside the budget is not a server');
  const off = mode.serverModeFrom(undefined, '?server=0', '', null);
  eq(await mode.probeServer(client, off), false, '?server=0 is never probed');
  eq(calls(fake, 'ListRuns').length, 2, 'ListRuns calls seen by the fake');
  return `probe true, HTML false, silence false in 20 ms, ?server=0 not probed`;
});

// ---------------------------------------------------------------------------
// (b) start + subscribe yields the frames replay decodes from the same rows
// ---------------------------------------------------------------------------

await checkAsync('(b) live frames equal replay frames for the same rows', async () => {
  const { fake, engine } = rig();
  const id = await engine.start();
  eq(id, 'r-1', 'run id');
  eq(fake.run('r-1')?.state, 'STATE_RUNNING', 'state after StartRun');
  const started = calls(fake, 'StartRun')[0].body as { scenario: { text: string; overrides: Record<string, string> }; record_traces: boolean };
  ok(started.scenario.text.includes('arrival_rps = 70'), 'the whole config went as scenario text');
  eq(Object.keys(started.scenario.overrides).length, 0, 'no overrides on a fresh start');
  const opened = calls(fake, 'OpenSubscription')[0];
  eq(opened.body.scope, 'SCOPE_FLEET', 'fleet scope');
  eq(opened.body.samples_per_sim_second, '4', 'sample rate from the config');
  eq(opened.body.lease_ns, '30000000000', 'a 30 s lease');
  ok(String(opened.body.metrics).includes('METRIC_TTFT'), 'ttft is asked for');

  fake.advance(ROWS);
  await until(() => engine.frames.length === ROWS, `${ROWS} frames`);
  await until(() => engine.connection === 'complete', 'the final update to end the stream');
  eq(fake.run('r-1')?.state, 'STATE_COMPLETE', 'the run finished');

  for (let i = 0; i < ROWS; i++) {
    const got = engine.frames[i];
    const want = EXPECTED[i];
    eq(got.tick, i, `frame ${i} tick`);
    eq(got.simTimeUnixNs, want.simTimeUnixNs, `frame ${i} instant`);
    eq(got.offeredRps, want.offeredRps, `frame ${i} offeredRps`);
    eq(got.completedRps, want.completedRps, `frame ${i} completedRps`);
    eq(adapter.latencyMs(got, 'ttft', 99), adapter.latencyMs(want, 'ttft', 99), `frame ${i} ttft p99`);
    eq(got.kvUtilization, want.kvUtilization, `frame ${i} kv`);
  }
  eq(engine.frames[0].simS, 0, 'simS is measured from the first sample seen');
  eq(engine.recordedToS, (ROWS - 1) / 4, 'recorded to the last sample');
  eq(engine.window(0, 100).length, ROWS, 'window at the configured rate keeps every frame');
  eq(engine.frameAt(1.3)?.tick, 5, 'frameAt is the last frame at or before');
  const p99 = adapter.latencyMs(engine.frames[0], 'ttft', 99);
  return `${ROWS} frames; row 0 offered ${engine.frames[0].offeredRps} rps, completed ${engine.frames[0].completedRps}, ttft p99 ${p99.toFixed(0)} ms`;
});

// ---------------------------------------------------------------------------
// (c) a dropped stream resumes with Last-Event-ID: no duplicate, no gap
// ---------------------------------------------------------------------------

await checkAsync('(c) drop mid-stream, resume with Last-Event-ID, ticks contiguous', async () => {
  const { fake, engine } = rig();
  await engine.start();
  fake.advance(8);
  await until(() => engine.frames.length === 8, '8 frames before the drop');
  eq(fake.dropStreams(), 1, 'one stream dropped');
  await until(() => calls(fake, 'OpenSubscription').length === 2, 'the reconnect');
  fake.advance(ROWS - 8);
  await until(() => engine.frames.length === ROWS, `${ROWS} frames after the resume`);
  await until(() => engine.connection === 'complete', 'completion after the resume');

  const opens = calls(fake, 'OpenSubscription');
  eq(opens.length, 2, 'exactly one reconnect');
  eq(opens[1].lastEventId, '8', 'resumed after the last id seen');
  eq(opens[1].body.subscription_id, 's-1', 'the same subscription, named in the query');
  eq(fake.subscriptions().length, 1, 'the server kept one subscription');
  const ticks = engine.frames.map((f) => f.tick);
  eq(ticks.join(','), Array.from({ length: ROWS }, (_, i) => i).join(','), 'ticks 0..19 with no gap and no duplicate');
  for (let i = 1; i < ROWS; i++) ok(engine.frames[i].simTimeUnixNs > engine.frames[i - 1].simTimeUnixNs, `instant ${i} advances`);
  for (let i = 0; i < ROWS; i++) eq(engine.frames[i].simTimeUnixNs, EXPECTED[i].simTimeUnixNs, `frame ${i} is the right row`);
  return `reconnected once with Last-Event-ID 8 on s-1; ${ROWS} frames, ticks contiguous`;
});

// ---------------------------------------------------------------------------
// (d) setSpeed(0) pauses; step() is a bounded StepForward
// ---------------------------------------------------------------------------

await checkAsync('(d) setSpeed(0) pauses, step calls StepForward with a bounded duration', async () => {
  const { fake, engine } = rig();
  await engine.start();
  engine.setSpeed(0);
  await until(() => engine.paused, 'the pause');
  eq(engine.status?.state, 'STATE_PAUSED', 'server state');
  eq(calls(fake, 'SetSpeed')[0].body.paused, true, 'SetSpeed asked for a pause');
  fake.advance(5);
  eq(engine.frames.length, 0, 'a paused run produces nothing');

  engine.step();
  await until(() => calls(fake, 'StepForward').length === 1, 'the StepForward');
  const step = calls(fake, 'StepForward')[0].body;
  eq(step.sim_duration_ns, api.secondsToNs(STEP_S).toString(), `a ${STEP_S} s step, as decimal nanoseconds`);
  ok(BigInt(step.sim_duration_ns as string) <= 10_000_000_000n, 'the step is bounded');
  const rows = STEP_S * 4;
  await until(() => engine.frames.length === rows, `${rows} frames from the step`);
  await until(() => engine.status?.simTimeUnixNs === EXPECTED[rows - 1].simTimeUnixNs, 'status at the end of the step');
  eq(engine.paused, true, 'still paused after the step');
  eq(engine.recordedToS, (rows - 1) / 4, 'recorded to the step end');

  engine.setSpeed(2);
  await until(() => engine.speed === 2 && !engine.paused, 'playing at 2x');
  eq(calls(fake, 'SetSpeed')[1].body.realtime_factor, 2, 'the factor went to the server');
  engine.scrubTo(0.5);
  eq(engine.cursorS, 0.5, 'scrub pins the cursor locally');
  engine.setPaused(false);
  eq(engine.cursorS, engine.recordedToS, 'play releases the pin');
  engine.rewindTo(0);
  ok((engine.refused ?? '').includes(SERVER_DISABLED_REASON), 'rewind is refused and named');
  eq(calls(fake, 'Rewind').length, 0, 'and never sent');
  engine.dispose();
  return `paused, stepped ${STEP_S} s = ${rows} rows, resumed at 2x; rewind refused: "${SERVER_DISABLED_REASON}"`;
});

// ---------------------------------------------------------------------------
// (e) update() calls UpdateWorkload; the answer lands on lastUpdate
// ---------------------------------------------------------------------------

await checkAsync('(e) update sends UpdateWorkload / UpdatePolicies and lastUpdate carries the answer', async () => {
  const label = FIELD_LABEL['workload.arrivalRps'] ?? 'workload.arrivalRps';

  // The first server answers 501: the control is wired, the answer says the server is not.
  const notYet = rig();
  await notYet.engine.start();
  const hotter = cloneConfig(BASE);
  hotter.workload.arrivalRps = 90;
  await notYet.engine.update(hotter);
  const sent = calls(notYet.fake, 'UpdateWorkload');
  eq(sent.length, 1, 'one UpdateWorkload');
  eq((sent[0].body.overrides as Record<string, string>).arrival_rps, '90', 'the override, as text');
  eq(notYet.engine.lastUpdate?.accepted, false, '501 is not accepted');
  ok((notYet.engine.lastUpdate?.rejectedReason ?? '').startsWith(SERVER_NOT_YET), `named: ${notYet.engine.lastUpdate?.rejectedReason}`);
  ok(notYet.engine.lastUpdate?.changed.includes(label) ?? false, 'the changed field is named');
  eq(notYet.engine.error, null, 'a 501 is not an error toast');
  eq(notYet.engine.config.workload.arrivalRps, 90, 'the control keeps its value');
  notYet.engine.dispose();

  // A server that honours the call: the echo lands on lastUpdate.
  const live = rig({ liveUpdates: true });
  await live.engine.start();
  await live.engine.update(hotter);
  eq(live.engine.lastUpdate?.accepted, true, 'accepted');
  eq(live.engine.lastUpdate?.requiredResimulation, false, 'no resimulation');
  eq(live.engine.lastUpdate?.rejectedReason, '', 'no reason');
  ok(live.engine.lastUpdate?.changed.includes(label) ?? false, 'the changed field is named');
  eq(live.fake.run('r-1')?.workloadUpdates[0].arrival_rps, '90', 'the server holds the new rate');
  eq(live.engine.resimulating, false, 'not resimulating afterwards');

  const rerouted = cloneConfig(hotter);
  rerouted.routing.kind = 'least_requests';
  await live.engine.update(rerouted);
  eq(calls(live.fake, 'UpdatePolicies').length, 1, 'a routing change is an UpdatePolicies');
  eq(calls(live.fake, 'UpdateWorkload').length, 1, 'and not another UpdateWorkload');
  eq(live.fake.run('r-1')?.policyUpdates[0].routing, 'least_requests', 'the policy, in engine spelling');

  const reshaped = cloneConfig(rerouted);
  reshaped.fleet.replicas += 1;
  await live.engine.update(reshaped);
  eq(live.engine.lastUpdate?.accepted, false, 'a fleet change is not live-tunable');
  eq(calls(live.fake, 'UpdatePolicies').length + calls(live.fake, 'UpdateWorkload').length, 2, 'and sends nothing');
  live.engine.dispose();
  return `501 -> lastUpdate "${SERVER_NOT_YET}"; echo -> accepted, changed [${live.engine.lastUpdate?.changed.join(', ')}]`;
});

// ---------------------------------------------------------------------------
// (f) dispose() closes the subscription and stops the run
// ---------------------------------------------------------------------------

await checkAsync('(f) dispose closes the subscription and stops the run', async () => {
  const { fake, engine } = rig();
  await engine.start();
  await until(() => engine.subscriptionId !== null, 'the open event');
  const sid = engine.subscriptionId;
  eq(sid, 's-1', 'subscription id');
  eq(fake.openStreams(), 1, 'one stream open');
  engine.dispose();
  await until(() => calls(fake, 'CloseSubscription').length === 1, 'the CloseSubscription');
  eq(calls(fake, 'CloseSubscription')[0].body.subscription_id, sid, 'closed by id');
  await until(() => calls(fake, 'StopRun').length === 1, 'the StopRun');
  eq(calls(fake, 'StopRun')[0].body.run_id, 'r-1', 'stopped by id');
  eq(fake.subscriptions().length, 0, 'the server forgot the subscription');
  eq(fake.openStreams(), 0, 'no stream left open');
  eq(engine.subscriptionId, null, 'the engine forgot it too');
  await until(() => engine.connection === 'closed', 'the phase');
  eq(fake.run('r-1')?.state, 'STATE_COMPLETE', 'StopRun finalises the run');
  return `CloseSubscription ${sid}, StopRun r-1, phase closed`;
});

// ---------------------------------------------------------------------------
// (g) the rest of the fake's contract, through the client's decoders
// ---------------------------------------------------------------------------

await checkAsync('(g) the fake\'s remaining answers decode: GetResult, 410, 501, ListRuns limit', async () => {
  const { fake, client } = rig({ ring: 4 });
  const id = await client.startRun({ scenario: api.scenarioEnvelope({}), maxRealtimeFactor: 0 });
  eq(id, 'r-1', 'run id');
  eq(fake.run(id)?.scenarioText, fx.FLEET_EXCERPT_SCENARIO, 'an empty scenario text means the fixture\'s');
  fake.advance(10);
  const stale: api.SseEvent[] = [];
  let gone: unknown = null;
  try {
    await api.readSseStream(client.openSubscriptionUrl({ runId: id, target: api.fleetTarget(), metrics: ['METRIC_OFFERED_RPS'], samplesPerSimSecond: 4 }), { onEvent: (e) => stale.push(e), lastEventId: '2', fetchImpl: fake });
  } catch (e) {
    gone = e;
  }
  ok(gone instanceof api.IngressError && gone.gone, 'an id older than the ring is 410');
  eq(stale.length, 0, 'and nothing streamed');
  // A stream that has not reached the final row stays open, as it should; the test closes it.
  const fresh: api.SseEvent[] = [];
  const resumed = api.openStream(client.openSubscriptionUrl({ runId: id, target: api.fleetTarget(), metrics: ['METRIC_OFFERED_RPS'], samplesPerSimSecond: 4 }), { onEvent: (e) => fresh.push(e), lastEventId: '7', fetchImpl: fake });
  await until(() => fresh.length === 4, 'the three rows after id 7');
  resumed.close();
  await resumed.done.catch(() => undefined);
  eq(fresh.map((e) => e.event).join(','), 'open,update,update,update', 'an id inside the ring resumes after it');
  eq(fresh[1].id, '8', 'from the next id');
  const u = api.decodeSubscriptionUpdate(JSON.parse(fresh[3].data));
  eq(u.simTimeUnixNs, EXPECTED[9].simTimeUnixNs, 'row 10 by instant');
  eq(u.final, false, 'not final before the last row');

  await client.stopRun(id);
  const result = await client.getResult(id);
  eq(result.runId, id, 'GetResult run id');
  eq(result.seed, 20260906n, 'seed from the scenario text');
  const status = await client.getRun(id);
  eq(status.state, 'STATE_COMPLETE', 'StopRun finalises as complete');

  let notImplemented: unknown = null;
  try {
    await client.rewind(id, EXPECTED[0].simTimeUnixNs);
  } catch (e) {
    notImplemented = e;
  }
  ok(notImplemented instanceof api.IngressError && notImplemented.httpStatus === 501, 'Rewind is 501');

  await client.startRun({ scenario: api.scenarioEnvelope({}) });
  eq((await client.listRuns({ limit: 1 })).runs.length, 1, 'ListRuns honours limit');
  eq((await client.listRuns()).runs.length, 2, 'and lists every run without one');
  const renew = await client.renewSubscription('s-9', 1n);
  eq(renew.expired, true, 'renewing an unknown subscription says expired');
  return 'GetResult, 410 outside the ring, resume inside it, Rewind 501, ListRuns limit, renew of unknown expired';
});

// ---------------------------------------------------------------------------

console.log(`${cases} cases, ${cases - failures} passed, ${failures} failed`);
if (failures > 0) throw new Error(`${failures} case(s) failed`);
