// Self-test for the walkthrough runner. No network, no browser, no framework.
//
//   cd web && node --experimental-strip-types src/lib/walkthroughRunner.selftest.ts
//
// Same shape as replay.selftest.ts: one line per case, a summary line, a throw when anything
// failed. The handle is a fake that records every call in order, because the contract under test
// is mostly ordering: conditions before motion, pause at the timestamp, refusal without a throw.
//
// The resolve hook is the one replay.selftest.ts uses, so the module under test may import its
// siblings extensionless the way the rest of the app does.

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

const runner = await load<typeof import('./walkthroughRunner')>('walkthroughRunner');
type RunnerHandle = import('./walkthroughRunner').RunnerHandle;
type WalkthroughScript = import('./walkthrough').WalkthroughScript;
type DataMode = import('./mode').DataMode;

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

async function check(name: string, fn: () => Promise<string>): Promise<void> {
  try {
    pass(name, await fn());
  } catch (e) {
    report(name, e);
  }
}

function eq(actual: unknown, expected: unknown, what: string): void {
  const a = JSON.stringify(actual);
  const b = JSON.stringify(expected);
  if (a !== b) throw new Error(`${what}: got ${a}, want ${b}`);
}

function ok(cond: boolean, what: string): void {
  if (!cond) throw new Error(what);
}

// ---------------------------------------------------------------------------
// The fake handle: a cursor the test moves by hand, and a log of every call in order
// ---------------------------------------------------------------------------

function fakeHandle(mode: DataMode, reject?: string) {
  const log: string[] = [];
  let cursor = 0;
  let endedFlag = false;
  const h: RunnerHandle & { log: string[]; moveTo(s: number): void; end(): void } = {
    mode,
    log,
    moveTo: (s) => {
      cursor = s;
    },
    end: () => {
      endedFlag = true;
    },
    ended: () => endedFlag,
    cursorS: () => cursor,
    scrubTo: (s) => {
      log.push(`scrub ${s}`);
      cursor = s;
    },
    setSpeed: (x) => log.push(`speed ${x}`),
    pause: () => log.push('pause'),
    play: () => log.push('play'),
    update: (patch) => {
      log.push(`update ${Object.keys(patch).join(',')}`);
      if (reject) return Promise.reject(new Error(reject));
      return Promise.resolve();
    },
  };
  return h;
}

/** A live handle whose `scrubTo` clamps to what has been recorded so far, the way the real
 * server handle clamps to `recordedToS` (see useServerRun.ts). */
function liveClampedHandle(recordedToS: number) {
  const log: string[] = [];
  let cursor = 0;
  const h: RunnerHandle & { log: string[]; moveTo(s: number): void } = {
    mode: 'server',
    log,
    moveTo: (s) => {
      cursor = s;
    },
    cursorS: () => cursor,
    scrubTo: (s) => {
      cursor = Math.min(s, recordedToS);
      log.push(`scrub ${s} -> ${cursor}`);
    },
    setSpeed: (x) => log.push(`speed ${x}`),
    pause: () => log.push('pause'),
    play: () => log.push('play'),
    update: () => Promise.resolve(),
  };
  return h;
}

/** A replay handle whose seek lands slightly past what was asked for, the way a frame-quantized
 * recording might snap to the nearest available frame. */
function overshootingReplayHandle(overshootBy: number) {
  const log: string[] = [];
  let cursor = 0;
  const h: RunnerHandle & { log: string[] } = {
    mode: 'replay',
    log,
    cursorS: () => cursor,
    scrubTo: (s) => {
      log.push(`scrub ${s}`);
      cursor = s + overshootBy;
    },
    setSpeed: (x) => log.push(`speed ${x}`),
    pause: () => log.push('pause'),
    play: () => log.push('play'),
    update: () => Promise.resolve(),
  };
  return h;
}

const script: WalkthroughScript = {
  id: 'fake',
  dynamic: 0,
  title: 'fake',
  summary: '',
  scenario: {},
  steps: [
    { at_sim_s: 60, title: 'one', body: [] },
    { at_sim_s: 120, speed: 5, set: { 'workload.rps': 40 }, title: 'two', body: [] },
    { at_sim_s: 180, title: 'three', body: [] },
  ],
};

// ---------------------------------------------------------------------------

await check('(a) steps advance in order to each at_sim_s', async () => {
  const h = fakeHandle('mock');
  const r = new runner.WalkthroughRunner(script, h);
  eq(r.state().index, -1, 'before the first step');
  const reached: number[] = [];
  for (let i = 0; i < script.steps.length; i++) {
    const s = await r.next();
    eq(s.index, i, 'step index');
    ok(s.advancing, `step ${i} advancing`);
    h.moveTo(s.step.at_sim_s - 1);
    ok(r.tick().advancing, `step ${i} still advancing one second short`);
    h.moveTo(s.step.at_sim_s);
    const t = r.tick();
    ok(!t.advancing, `step ${i} paused at its timestamp`);
    reached.push(h.cursorS());
  }
  eq(reached, [60, 120, 180], 'timestamps reached in order');
  eq(h.log.filter((l) => l === 'pause').length, 3, 'one pause per step');
  return `pauses at ${reached.join(', ')}`;
});

await check('(b) set is applied before the advance, live', async () => {
  const h = fakeHandle('server');
  const r = new runner.WalkthroughRunner(script, h);
  await r.next();
  h.log.length = 0;
  const s = await r.next();
  eq(s.reason, undefined, 'no reason when the update is accepted');
  eq(h.log.slice(0, 3), ['update workload.rps', 'speed 5', 'play'], 'update, then speed, then play');
  return h.log.join(' > ');
});

await check('(b2) a live step without a speed runs at the live default', async () => {
  const h = fakeHandle('server');
  const r = new runner.WalkthroughRunner(script, h);
  await r.next();
  eq(h.log, ['speed 1', 'play'], 'first step at 1x');
  return h.log.join(' > ');
});

await check('(c) in replay, set is refused with the reason and the step still advances', async () => {
  const h = fakeHandle('replay');
  const r = new runner.WalkthroughRunner(script, h);
  await r.next();
  h.log.length = 0;
  const s = await r.next();
  eq(s.reason, runner.REPLAY_SET_REFUSED, 'refusal reason');
  ok(!h.log.some((l) => l.startsWith('update')), 'no update sent to a recording');
  eq(h.log, ['scrub 120', 'pause'], 'seeks to the timestamp and pauses');
  eq(s.advancing, false, 'already there');
  eq(s.index, 1, 'on step two');
  return s.reason ?? '';
});

await check('(d) a rejected live update yields its reason, and the step still advances', async () => {
  const h = fakeHandle('server', 'UpdateWorkload: HTTP 501 not implemented');
  const r = new runner.WalkthroughRunner(script, h);
  await r.next();
  const s = await r.next();
  eq(s.reason, 'UpdateWorkload: HTTP 501 not implemented', 'the rejection is the reason');
  ok(s.advancing, 'still advancing after the refusal');
  ok(h.log.includes('speed 5'), 'speed set after the refusal');
  return s.reason ?? '';
});

await check('(e) done after the last step, and next() past it is a no-op', async () => {
  const h = fakeHandle('mock');
  const r = new runner.WalkthroughRunner(script, h);
  for (let i = 0; i < script.steps.length; i++) {
    const s = await r.next();
    ok(!s.done, `step ${i} not done while advancing`);
    h.moveTo(s.step.at_sim_s);
    r.tick();
  }
  const last = r.state();
  ok(last.done, 'done on the last step once paused');
  const again = await r.next();
  ok(again === last, 'next() past the end returns the same state');
  return `done at ${last.step.at_sim_s}s`;
});

await check('skip seeks to the timestamp and pauses; idle skip is a no-op', async () => {
  const h = fakeHandle('mock');
  const r = new runner.WalkthroughRunner(script, h);
  const before = await r.next();
  const s = r.skip();
  eq(h.log.slice(-2), ['scrub 60', 'pause'], 'seek then pause');
  ok(!s.advancing && s !== before, 'settled into a new state');
  ok(r.skip() === s, 'no-op when paused');
  return h.log.join(' > ');
});

await check('skip_on_a_live_run_keeps_advancing_until_the_cursor_arrives', async () => {
  const h = liveClampedHandle(40);
  const r = new runner.WalkthroughRunner(script, h);
  await r.next(); // step 0, at_sim_s 60
  const s = r.skip();
  ok(s.advancing, 'the live run has not reached 60s yet, so the step keeps advancing');
  ok(!h.log.includes('pause'), 'not settled while short of the timestamp');
  h.moveTo(60);
  const t = r.tick();
  ok(!t.advancing, 'settles once the cursor actually arrives');
  eq(h.log[h.log.length - 1], 'pause', 'paused after catching up');
  return h.log.join(' > ');
});

await check('a_cursor_already_past_the_step_rewinds_on_replay_and_explains_on_live', async () => {
  // Replay: an imperfect seek that lands past the target is corrected before settling.
  const hr = overshootingReplayHandle(5);
  const rr = new runner.WalkthroughRunner(script, hr);
  const sr = await rr.next(); // target 60
  eq(hr.log.filter((l) => l === 'scrub 60').length, 2, 'scrubbed to seek, then again to rewind onto the target');
  ok(!sr.advancing, 'settled despite the overshoot');
  eq(sr.reason, undefined, 'replay puts itself back exactly, so there is nothing to explain');

  // Live: the cursor is already past the target (the viewer scrubbed ahead); it cannot rewind.
  const hl = fakeHandle('server');
  const rl = new runner.WalkthroughRunner(script, hl);
  await rl.next(); // target 60, cursor still 0
  hl.moveTo(200);
  const sl = rl.tick();
  eq(sl.reason, 'the run is already past this step', 'live explains rather than rewinding');
  ok(!sl.advancing, 'settled in place');
  eq(hl.cursorS(), 200, 'the live cursor is not moved backward');
  ok(!hl.log.some((l) => l.startsWith('scrub')), 'no seek attempted on a live run');
  return `replay: ${sr.reason ?? 'rewound'}; live: ${sl.reason}`;
});

await check('a_run_that_ends_early_settles_with_a_reason', async () => {
  const h = fakeHandle('server');
  const r = new runner.WalkthroughRunner(script, h);
  await r.next(); // target 60, cursor 0
  h.moveTo(45);
  h.end();
  const s = r.tick();
  eq(s.reason, 'the run ended at 45s before this step', 'explains why the step never arrived');
  ok(!s.advancing, 'settled rather than stuck "advancing…" forever');
  return s.reason ?? '';
});

console.log(`${cases - failures}/${cases} passed`);
if (failures > 0) throw new Error(`${failures} walkthrough runner case(s) failed`);
