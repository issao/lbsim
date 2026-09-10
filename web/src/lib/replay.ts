// Replay source: a recorded run, loaded from static files instead of an Ingress server.
//
// `sim-run export` writes each run as the exact documents the live stream carries
// (crates/sim-ingress/src/export.rs): `runs/index.json`, then per run `status.json` (a RunStatus),
// `fleet.jsonl` (one SubscriptionUpdate per sample instant, SCOPE_FLEET), `result.json` (a
// RunResult) and `scenario.txt` (the resolved scenario). The transport's decoders in api.ts read
// them unchanged, so there is no second wire format here; what this file adds is the fetching, the
// one document without a proto behind it (the index), the scenario text back into the control
// panel's `ScenarioConfig`, and a `FrameSource` over the decoded frames so the panels can read a
// replay exactly as they read a live run.
//
// Everything below is React-free and network-injectable so the self-test can drive it under Node.
// Time is handled as the transport handles it: absolute instants stay `bigint`, and the only float
// form is seconds relative to the run's `sim_start_unix_ns` from the index, which is where the
// engine's clock started (warm-up included), so `simS` runs from 0 to the scenario's `duration_s`.

import {
  type Json,
  type RunResult,
  type WireRequestTrace,
  type RunStatus,
  type SubscriptionUpdate,
  EXTRA_KEYS,
  ROUTING_TO_ENGINE,
  decodeRequestTrace,
  decodeRunResult,
  decodeRunStatus,
  decodeSubscriptionUpdate,
  dbl,
  i32,
  isObject,
  parseScenarioText,
  relSeconds,
  secondsToNs,
  str,
  u64,
} from './api';
import { BASE, cloneConfig, type ScenarioConfig } from './config';
import type { RoutingKind } from './types';
import type { Frame, FleetEvent } from './frame';
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
  /**
   * Every this-many-th fleet sample (and the last) carries per-replica rows in `replicas.jsonl`;
   * 1 when the run fit the exporter's budget whole, and for an index older than the field.
   */
  replicaSampleStride: number;
  /** The demo group, i.e. the run id's directory (`1-routing`), or `''` for a flat id. */
  group: string;
  /** The run id's last segment, what the picker shows inside a group. */
  label: string;
}

/** A live server's own run id, `r-<n>`: no group segment of its own, unlike a demo's `<group>/<run>`. */
const LIVE_RUN_ID = /^r-\d+$/;

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
      replicaSampleStride: e.replica_sample_stride === undefined ? 1 : Math.max(1, i32(e.replica_sample_stride, `${where}.replica_sample_stride`)),
      // A demo's id (`1-routing/p2c`) groups by its own directory; a released live run (`r-3`) has
      // no directory of its own, so it gets one group of its own instead of falling into every
      // flat id's empty-string bucket (Issao, 2026-09-10: "the dashboard's replay mode cannot
      // open it" — once it can, it should read as a run, not an unlabeled leftover).
      group: slash === -1 ? (LIVE_RUN_ID.test(runId) ? 'released live runs' : '') : runId.slice(0, slash),
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
 * is the normal state of a dev server with nothing copied into `web/public/runs/`: nothing to
 * replay, not an error. A dev server answers a missing file with its HTML shell and HTTP 200, which is why a
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
  const frames = parseFleetJsonl(fleetText, entry.simStartUnixNs, replicas, entry.replicaSampleStride);
  if (frames.length === 0) throw new Error(`${entry.runId}/fleet.jsonl: no samples`);
  return { entry, status, result, scenarioText, config, unmapped, frames };
}

/** What `loadRunFromCheckpoint` returns: `loadRun`'s shape minus the index entry it has none of,
 *  and `result` optional since a failed run's checkpoint has no `result.json` (WIRE.md, "released"). */
export interface CheckpointRun {
  runId: string;
  status: RunStatus;
  result: RunResult | null;
  scenarioText: string;
  config: ScenarioConfig;
  unmapped: string[];
  frames: ReplayFrame[];
  /** The origin `frames[*].simS` is relative to, recovered rather than told; see below. */
  originUnixNs: bigint;
}

/**
 * A run's checkpoint read straight from `runs/<id>/` on the Ingress server that released it,
 * bypassing `runs/index.json` (a released run is not merged into it, WIRE.md "released"). This is
 * `useServerRun.ts`'s fallback when an `OpenSubscription` 410s because the run itself left memory
 * while the page still held it: the same documents `loadRun` reads, addressed directly by run id
 * rather than through an index entry.
 *
 * The one thing an index entry supplies that nothing else here does is `sim_start_unix_ns`: every
 * run's simulated clock starts at the same constant and the first *closed* frame lands one sample
 * interval later (`Checkpoint::inputs` in `run.rs` keeps every frame from the start, unlike
 * `traces.jsonl`), so the origin is recovered from the checkpoint's own first two frames instead of
 * being told. `replica_sample_stride` is read from `checkpoint.json` when a terminal checkpoint
 * wrote one, 1 otherwise (an idle checkpoint's, or an older export's).
 */
export async function loadRunFromCheckpoint(runId: string, f: FetchLike = defaultFetch(), base = runsBase()): Promise<CheckpointRun> {
  const dir = `${base}${runId}/`;
  const [statusJson, fleetText, resultText, scenarioText, replicasText, checkpointText] = await Promise.all([
    getJson(`${dir}status.json`, f),
    getText(`${dir}fleet.jsonl`, f),
    getOptionalText(`${dir}result.json`, f),
    getText(`${dir}scenario.txt`, f),
    getOptionalText(`${dir}replicas.jsonl`, f),
    getOptionalText(`${dir}checkpoint.json`, f),
  ]);
  const status = decodeRunStatus(statusJson, `${runId}/status.json`);
  const result = resultText === null ? null : decodeRunResult(JSON.parse(resultText) as Json, `${runId}/result.json`);
  const { config, unmapped } = configFromScenarioText(scenarioText);
  const replicas = replicasText === null ? new Map() : parseReplicasJsonl(replicasText);
  let stride = 1;
  if (checkpointText !== null) {
    const cp = JSON.parse(checkpointText) as { replica_sample_stride?: unknown };
    if (typeof cp.replica_sample_stride === 'number' && cp.replica_sample_stride > 0) stride = cp.replica_sample_stride;
  }
  // A first pass to recover the origin from the checkpoint's own frames, then the real parse.
  const probe = parseFleetJsonl(fleetText, 0n, replicas, stride);
  if (probe.length === 0) throw new Error(`${runId}/fleet.jsonl: no samples`);
  const sampleIntervalNs = probe.length > 1 ? probe[1].simTimeUnixNs - probe[0].simTimeUnixNs : secondsToNs(1 / config.samplesPerSimSecond);
  const originUnixNs = probe[0].simTimeUnixNs - sampleIntervalNs;
  const frames = parseFleetJsonl(fleetText, originUnixNs, replicas, stride);
  return { runId, status, result, scenarioText, config, unmapped, frames, originUnixNs };
}

/**
 * `runs/<id>/traces.jsonl`: one RequestTrace per line, the journeys the export kept of what the run
 * sampled (`manifest.json` beside it says how many of how many). Loaded apart from `loadRun`, and
 * only by the Traces tab when it mounts, because it is the one document a dashboard can be shown
 * without; `null` when the export predates traces, which the tab says in words.
 */
export async function loadTraces(runId: string, f: FetchLike = defaultFetch(), base = runsBase()): Promise<WireRequestTrace[] | null> {
  const text = await getOptionalText(`${base}${runId}/traces.jsonl`, f);
  if (text === null) return null;
  const out: WireRequestTrace[] = [];
  let lineNo = 0;
  for (const raw of text.split('\n')) {
    lineNo++;
    const line = raw.trim();
    if (line === '') continue;
    let parsed: Json;
    try {
      parsed = JSON.parse(line) as Json;
    } catch {
      throw new Error(`${runId}/traces.jsonl line ${lineNo}: not JSON`);
    }
    out.push(decodeRequestTrace(parsed, `${runId}/traces.jsonl line ${lineNo}`));
  }
  return out;
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
 *
 * `stride` is the index's `replica_sample_stride`: the exporter thins a large run's rows to every
 * stride-th sample (and the last) to keep `replicas.jsonl` under its budget, so at a stride above 1
 * a frame with no rows of its own shows the last sampled instant's rows, for fewer than a stride
 * of frames. At stride 1 an instant without rows has none: that is a gap, not a cadence.
 */
export function parseFleetJsonl(
  text: string,
  originUnixNs: bigint,
  replicas: ReadonlyMap<bigint, SubscriptionUpdate[]> = new Map(),
  stride = 1
): ReplayFrame[] {
  let carried: SubscriptionUpdate[] = [];
  let behind = 0;
  return parseUpdates(text, 'fleet.jsonl').map((u, i) => {
    const own = replicas.get(u.simTimeUnixNs);
    if (own !== undefined) {
      carried = own;
      behind = 0;
    } else if (stride > 1 && behind + 1 < stride) {
      behind++;
    } else {
      carried = [];
    }
    return frameFromUpdate(u, originUnixNs, i, carried);
  });
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
    // `ENGINE_TO_ROUTING` is built from api.ts's `ROUTING_TO_ENGINE` by inversion, which now maps
    // `prefix_affinity` both ways (it has an engine implementation), so no local patch is needed
    // here any more.
    const kind = ENGINE_TO_ROUTING[f.routing];
    if (kind === undefined) unmapped.push(`routing = ${f.routing}`);
    else c.routing.kind = kind;
  }
  num('p2c_choices', (v) => { c.routing.choices = v; });
  if (f.probe_live !== undefined) {
    used.add('probe_live');
    c.routing.probeLive = f.probe_live === 'true';
  }
  // U27c: PrefixAffinity's two knobs. Typed fields, not `extra`, because the mock engine already
  // reads `c.routing.maxLoadRatio`/`fallbackChoices` for its own simulation of the policy.
  num('affinity_max_load_ratio', (v) => { c.routing.maxLoadRatio = v; });
  num('affinity_fallback_choices', (v) => { c.routing.fallbackChoices = v; });

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
  // U27c's five other prefix-model keys have no typed field and are not in api.ts's `EXTRA_KEYS`
  // (that list predates the prefix model), so they are named here directly rather than there.
  const PREFIX_EXTRA_KEYS = ['prefix_roots', 'prefix_root_tokens', 'prefix_zipf_s', 'session_fork_rate', 'prefix_cache_tokens'];
  for (const k of [...EXTRA_KEYS, ...PREFIX_EXTRA_KEYS]) {
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
 * The panels' view of a recorded run: the same three reads the server engine answers (`frameAt`,
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
   * Frames with `simS` in [fromS, toS], decimated to the configured chart sample rate, and always
   * ending on the last one so the cursor's own frame is drawn.
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
