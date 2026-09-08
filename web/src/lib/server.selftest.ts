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

import type { FetchLike, SseEvent, StreamHandle } from './api';
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
const { ServerRunEngine, SERVER_DISABLED_REASON, SERVER_NOT_YET, speedLabel, ReplicaStreams, openStreamCount } =
  await load<typeof import('./useServerRun')>('useServerRun');
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

/**
 * Wraps the fake so that calls to `rpc` which are in flight at the same time reach it in reverse
 * order of issue. A server behind a load balancer may apply two requests in either order; the
 * client must not depend on the one it sent first landing first.
 */
function reversing(fake: FetchLike, rpc: string, settleMs = 5): FetchLike {
  let pending: Array<() => Promise<void>> = [];
  let timer: ReturnType<typeof setTimeout> | null = null;
  return (input, init) => {
    if (!input.endsWith(`/${rpc}`)) return fake(input, init);
    return new Promise<Response>((resolve, reject) => {
      pending.push(() => fake(input, init).then(resolve, reject));
      if (timer === null) {
        timer = setTimeout(async () => {
          const batch = pending.reverse();
          pending = [];
          timer = null;
          for (const f of batch) await f();
        }, settleMs);
      }
    });
  };
}

/** Holds calls to `rpc` while closed, so the test can act between a request leaving and answering. */
function gate(fake: FetchLike, rpc: string, open = false) {
  const held: Array<() => void> = [];
  const waiting: Array<() => void> = [];
  const fetch: FetchLike = (input, init) => {
    if (open || !input.endsWith(`/${rpc}`)) return fake(input, init);
    return new Promise<Response>((resolve, reject) => {
      held.push(() => void fake(input, init).then(resolve, reject));
      for (const w of waiting.splice(0)) w();
    });
  };
  return {
    fetch,
    hold: () => (open = false),
    release: () => {
      open = true;
      for (const h of held.splice(0)) h();
    },
    /** Resolves once a call is held. */
    arrived: () => (held.length ? Promise.resolve() : new Promise<void>((r) => waiting.push(r))),
  };
}

type RigExtra = { fetch?: (fake: FetchLike) => FetchLike; statusPollMs?: number };

function rig(opts: Parameters<typeof fakeIngress>[1] = {}, extra: RigExtra = {}) {
  const fake = fakeIngress(FIXTURE, opts);
  const client = new api.IngressClient({ baseUrl: '', fetchImpl: extra.fetch ? extra.fetch(fake) : fake });
  let changes = 0;
  // Set after construction, so a test can react to the engine's own state (the run id appearing).
  const hooks: { onChange: (() => void) | null } = { onChange: null };
  const engine = new ServerRunEngine(cloneConfig(BASE), {
    client,
    statusPollMs: extra.statusPollMs ?? 0,
    onChange: () => {
      changes++;
      hooks.onChange?.();
    },
    // No backoff and no jitter: a reconnect in the test is immediate and deterministic.
    sleep: async () => undefined,
    rnd: () => 0,
  });
  return { fake, client, engine, hooks, changes: () => changes };
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
  eq(mode.dataModeFrom(UNCONFIGURED, true, false, true), 'none', '?replay=0 skips the recordings');
  eq(mode.dataModeFrom(UNCONFIGURED, true, null, false), 'replay', 'unreachable falls back to the index');
  eq(mode.dataModeFrom(UNCONFIGURED, false, null, false), 'none', 'unreachable and no index is none');

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
  ok(started.scenario.text.includes('arrival_rps = 560'), 'the whole config went as scenario text');
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
  // The generic update banner reads only `requiredResimulation`; a refusal must reach the one
  // banner that prints a reason, or the 501 is swallowed on screen.
  ok((notYet.engine.refused ?? '').includes(SERVER_NOT_YET), `refused names the 501: ${notYet.engine.refused}`);
  ok((notYet.engine.refused ?? '').includes(label), 'refused names the field');
  // The update was refused, so the control must not keep showing a value the fleet never took.
  eq(notYet.engine.config.workload.arrivalRps, BASE.workload.arrivalRps, 'a refusal restores the pre-update value');
  notYet.engine.dispose();

  // A server that honours the call: the echo lands on lastUpdate.
  const live = rig({ liveUpdates: true });
  await live.engine.start();
  await live.engine.update(hotter);
  eq(live.engine.lastUpdate?.accepted, true, 'accepted');
  eq(live.engine.lastUpdate?.requiredResimulation, false, 'no resimulation');
  eq(live.engine.lastUpdate?.rejectedReason, '', 'no reason');
  eq(live.engine.refused, null, 'an accepted update refuses nothing');
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
  // U106: a fleet change is not live-tunable, so it is staged for a restart rather than refused.
  eq(live.engine.lastUpdate, null, 'a fleet change raises no update banner');
  eq(live.engine.refused, null, 'and no refusal');
  eq(live.engine.pendingKeys.join(','), 'replica count', 'it waits for a restart instead');
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
  const stale: SseEvent[] = [];
  let gone: unknown = null;
  try {
    await api.readSseStream(client.openSubscriptionUrl({ runId: id, target: api.fleetTarget(), metrics: ['METRIC_OFFERED_RPS'], samplesPerSimSecond: 4 }), { onEvent: (e) => stale.push(e), lastEventId: '2', fetchImpl: fake });
  } catch (e) {
    gone = e;
  }
  ok(gone instanceof api.IngressError && gone.gone, 'an id older than the ring is 410');
  eq(stale.length, 0, 'and nothing streamed');
  // A stream that has not reached the final row stays open, as it should; the test closes it.
  const fresh: SseEvent[] = [];
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
// (h) restart() revives nothing when dispose lands while StopRun is still in flight
// ---------------------------------------------------------------------------

await checkAsync('(h) restart_after_dispose_starts_nothing', async () => {
  const { fake, engine } = rig();
  const id = await engine.start();
  eq(id, 'r-1', 'the first run starts');
  eq(calls(fake, 'OpenSubscription').length, 1, 'one subscription so far');
  const next = cloneConfig(BASE);
  next.workload.arrivalRps = 55;
  const p = engine.restart(next);
  // Unmount (dispose) races the pending StopRun for the old run: it lands in the same tick,
  // before the fake's StopRun promise has a chance to settle and restart() to resume.
  engine.dispose();
  await p;
  eq(calls(fake, 'StartRun').length, 1, 'no second StartRun for a restart that lost its owner');
  eq(calls(fake, 'OpenSubscription').length, 1, 'no new subscription opened for it');
  eq(calls(fake, 'StopRun').length, 1, 'the original run is stopped exactly once');
  eq(engine.runId, null, 'the engine ends up owning no run');
  eq(fake.openStreams(), 0, 'no stream left open');
  return 'dispose during a pending restart starts nothing: 1 StartRun, 1 StopRun, no orphaned subscription';
});

// ---------------------------------------------------------------------------
// (i) a refused update restores the config it never applied
// ---------------------------------------------------------------------------

await checkAsync('(i) a_refused_update_restores_the_config', async () => {
  const { fake, engine } = rig();
  await engine.start();
  const before = engine.config.workload.arrivalRps;
  const revBefore = engine.revision;
  const hotter = cloneConfig(engine.config);
  hotter.workload.arrivalRps = 123;
  await engine.update(hotter);
  eq(engine.lastUpdate?.accepted, false, 'the first server answers 501');
  eq(engine.config.workload.arrivalRps, before, 'config equals the pre-update config');
  ok(engine.revision > revBefore, 'revision advanced');
  const revAfterRefusal = engine.revision;
  await engine.update(hotter);
  eq(calls(fake, 'UpdateWorkload').length, 2, 'the same value diffs to something and is sent again');
  ok(engine.revision > revAfterRefusal, 'revision advances again on the resend');
  engine.dispose();
  return `refused update restores arrivalRps to ${before}; resubmitting the same value sends UpdateWorkload again`;
});

// ---------------------------------------------------------------------------
// (j) a run starts paced at the speed control's value, on both paths
// ---------------------------------------------------------------------------

await checkAsync('(j) a_run_starts_paced_at_the_speed_controls_value', async () => {
  const { fake, engine } = rig();
  // Before any status: the pressed speed button is the speed the run will start at, never 0.
  eq(engine.speed, 1, 'speed before any status arrives');
  eq(speedLabel(engine.status, engine.paused, NaN), '…', 'label before any status');
  await engine.start();
  const first = calls(fake, 'StartRun')[0].body;
  eq(first.max_realtime_factor, 1, 'the playing path starts paced at 1x');
  eq(fake.run('r-1')?.realtimeFactor, 1, 'the fake\'s run holds 1');

  engine.setSpeed(2);
  await until(() => engine.speed === 2, 'playing at 2x');
  await engine.restart(cloneConfig(BASE));
  eq(calls(fake, 'StartRun').length, 2, 'the restart is a second StartRun');
  eq(calls(fake, 'StartRun')[1].body.max_realtime_factor, 2, 'a restart starts at the speed last chosen');
  eq(fake.run('r-2')?.realtimeFactor, 2, 'the fake\'s second run holds 2');
  eq(engine.speed, 2, 'speed after the restart, before its first status, is the chosen one');
  engine.dispose();

  // The paused path is paced at the same value, so a pause cannot lose the race to completion.
  const pausedRig = rig();
  await pausedRig.engine.start(false);
  eq(calls(pausedRig.fake, 'StartRun')[0].body.max_realtime_factor, 1, 'the paused path starts paced at 1x too');
  await until(() => pausedRig.engine.paused, 'the pause');
  eq(speedLabel(pausedRig.engine.status, pausedRig.engine.paused, NaN), 'paused', 'label while paused');
  pausedRig.engine.dispose();

  const running = { state: 'STATE_RUNNING', realtimeFactor: 2 } as NonNullable<typeof engine.status>;
  eq(speedLabel(running, false, NaN), '2×', 'label at a positive factor, achieved unknown');
  eq(speedLabel({ ...running, realtimeFactor: 0 }, false, NaN), 'unpaced', 'label for a run an older client started unpaced');
  eq(speedLabel(null, false, NaN), '…', 'label with no status');
  // The banner's word for the pace actually achieved, not just the target asked for.
  eq(speedLabel(running, false, 1.3), '2× (achieving 1.3×)', 'label when achieving well under the target');
  eq(speedLabel(running, false, 1.95), '2×', 'label when achieving within 90% of the target');
  eq(speedLabel({ ...running, realtimeFactor: 0 }, false, 1.3), 'unpaced (1.3×)', 'label for an unpaced run with a known achieved pace');
  return 'StartRun max_realtime_factor 1 on both paths, 2 after setSpeed(2) + restart; speed 1 before status; eight labels';
});

// ---------------------------------------------------------------------------
// (k) controls issued before StartRun answers are applied once the id exists
// ---------------------------------------------------------------------------

await checkAsync('(k) controls_before_the_run_id_are_applied_after_StartRun', async () => {
  // The showcase's walkthrough runner calls setSpeed(2) and play() from the dashboard's first
  // onRun, while StartRun is still in flight; start(false) then paused the run once the id came
  // back and nothing ever unpaused it. Every card sat at "0 samples".
  const { fake, engine } = rig();
  const starting = engine.start(false);
  engine.setSpeed(2);
  engine.setPaused(false);
  eq(calls(fake, 'SetSpeed').length, 0, 'nothing on the wire before the id exists');
  eq(await starting, 'r-1', 'the run starts');
  eq(calls(fake, 'StartRun')[0].body.max_realtime_factor, 1, 'StartRun went out at the factor of the time');
  eq(calls(fake, 'SetSpeed').length, 1, 'exactly one SetSpeed after StartRun');
  eq(calls(fake, 'SetSpeed')[0].body.realtime_factor, 2, 'at the factor chosen while in flight');
  eq(calls(fake, 'SetSpeed')[0].body.paused, false, 'and playing');
  eq(fake.run('r-1')?.state, 'STATE_RUNNING', 'the fake\'s run plays');
  eq(fake.run('r-1')?.realtimeFactor, 2, 'at 2x');
  eq(engine.paused, false, 'the engine agrees');
  eq(engine.speed, 2, 'and shows 2x');
  engine.dispose();

  // The real order on the showcase: the runner's step fires from a child effect, before the
  // parent effect calls start(false) at all. The controls must survive the start.
  const early = rig();
  early.engine.setSpeed(2);
  early.engine.setPaused(false);
  eq(await early.engine.start(false), 'r-1', 'the run starts');
  eq(calls(early.fake, 'StartRun')[0].body.max_realtime_factor, 2, 'StartRun carries the factor chosen before it');
  eq(calls(early.fake, 'SetSpeed').length, 0, 'so no SetSpeed is needed');
  eq(early.fake.run('r-1')?.state, 'STATE_RUNNING', 'the fake\'s run plays');
  eq(early.fake.run('r-1')?.realtimeFactor, 2, 'at 2x');
  // A restart after that has no pending control and plays, as it always did.
  await early.engine.restart(cloneConfig(BASE));
  eq(calls(early.fake, 'SetSpeed').length, 0, 'still no SetSpeed');
  eq(early.fake.run('r-2')?.state, 'STATE_RUNNING', 'the restarted run plays');
  early.engine.dispose();

  // start(false) with no controls: paused at 1, as before.
  const quiet = rig();
  eq(await quiet.engine.start(false), 'r-1', 'the run starts');
  eq(calls(quiet.fake, 'SetSpeed').length, 1, 'one SetSpeed, the pause');
  eq(calls(quiet.fake, 'SetSpeed')[0].body.paused, true, 'paused');
  eq(quiet.fake.run('r-1')?.state, 'STATE_PAUSED', 'the fake\'s run is paused');
  eq(quiet.fake.run('r-1')?.realtimeFactor, 1, 'at 1x');
  eq(quiet.engine.paused, true, 'the engine agrees');
  quiet.engine.dispose();

  // start(true) with no controls needs no SetSpeed: StartRun already paced it.
  const playing = rig();
  eq(await playing.engine.start(true), 'r-1', 'the run starts');
  eq(calls(playing.fake, 'SetSpeed').length, 0, 'no SetSpeed for a playing run at the factor it started with');
  eq(playing.fake.run('r-1')?.state, 'STATE_RUNNING', 'playing');
  playing.engine.dispose();

  // A pause issued while start(true) is in flight lands too.
  const pausing = rig();
  const p = pausing.engine.start(true);
  pausing.engine.setPaused(true);
  eq(await p, 'r-1', 'the run starts');
  eq(calls(pausing.fake, 'SetSpeed').length, 1, 'one SetSpeed, the pause');
  eq(pausing.fake.run('r-1')?.state, 'STATE_PAUSED', 'paused as asked');
  pausing.engine.dispose();
  await until(() => calls(pausing.fake, 'StopRun').length === 1, 'the StopRun');
  eq(calls(pausing.fake, 'StopRun')[0].body.run_id, 'r-1', 'dispose stops the run it owns');
  return 'setSpeed(2)+play before the id: one SetSpeed(2, playing); before start(false): none needed; plain start(false): paused at 1; start(true): no SetSpeed; pause in flight lands';
});

// ---------------------------------------------------------------------------
// (l) a play issued the moment the run id appears beats the starting pause, whatever order the
//     server applies the two in
// ---------------------------------------------------------------------------

await checkAsync('(l) a_play_racing_the_starting_pause_wins_in_either_server_order', async () => {
  // The showcase mounts its dashboard with autoplay=false, so start(false) sends SetSpeed(paused)
  // once StartRun answers; the walkthrough runner is built by the render that first sees the run
  // id and its first step sends SetSpeed(playing) a moment later. On lbsim.ai one card in fourteen
  // sat at "0 samples" with its stream open: the two were in flight together and the pause landed
  // last. The fake here applies concurrent SetSpeeds in reverse order, so the client has to keep
  // one in flight at a time for the run to end up playing.
  const r = rig({}, { fetch: (f) => reversing(f, 'SetSpeed') });
  let seenId = false;
  r.hooks.onChange = () => {
    if (r.engine.runId === null || seenId) return;
    seenId = true;
    // React renders after the current task; the runner's controls follow start's own pause.
    queueMicrotask(() => {
      r.engine.setSpeed(2);
      r.engine.setPaused(false);
    });
  };
  eq(await r.engine.start(false), 'r-1', 'the run starts');
  await until(() => calls(r.fake, 'SetSpeed').length >= 2, 'both controls at the server');
  await until(() => r.engine.status?.state === 'STATE_RUNNING' && r.engine.status.realtimeFactor === 2, 'the engine sees the run playing at 2x');
  await new Promise<void>((res) => setTimeout(res, 20));
  const sent = calls(r.fake, 'SetSpeed');
  const last = sent[sent.length - 1].body;
  eq(last.paused, false, 'the last SetSpeed the server saw plays');
  eq(last.realtime_factor, 2, 'at 2x');
  eq(r.fake.run('r-1')?.state, 'STATE_RUNNING', 'the fake\'s run plays');
  eq(r.fake.run('r-1')?.realtimeFactor, 2, 'at 2x');
  eq(r.engine.paused, false, 'the engine agrees');
  eq(r.engine.error, null, 'no error');
  r.engine.dispose();
  return `${sent.length} SetSpeed(s), applied in reverse; the run plays at 2x`;
});

// ---------------------------------------------------------------------------
// (m) dispose during the starting SetSpeed leaves no stream, no poll, and no leaked interval
// ---------------------------------------------------------------------------

await checkAsync('(m) dispose_during_the_starting_SetSpeed_leaves_no_stream_and_no_poll', async () => {
  // The harness closes the page while start's SetSpeed is in flight. start then went on to open a
  // subscription and set a status poll on a disposed engine; a later start (React's double mount,
  // a restart) reset `disposed` and overwrote `poll`, leaving the earlier interval with no handle.
  let speed!: ReturnType<typeof gate>;
  let starting!: ReturnType<typeof gate>;
  const r = rig({}, { fetch: (f) => (starting = gate((speed = gate(f, 'SetSpeed')).fetch, 'StartRun', true)).fetch, statusPollMs: 20 });
  const s1 = r.engine.start(false);
  await speed.arrived();
  r.engine.dispose();
  speed.release();
  eq(await s1, null, 'a start that lost its owner during its SetSpeed returns nothing');
  await until(() => calls(r.fake, 'StopRun').length === 1, 'the StopRun');
  const opens = calls(r.fake, 'OpenSubscription').length;
  const polls = calls(r.fake, 'GetRun').length;
  await new Promise<void>((res) => setTimeout(res, 60));
  eq(calls(r.fake, 'OpenSubscription').length, opens, 'no subscription opened after dispose');
  eq(opens, 0, 'none at all');
  eq(calls(r.fake, 'GetRun').length, polls, 'no GetRun after dispose');
  eq(r.fake.openStreams(), 0, 'no stream left open');

  // start, dispose during SetSpeed, start again, dispose: the second start must not inherit an
  // interval from the first. A third start whose StartRun never answers exposes one: `disposed`
  // is false and the old id is still held, so a leaked interval polls GetRun while nothing else does.
  speed.hold();
  const s2 = r.engine.start(false);
  await speed.arrived();
  r.engine.dispose();
  const s3 = r.engine.start(false);
  speed.release();
  await s2;
  eq(await s3, 'r-3', 'the third run starts');
  r.engine.dispose();
  await until(() => calls(r.fake, 'StopRun').length === 3, 'every run stopped');
  starting.hold();
  const s4 = r.engine.start(false);
  const before = calls(r.fake, 'GetRun').length;
  await new Promise<void>((res) => setTimeout(res, 60));
  eq(calls(r.fake, 'GetRun').length, before, 'no GetRun while a start is waiting on StartRun: no interval survived the disposes');
  r.engine.dispose();
  starting.release();
  await s4;
  await until(() => calls(r.fake, 'StopRun').some((c) => c.body.run_id === 'r-4'), 'the fourth run is stopped by the dispose that beat its StartRun');
  eq(r.fake.openStreams(), 0, 'no stream left open');
  return 'no OpenSubscription or GetRun after a dispose mid-SetSpeed; no interval outlives start/dispose/start/dispose';
});

// ---------------------------------------------------------------------------
// (n) a structural edit is staged for a restart, not sent as an update (U106)
// ---------------------------------------------------------------------------

await checkAsync('(n) a structural edit stages a restart; restart sends StopRun then StartRun with the value', async () => {
  // Issao asked, verbatim, "where do i tune step token budget?" The knob was a slider whose every move ended in a
  // refusal banner, because no Update call carries a physics key. Now it stages.
  const { fake, engine } = rig({ liveUpdates: true });
  await engine.start(false);
  eq(engine.pendingRestart, null, 'nothing pending on a fresh run');
  const before = fake.calls.length;
  const next = cloneConfig(engine.config);
  next.fleet.stepTokenBudget = 2048;
  await engine.update(next);
  eq(fake.calls.length, before, 'no RPC for a structural edit');
  eq(calls(fake, 'UpdateWorkload').length + calls(fake, 'UpdatePolicies').length, 0, 'no update call');
  eq(engine.pendingKeys.join(','), 'step token budget', 'the key is staged under its label');
  eq(engine.pendingRestart?.fleet.stepTokenBudget, 2048, 'the staged config carries the value');
  eq(engine.config.fleet.stepTokenBudget, 2048, 'the panel keeps showing the edit');
  eq(engine.lastUpdate, null, 'no update banner');
  eq(engine.refused, null, 'no refusal banner');

  // A workload edit on top still goes live, and leaves the staged key staged.
  const live = cloneConfig(engine.config);
  live.workload.arrivalRps = 600;
  await engine.update(live);
  eq(calls(fake, 'UpdateWorkload').length, 1, 'the workload edit went as UpdateWorkload');
  eq(engine.lastUpdate?.accepted, true, 'and was accepted');
  eq(engine.pendingKeys.join(','), 'step token budget', 'the structural key is still pending');

  // Moving the knob back to the running fleet's value leaves nothing to restart for.
  const back = cloneConfig(engine.config);
  back.fleet.stepTokenBudget = 1024;
  await engine.update(back);
  eq(engine.pendingRestart, null, 'restored value: nothing pending');
  await engine.update(next);
  eq(engine.pendingKeys.join(','), 'step token budget', 'staged again');

  const pending = engine.pendingRestart!;
  await engine.restart(pending);
  const seq = fake.calls.slice(before).map((c) => c.rpc).filter((r) => r === 'StopRun' || r === 'StartRun');
  eq(seq.join(','), 'StopRun,StartRun', 'StopRun, then StartRun');
  eq(calls(fake, 'StopRun')[0].body.run_id, 'r-1', 'the old run is the one stopped');
  eq(engine.runId, 'r-2', 'a new run');
  const started = calls(fake, 'StartRun')[1].body as { scenario: { text: string } };
  ok(started.scenario.text.includes('step_token_budget = 2048'), 'the new run carries the value');
  ok(started.scenario.text.includes(`seed = ${BASE.seed}`), 'at the same seed');
  eq(engine.pendingRestart, null, 'nothing pending after the restart');
  eq(engine.pendingKeys.length, 0, 'no keys either');
  await until(() => calls(fake, 'OpenSubscription').length === 2, 'the subscription reopened');
  engine.dispose();
  return 'staged without an RPC; StopRun r-1, StartRun with step_token_budget = 2048 at the same seed, stream reopened';
});

// ---------------------------------------------------------------------------
// (o) replica streams stay within the stream budget (U112)
// ---------------------------------------------------------------------------

await checkAsync('(o) a page of ten replicas holds three streams under budget 4 and rotates over all ten; ten under budget 32', async () => {
  // U102's finding: over HTTP/1.1 the ten replica streams of the Machines page plus the fleet stream
  // filled Chrome's six connections per host, and every later SetSpeed / GetRun / Renew queued behind
  // them. The page must never starve its own RPCs, so the streams are budgeted and the rest of the
  // page takes turns.
  const { client } = rig();
  let open = 0;
  let peak = 0;
  let opens = 0;
  const openStreamImpl = (): StreamHandle => {
    open++;
    opens++;
    peak = Math.max(peak, open);
    let finish!: () => void;
    const done = new Promise<void>((r) => (finish = r));
    let closed = false;
    return {
      done,
      close: () => {
        if (closed) return;
        closed = true;
        open--;
        finish();
      },
    };
  };
  const ids = Array.from({ length: 10 }, (_, i) => i);
  const base = { client, runId: 'r-1', ids, samplesPerSimSecond: 4, onRow: () => undefined, openStreamImpl };

  const narrow = new ReplicaStreams({ ...base, budget: 4 });
  eq(open, 3, 'budget 4: three replica streams open at once (one slot is the fleet stream, one is kept free)');
  eq(narrow.slots(), 3, 'slots() says so');
  eq(openStreamCount(), 3, 'the module counter sees them');
  ok(narrow.rotates(), 'ten rows in three slots rotate');
  const seen = new Set<number>(narrow.streaming());
  let ticks = 0;
  for (let rotation = 0; rotation < 4; rotation++) {
    for (let k = 0; k < narrow.slots(); k++) {
      narrow.tick();
      ticks++;
      for (const id of narrow.streaming()) seen.add(id);
      ok(open <= 3, `never more than three open (tick ${ticks}: ${open})`);
      eq(narrow.streaming().length, 3, `three streaming after tick ${ticks}`);
    }
  }
  eq(peak, 3, 'peak open streams over four rotations');
  eq(seen.size, 10, `every id streamed at least once within four rotations (${ticks} ticks, ${opens} opens)`);
  eq(new Set(narrow.streaming()).size, 3, 'no id streams twice at once');
  narrow.close();
  eq(open, 0, 'close() closes every stream');
  eq(openStreamCount(), 0, 'and the counter returns to zero');
  narrow.tick();
  eq(open, 0, 'a tick after close opens nothing');

  const wide = new ReplicaStreams({ ...base, budget: 32 });
  eq(open, 10, 'budget 32: all ten open at once');
  ok(!wide.rotates(), 'nothing rotates when the page fits');
  const before = opens;
  wide.tick();
  eq(opens, before, 'a tick then opens nothing');
  eq(open, 10, 'and closes nothing');
  wide.close();
  eq(open, 0, 'closed');
  return `budget 4: 3 at once, ten ids within ${ticks} ticks; budget 32: 10 at once`;
});

// ---------------------------------------------------------------------------
// (p) a smoothing change reopens the fleet subscription with the window and keeps everything else
// ---------------------------------------------------------------------------

await checkAsync('(p) setSmoothing reopens the fleet stream with smoothing_window_ns, keeps the cursor and the pause, swaps the history once caught up', async () => {
  const { fake, engine } = rig();
  await engine.start();
  fake.advance(8);
  await until(() => engine.frames.length === 8, '8 frames');
  engine.setSpeed(0);
  await until(() => engine.paused, 'paused');
  engine.scrubTo(1);
  eq(engine.cursorS, 1, 'cursor pinned at 1 s');
  eq(calls(fake, 'OpenSubscription')[0].body.smoothing_window_ns, undefined, 'the first open is raw: no window on the query');

  engine.setSmoothing(30_000_000_000n);
  await until(() => calls(fake, 'OpenSubscription').length === 2, 'the reopen');
  const reopened = calls(fake, 'OpenSubscription')[1];
  eq(reopened.body.smoothing_window_ns, '30000000000', 'the window, as decimal nanoseconds');
  eq(reopened.body.subscription_id, undefined, 'a fresh open, not a resume: the ring holds raw rows');
  eq(reopened.lastEventId, undefined, 'no Last-Event-ID');
  eq(calls(fake, 'SetSpeed').length, 1, 'no control was sent: the run stays as it was');
  // The fake re-streams the eight released rows; the history is swapped once, whole.
  await until(() => engine.subscriptionId === 's-2' && fake.subscriptions().length === 1, 'the old subscription closed');
  await until(() => engine.frames.length === 8 && engine.frames[0].tick === 0, 'eight frames again after the swap');
  eq(engine.frames.map((f) => f.tick).join(','), '0,1,2,3,4,5,6,7', 'ticks contiguous from 0');
  for (let i = 0; i < 8; i++) eq(engine.frames[i].simTimeUnixNs, EXPECTED[i].simTimeUnixNs, `frame ${i} is the right row`);
  eq(engine.cursorS, 1, 'the cursor is still pinned at 1 s');
  eq(engine.paused, true, 'still paused');
  eq(engine.recordedToS, 7 / 4, 'recorded extent unchanged');

  engine.setSmoothing(30_000_000_000n);
  eq(calls(fake, 'OpenSubscription').length, 2, 'the same window again opens nothing');
  // The stream goes on from where the run is.
  engine.setPaused(false);
  await until(() => !engine.paused, 'playing');
  fake.advance(ROWS - 8);
  await until(() => engine.frames.length === ROWS, `${ROWS} frames`);
  eq(engine.frames.map((f) => f.tick).join(','), Array.from({ length: ROWS }, (_, i) => i).join(','), 'ticks contiguous to the end');
  engine.setSmoothing(0n);
  await until(() => calls(fake, 'OpenSubscription').length === 3, 'back to raw reopens once more');
  eq(calls(fake, 'OpenSubscription')[2].body.smoothing_window_ns, undefined, 'raw carries no window');
  engine.dispose();
  return 'reopened once with smoothing_window_ns=30000000000; cursor 1 s and pause kept; history swapped whole';
});

// ---------------------------------------------------------------------------
// (q) a completed run restarts from its own config, at the same seed, cursor back at 0 (U120)
// ---------------------------------------------------------------------------

await checkAsync('(q) a run that reaches STATE_COMPLETE restarts from its own config: same seed, fresh run id, cursor at 0', async () => {
  // The playback bar's end-of-run Restart button is exactly this: `run.restart(run.config)`, no
  // diff at all. `status.state === 'STATE_COMPLETE'` is what the bar's `ended` reads, so the test
  // drives the fake to that state the same way (b) does, then polls once rather than waiting on
  // the interval, since this rig uses statusPollMs 0 for determinism.
  const { fake, engine } = rig();
  const id = await engine.start();
  eq(id, 'r-1', 'run id');
  fake.advance(ROWS);
  await until(() => engine.frames.length === ROWS, `${ROWS} frames`);
  await engine.pollStatus();
  eq(engine.status?.state, 'STATE_COMPLETE', 'the poll sees the run finished');
  eq(engine.status?.state === 'STATE_COMPLETE', true, "the bar's `ended` reads true here");

  const before = fake.calls.length;
  const seed = engine.config.seed;
  const arrivalRps = engine.config.workload.arrivalRps;
  await engine.restart(engine.config);
  const seq = fake.calls.slice(before).map((c) => c.rpc).filter((r) => r === 'StopRun' || r === 'StartRun');
  eq(seq.join(','), 'StopRun,StartRun', 'StopRun, then StartRun, same as a staged restart');
  eq(calls(fake, 'StopRun')[0].body.run_id, 'r-1', 'the finished run is the one stopped');
  eq(engine.runId, 'r-2', 'a fresh run id');
  const started = calls(fake, 'StartRun')[1].body as { scenario: { text: string } };
  ok(started.scenario.text.includes(`seed = ${seed}`), 'restarted at the same seed');
  ok(started.scenario.text.includes(`arrival_rps = ${arrivalRps}`), 'and the same config, unchanged');
  eq(engine.frames.length, 0, 'frames cleared: the cursor is back at 0');
  eq(engine.cursorS, 0, 'cursorS reports 0 with no frames and no status yet');
  eq(engine.status, null, "status cleared, so `ended` reads false again until the next poll");
  await until(() => calls(fake, 'OpenSubscription').length === 2, 'the subscription reopened');
  engine.dispose();
  return `r-1 finished -> restart(config) -> r-2 at seed ${seed}, arrival_rps ${arrivalRps}, cursor 0`;
});

// ---------------------------------------------------------------------------

console.log(`${cases} cases, ${cases - failures} passed, ${failures} failed`);
if (failures > 0) throw new Error(`${failures} case(s) failed`);
