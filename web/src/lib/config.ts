// The scenario shape the control panel edits, and the classification that decides whether a change
// is view-only or physics. `scenarios/*.txt` in the repo root is the source for the preset numbers.

import type { RoutingKind } from './types';

export interface Workload {
  arrivalRps: number;
  promptMean: number;
  promptCv: number;
  outputMean: number;
  outputCv: number;
  longProbability: number;
  longPromptMean: number;
  longOutputMean: number;
  perturbation: 'none' | 'step' | 'sinusoid';
  perturbAmplitude: number;
  perturbFrequencyHz: number;
}

export interface Fleet {
  replicas: number;
  maxBatch: number;
  stepBaseMs: number;
  stepPerSeqMs: number;
  /** Engine `step_per_kv_ktoken_ms`: the bandwidth term of the step cost, per thousand resident KV tokens. */
  stepPerKvKtokenMs: number;
  prefillTokensPerS: number;
  kvTokensPerReplica: number;
  /** Engine `step_token_budget`: the prefill token budget per step, the other cap beside `maxBatch`. */
  stepTokenBudget: number;
  maxQueue: number;
  accelerator: string;
}

export interface RoutingConfig {
  kind: RoutingKind;
  /** PowerOfTwoChoices.choices */
  choices: number;
  /** PowerOfTwoChoices.probe_live */
  probeLive: boolean;
  /** PrefixAffinity.max_load_ratio */
  maxLoadRatio: number;
  /** PrefixAffinity.fallback_choices */
  fallbackChoices: number;
}

export interface Slo {
  ttftMs: number;
  itlMs: number;
  e2eS: number;
}

export interface ScenarioConfig {
  name: string;
  seed: number;
  durationS: number;
  warmupS: number;
  workload: Workload;
  fleet: Fleet;
  routing: RoutingConfig;
  telemetryIntervalMs: number;
  telemetryDelayMs: number;
  /** Engine `client_timeout_s` and `max_attempts`: the client's patience and its retry cap. */
  clientTimeoutS: number;
  maxAttempts: number;
  slo: Slo;
  /** Points per simulated second. A view parameter: it changes the chart, not the physics. */
  samplesPerSimSecond: number;
}

/**
 * scenarios/base.txt, verbatim. Every engine key that file sets has a field here, so a run started
 * from this config is the same run `sim-run run scenarios/base.txt` performs; the transport
 * self-test reads the file and checks. The mock engine is tuned around these numbers too, so a
 * drift here would make the stand-in's dynamics and the server's disagree for no reason.
 */
export const BASE: ScenarioConfig = {
  name: 'base',
  seed: 20260906,
  durationS: 120,
  warmupS: 15,
  workload: {
    arrivalRps: 70,
    promptMean: 1200,
    promptCv: 1.2,
    outputMean: 300,
    outputCv: 1.5,
    longProbability: 0.08,
    longPromptMean: 24000,
    longOutputMean: 400,
    perturbation: 'none',
    perturbAmplitude: 0.3,
    perturbFrequencyHz: 0.05,
  },
  fleet: {
    replicas: 32,
    maxBatch: 256,
    stepBaseMs: 10.2,
    stepPerSeqMs: 0.0,
    stepPerKvKtokenMs: 0.0175,
    prefillTokensPerS: 28286,
    kvTokensPerReplica: 1370000,
    stepTokenBudget: 1024,
    maxQueue: 400,
    accelerator: '8xH100-80GB',
  },
  routing: {
    kind: 'round_robin',
    choices: 2,
    probeLive: false,
    maxLoadRatio: 1.3,
    fallbackChoices: 2,
  },
  telemetryIntervalMs: 1000,
  telemetryDelayMs: 200,
  clientTimeoutS: 60,
  maxAttempts: 1,
  slo: { ttftMs: 2000, itlMs: 80, e2eS: 60 },
  // base.txt records at sample_interval_ms = 250.
  samplesPerSimSecond: 4,
};

export function cloneConfig(c: ScenarioConfig): ScenarioConfig {
  return {
    ...c,
    workload: { ...c.workload },
    fleet: { ...c.fleet },
    routing: { ...c.routing },
    slo: { ...c.slo },
  };
}

export interface Preset {
  id: string;
  title: string;
  file: string;
  summary: string;
  apply: (c: ScenarioConfig) => ScenarioConfig;
}

/** One click each. `file` names the scenario file the real StartRun would send. */
export const PRESETS: Preset[] = [
  {
    id: 'base',
    title: 'Baseline, round robin',
    file: 'scenarios/base.txt',
    summary: '32 replicas, 70 rps, 8 % long requests. The shared baseline every comparison changes one thing from.',
    apply: () => cloneConfig(BASE),
  },
  {
    id: 'p2c',
    title: 'Baseline, power of two choices',
    file: 'scenarios/route_p2c.txt',
    summary: 'Identical to base including the seed, routing only differs. The A/B pair.',
    apply: () => {
      const c = cloneConfig(BASE);
      c.name = 'p2c';
      c.routing = { ...c.routing, kind: 'power_of_two_choices', choices: 2 };
      return c;
    },
  },
  {
    id: 'hotspot',
    title: 'Rolling hotspot, heterogeneous sizes',
    file: 'scenarios/route_round_robin.txt',
    summary: 'Dynamic 1. Round robin plus a heavy tail: equal request counts, wildly unequal work.',
    apply: () => {
      const c = cloneConfig(BASE);
      c.name = 'rolling-hotspot';
      c.routing = { ...c.routing, kind: 'round_robin' };
      c.workload = { ...c.workload, arrivalRps: 85, longProbability: 0.12 };
      return c;
    },
  },
  {
    id: 'stale',
    title: 'Stale telemetry oscillation',
    file: 'scenarios/route_least_queue_tokens.txt',
    summary: 'Dynamic 2. Least-KV-tokens with a 900 ms scrape delay: every router picks the same idle replica.',
    apply: () => {
      const c = cloneConfig(BASE);
      c.name = 'stale-telemetry';
      c.routing = { ...c.routing, kind: 'least_kv_tokens' };
      c.telemetryDelayMs = 900;
      c.telemetryIntervalMs = 1500;
      c.workload = { ...c.workload, arrivalRps: 105 };
      return c;
    },
  },
  {
    id: 'kv-spiral',
    title: 'KV pressure at modest rps',
    file: 'scenarios/base.txt',
    summary: 'Dynamic 4. Long contexts held between turns: sessions fill the cache at a request rate the fleet could otherwise serve.',
    apply: () => {
      const c = cloneConfig(BASE);
      c.name = 'kv-pressure';
      c.workload = { ...c.workload, arrivalRps: 55, longProbability: 0.25, longPromptMean: 16000 };
      c.fleet = { ...c.fleet, kvTokensPerReplica: 1140000 };
      return c;
    },
  },
  {
    id: 'overload',
    title: 'Past the knee',
    file: 'scenarios/base.txt',
    summary: 'Offered load above rated capacity, so queues fill and goodput separates from throughput.',
    apply: () => {
      const c = cloneConfig(BASE);
      c.name = 'overload';
      c.workload = { ...c.workload, arrivalRps: 210 };
      c.routing = { ...c.routing, kind: 'power_of_two_choices' };
      return c;
    },
  },
];

export const ROUTING_LABEL: Record<RoutingKind, string> = {
  round_robin: 'Round robin',
  random: 'Random',
  least_requests: 'Least requests',
  least_kv_tokens: 'Least KV tokens',
  power_of_two_choices: 'Power of two choices',
  prefix_affinity: 'Prefix affinity',
};

/** The comment on each `RoutingPolicy` variant in scenario.proto, condensed. */
export const ROUTING_NOTE: Record<RoutingKind, string> = {
  round_robin: 'Equal request counts. Wrong unit: capacity is denominated in tokens, so one long context costs what 25 chat turns cost.',
  random: 'The null hypothesis. Same expected share as round robin, higher variance, no rotating structure.',
  least_requests: 'A baseline that is wrong for this domain, for the same reason round robin is: it counts requests.',
  least_kv_tokens: 'The same idea in the right unit. Correct when telemetry is fresh; herds when it is not.',
  power_of_two_choices: 'Sample d replicas, take the least loaded. Bounds herding by construction, and is O(1).',
  prefix_affinity: 'Prefer the replica holding this prefix until imbalance exceeds the cap. Trades balance for cache hits.',
};

/**
 * Which parameters change physics, per UpdateResponse.required_resimulation.
 *
 * The dotted paths that are absent from this set are view-only: the SLO thresholds, because
 * attainment is derived from recorded histograms, and the sample rate, because it only decides how
 * many points a chart draws.
 */
const PHYSICS_PATHS = new Set([
  'seed', 'durationS', 'warmupS',
  'workload.arrivalRps', 'workload.promptMean', 'workload.promptCv',
  'workload.outputMean', 'workload.outputCv', 'workload.longProbability',
  'workload.longPromptMean', 'workload.longOutputMean',
  'workload.perturbation', 'workload.perturbAmplitude', 'workload.perturbFrequencyHz',
  'fleet.replicas', 'fleet.maxBatch', 'fleet.stepBaseMs', 'fleet.stepPerSeqMs', 'fleet.stepPerKvKtokenMs',
  'fleet.prefillTokensPerS', 'fleet.kvTokensPerReplica', 'fleet.stepTokenBudget', 'fleet.maxQueue', 'fleet.accelerator',
  'routing.kind', 'routing.choices', 'routing.probeLive',
  'routing.maxLoadRatio', 'routing.fallbackChoices',
  'telemetryIntervalMs', 'telemetryDelayMs', 'clientTimeoutS', 'maxAttempts',
]);

export const VIEW_ONLY_EXPLANATION: Record<string, string> = {
  'slo.ttftMs': 'attainment is recomputed from the recorded histograms',
  'slo.itlMs': 'attainment is recomputed from the recorded histograms',
  'slo.e2eS': 'attainment is recomputed from the recorded histograms',
  samplesPerSimSecond: 'a chart density, not a physical quantity',
};

export const FIELD_LABEL: Record<string, string> = {
  seed: 'seed',
  durationS: 'duration',
  warmupS: 'warm-up',
  'workload.arrivalRps': 'arrival rate',
  'workload.promptMean': 'prompt mean',
  'workload.promptCv': 'prompt spread',
  'workload.outputMean': 'output mean',
  'workload.outputCv': 'output spread',
  'workload.longProbability': 'long-request probability',
  'workload.longPromptMean': 'long prompt mean',
  'workload.longOutputMean': 'long output mean',
  'workload.perturbation': 'perturbation',
  'workload.perturbAmplitude': 'perturbation amplitude',
  'workload.perturbFrequencyHz': 'perturbation frequency',
  'fleet.replicas': 'replica count',
  'fleet.maxBatch': 'max batch',
  'fleet.stepBaseMs': 'step base',
  'fleet.stepPerSeqMs': 'step per sequence',
  'fleet.stepPerKvKtokenMs': 'step per KV ktoken',
  'fleet.prefillTokensPerS': 'prefill rate',
  'fleet.kvTokensPerReplica': 'KV capacity',
  'fleet.stepTokenBudget': 'step token budget',
  'fleet.maxQueue': 'max queue',
  'fleet.accelerator': 'accelerator',
  'routing.kind': 'routing policy',
  'routing.choices': 'choices (d)',
  'routing.probeLive': 'probe live',
  'routing.maxLoadRatio': 'max load ratio',
  'routing.fallbackChoices': 'fallback choices',
  telemetryIntervalMs: 'telemetry interval',
  telemetryDelayMs: 'telemetry delay',
  clientTimeoutS: 'client timeout',
  maxAttempts: 'max attempts',
  'slo.ttftMs': 'TTFT SLO',
  'slo.itlMs': 'ITL SLO',
  'slo.e2eS': 'end-to-end SLO',
  samplesPerSimSecond: 'sample rate',
};

function flat(c: ScenarioConfig): Record<string, unknown> {
  const out: Record<string, unknown> = {
    seed: c.seed, durationS: c.durationS, warmupS: c.warmupS,
    telemetryIntervalMs: c.telemetryIntervalMs, telemetryDelayMs: c.telemetryDelayMs,
    clientTimeoutS: c.clientTimeoutS, maxAttempts: c.maxAttempts,
    samplesPerSimSecond: c.samplesPerSimSecond,
  };
  for (const [k, v] of Object.entries(c.workload)) out[`workload.${k}`] = v;
  for (const [k, v] of Object.entries(c.fleet)) out[`fleet.${k}`] = v;
  for (const [k, v] of Object.entries(c.routing)) out[`routing.${k}`] = v;
  for (const [k, v] of Object.entries(c.slo)) out[`slo.${k}`] = v;
  return out;
}

export interface ConfigDiff {
  paths: string[];
  physicsPaths: string[];
  viewPaths: string[];
}

export function diffConfig(a: ScenarioConfig, b: ScenarioConfig): ConfigDiff {
  const fa = flat(a);
  const fb = flat(b);
  const paths: string[] = [];
  for (const k of Object.keys(fa)) if (fa[k] !== fb[k]) paths.push(k);
  return {
    paths,
    physicsPaths: paths.filter((p) => PHYSICS_PATHS.has(p)),
    viewPaths: paths.filter((p) => !PHYSICS_PATHS.has(p)),
  };
}

/** Does the routing policy alone differ? The A/B view's admission test. */
export function comparability(a: ScenarioConfig, b: ScenarioConfig): {
  ok: boolean;
  blocking: string[];
  differing: string[];
} {
  const d = diffConfig(a, b);
  const routingOnly = d.paths.filter((p) => !p.startsWith('routing.'));
  const blocking = routingOnly.filter((p) => p !== 'name' && p !== 'samplesPerSimSecond');
  return { ok: blocking.length === 0, blocking, differing: d.paths };
}
