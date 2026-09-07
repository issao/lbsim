// Self-test for the update-banner wording. No network, no browser, no framework.
//
//   cd web && node --experimental-strip-types src/lib/updateBanner.selftest.ts
//
// Same shape as replay.selftest.ts: one line per case, a summary line, a throw when anything
// failed.
//
// Why the resolve hook below: updateBanner.ts imports `./types` the way every other module does,
// extensionless, which Vite resolves and Node's ESM loader does not. The hook tries
// `<specifier>.ts` for a bare relative specifier and otherwise defers, so the module under test
// stays written like the rest of the app.

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

const updateBanner = await load<typeof import('./updateBanner')>('updateBanner');
type UpdateResponse = import('./types').UpdateResponse;

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

function eq(actual: unknown, expected: unknown, what: string): void {
  if (actual !== expected) throw new Error(`${what}: expected ${JSON.stringify(expected)}, got ${JSON.stringify(actual)}`);
}

function ok(cond: boolean, what: string): void {
  if (!cond) throw new Error(what);
}

// ---------------------------------------------------------------------------

check('a rejected update is kind rejected with the reason verbatim', () => {
  const u: UpdateResponse = {
    accepted: false,
    requiredResimulation: false,
    rewoundToS: 12,
    rejectedReason: 'replay of a recorded run; changing load or policy needs a live engine',
    changed: ['fleet.replicas'],
  };
  const t = updateBanner.updateBannerText(u);
  eq(t.kind, 'rejected', 'kind');
  eq(t.headline, 'not applied', 'headline');
  eq(t.detail, u.rejectedReason, 'detail is the reason verbatim');
  return `${t.kind} / ${t.headline}`;
});

check('a rejected update from a 501 also reaches rejected, not resim', () => {
  const u: UpdateResponse = {
    accepted: false,
    requiredResimulation: false,
    rewoundToS: 0,
    rejectedReason: 'not supported yet',
    changed: [],
  };
  const t = updateBanner.updateBannerText(u);
  eq(t.kind, 'rejected', 'kind');
  eq(t.detail, 'not supported yet', 'detail is the 501 reason verbatim');
  return t.detail;
});

check('requiredResimulation true is kind resim with the rewind time', () => {
  const u: UpdateResponse = {
    accepted: true,
    requiredResimulation: true,
    rewoundToS: 30,
    rejectedReason: '',
    changed: ['fleet.replicas'],
  };
  const t = updateBanner.updateBannerText(u);
  eq(t.kind, 'resim', 'kind');
  eq(t.headline, 'required_resimulation = true', 'headline');
  ok(t.detail.includes('30 s'), 'detail names the rewind time');
  ok(t.detail.includes('re-simulated'), 'detail says re-simulated');
  return t.detail;
});

check('accepted without resimulation is kind applied', () => {
  const u: UpdateResponse = {
    accepted: true,
    requiredResimulation: false,
    rewoundToS: 42,
    rejectedReason: '',
    changed: ['slo.thresholdMs'],
  };
  const t = updateBanner.updateBannerText(u);
  eq(t.kind, 'applied', 'kind');
  eq(t.headline, 'required_resimulation = false', 'headline');
  ok(t.detail.includes('nothing was re-simulated'), 'detail says nothing was re-simulated');
  return t.detail;
});

check('an accepted response never shows the words "not applied"', () => {
  const resim: UpdateResponse = { accepted: true, requiredResimulation: true, rewoundToS: 5, rejectedReason: '', changed: ['x'] };
  const applied: UpdateResponse = { accepted: true, requiredResimulation: false, rewoundToS: 5, rejectedReason: '', changed: ['x'] };
  for (const u of [resim, applied]) {
    const t = updateBanner.updateBannerText(u);
    ok(!t.headline.includes('not applied'), 'headline must not say not applied');
    ok(!t.detail.includes('not applied'), 'detail must not say not applied');
  }
  return 'neither accepted case says "not applied"';
});

// ---------------------------------------------------------------------------

console.log(`${cases} cases, ${cases - failures} passed, ${failures} failed`);
if (failures > 0) throw new Error(`${failures} of ${cases} update-banner self-test cases failed`);
