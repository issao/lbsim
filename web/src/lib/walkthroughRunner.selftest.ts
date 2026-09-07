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
  const h: RunnerHandle & { log: string[]; moveTo(s: number): void } = {
    mode,
    log,
    moveTo: (s) => {
      cursor = s;
    },
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

console.log(`${cases - failures}/${cases} passed`);
if (failures > 0) throw new Error(`${failures} walkthrough runner case(s) failed`);
