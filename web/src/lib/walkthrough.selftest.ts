// Self-test for the walkthrough script format. No network, no browser, no framework.
//
//   cd web && node --experimental-strip-types src/lib/walkthrough.selftest.ts
//
// Same shape as replay.selftest.ts: one line per case, a summary line, a throw when anything
// failed. Loads every script named in public/walkthroughs/index.json straight off disk (not
// through fetch, so this runs outside a browser) and pushes it through the same `validate()` the
// runtime loader calls.
//
// The allow-list below is the run ids `sim-run export --demos` actually wrote, taken from
// `runs/index.json` at export time. It covers all ten demo groups, including 7-10
// (disable_decode, least_kv_probe, deadline_aware, fair_share), whose ids are now confirmed.

import type { WalkthroughScript } from './walkthrough';

// The imports below are dynamic and, for local modules, carry the `.ts` extension the same way
// api.selftest.ts's do: Node's type stripping resolves the real file name, while the project's
// tsconfig does not enable `allowImportingTsExtensions` and this file does not own tsconfig. Node
// builtins go through a variable specifier for the same reason api.selftest.ts's fs import does —
// there is no @types/node in this project, so a literal `from 'node:fs'` fails to type-check.

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

const walkthrough = await load<typeof import('./walkthrough')>('walkthrough');

const fsModule = 'node:fs';
const fs = (await import(fsModule)) as { readFileSync: (p: string, enc: 'utf8') => string };
const urlModule = 'node:url';
const nodeUrl = (await import(urlModule)) as { fileURLToPath: (u: string | URL) => string };
const pathModule = 'node:path';
const path = (await import(pathModule)) as { dirname: (p: string) => string; join: (...parts: string[]) => string };

const HERE = path.dirname(nodeUrl.fileURLToPath(import.meta.url));
const WALKTHROUGHS_DIR = path.join(HERE, '..', '..', 'public', 'walkthroughs');

function readJson<T>(file: string): T {
  return JSON.parse(fs.readFileSync(path.join(WALKTHROUGHS_DIR, file), 'utf8')) as T;
}

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

function eq<T>(actual: T, expected: T, what: string): void {
  if (actual !== expected) throw new Error(`${what}: expected ${JSON.stringify(expected)}, got ${JSON.stringify(actual)}`);
}

// ---------------------------------------------------------------------------
// fixtures: the exported run ids, and the eleven selected walkthrough ids
// ---------------------------------------------------------------------------

/** Every run id `sim-run export --demos` wrote for the six existing demo groups. */
const EXPORTED_RUN_IDS = new Set([
  '1-routing/round-robin',
  '1-routing/p2c',
  '1-routing/least-requests',
  '1-routing/random',
  '2-staleness/telemetry_interval_ms=100',
  '2-staleness/telemetry_interval_ms=250',
  '2-staleness/telemetry_interval_ms=500',
  '2-staleness/telemetry_interval_ms=1000',
  '2-staleness/telemetry_interval_ms=2000',
  '2-staleness/telemetry_interval_ms=4000',
  '3-chunking/step_token_budget=512',
  '3-chunking/step_token_budget=1024',
  '3-chunking/step_token_budget=2048',
  '3-chunking/step_token_budget=4096',
  '3-chunking/step_token_budget=8192',
  '3-chunking/step_token_budget=16384',
  '4-load-curve/arrival_rps=30',
  '4-load-curve/arrival_rps=70',
  '4-load-curve/arrival_rps=110',
  '4-load-curve/arrival_rps=150',
  '4-load-curve/arrival_rps=190',
  '4-load-curve/arrival_rps=230',
  '5-long-context/long_probability=0.0',
  '5-long-context/long_probability=0.04',
  '5-long-context/long_probability=0.08',
  '5-long-context/long_probability=0.16',
  '5-long-context/long_probability=0.32',
  '6-retry/no-retries',
  '6-retry/retry-with-budget',
  '6-retry/retry-storm-no-budget',
  '7-no-decode/round-robin-no-decode',
  '7-no-decode/p2c-no-decode',
  '8-admission/accept-all',
  '8-admission/deadline-aware',
  '9-fair-share/tenants-accept-all',
  '9-fair-share/fair-share',
  '10-probes/p2c',
  '10-probes/least-kv-probe',
  '11-preemption/kv-spiral-never',
  '11-preemption/kv-spiral-swap',
  '12-spec-decode/spec-off',
  '12-spec-decode/spec-n4',
  '13-herd-fleet/replicas=32',
  '13-herd-fleet/replicas=64',
  '13-herd-fleet/replicas=128',
  '13-herd-fleet/replicas=256',
  '13-herd-fleet/replicas=512',
  '14-affinity/affinity-off',
  '14-affinity/affinity-spread',
  '14-affinity/affinity-sticky',
  '15-gray-failure/gray-failure-none',
  '15-gray-failure/gray-failure-eject',
  '16-scheduling/sched-fifo',
  '16-scheduling/sched-class',
  '16-scheduling/sched-deadline',
  '17-cascade/cascade-p2c',
  '17-cascade/cascade-affinity',
  '18-bode/perturb_frequency_hz=0.01',
  '18-bode/perturb_frequency_hz=0.02',
  '18-bode/perturb_frequency_hz=0.05',
  '18-bode/perturb_frequency_hz=0.1',
  '18-bode/perturb_frequency_hz=0.2',
  '18-bode/perturb_frequency_hz=0.5',
  '18-bode/perturb_frequency_hz=1.0',
  '19-tiering/tier-dram',
  '19-tiering/tier-dram-ssd',
  '19-tiering/tier-contended',
  '20-autoscaling/autoscale-none',
  '20-autoscaling/autoscale-cold30',
  '20-autoscaling/autoscale-cold5',
]);

/** U48's ten selected dynamics, plus spec-decode (U26b), prefix affinity (U27b), gray failure
 * (U31b), the affinity failover cascade (U27d), the scheduling seam's slo-classes (U108), the
 * staleness loop's Bode plot (U33), KV tiering (U30) and autoscaling with a cold start (U32):
 * every one of these must have a script with a `run` field. */
const SELECTED_IDS = [
  'rolling-hotspot',
  'stale-telemetry',
  'prefill-decode',
  'load-knee',
  'kv-capacity',
  'retry-storm',
  'decode-off',
  'least-kv-probe',
  'deadline-admission',
  'fair-share',
  'spec-decode',
  'herd-fleet-size',
  'affinity-vs-spread',
  'gray-failure',
  'affinity-cascade',
  'slo-classes',
  'stale-oscillation',
  'kv-tiering',
  'diurnal-autoscale',
];

// ---------------------------------------------------------------------------
// cases
// ---------------------------------------------------------------------------

interface Card {
  id: string;
  dynamic: number;
  script?: string;
}

const index = readJson<{ cards: Card[] }>('index.json');

check('index.json parses and has cards', () => {
  if (!Array.isArray(index.cards) || index.cards.length === 0) throw new Error('no cards');
  return `${index.cards.length} cards`;
});

const scripted = index.cards.filter((c) => c.script);
const scripts = new Map<string, WalkthroughScript>();

for (const card of scripted) {
  check(`${card.id}: loads and validates`, () => {
    const script = readJson<WalkthroughScript>(card.script as string);
    walkthrough.validate(script);
    scripts.set(card.id, script);
    return `${script.steps.length} steps`;
  });

  check(`${card.id}: at_sim_s strictly increasing`, () => {
    const script = scripts.get(card.id);
    if (!script) throw new Error('script did not load');
    let last = -Infinity;
    for (const step of script.steps) {
      if (step.at_sim_s <= last) throw new Error(`${step.at_sim_s} does not exceed ${last}`);
      last = step.at_sim_s;
    }
    return `${script.steps.length} steps, ${script.steps[0]?.at_sim_s}..${last}`;
  });

  check(`${card.id}: script id and dynamic match the card`, () => {
    const script = scripts.get(card.id);
    if (!script) throw new Error('script did not load');
    eq(script.id, card.id, 'id');
    eq(script.dynamic, card.dynamic, 'dynamic');
    return `id ${script.id}, dynamic ${script.dynamic}`;
  });
}

for (const id of SELECTED_IDS) {
  check(`${id}: is one of the selected dynamics and has a card with a script`, () => {
    const card = index.cards.find((c) => c.id === id);
    if (!card) throw new Error('no card in index.json');
    if (!card.script) throw new Error('card has no script');
    return `card present, script ${card.script}`;
  });

  check(`${id}: run is present`, () => {
    const script = scripts.get(id);
    if (!script) throw new Error('script did not load');
    if (typeof script.run !== 'string' || !script.run) throw new Error('run missing');
    return script.run;
  });

  check(`${id}: run (and compare, if set) is an exported run id`, () => {
    const script = scripts.get(id);
    if (!script) throw new Error('script did not load');
    if (!script.run || !EXPORTED_RUN_IDS.has(script.run)) {
      throw new Error(`run ${JSON.stringify(script.run)} is not in the exported run id allow-list`);
    }
    if (script.compare !== undefined && !EXPORTED_RUN_IDS.has(script.compare)) {
      throw new Error(`compare ${JSON.stringify(script.compare)} is not in the exported run id allow-list`);
    }
    return `run ${script.run}${script.compare ? `, compare ${script.compare}` : ''}`;
  });
}

check('no two scripts share an id', () => {
  const seen = new Map<string, string>();
  for (const [cardId, script] of scripts) {
    const prior = seen.get(script.id);
    if (prior) throw new Error(`${cardId} and ${prior} both produce script id ${script.id}`);
    seen.set(script.id, cardId);
  }
  return `${seen.size} distinct script ids`;
});

// ---------------------------------------------------------------------------
// scenario keys with no control-panel field: they must survive into the StartRun text
// ---------------------------------------------------------------------------

const api = await load<typeof import('./api')>('api');

check('spec-decode: spec_draft_tokens and spec_accept_rate reach the wire', () => {
  const script = readJson<WalkthroughScript>('spec-decode.json');
  const fields = api.scenarioConfigToWire(walkthrough.scenarioFor(script)).fields;
  eq(fields.spec_draft_tokens, 4, 'spec_draft_tokens');
  eq(fields.spec_accept_rate, 0.7, 'spec_accept_rate');
  return 'spec_draft_tokens = 4, spec_accept_rate = 0.7';
});

check('kv-spiral: the live run is kv_spiral_never.txt, sessions and preemption included', () => {
  const script = readJson<WalkthroughScript>('kv-spiral.json');
  const fields = api.scenarioConfigToWire(walkthrough.scenarioFor(script)).fields;
  eq(fields.preemption, 'never', 'preemption');
  eq(fields.session_turns_mean, 8, 'session_turns_mean');
  eq(fields.session_think_s, 8, 'session_think_s');
  eq(fields.replicas, 4, 'replicas');
  eq(fields.max_batch, 64, 'max_batch');
  eq(fields.kv_capacity_tokens, 30000, 'kv_capacity_tokens');
  eq(fields.arrival_rps, 2, 'arrival_rps');
  eq(fields.long_probability, 0, 'long_probability');
  eq(fields.routing, 'p2c', 'routing');
  return 'preemption = never, session_turns_mean = 8, fleet of 4 at 30k KV';
});

check('least-kv-probe: routing.kind reaches the wire as least_kv_probe, not dropped', () => {
  const script = readJson<WalkthroughScript>('least-kv-probe.json');
  const wire = api.scenarioConfigToWire(walkthrough.scenarioFor(script));
  eq(wire.fields.routing, 'least_kv_probe', 'routing');
  if (wire.dropped.includes('routing.kind')) throw new Error(`routing.kind was dropped: ${JSON.stringify(wire.dropped)}`);
  return 'routing = least_kv_probe, routing.kind not dropped';
});

check('a scenario key the engine does not accept fails at encode time', () => {
  const script = readJson<WalkthroughScript>('spec-decode.json');
  const config = walkthrough.scenarioFor({ ...script, scenario: { ...script.scenario, preemptoin: 'never' } });
  let threw = '';
  try {
    api.scenarioConfigToWire(config);
  } catch (e) {
    threw = e instanceof Error ? e.message : String(e);
  }
  if (!/preemptoin/.test(threw)) throw new Error(`expected a throw naming the key, got ${JSON.stringify(threw)}`);
  return threw;
});

// ---------------------------------------------------------------------------

console.log(`${cases} cases, ${cases - failures} passed, ${failures} failed`);
if (failures > 0) throw new Error(`${failures} of ${cases} walkthrough self-test cases failed`);
