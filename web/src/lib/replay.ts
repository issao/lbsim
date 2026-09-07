// Replay source: a recorded run, loaded from static files instead of an Ingress server.
//
// `sim-run export` writes each run as the exact documents the live stream carries
// (crates/sim-ingress/src/export.rs): `runs/index.json`, then per run `status.json` (a RunStatus),
// `fleet.jsonl` (one SubscriptionUpdate per sample instant, SCOPE_FLEET), `result.json` (a
// RunResult) and `scenario.txt` (the resolved scenario). The transport's decoders in api.ts read
// them unchanged, so there is no second wire format here; what this file adds is the fetching, the
// one document without a proto behind it (the index), the scenario text back into the control
// panel's `ScenarioConfig`, and a `FrameSource` over the decoded frames so the panels can read a
// replay exactly as they read the mock engine.
//
// Everything below is React-free and network-injectable so the self-test can drive it under Node.
// Time is handled as the transport handles it: absolute instants stay `bigint`, and the only float
// form is seconds relative to the run's `sim_start_unix_ns` from the index, which is where the
// engine's clock started (warm-up included), so `simS` runs from 0 to the scenario's `duration_s`.

import {
  type Json,
  type RunResult,
  type RunStatus,
  type SubscriptionUpdate,
  EXTRA_KEYS,
  ROUTING_TO_ENGINE,
  decodeRunResult,
  decodeRunStatus,
  decodeSubscriptionUpdate,
  dbl,
  i32,
  isObject,
  parseScenarioText,
  relSeconds,
  str,
  u64,
} from './api';
import { BASE, cloneConfig, type ScenarioConfig } from './config';
import type { RoutingKind } from './types';
import type { Frame, FleetEvent } from './engine';
import type { FrameSource } from './useRun';
import { frameFromUpdate, type ReplayFrame } from './adapter';

// ---------------------------------------------------------------------------
// The index
// ---------------------------------------------------------------------------

/** One entry of `runs/index.json`. Field names are the exporter's; see export.rs `index_entry`. */
export interface RunIndexEntry {
  runId: string;
  name: string;
  routing: string;
  scenarioFile: string;
  simStartUnixNs: bigint;
  simEndUnixNs: bigint;
  sampleIntervalMs: number;
  replicas: number;
  /** The demo group, i.e. the run id's directory (`1-routing`), or `''` for a flat id. */
  group: string;
  /** The run id's last segment, what the picker shows inside a group. */
  label: string;
}

export function decodeRunIndex(v: Json): RunIndexEntry[] {
  if (!Array.isArray(v)) throw new TypeError('runs/index.json: expected an array of runs');
  return v.map((e, i) => {
    const where = `runs/index.json[${i}]`;
    if (!isObject(e)) throw new TypeError(`${where}: expected an object`);
    const runId = str(e.run_id, `${where}.run_id`);
    if (runId === '') throw new TypeError(`${where}: run_id is empty`);
    const slash = runId.lastIndexOf('/');
    return {
      runId,
      name: str(e.name, `${where}.name`),
      routing: str(e.routing, `${where}.routing`),
      scenarioFile: str(e.scenario_file, `${where}.scenario_file`),
      simStartUnixNs: u64(e.sim_start_unix_ns, `${where}.sim_start_unix_ns`),
      simEndUnixNs: u64(e.sim_end_unix_ns, `${where}.sim_end_unix_ns`),
      sampleIntervalMs: dbl(e.sample_interval_ms, `${where}.sample_interval_ms`),
      replicas: i32(e.replicas, `${where}.replicas`),
      group: slash === -1 ? '' : runId.slice(0, slash),
      label: slash === -1 ? runId : runId.slice(slash + 1),
    };
  });
}

/** The run's length in simulated seconds, warm-up included. */
export function runDurationS(e: RunIndexEntry): number {
  return relSeconds(e.simEndUnixNs, e.simStartUnixNs);
}

/** Groups in index order, each with its runs in index order. */
export function groupRuns(runs: RunIndexEntry[]): { group: string; runs: RunIndexEntry[] }[] {
  const out: { group: string; runs: RunIndexEntry[] }[] = [];
  for (const r of runs) {
    let g = out.find((x) => x.group === r.group);
    if (!g) out.push((g = { group: r.group, runs: [] }));
    g.runs.push(r);
  }
  return out;
}

// ---------------------------------------------------------------------------
// Fetching
// ---------------------------------------------------------------------------

export type FetchLike = (input: string, init?: RequestInit) => Promise<Response>;

/** Where the runs live: `runs/` under the served origin, exactly where the exporter puts them. */
export function runsBase(): string {
  // `import.meta.env` exists under Vite; under Node (the self-test) it does not, and `/` is right.
  const env = typeof import.meta !== 'undefined' ? (import.meta as { env?: { BASE_URL?: string } }).env : undefined;
  return `${env?.BASE_URL ?? '/'}runs/`;
}

function defaultFetch(): FetchLike {
  return (input, init) => fetch(input, init);
}

async function getText(url: string, f: FetchLike): Promise<string> {
  const res = await f(url, { method: 'GET', headers: { accept: 'application/json, text/plain' } });
  if (!res.ok) throw new Error(`GET ${url}: HTTP ${res.status}`);
  return res.text();
}

/**
 * `getText` for a document an older export may not have written: `null` on a 404, and on the HTML
 * shell a dev server serves with HTTP 200 in place of a missing file (see `probeRunIndex`).
 */
async function getOptionalText(url: string, f: FetchLike): Promise<string | null> {
  const res = await f(url, { method: 'GET', headers: { accept: 'application/json, text/plain' } });
  if (res.status === 404) return null;
  if (!res.ok) throw new Error(`GET ${url}: HTTP ${res.status}`);
  const text = await res.text();
  return text.trimStart().startsWith('<') ? null : text;
}

async function getJson(url: string, f: FetchLike): Promise<Json> {
  const text = await getText(url, f);
  try {
    return JSON.parse(text) as Json;
  } catch {
    throw new Error(`GET ${url}: not JSON: ${text.slice(0, 120)}`);
  }
}

/** `runs/index.json`, decoded. Throws when it is missing or malformed. */
export async function fetchRunIndex(f: FetchLike = defaultFetch(), base = runsBase()): Promise<RunIndexEntry[]> {
  return decodeRunIndex(await getJson(`${base}index.json`, f));
}

/**
 * Is there anything to replay? `null` when the index is absent, unreachable or not an index, which
 * is the normal state of a dev server with nothing copied into `web/public/runs/`: mock mode, not
 * an error. A dev server answers a missing file with its HTML shell and HTTP 200, which is why a
 * 200 alone is not enough and the body has to decode.
 */
export async function probeRunIndex(f: FetchLike = defaultFetch(), base = runsBase()): Promise<RunIndexEntry[] | null> {
  try {
    const runs = await fetchRunIndex(f, base);
    return runs.length > 0 ? runs : null;
  } catch {
    return null;
  }
}

/** Everything one dashboard needs about one recorded run. */
export interface LoadedRun {
  entry: RunIndexEntry;
  status: RunStatus;
  result: RunResult;
  scenarioText: string;
  config: ScenarioConfig;
  /** Scenario keys the control panel has no field for, so the UI can say what it is not showing. */
  unmapped: string[];
  frames: ReplayFrame[];
}

export async function loadRun(entry: RunIndexEntry, f: FetchLike = defaultFetch(), base = runsBase()): Promise<LoadedRun> {
  const dir = `${base}${entry.runId}/`;
  const [statusJson, fleetText, resultJson, scenarioText, replicasText] = await Promise.all([
    getJson(`${dir}status.json`, f),
    getText(`${dir}fleet.jsonl`, f),
    getJson(`${dir}result.json`, f),
    getText(`${dir}scenario.txt`, f),
    getOptionalText(`${dir}replicas.jsonl`, f),
  ]);
  const status = decodeRunStatus(statusJson, `${entry.runId}/status.json`);
  const result = decodeRunResult(resultJson, `${entry.runId}/result.json`);
  const { config, unmapped } = configFromScenarioText(scenarioText);
  const replicas = replicasText === null ? new Map() : parseReplicasJsonl(replicasText);
  const frames = parseFleetJsonl(fleetText, entry.simStartUnixNs, replicas);
  if (frames.length === 0) throw new Error(`${entry.runId}/fleet.jsonl: no samples`);
  return { entry, status, result, scenarioText, config, unmapped, frames };
}

/** Every SubscriptionUpdate in a `.jsonl` document, blank lines skipped, in file order. */
function parseUpdates(text: string, name: string): SubscriptionUpdate[] {
  const out: SubscriptionUpdate[] = [];
  let lineNo = 0;
  for (const raw of text.split('\n')) {
    lineNo++;
    const line = raw.trim();
    if (line === '') continue;
    let parsed: Json;
    try {
      parsed = JSON.parse(line) as Json;
    } catch {
      throw new Error(`${name} line ${lineNo}: not JSON`);
    }
    out.push(decodeSubscriptionUpdate(parsed, `${name} line ${lineNo}`));
  }
  return out;
}

/**
 * `fleet.jsonl`: one SubscriptionUpdate per line, frames in file order. `replicas` is
 * `parseReplicasJsonl`'s grouping; a frame takes the replica rows recorded at its own instant, and
 * an export without them (older than U23) gives every frame an empty fleet.
 */
export function parseFleetJsonl(
  text: string,
  originUnixNs: bigint,
  replicas: ReadonlyMap<bigint, SubscriptionUpdate[]> = new Map()
): ReplayFrame[] {
  return parseUpdates(text, 'fleet.jsonl').map((u, i) =>
    frameFromUpdate(u, originUnixNs, i, replicas.get(u.simTimeUnixNs) ?? [])
  );
}

/**
 * `replicas.jsonl`: SCOPE_REPLICA updates grouped by instant, in file order within an instant. Keyed
 * by the exact `sim_time_unix_ns` because the exporter stamps a replica row with its fleet row's
 * instant, never an interpolated one.
 */
export function parseReplicasJsonl(text: string): Map<bigint, SubscriptionUpdate[]> {
  const out = new Map<bigint, SubscriptionUpdate[]>();
  for (const u of parseUpdates(text, 'replicas.jsonl')) {
    const rows = out.get(u.simTimeUnixNs);
    if (rows === undefined) out.set(u.simTimeUnixNs, [u]);
    else rows.push(u);
  }
  return out;
}

// ---------------------------------------------------------------------------
// scenario.txt -> ScenarioConfig
// ---------------------------------------------------------------------------

/** The engine's routing names back to config.ts's kinds: the inverse of `ROUTING_TO_ENGINE`. */
const ENGINE_TO_ROUTING: Record<string, RoutingKind> = Object.fromEntries(
  (Object.entries(ROUTING_TO_ENGINE) as [RoutingKind, string | null][])
    .filter((kv): kv is [RoutingKind, string] => kv[1] !== null)
    .map(([kind, engine]) => [engine, kind])
);

/**
 * The control panel's config from the run's resolved scenario, so the panels show the run's real
 * parameters: SLO thresholds, fleet size, arrival rate, routing. The inverse of api.ts's
 * `scenarioConfigToWire`; the self-test checks the round trip key for key. Keys the panel has no
 * field for (sessions, preemption, admission, tenants, the load step, retries) ride in `extra`,
 * so `unmapped` names only keys the engine itself does not have; a routing the panel cannot name
 * keeps BASE's kind and is reported there too.
 */
export function configFromScenarioText(text: string): { config: ScenarioConfig; unmapped: string[] } {
  const f = parseScenarioText(text);
  const c = cloneConfig(BASE);
  const used = new Set<string>();
  const num = (key: string, apply: (v: number) => void) => {
    if (f[key] === undefined) return;
    used.add(key);
    const v = Number(f[key]);
    if (!Number.isFinite(v)) throw new Error(`scenario.txt: ${key} = ${JSON.stringify(f[key])} is not a number`);
    apply(v);
  };
  if (f.name !== undefined) {
    used.add('name');
    c.name = f.name;
  }
  num('seed', (v) => { c.seed = v; });
  num('duration_s', (v) => { c.durationS = v; });
  num('warmup_s', (v) => { c.warmupS = v; });

  num('replicas', (v) => { c.fleet.replicas = v; });
  num('max_batch', (v) => { c.fleet.maxBatch = v; });
  num('step_base_ms', (v) => { c.fleet.stepBaseMs = v; });
  num('step_per_seq_ms', (v) => { c.fleet.stepPerSeqMs = v; });
  num('step_per_kv_ktoken_ms', (v) => { c.fleet.stepPerKvKtokenMs = v; });
  num('prefill_tokens_per_s', (v) => { c.fleet.prefillTokensPerS = v; });
  num('kv_capacity_tokens', (v) => { c.fleet.kvTokensPerReplica = v; });
  num('step_token_budget', (v) => { c.fleet.stepTokenBudget = v; });
  num('max_queue', (v) => { c.fleet.maxQueue = v; });

  num('arrival_rps', (v) => { c.workload.arrivalRps = v; });
  num('prompt_mean', (v) => { c.workload.promptMean = v; });
  num('prompt_cv', (v) => { c.workload.promptCv = v; });
  num('output_mean', (v) => { c.workload.outputMean = v; });
  num('output_cv', (v) => { c.workload.outputCv = v; });
  num('long_probability', (v) => { c.workload.longProbability = v; });
  num('long_prompt_mean', (v) => { c.workload.longPromptMean = v; });
  num('long_output_mean', (v) => { c.workload.longOutputMean = v; });

  const unmapped: string[] = [];
  if (f.routing !== undefined) {
    used.add('routing');
    const kind = ENGINE_TO_ROUTING[f.routing];
    if (kind === undefined) unmapped.push(`routing = ${f.routing}`);
    else c.routing.kind = kind;
  }
  num('p2c_choices', (v) => { c.routing.choices = v; });
  if (f.probe_live !== undefined) {
    used.add('probe_live');
    c.routing.probeLive = f.probe_live === 'true';
  }

  num('telemetry_interval_ms', (v) => { c.telemetryIntervalMs = v; });
  num('telemetry_delay_ms', (v) => { c.telemetryDelayMs = v; });
  num('client_timeout_s', (v) => { c.clientTimeoutS = v; });
  num('max_attempts', (v) => { c.maxAttempts = v; });

  num('ttft_slo_ms', (v) => { c.slo.ttftMs = v; });
  num('itl_slo_ms', (v) => { c.slo.itlMs = v; });
  num('e2e_slo_s', (v) => { c.slo.e2eS = v; });
  // Exact reciprocal of the encoder's `1000 / samplesPerSimSecond`.
  num('sample_interval_ms', (v) => { c.samplesPerSimSecond = 1000 / v; });

  // Numbers as numbers so the round trip compares `0.0` with `0`; anything else (`preemption =
  // never`, a tenant weight list) stays the text the engine will parse itself.
  for (const k of EXTRA_KEYS) {
    if (f[k] === undefined) continue;
    used.add(k);
    c.extra[k] = Number.isFinite(Number(f[k])) ? Number(f[k]) : f[k];
  }

  for (const k of Object.keys(f)) if (!used.has(k)) unmapped.push(`${k} = ${f[k]}`);
  return { config: c, unmapped };
}

// ---------------------------------------------------------------------------
// The frame source, and the cursor
// ---------------------------------------------------------------------------

/**
 * The panels' view of a recorded run: the same three reads the mock engine answers (`frameAt`,
 * `window`, `eventsUpTo`), over frames that already exist. A whole run is recorded, so
 * `recordedToS` is the duration and nothing is ever simulated here.
 */
export class ReplayEngine implements FrameSource {
  config: ScenarioConfig;
  readonly frames: ReplayFrame[];
  /** The recording's sample interval, from the frames themselves. */
  readonly dtS: number;
  readonly recordedToS: number;

  constructor(frames: ReplayFrame[], config: ScenarioConfig) {
    if (frames.length === 0) throw new Error('a replay needs at least one frame');
    this.frames = frames;
    this.config = config;
    this.dtS = frames.length > 1 ? (frames[frames.length - 1].simS - frames[0].simS) / (frames.length - 1) : 1 / config.samplesPerSimSecond;
    this.recordedToS = Math.max(frames[frames.length - 1].simS, config.durationS);
  }

  get durationS(): number {
    return this.recordedToS;
  }

  /** Index of the last frame at or before `simS`; the first frame before any sample. */
  indexAt(simS: number): number {
    let lo = 0;
    let hi = this.frames.length - 1;
    if (simS < this.frames[0].simS) return 0;
    while (lo < hi) {
      const mid = (lo + hi + 1) >> 1;
      if (this.frames[mid].simS <= simS) lo = mid;
      else hi = mid - 1;
    }
    return lo;
  }

  frameAt(simS: number): Frame | undefined {
    return this.frames[this.indexAt(simS)];
  }

  /**
   * Frames with `simS` in [fromS, toS], decimated to the configured chart sample rate the way the
   * mock decimates its ticks, and always ending on the last one so the cursor's own frame is drawn.
   */
  window(fromS: number, toS: number): Frame[] {
    const stride = Math.max(1, Math.round(1 / (this.config.samplesPerSimSecond * this.dtS)));
    const out: Frame[] = [];
    let a = 0;
    while (a < this.frames.length && this.frames[a].simS < fromS) a++;
    let b = this.frames.length;
    while (b > 0 && this.frames[b - 1].simS > toS) b--;
    for (let i = a; i < b; i += stride) out.push(this.frames[i]);
    if (b > a) {
      const last = this.frames[b - 1];
      if (out[out.length - 1] !== last) out.push(last);
    }
    return out;
  }

  /** No failure injection exists in the engine yet, so a recording carries no events. */
  eventsUpTo(_simS: number): FleetEvent[] {
    return [];
  }
}

/** The cursor stays inside the recording: there is nothing to simulate past its end. */
export function clampCursor(s: number, durationS: number): number {
  if (!Number.isFinite(s)) return 0;
  return Math.min(Math.max(s, 0), durationS);
}

/** StepForward on a recording: `stepS` forward, stopping at the end and saying where it stopped. */
export function stepCursor(s: number, stepS: number, durationS: number): number {
  return clampCursor(s + stepS, durationS);
}

/** What the replay handle says when asked for something only a live engine can do. */
export const REPLAY_DISABLED_REASON = 'replay of a recorded run; changing load or policy needs a live engine';
