// Transport client for the lbsim Ingress server.
//
// The server does not speak gRPC. It serves the message shapes of `proto/lbsim/v1/ingress.proto`
// and `subscription.proto` as JSON over HTTP/1.1, with the one server-streaming RPC mapped onto
// server-sent events. The protos remain the schema of record; `crates/sim-ingress/WIRE.md` is the
// exact mapping and this file follows it line by line:
//
//   - unary RPCs are `POST /v1/ingress/<RpcName>`, JSON in, JSON out;
//   - `OpenSubscription` is `GET /v1/ingress/OpenSubscription?<query>` returning an SSE stream whose
//     first event is `event: open` (an `OpenSubscriptionResponse`) and every later event is
//     `event: update` (a `SubscriptionUpdate`), each carrying `id: <n>`;
//   - field names are the proto names verbatim, snake_case;
//   - every `uint64` is a decimal string on the wire and a `bigint` here;
//   - enums are their proto names; enum-keyed maps use the enum number as the key;
//   - errors are `{"error": "<message>"}` with the HTTP status carrying the class.
//
// Why `bigint` and not `number` for every uint64: simulated time is absolute Unix epoch
// nanoseconds (common.proto), around 1.79e18 for 2026, where a float64 resolves about 256 ns.
// A timestamp that passes through `number` is therefore silently wrong, and wrong in a way no
// chart reveals. Absolute instants stay `bigint` up to the point a chart needs a *relative*
// quantity, which is what `relSeconds` exists for.
//
// Why the field names are hand-written: there is no codegen yet. `WIRE_FIELDS` records every wire
// name this file reads or writes, and the self-test checks each one against the proto files, which
// is the client-side half of the guard WIRE.md describes for the server.
//
// The one deviation from the protos, per WIRE.md: `StartRunRequest.scenario` carries the engine's
// flat `key = value` text plus `--set`-style overrides, not the nested `Scenario` message. The
// encoding lives at the bottom of this file so it is the only part that changes when codegen lands.

import type { RoutingKind, Target as UiTarget, Scope as UiScope } from './types';
import type { ScenarioConfig } from './config';

// ---------------------------------------------------------------------------
// Scalar decoding
// ---------------------------------------------------------------------------

/** A decoded JSON value, before it is known to be a message. */
export type Json = unknown;

export function isObject(v: Json): v is Record<string, Json> {
  return typeof v === 'object' && v !== null && !Array.isArray(v);
}

function obj(v: Json, where: string): Record<string, Json> {
  if (v === undefined || v === null) return {};
  if (!isObject(v)) throw new TypeError(`${where}: expected a JSON object, got ${describe(v)}`);
  return v;
}

function describe(v: Json): string {
  if (v === null) return 'null';
  if (Array.isArray(v)) return 'an array';
  return typeof v;
}

/** Every wire field name read or written by this file. The self-test checks them against the protos. */
export const WIRE_FIELDS = new Set<string>();

/** Read one wire field, recording its name. */
function rd(o: Record<string, Json>, name: string): Json {
  WIRE_FIELDS.add(name);
  return o[name];
}

/** Build one wire message, recording its field names. */
function wr(o: Record<string, Json>): Record<string, Json> {
  for (const k of Object.keys(o)) WIRE_FIELDS.add(k);
  return o;
}

/**
 * uint64 / int64. The wire form is a decimal string; a number is accepted only while it is still
 * exactly representable, because a large float has already lost the value and quietly carrying it
 * on would hide the loss instead of reporting it.
 */
export function u64(v: Json, where = 'uint64'): bigint {
  if (v === undefined || v === null || v === '') return 0n;
  if (typeof v === 'bigint') return v;
  if (typeof v === 'string') {
    if (!/^-?\d+$/.test(v)) throw new TypeError(`${where}: ${JSON.stringify(v)} is not an integer`);
    return BigInt(v);
  }
  if (typeof v === 'number') {
    if (!Number.isSafeInteger(v)) {
      throw new TypeError(`${where}: ${v} is a JSON number beyond 2^53, so it has already lost precision; the server must send uint64 as a string`);
    }
    return BigInt(v);
  }
  throw new TypeError(`${where}: expected a string holding an integer, got ${describe(v)}`);
}

/** int32 / uint32. A JSON number, and safely a `number`. */
export function i32(v: Json, where = 'int32'): number {
  if (v === undefined || v === null || v === '') return 0;
  if (typeof v === 'string' && /^-?\d+$/.test(v)) return Number(v);
  if (typeof v !== 'number' || !Number.isFinite(v)) throw new TypeError(`${where}: expected a number, got ${describe(v)}`);
  return Math.trunc(v);
}

/** double / float, including proto3-JSON's "NaN", "Infinity", "-Infinity". */
export function dbl(v: Json, where = 'double'): number {
  if (v === undefined || v === null || v === '') return 0;
  if (typeof v === 'number') return v;
  if (typeof v === 'string') {
    if (v === 'NaN') return NaN;
    if (v === 'Infinity') return Infinity;
    if (v === '-Infinity') return -Infinity;
    const n = Number(v);
    if (Number.isNaN(n)) throw new TypeError(`${where}: ${JSON.stringify(v)} is not a number`);
    return n;
  }
  throw new TypeError(`${where}: expected a number, got ${describe(v)}`);
}

export function str(v: Json, where = 'string'): string {
  if (v === undefined || v === null) return '';
  if (typeof v !== 'string') throw new TypeError(`${where}: expected a string, got ${describe(v)}`);
  return v;
}

export function bool(v: Json, where = 'bool'): boolean {
  if (v === undefined || v === null) return false;
  if (typeof v === 'boolean') return v;
  throw new TypeError(`${where}: expected a boolean, got ${describe(v)}`);
}

function arr(v: Json, where: string): Json[] {
  if (v === undefined || v === null) return [];
  if (!Array.isArray(v)) throw new TypeError(`${where}: expected an array, got ${describe(v)}`);
  return v;
}

/**
 * Chart-friendly time. The only sanctioned way an absolute instant becomes a `number`: relative to
 * an origin, where the difference is small enough that a float64 is exact to the nanosecond.
 */
export function relSeconds(t: bigint, t0: bigint): number {
  return Number(t - t0) / 1e9;
}

/** A duration in nanoseconds as seconds. Safe because durations here are seconds, not epochs. */
export function nsToSeconds(ns: bigint): number {
  return Number(ns) / 1e9;
}

export function secondsToNs(s: number): bigint {
  return BigInt(Math.round(s * 1e9));
}

// ---------------------------------------------------------------------------
// Enums: names on the wire, numbers as map keys
// ---------------------------------------------------------------------------

/** subscription.proto Metric. Names are what the `metrics` query parameter carries. */
export const METRIC_NUMBER = {
  METRIC_UNSPECIFIED: 0,
  METRIC_TTFT: 1,
  METRIC_ITL: 2,
  METRIC_E2E: 3,
  METRIC_QUEUE_WAIT: 4,
  METRIC_PREEMPTED_TIME: 5,
  METRIC_PROMPT_TOKENS: 6,
  METRIC_OUTPUT_TOKENS: 7,
  METRIC_STEP_TIME: 8,
  METRIC_KV_UTILIZATION: 20,
  METRIC_KV_TOKENS_RESIDENT: 21,
  METRIC_RUNNING_SEQS: 22,
  METRIC_QUEUED_SEQS: 23,
  METRIC_QUEUED_PREFILL_TOKENS: 24,
  METRIC_BATCH_SIZE: 25,
  METRIC_PREFIX_CACHE_TOKENS: 26,
  METRIC_TIER_UTILIZATION: 27,
  METRIC_TIER_BANDWIDTH_UTILIZATION: 28,
  METRIC_TELEMETRY_STALENESS: 29,
  METRIC_OFFERED_RPS: 40,
  METRIC_ADMITTED_RPS: 41,
  METRIC_COMPLETED_RPS: 42,
  METRIC_REJECTED_RPS: 43,
  METRIC_OUTPUT_TOKENS_PER_S: 44,
  METRIC_GOODPUT_TOKENS_PER_S: 45,
  METRIC_PREEMPTIONS_PER_S: 46,
  METRIC_RETRIES_PER_S: 47,
  METRIC_PREFIX_HIT_RATE: 48,
  METRIC_READY_REPLICAS: 60,
  METRIC_WARMING_REPLICAS: 61,
  METRIC_DRAINING_REPLICAS: 62,
  METRIC_WARM_IDLE_REPLICAS: 63,
  METRIC_LOAD_IMBALANCE_CV: 64,
  METRIC_WASTED_GPU_FRACTION: 65,
  METRIC_SLO_ATTAINMENT: 66,
  METRIC_GPU_UTILIZATION: 67,
  METRIC_GPU_COMPUTE_BOUND_FRACTION: 68,
  METRIC_REPLICA_STATE: 69,
  METRIC_TRUE_SPEED_MULTIPLIER: 70,
} as const;

export type MetricName = keyof typeof METRIC_NUMBER;

export const METRIC_NAME: Record<number, MetricName> = invert(METRIC_NUMBER);

/** The metrics that arrive in `MetricRow.distributions` rather than `values`. */
export const DISTRIBUTION_METRICS: readonly MetricName[] = [
  'METRIC_TTFT', 'METRIC_ITL', 'METRIC_E2E', 'METRIC_QUEUE_WAIT',
  'METRIC_PREEMPTED_TIME', 'METRIC_PROMPT_TOKENS', 'METRIC_OUTPUT_TOKENS', 'METRIC_STEP_TIME',
];

export const SCOPE_NUMBER = {
  SCOPE_UNSPECIFIED: 0,
  SCOPE_FLEET: 1,
  SCOPE_CLUSTER: 2,
  SCOPE_POOL: 3,
  SCOPE_TENANT: 4,
  SCOPE_SLO_CLASS: 5,
  SCOPE_REPLICA: 6,
} as const;
export type ScopeName = keyof typeof SCOPE_NUMBER;
const SCOPE_NAME: Record<number, ScopeName> = invert(SCOPE_NUMBER);

export const OUTCOME_NUMBER = {
  OUTCOME_UNSPECIFIED: 0,
  OUTCOME_OK: 1,
  OUTCOME_OK_SLO_VIOLATED: 2,
  OUTCOME_REJECTED: 3,
  OUTCOME_TIMEOUT_QUEUED: 4,
  OUTCOME_TIMEOUT_RUNNING: 5,
  OUTCOME_CANCELLED: 6,
  OUTCOME_FAILED: 7,
} as const;
export type OutcomeName = keyof typeof OUTCOME_NUMBER;
export const OUTCOME_NAME: Record<number, OutcomeName> = invert(OUTCOME_NUMBER);

export const SLO_CLASS_NUMBER = {
  SLO_CLASS_UNSPECIFIED: 0,
  SLO_CLASS_INTERACTIVE: 1,
  SLO_CLASS_AGENT: 2,
  SLO_CLASS_BATCH: 3,
} as const;
export type SloClassName = keyof typeof SLO_CLASS_NUMBER;
const SLO_CLASS_NAME: Record<number, SloClassName> = invert(SLO_CLASS_NUMBER);

export const MEMORY_TIER_NUMBER = {
  MEMORY_TIER_UNSPECIFIED: 0,
  MEMORY_TIER_HBM: 1,
  MEMORY_TIER_DRAM: 2,
  MEMORY_TIER_SSD: 3,
  MEMORY_TIER_NONE: 4,
} as const;
export type MemoryTierName = keyof typeof MEMORY_TIER_NUMBER;
const MEMORY_TIER_NAME: Record<number, MemoryTierName> = invert(MEMORY_TIER_NUMBER);

export const STEP_BOUND_NUMBER = {
  STEP_BOUND_UNSPECIFIED: 0,
  STEP_BOUND_BANDWIDTH: 1,
  STEP_BOUND_COMPUTE: 2,
} as const;
export type StepBoundName = keyof typeof STEP_BOUND_NUMBER;
const STEP_BOUND_NAME: Record<number, StepBoundName> = invert(STEP_BOUND_NUMBER);

export const TRACE_BUCKET_NUMBER = {
  TRACE_BUCKET_UNSPECIFIED: 0,
  TRACE_BUCKET_P50: 1,
  TRACE_BUCKET_P90: 2,
  TRACE_BUCKET_P99: 3,
  TRACE_BUCKET_P999: 4,
} as const;
export type TraceBucketName = keyof typeof TRACE_BUCKET_NUMBER;
const TRACE_BUCKET_NAME: Record<number, TraceBucketName> = invert(TRACE_BUCKET_NUMBER);

/** policy.proto RefereeVerdict, the key of `RunResult.referee_violations`. */
export const REFEREE_VERDICT_NUMBER = {
  REFEREE_VERDICT_UNSPECIFIED: 0,
  REFEREE_VERDICT_ACCEPTED: 1,
  REFEREE_VERDICT_CLAMPED: 2,
  REFEREE_VERDICT_KV_OVERCOMMIT: 3,
  REFEREE_VERDICT_BUDGET_EXCEEDED: 4,
  REFEREE_VERDICT_UNKNOWN_ENTITY: 5,
  REFEREE_VERDICT_WRONG_PHASE: 6,
  REFEREE_VERDICT_STALE_REFERENCE: 7,
  REFEREE_VERDICT_CAUSALITY_VIOLATION: 8,
  REFEREE_VERDICT_BANDWIDTH_OVERCOMMIT: 9,
  REFEREE_VERDICT_CONSERVATION_VIOLATION: 10,
} as const;
export type RefereeVerdictName = keyof typeof REFEREE_VERDICT_NUMBER;
export const REFEREE_VERDICT_NAME: Record<number, RefereeVerdictName> = invert(REFEREE_VERDICT_NUMBER);

export const RUN_STATES = [
  'STATE_UNSPECIFIED', 'STATE_QUEUED', 'STATE_RUNNING', 'STATE_PAUSED', 'STATE_COMPLETE', 'STATE_FAILED',
] as const;
export type RunState = (typeof RUN_STATES)[number];
const RUN_STATE_NUMBER = Object.fromEntries(RUN_STATES.map((s, i) => [s, i])) as Record<RunState, number>;
const RUN_STATE_NAME: Record<number, RunState> = invert(RUN_STATE_NUMBER);

function invert<K extends string>(table: Record<K, number>): Record<number, K> {
  const out: Record<number, K> = {};
  for (const k of Object.keys(table) as K[]) out[table[k]] = k;
  return out;
}

/**
 * An enum field. The wire sends the name; the number is accepted too, because that is what a
 * hand-rolled server most easily emits and rejecting it would be pedantry rather than safety.
 */
function enumName<K extends string>(v: Json, table: Record<K, number>, names: Record<number, K>, dflt: K, where: string): K {
  if (v === undefined || v === null || v === '') return dflt;
  if (typeof v === 'string') {
    if (v in table) return v as K;
    throw new TypeError(`${where}: ${JSON.stringify(v)} is not a value of this enum`);
  }
  if (typeof v === 'number') {
    const n = names[v];
    if (n === undefined) throw new TypeError(`${where}: ${v} is not a value of this enum`);
    return n;
  }
  throw new TypeError(`${where}: expected an enum name, got ${describe(v)}`);
}

/** A `map<int32, X>` whose keys are stringified enum numbers. Unknown numbers are kept aside. */
function enumKeyedMap<K extends string, V>(
  v: Json,
  names: Record<number, K>,
  decodeValue: (x: Json, where: string) => V,
  where: string
): { known: Partial<Record<K, V>>; unknown: Record<string, V> } {
  const known: Partial<Record<K, V>> = {};
  const unknown: Record<string, V> = {};
  for (const [k, raw] of Object.entries(obj(v, where))) {
    if (!/^-?\d+$/.test(k)) throw new TypeError(`${where}: map key ${JSON.stringify(k)} is not an integer; an enum-keyed map is encoded with the enum number as the key`);
    const value = decodeValue(raw, `${where}[${k}]`);
    const name = names[Number(k)];
    if (name === undefined) unknown[k] = value;
    else known[name] = value;
  }
  return { known, unknown };
}

// ---------------------------------------------------------------------------
// Messages
// ---------------------------------------------------------------------------

/** subscription.proto Target: exactly one id, and none for SCOPE_FLEET. */
export interface WireTarget {
  scope: ScopeName;
  clusterId?: bigint;
  poolId?: bigint;
  tenantId?: bigint;
  replicaId?: bigint;
  sloClass?: SloClassName;
}

export function decodeTarget(v: Json, where = 'Target'): WireTarget {
  const o = obj(v, where);
  const t: WireTarget = { scope: enumName(rd(o, 'scope'), SCOPE_NUMBER, SCOPE_NAME, 'SCOPE_UNSPECIFIED', `${where}.scope`) };
  if (o.cluster_id !== undefined) t.clusterId = u64(rd(o, 'cluster_id'), `${where}.cluster_id`);
  if (o.pool_id !== undefined) t.poolId = u64(rd(o, 'pool_id'), `${where}.pool_id`);
  if (o.tenant_id !== undefined) t.tenantId = u64(rd(o, 'tenant_id'), `${where}.tenant_id`);
  if (o.replica_id !== undefined) t.replicaId = u64(rd(o, 'replica_id'), `${where}.replica_id`);
  if (o.slo_class !== undefined) t.sloClass = enumName(rd(o, 'slo_class'), SLO_CLASS_NUMBER, SLO_CLASS_NAME, 'SLO_CLASS_UNSPECIFIED', `${where}.slo_class`);
  return t;
}

/** The target's fields as they go on the wire: in a JSON body, or as query parameters. */
export function encodeTarget(t: WireTarget): Record<string, string> {
  const o: Record<string, string> = { scope: t.scope };
  if (t.clusterId !== undefined) o.cluster_id = t.clusterId.toString();
  if (t.poolId !== undefined) o.pool_id = t.poolId.toString();
  if (t.tenantId !== undefined) o.tenant_id = t.tenantId.toString();
  if (t.replicaId !== undefined) o.replica_id = t.replicaId.toString();
  if (t.sloClass !== undefined) o.slo_class = t.sloClass;
  return wr(o) as Record<string, string>;
}

export const fleetTarget = (): WireTarget => ({ scope: 'SCOPE_FLEET' });
export const replicaTarget = (id: number | bigint): WireTarget => ({ scope: 'SCOPE_REPLICA', replicaId: BigInt(id) });
export const poolTarget = (id: number | bigint): WireTarget => ({ scope: 'SCOPE_POOL', poolId: BigInt(id) });
export const clusterTarget = (id: number | bigint): WireTarget => ({ scope: 'SCOPE_CLUSTER', clusterId: BigInt(id) });
export const tenantTarget = (id: number | bigint): WireTarget => ({ scope: 'SCOPE_TENANT', tenantId: BigInt(id) });

const UI_SCOPE_TO_WIRE: Record<UiScope, ScopeName> = {
  FLEET: 'SCOPE_FLEET',
  CLUSTER: 'SCOPE_CLUSTER',
  POOL: 'SCOPE_POOL',
  TENANT: 'SCOPE_TENANT',
  SLO_CLASS: 'SCOPE_SLO_CLASS',
  REPLICA: 'SCOPE_REPLICA',
};

/** The panels speak types.ts's `Target` (`{scope: 'REPLICA', id: 3}`); the wire wants the proto's. */
export function uiTargetToWire(t: UiTarget): WireTarget {
  const scope = UI_SCOPE_TO_WIRE[t.scope];
  if (t.id === undefined || scope === 'SCOPE_FLEET') return { scope };
  const id = BigInt(t.id);
  switch (scope) {
    case 'SCOPE_CLUSTER': return { scope, clusterId: id };
    case 'SCOPE_POOL': return { scope, poolId: id };
    case 'SCOPE_TENANT': return { scope, tenantId: id };
    case 'SCOPE_REPLICA': return { scope, replicaId: id };
    // An SLO class is an enum on the wire, not an id; the UI's numeric id is its enum number.
    case 'SCOPE_SLO_CLASS': return { scope, sloClass: SLO_CLASS_NAME[Number(t.id)] ?? 'SLO_CLASS_UNSPECIFIED' };
    default: return { scope };
  }
}

export function wireTargetToUi(t: WireTarget): UiTarget {
  const scope = (Object.keys(UI_SCOPE_TO_WIRE) as UiScope[]).find((k) => UI_SCOPE_TO_WIRE[k] === t.scope) ?? 'FLEET';
  const id = t.replicaId ?? t.poolId ?? t.clusterId ?? t.tenantId;
  if (id !== undefined) return { scope, id: Number(id) };
  if (t.sloClass !== undefined) return { scope, id: SLO_CLASS_NUMBER[t.sloClass] };
  return { scope };
}

/**
 * subscription.proto Distribution, minus the Leaf-hop histogram. `count` is a uint64 and so a
 * bigint; `mean`, `min`, `max` and `value[]` are doubles in nanoseconds, exactly as the proto
 * declares them.
 */
export interface WireDistribution {
  count: bigint;
  mean: number;
  min: number;
  max: number;
  percentile: number[];
  value: number[];
  fromMergedHistogram: boolean;
}

export function decodeDistribution(v: Json, where = 'Distribution'): WireDistribution {
  const o = obj(v, where);
  return {
    count: u64(rd(o, 'count'), `${where}.count`),
    mean: dbl(rd(o, 'mean'), `${where}.mean`),
    min: dbl(rd(o, 'min'), `${where}.min`),
    max: dbl(rd(o, 'max'), `${where}.max`),
    percentile: arr(rd(o, 'percentile'), `${where}.percentile`).map((x, i) => dbl(x, `${where}.percentile[${i}]`)),
    value: arr(rd(o, 'value'), `${where}.value`).map((x, i) => dbl(x, `${where}.value[${i}]`)),
    fromMergedHistogram: bool(rd(o, 'from_merged_histogram'), `${where}.from_merged_histogram`),
  };
}

export interface MetricRow {
  target: WireTarget;
  values: Partial<Record<MetricName, number>>;
  distributions: Partial<Record<MetricName, WireDistribution>>;
  /** Metric numbers this build of the client does not know. Kept so a newer server is not a crash. */
  unknownValues: Record<string, number>;
  unknownDistributions: Record<string, WireDistribution>;
}

export function decodeMetricRow(v: Json, where = 'MetricRow'): MetricRow {
  const o = obj(v, where);
  const values = enumKeyedMap(rd(o, 'values'), METRIC_NAME, dbl, `${where}.values`);
  const dists = enumKeyedMap(rd(o, 'distributions'), METRIC_NAME, decodeDistribution, `${where}.distributions`);
  return {
    target: decodeTarget(rd(o, 'target'), `${where}.target`),
    values: values.known,
    distributions: dists.known,
    unknownValues: values.unknown,
    unknownDistributions: dists.unknown,
  };
}

export interface SubscriptionUpdate {
  subscriptionId: string;
  simTimeUnixNs: bigint;
  realtimeFactor: number;
  row: MetricRow;
  final: boolean;
}

export function decodeSubscriptionUpdate(v: Json, where = 'SubscriptionUpdate'): SubscriptionUpdate {
  const o = obj(v, where);
  return {
    subscriptionId: str(rd(o, 'subscription_id'), `${where}.subscription_id`),
    simTimeUnixNs: u64(rd(o, 'sim_time_unix_ns'), `${where}.sim_time_unix_ns`),
    realtimeFactor: dbl(rd(o, 'realtime_factor'), `${where}.realtime_factor`),
    row: decodeMetricRow(rd(o, 'row'), `${where}.row`),
    final: bool(rd(o, 'final'), `${where}.final`),
  };
}

export interface RunStatus {
  runId: string;
  state: RunState;
  simTimeUnixNs: bigint;
  simEndUnixNs: bigint;
  realtimeFactor: number;
  error: string;
}

export function decodeRunStatus(v: Json, where = 'RunStatus'): RunStatus {
  const o = obj(v, where);
  return {
    runId: str(rd(o, 'run_id'), `${where}.run_id`),
    state: enumName(rd(o, 'state'), RUN_STATE_NUMBER, RUN_STATE_NAME, 'STATE_UNSPECIFIED', `${where}.state`),
    simTimeUnixNs: u64(rd(o, 'sim_time_unix_ns'), `${where}.sim_time_unix_ns`),
    simEndUnixNs: u64(rd(o, 'sim_end_unix_ns'), `${where}.sim_end_unix_ns`),
    realtimeFactor: dbl(rd(o, 'realtime_factor'), `${where}.realtime_factor`),
    error: str(rd(o, 'error'), `${where}.error`),
  };
}

export function decodeStartRun(v: Json, where = 'StartRunResponse'): string {
  return str(rd(obj(v, where), 'run_id'), `${where}.run_id`);
}

export interface ListRunsResult {
  runs: RunStatus[];
  nextCursor: string;
}

export function decodeListRuns(v: Json, where = 'ListRunsResponse'): ListRunsResult {
  const o = obj(v, where);
  return {
    runs: arr(rd(o, 'runs'), `${where}.runs`).map((r, i) => decodeRunStatus(r, `${where}.runs[${i}]`)),
    nextCursor: str(rd(o, 'next_cursor'), `${where}.next_cursor`),
  };
}

export interface RewindResult {
  simTimeUnixNs: bigint;
  fromLog: boolean;
  restoredFromSnapshotUnixNs: bigint;
}

export function decodeRewind(v: Json, where = 'RewindResponse'): RewindResult {
  const o = obj(v, where);
  return {
    simTimeUnixNs: u64(rd(o, 'sim_time_unix_ns'), `${where}.sim_time_unix_ns`),
    fromLog: bool(rd(o, 'from_log'), `${where}.from_log`),
    restoredFromSnapshotUnixNs: u64(rd(o, 'restored_from_snapshot_unix_ns'), `${where}.restored_from_snapshot_unix_ns`),
  };
}

export interface UpdateResult {
  accepted: boolean;
  requiredResimulation: boolean;
  rewoundToUnixNs: bigint;
  rejectedReason: string;
}

export function decodeUpdate(v: Json, where = 'UpdateResponse'): UpdateResult {
  const o = obj(v, where);
  return {
    accepted: bool(rd(o, 'accepted'), `${where}.accepted`),
    requiredResimulation: bool(rd(o, 'required_resimulation'), `${where}.required_resimulation`),
    rewoundToUnixNs: u64(rd(o, 'rewound_to_unix_ns'), `${where}.rewound_to_unix_ns`),
    rejectedReason: str(rd(o, 'rejected_reason'), `${where}.rejected_reason`),
  };
}

/**
 * OpenSubscriptionResponse. `leaseExpiresAtWallNs` is the server's wall clock, which is not the
 * browser's: it is surfaced for display and never compared to `Date.now()`. The lease is counted
 * down locally from `lease_ns`, and the renew response's `expired` flag is the only authority.
 */
export interface OpenSubscriptionResult {
  subscriptionId: string;
  leaseExpiresAtWallNs: bigint;
  rejectedReason: string;
}

export function decodeOpenSubscription(v: Json, where = 'OpenSubscriptionResponse'): OpenSubscriptionResult {
  const o = obj(v, where);
  return {
    subscriptionId: str(rd(o, 'subscription_id'), `${where}.subscription_id`),
    leaseExpiresAtWallNs: u64(rd(o, 'lease_expires_at_wall_ns'), `${where}.lease_expires_at_wall_ns`),
    rejectedReason: str(rd(o, 'rejected_reason'), `${where}.rejected_reason`),
  };
}

export interface RenewSubscriptionResult {
  leaseExpiresAtWallNs: bigint;
  expired: boolean;
}

export function decodeRenew(v: Json, where = 'RenewSubscriptionResponse'): RenewSubscriptionResult {
  const o = obj(v, where);
  return {
    leaseExpiresAtWallNs: u64(rd(o, 'lease_expires_at_wall_ns'), `${where}.lease_expires_at_wall_ns`),
    expired: bool(rd(o, 'expired'), `${where}.expired`),
  };
}

export interface Scorecard {
  values: Partial<Record<MetricName, number>>;
  distributions: Partial<Record<MetricName, WireDistribution>>;
  outcomeCounts: Partial<Record<OutcomeName, bigint>>;
  declaredRatedCapacityRps: number;
  declaredRatedCapacityTokensPerS: number;
  metastableCollapse: boolean;
  recoveryTimeNs: bigint;
}

export function decodeScorecard(v: Json, where = 'Scorecard'): Scorecard {
  const o = obj(v, where);
  return {
    values: enumKeyedMap(rd(o, 'values'), METRIC_NAME, dbl, `${where}.values`).known,
    distributions: enumKeyedMap(rd(o, 'distributions'), METRIC_NAME, decodeDistribution, `${where}.distributions`).known,
    outcomeCounts: enumKeyedMap(rd(o, 'outcome_counts'), OUTCOME_NAME, u64, `${where}.outcome_counts`).known,
    declaredRatedCapacityRps: dbl(rd(o, 'declared_rated_capacity_rps'), `${where}.declared_rated_capacity_rps`),
    declaredRatedCapacityTokensPerS: dbl(rd(o, 'declared_rated_capacity_tokens_per_s'), `${where}.declared_rated_capacity_tokens_per_s`),
    metastableCollapse: bool(rd(o, 'metastable_collapse'), `${where}.metastable_collapse`),
    recoveryTimeNs: u64(rd(o, 'recovery_time_ns'), `${where}.recovery_time_ns`),
  };
}

export interface ScopedScorecard {
  target: WireTarget;
  scorecard: Scorecard;
}

export interface WireTraceSpan {
  startUnixNs: bigint;
  endUnixNs: bigint;
  component: string;
  operation: string;
  replicaId: bigint;
  concurrentSeqs: number;
  kvUtilization: number;
  tokensProcessed: number;
  kvTier: MemoryTierName;
  // The resource while the span ran (metrics.proto fields 14-21). The server omits a zero, so a
  // zero here is either "none" or "not carried": the panel says which by the span's component.
  batchSize: number;
  queued: number;
  kvTokensResident: bigint;
  kvCapacity: bigint;
  stepNs: bigint;
  bound: StepBoundName;
  /** Routing spans only: the replicas the router looked at. */
  candidates: bigint[];
  staleViewAgeNs: bigint;
}

export function decodeTraceSpan(v: Json, where = 'TraceSpan'): WireTraceSpan {
  const o = obj(v, where);
  return {
    startUnixNs: u64(rd(o, 'start_unix_ns'), `${where}.start_unix_ns`),
    endUnixNs: u64(rd(o, 'end_unix_ns'), `${where}.end_unix_ns`),
    component: str(rd(o, 'component'), `${where}.component`),
    operation: str(rd(o, 'operation'), `${where}.operation`),
    replicaId: u64(rd(o, 'replica_id'), `${where}.replica_id`),
    concurrentSeqs: i32(rd(o, 'concurrent_seqs'), `${where}.concurrent_seqs`),
    kvUtilization: dbl(rd(o, 'kv_utilization'), `${where}.kv_utilization`),
    tokensProcessed: i32(rd(o, 'tokens_processed'), `${where}.tokens_processed`),
    kvTier: enumName(rd(o, 'kv_tier'), MEMORY_TIER_NUMBER, MEMORY_TIER_NAME, 'MEMORY_TIER_UNSPECIFIED', `${where}.kv_tier`),
    batchSize: i32(rd(o, 'batch_size'), `${where}.batch_size`),
    queued: i32(rd(o, 'queued'), `${where}.queued`),
    kvTokensResident: u64(rd(o, 'kv_tokens_resident'), `${where}.kv_tokens_resident`),
    kvCapacity: u64(rd(o, 'kv_capacity'), `${where}.kv_capacity`),
    stepNs: u64(rd(o, 'step_ns'), `${where}.step_ns`),
    bound: enumName(rd(o, 'bound'), STEP_BOUND_NUMBER, STEP_BOUND_NAME, 'STEP_BOUND_UNSPECIFIED', `${where}.bound`),
    candidates: arr(rd(o, 'candidates'), `${where}.candidates`).map((x, i) => u64(x, `${where}.candidates[${i}]`)),
    staleViewAgeNs: u64(rd(o, 'stale_view_age_ns'), `${where}.stale_view_age_ns`),
  };
}

export interface WireRequestRecord {
  id: bigint;
  tenantId: bigint;
  sloClass: SloClassName;
  outcome: OutcomeName;
  arrivedAtUnixNs: bigint;
  admittedAtUnixNs: bigint;
  firstTokenAtUnixNs: bigint;
  finishedAtUnixNs: bigint;
  promptTokens: number;
  outputTokens: number;
  cachedPrefixTokens: number;
  clusterId: bigint;
  replicaId: bigint;
  replicaPathId: bigint[];
  attempts: number;
  preemptions: number;
  queueWaitNs: bigint;
  preemptedNs: bigint;
  ttftNs: bigint;
  e2eNs: bigint;
  meanItlNs: bigint;
  p99ItlNs: bigint;
}

export function decodeRequestRecord(v: Json, where = 'RequestRecord'): WireRequestRecord {
  const o = obj(v, where);
  return {
    id: u64(rd(o, 'id'), `${where}.id`),
    tenantId: u64(rd(o, 'tenant_id'), `${where}.tenant_id`),
    sloClass: enumName(rd(o, 'slo_class'), SLO_CLASS_NUMBER, SLO_CLASS_NAME, 'SLO_CLASS_UNSPECIFIED', `${where}.slo_class`),
    outcome: enumName(rd(o, 'outcome'), OUTCOME_NUMBER, OUTCOME_NAME, 'OUTCOME_UNSPECIFIED', `${where}.outcome`),
    arrivedAtUnixNs: u64(rd(o, 'arrived_at_unix_ns'), `${where}.arrived_at_unix_ns`),
    admittedAtUnixNs: u64(rd(o, 'admitted_at_unix_ns'), `${where}.admitted_at_unix_ns`),
    firstTokenAtUnixNs: u64(rd(o, 'first_token_at_unix_ns'), `${where}.first_token_at_unix_ns`),
    finishedAtUnixNs: u64(rd(o, 'finished_at_unix_ns'), `${where}.finished_at_unix_ns`),
    promptTokens: i32(rd(o, 'prompt_tokens'), `${where}.prompt_tokens`),
    outputTokens: i32(rd(o, 'output_tokens'), `${where}.output_tokens`),
    cachedPrefixTokens: i32(rd(o, 'cached_prefix_tokens'), `${where}.cached_prefix_tokens`),
    clusterId: u64(rd(o, 'cluster_id'), `${where}.cluster_id`),
    replicaId: u64(rd(o, 'replica_id'), `${where}.replica_id`),
    replicaPathId: arr(rd(o, 'replica_path_id'), `${where}.replica_path_id`).map((x, i) => u64(x, `${where}.replica_path_id[${i}]`)),
    attempts: i32(rd(o, 'attempts'), `${where}.attempts`),
    preemptions: i32(rd(o, 'preemptions'), `${where}.preemptions`),
    queueWaitNs: u64(rd(o, 'queue_wait_ns'), `${where}.queue_wait_ns`),
    preemptedNs: u64(rd(o, 'preempted_ns'), `${where}.preempted_ns`),
    ttftNs: u64(rd(o, 'ttft_ns'), `${where}.ttft_ns`),
    e2eNs: u64(rd(o, 'e2e_ns'), `${where}.e2e_ns`),
    meanItlNs: u64(rd(o, 'mean_itl_ns'), `${where}.mean_itl_ns`),
    p99ItlNs: u64(rd(o, 'p99_itl_ns'), `${where}.p99_itl_ns`),
  };
}

export interface WireRequestTrace {
  record: WireRequestRecord;
  spans: WireTraceSpan[];
  /** Which latency bucket the sampler placed the request in. */
  bucket: TraceBucketName;
}

export function decodeRequestTrace(v: Json, where = 'RequestTrace'): WireRequestTrace {
  const o = obj(v, where);
  return {
    record: decodeRequestRecord(rd(o, 'record'), `${where}.record`),
    spans: arr(rd(o, 'spans'), `${where}.spans`).map((s, i) => decodeTraceSpan(s, `${where}.spans[${i}]`)),
    bucket: enumName(rd(o, 'bucket'), TRACE_BUCKET_NUMBER, TRACE_BUCKET_NAME, 'TRACE_BUCKET_UNSPECIFIED', `${where}.bucket`),
  };
}

/** GetTracesResponse. Exported so a caller (and the self-test) can decode a captured body. */
export function decodeGetTraces(v: Json, where = 'GetTracesResponse'): WireRequestTrace[] {
  return arr(rd(obj(v, where), 'traces'), `${where}.traces`).map((t, i) => decodeRequestTrace(t, `${where}.traces[${i}]`));
}

export interface RunResult {
  runId: string;
  seed: bigint;
  eventCount: bigint;
  stateChecksum: bigint;
  overall: Scorecard;
  byScope: ScopedScorecard[];
  recorded: SubscriptionUpdate[];
  traces: WireRequestTrace[];
  refereeViolations: Partial<Record<RefereeVerdictName, bigint>>;
  wallClockSeconds: number;
  realtimeFactor: number;
}

export function decodeRunResult(v: Json, where = 'RunResult'): RunResult {
  const o = obj(v, where);
  return {
    runId: str(rd(o, 'run_id'), `${where}.run_id`),
    seed: u64(rd(o, 'seed'), `${where}.seed`),
    eventCount: u64(rd(o, 'event_count'), `${where}.event_count`),
    stateChecksum: u64(rd(o, 'state_checksum'), `${where}.state_checksum`),
    overall: decodeScorecard(rd(o, 'overall'), `${where}.overall`),
    byScope: arr(rd(o, 'by_scope'), `${where}.by_scope`).map((s, i) => {
      const so = obj(s, `${where}.by_scope[${i}]`);
      return {
        target: decodeTarget(rd(so, 'target'), `${where}.by_scope[${i}].target`),
        scorecard: decodeScorecard(rd(so, 'scorecard'), `${where}.by_scope[${i}].scorecard`),
      };
    }),
    recorded: arr(rd(o, 'recorded'), `${where}.recorded`).map((u, i) => decodeSubscriptionUpdate(u, `${where}.recorded[${i}]`)),
    traces: arr(rd(o, 'traces'), `${where}.traces`).map((t, i) => decodeRequestTrace(t, `${where}.traces[${i}]`)),
    refereeViolations: enumKeyedMap(rd(o, 'referee_violations'), REFEREE_VERDICT_NAME, u64, `${where}.referee_violations`).known,
    wallClockSeconds: dbl(rd(o, 'wall_clock_seconds'), `${where}.wall_clock_seconds`),
    realtimeFactor: dbl(rd(o, 'realtime_factor'), `${where}.realtime_factor`),
  };
}

// ---------------------------------------------------------------------------
// Errors and the HTTP layer
// ---------------------------------------------------------------------------

/**
 * A non-2xx response. WIRE.md: 400 bad request, 404 unknown run or subscription, 409 wrong state,
 * 410 a subscription stream that cannot be resumed, 500; body `{"error": "<message>"}`.
 */
export class IngressError extends Error {
  readonly httpStatus: number;
  constructor(message: string, httpStatus: number) {
    super(message);
    this.name = 'IngressError';
    this.httpStatus = httpStatus;
  }
  get gone(): boolean {
    return this.httpStatus === 410;
  }
}

export function toIngressError(body: Json, httpStatus: number, where: string): IngressError {
  if (isObject(body)) {
    const e = rd(body, 'error');
    if (typeof e === 'string' && e !== '') return new IngressError(e, httpStatus);
  }
  return new IngressError(`${where}: HTTP ${httpStatus}`, httpStatus);
}

export type FetchLike = (input: string, init?: RequestInit) => Promise<Response>;

export interface ClientOptions {
  baseUrl: string;
  /** A seam for tests and for a caller that wants to add headers or a timeout. */
  fetchImpl?: FetchLike;
}

/** The proto method names, verbatim: the path is `/v1/ingress/<RpcName>`. */
export type RpcName =
  | 'StartRun' | 'StopRun' | 'GetRun' | 'ListRuns'
  | 'SetSpeed' | 'StepForward' | 'Rewind' | 'UpdateWorkload' | 'UpdatePolicies'
  | 'OpenSubscription' | 'RenewSubscription' | 'CloseSubscription'
  | 'GetResult' | 'GetTraces';

export function rpcPath(name: RpcName): string {
  return `/v1/ingress/${name}`;
}

/** Query-string form of a message: same field names, uint64 as decimal strings, lists comma-joined. */
export function query(params: Record<string, string | number | bigint | readonly (string | number)[] | undefined>): string {
  const q = new URLSearchParams();
  for (const [k, v] of Object.entries(params)) {
    if (v === undefined || v === '') continue;
    if (Array.isArray(v)) {
      if (v.length === 0) continue;
      q.set(k, v.map(String).join(','));
    } else {
      q.set(k, typeof v === 'bigint' ? v.toString() : String(v));
    }
  }
  const s = q.toString();
  return s ? `?${s}` : '';
}

export interface StartRunRequest {
  scenario: ScenarioEnvelope;
  maxRealtimeFactor?: number;
  recordTraces?: boolean;
}

export interface OpenSubscriptionRequest {
  runId: string;
  target: WireTarget;
  metrics: MetricName[];
  samplesPerSimSecond: number;
  percentiles?: number[];
  leaseNs?: bigint;
}

/**
 * The `OpenSubscription` query parameters, mirroring `OpenSubscriptionRequest` field for field with
 * the target flattened. `subscriptionId` is the one addition, set only on a reconnect so the server
 * can find the ring to replay from; it is not in the proto, and WIRE.md leaves the point open.
 */
export function encodeOpenSubscriptionQuery(req: OpenSubscriptionRequest, subscriptionId?: string): string {
  const target = encodeTarget(req.target);
  return query(wr({
    run_id: req.runId,
    ...target,
    metrics: req.metrics,
    samples_per_sim_second: req.samplesPerSimSecond,
    percentiles: req.percentiles ?? [],
    lease_ns: req.leaseNs,
    subscription_id: subscriptionId,
  }) as Record<string, string | number | bigint | readonly (string | number)[] | undefined>);
}

export class IngressClient {
  readonly baseUrl: string;
  /** Public so the SSE reader uses the same seam as the unary calls; a stub fetch sees both. */
  readonly fetchImpl: FetchLike;

  constructor(opts: ClientOptions) {
    this.baseUrl = opts.baseUrl.replace(/\/+$/, '');
    // Bound late so a page that installs a fetch wrapper after construction still gets it.
    this.fetchImpl = opts.fetchImpl ?? ((input, init) => fetch(input, init));
  }

  url(path: string): string {
    return `${this.baseUrl}${path}`;
  }

  /** One unary call: `POST /v1/ingress/<RpcName>` with a JSON body. */
  private async rpc<T>(name: RpcName, body: Record<string, Json>, decode: (v: Json) => T, signal?: AbortSignal): Promise<T> {
    const path = rpcPath(name);
    const res = await this.fetchImpl(this.url(path), {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify(body),
      signal,
    });
    const text = await res.text();
    let parsed: Json = undefined;
    if (text.length > 0) {
      try {
        parsed = JSON.parse(text);
      } catch {
        if (res.ok) throw new IngressError(`POST ${path}: response body is not JSON: ${text.slice(0, 200)}`, res.status);
        throw new IngressError(`POST ${path}: HTTP ${res.status}: ${text.slice(0, 200)}`, res.status);
      }
    }
    if (!res.ok) throw toIngressError(parsed, res.status, `POST ${path}`);
    return decode(parsed);
  }

  // -- lifecycle ----------------------------------------------------------

  startRun(req: StartRunRequest, signal?: AbortSignal): Promise<string> {
    const body: Record<string, Json> = { scenario: req.scenario };
    if (req.maxRealtimeFactor !== undefined) body.max_realtime_factor = req.maxRealtimeFactor;
    if (req.recordTraces !== undefined) body.record_traces = req.recordTraces;
    return this.rpc('StartRun', wr(body), decodeStartRun, signal);
  }

  stopRun(runId: string, signal?: AbortSignal): Promise<RunStatus> {
    return this.rpc('StopRun', wr({ run_id: runId }), decodeRunStatus, signal);
  }

  getRun(runId: string, signal?: AbortSignal): Promise<RunStatus> {
    return this.rpc('GetRun', wr({ run_id: runId }), decodeRunStatus, signal);
  }

  listRuns(opts: { limit?: number; cursor?: string } = {}, signal?: AbortSignal): Promise<ListRunsResult> {
    const body: Record<string, Json> = {};
    if (opts.limit !== undefined) body.limit = opts.limit;
    if (opts.cursor !== undefined && opts.cursor !== '') body.cursor = opts.cursor;
    return this.rpc('ListRuns', wr(body), decodeListRuns, signal);
  }

  // -- interactive control ------------------------------------------------

  setSpeed(runId: string, realtimeFactor: number, paused: boolean, signal?: AbortSignal): Promise<RunStatus> {
    return this.rpc('SetSpeed', wr({ run_id: runId, realtime_factor: realtimeFactor, paused }), decodeRunStatus, signal);
  }

  /** StepForward by duration. Bounded server-side: the returned status says where it actually stopped. */
  stepForward(runId: string, simDurationNs: bigint, signal?: AbortSignal): Promise<RunStatus> {
    return this.rpc('StepForward', wr({ run_id: runId, sim_duration_ns: simDurationNs.toString() }), decodeRunStatus, signal);
  }

  /** StepForward by barrier windows: the other arm of the proto's exactly-one. */
  stepBarriers(runId: string, barrierWindows: number, signal?: AbortSignal): Promise<RunStatus> {
    return this.rpc('StepForward', wr({ run_id: runId, barrier_windows: barrierWindows }), decodeRunStatus, signal);
  }

  rewind(runId: string, toSimTimeUnixNs: bigint, signal?: AbortSignal): Promise<RewindResult> {
    return this.rpc('Rewind', wr({ run_id: runId, to_sim_time_unix_ns: toSimTimeUnixNs.toString() }), decodeRewind, signal);
  }

  /** `overrides` are `--set k=v` pairs restricted to workload keys; the server rejects any other with 400. */
  updateWorkload(runId: string, overrides: Overrides, signal?: AbortSignal): Promise<UpdateResult> {
    return this.rpc('UpdateWorkload', wr({ run_id: runId, overrides }), decodeUpdate, signal);
  }

  updatePolicies(runId: string, overrides: Overrides, signal?: AbortSignal): Promise<UpdateResult> {
    return this.rpc('UpdatePolicies', wr({ run_id: runId, overrides }), decodeUpdate, signal);
  }

  // -- observation --------------------------------------------------------

  /** The SSE endpoint for one subscription. Opening it is `openStream`; this is only the address. */
  openSubscriptionUrl(req: OpenSubscriptionRequest, subscriptionId?: string): string {
    return this.url(`${rpcPath('OpenSubscription')}${encodeOpenSubscriptionQuery(req, subscriptionId)}`);
  }

  renewSubscription(subscriptionId: string, leaseNs: bigint, signal?: AbortSignal): Promise<RenewSubscriptionResult> {
    return this.rpc('RenewSubscription', wr({ subscription_id: subscriptionId, lease_ns: leaseNs.toString() }), decodeRenew, signal);
  }

  closeSubscription(subscriptionId: string, signal?: AbortSignal): Promise<void> {
    return this.rpc('CloseSubscription', wr({ subscription_id: subscriptionId }), () => undefined, signal);
  }

  // -- results ------------------------------------------------------------

  getResult(runId: string, signal?: AbortSignal): Promise<RunResult> {
    return this.rpc('GetResult', wr({ run_id: runId }), decodeRunResult, signal);
  }

  getTraces(runId: string, opts: { outcome?: OutcomeName; minE2eNs?: bigint; tenantId?: bigint; limit?: number } = {}, signal?: AbortSignal): Promise<WireRequestTrace[]> {
    const body: Record<string, Json> = { run_id: runId };
    if (opts.outcome !== undefined) body.outcome = opts.outcome;
    if (opts.minE2eNs !== undefined) body.min_e2e_ns = opts.minE2eNs.toString();
    if (opts.tenantId !== undefined) body.tenant_id = opts.tenantId.toString();
    if (opts.limit !== undefined) body.limit = opts.limit;
    return this.rpc('GetTraces', wr(body), decodeGetTraces, signal);
  }
}

// ---------------------------------------------------------------------------
// Server-sent events
// ---------------------------------------------------------------------------

/** One SSE event: its `event:` name, its `id:` if it carried one, and the joined `data:` lines. */
export interface SseEvent {
  event: string;
  id: string | null;
  data: string;
}

export interface SseSplit {
  events: SseEvent[];
  /** What is left over: a partial event, to be prefixed to the next chunk. */
  rest: string;
}

/**
 * Split a buffer into complete SSE events, returning the incomplete tail.
 *
 * Pure and exported on purpose: framing is the part of this transport most likely to be wrong, and
 * a chunk boundary in the middle of a JSON object is the failure that only shows up under load.
 * Testing it needs no network.
 */
export function parseSseFrames(buffer: string): SseSplit {
  const events: SseEvent[] = [];
  const boundary = /\r?\n\r?\n/g;
  let start = 0;
  let m: RegExpExecArray | null;
  while ((m = boundary.exec(buffer)) !== null) {
    const block = buffer.slice(start, m.index);
    start = m.index + m[0].length;
    const ev = sseEvent(block);
    if (ev !== null) events.push(ev);
  }
  return { events, rest: buffer.slice(start) };
}

/** One event block, or null for a block that carries nothing (a comment keepalive). */
function sseEvent(block: string): SseEvent | null {
  const data: string[] = [];
  let event = 'message';
  let id: string | null = null;
  let seen = false;
  for (const line of block.split(/\r?\n/)) {
    if (line === '' || line.startsWith(':')) continue;
    const colon = line.indexOf(':');
    const field = colon === -1 ? line : line.slice(0, colon);
    let value = colon === -1 ? '' : line.slice(colon + 1);
    if (value.startsWith(' ')) value = value.slice(1);
    if (field === 'data') {
      data.push(value);
      seen = true;
    } else if (field === 'event') {
      event = value;
      seen = true;
    } else if (field === 'id') {
      id = value;
      seen = true;
    }
    // `retry:` is EventSource's reconnect hint; this client has its own backoff.
  }
  return seen ? { event, id, data: data.join('\n') } : null;
}

/** Stateful wrapper over `parseSseFrames`, so a caller never has to hold the tail itself. */
export class SseBuffer {
  private rest = '';
  push(chunk: string): SseEvent[] {
    const { events, rest } = parseSseFrames(this.rest + chunk);
    this.rest = rest;
    return events;
  }
  /** Anything still buffered when the stream ends. A well-behaved server leaves nothing. */
  pending(): string {
    return this.rest;
  }
}

export interface StreamOptions {
  onEvent: (ev: SseEvent) => void;
  /** Sent as `Last-Event-ID` so the server replays from its ring. Absent on a fresh open. */
  lastEventId?: string;
  signal?: AbortSignal;
  fetchImpl?: FetchLike;
}

/**
 * Read an SSE stream with `fetch` and the frame parser above.
 *
 * `fetch` rather than `EventSource`, deliberately. `EventSource` reconnects on its own with its own
 * `Last-Event-ID`, cannot tell a 410 from a dropped socket, and hides the response status entirely;
 * WIRE.md makes all three the client's business. Resolves when the server closes the stream, which
 * for this transport is expected rather than exceptional; rejects with an `IngressError` carrying
 * the HTTP status when the server refused the stream, `gone` for a 410.
 */
export async function readSseStream(url: string, o: StreamOptions): Promise<void> {
  const f = o.fetchImpl ?? ((input: string, init?: RequestInit) => fetch(input, init));
  const headers: Record<string, string> = { accept: 'text/event-stream' };
  if (o.lastEventId !== undefined) headers['last-event-id'] = o.lastEventId;
  const res = await f(url, { method: 'GET', headers, signal: o.signal });
  if (!res.ok) {
    const text = await res.text();
    let parsed: Json;
    try {
      parsed = JSON.parse(text);
    } catch {
      parsed = undefined;
    }
    throw toIngressError(parsed, res.status, `GET ${url}`);
  }
  if (!res.body) throw new IngressError(`GET ${url}: response has no body to stream`, res.status);
  const reader = res.body.getReader();
  const decoder = new TextDecoder();
  const buf = new SseBuffer();
  for (;;) {
    const { done, value } = await reader.read();
    if (done) break;
    // `stream: true` because a multi-byte character can straddle a chunk just as a frame can.
    for (const ev of buf.push(decoder.decode(value, { stream: true }))) o.onEvent(ev);
  }
  for (const ev of buf.push(decoder.decode())) o.onEvent(ev);
}

export interface StreamHandle {
  /** Resolves when the server closed the stream; rejects on a refused stream or a transport error. */
  done: Promise<void>;
  close(): void;
}

export function openStream(url: string, o: StreamOptions): StreamHandle {
  const ctl = new AbortController();
  o.signal?.addEventListener('abort', () => ctl.abort());
  const done = readSseStream(url, { ...o, signal: ctl.signal });
  return { done, close: () => ctl.abort() };
}

// ---------------------------------------------------------------------------
// Subscription lifecycle: lease renewal, reconnection, reopening
// ---------------------------------------------------------------------------

export const DEFAULT_LEASE_NS = 60_000_000_000n; // 60 s
export const BACKOFF_BASE_MS = 250;
export const BACKOFF_CAP_MS = 5_000;

/**
 * Full jitter: uniform in [0, min(cap, base * 2^attempt)). Full rather than equal jitter because
 * every browser tab showing the same run reconnects on the same server-side close, and the point of
 * the jitter is to stop them arriving together.
 */
export function backoffDelayMs(attempt: number, rnd: () => number = Math.random): number {
  const ceiling = Math.min(BACKOFF_CAP_MS, BACKOFF_BASE_MS * 2 ** Math.max(0, attempt));
  return Math.floor(rnd() * ceiling);
}

/**
 * `opening` is the first stream; `streaming` once its `open` event arrived; `reconnecting` is a
 * resumed stream on the same subscription with `Last-Event-ID`; `reopening` is a new subscription
 * after the old one was lost (lease expired, server said 410, or renew said `expired`); `complete`
 * is the run's `final` update; `closed` is the caller's doing; `failed` is a rejected open.
 */
export type StreamPhase = 'opening' | 'streaming' | 'reconnecting' | 'reopening' | 'complete' | 'closed' | 'failed';

export interface SubscribeOptions {
  runId: string;
  target: WireTarget | UiTarget;
  metrics: MetricName[];
  samplesPerSimSecond: number;
  percentiles?: number[];
  leaseNs?: bigint;
  onUpdate: (u: SubscriptionUpdate) => void;
  onPhase?: (phase: StreamPhase, detail?: string) => void;
  /** Seams, so the selftest can drive this without a clock or a network. */
  rnd?: () => number;
  sleep?: (ms: number) => Promise<void>;
  /** A monotonic-enough local clock in milliseconds. Only ever compared with itself. */
  now?: () => number;
  openStreamImpl?: (url: string, o: StreamOptions) => StreamHandle;
}

export interface SubscriptionHandle {
  close(): void;
  phase(): StreamPhase;
  subscriptionId(): string | null;
  /** The `id:` of the last update delivered, i.e. what the next reconnect will send. */
  lastEventId(): string | null;
  /** Resolves when the handle is closed and its loop has stopped. */
  done: Promise<void>;
}

function isWireTarget(t: WireTarget | UiTarget): t is WireTarget {
  return typeof t.scope === 'string' && t.scope.startsWith('SCOPE_');
}

/**
 * One subscription for one entity, kept alive for as long as the caller wants it.
 *
 * Everything the protos and WIRE.md make the client's job is here rather than in the panels:
 *
 *   - The lease is counted down locally from `lease_ns` and renewed at half its length, and not at
 *     all while the document is hidden, so a backgrounded tab expires instead of costing the server
 *     forever. The server's `lease_expires_at_wall_ns` is never compared to the browser clock; the
 *     renew response's `expired` flag is the only authority.
 *   - A stream the server closes while the lease lives is reconnected with jittered backoff and
 *     `Last-Event-ID`, so the server replays what was missed.
 *   - A 410, an `expired` renew, or a lease that ran down locally means the server has forgotten the
 *     subscription: reopen from scratch, with no `Last-Event-ID`.
 */
export function subscribeToTarget(client: IngressClient, o: SubscribeOptions): SubscriptionHandle {
  const leaseNs = o.leaseNs ?? DEFAULT_LEASE_NS;
  const leaseMs = Number(leaseNs) / 1e6;
  const rnd = o.rnd ?? Math.random;
  const now = o.now ?? (() => Date.now());
  const sleep = o.sleep ?? ((ms: number) => new Promise<void>((r) => setTimeout(r, ms)));
  const openStreamImpl = o.openStreamImpl ?? openStream;
  const target = isWireTarget(o.target) ? o.target : uiTargetToWire(o.target);
  const req: OpenSubscriptionRequest = {
    runId: o.runId,
    target,
    metrics: o.metrics,
    samplesPerSimSecond: o.samplesPerSimSecond,
    percentiles: o.percentiles,
    leaseNs,
  };

  let phase: StreamPhase = 'opening';
  let sid: string | null = null;
  let lastId: string | null = null;
  let closed = false;
  let finished = false;
  let attempt = 0;
  let current: StreamHandle | null = null;
  // Local countdown of the lease, on the browser's own clock and compared only with itself.
  let leaseDeadlineMs = now() + leaseMs;
  let renewTimer: ReturnType<typeof setInterval> | null = null;

  // What was last announced, kept apart from `phase` so the first `opening` is announced too.
  let announced: StreamPhase | null = null;
  const setPhase = (p: StreamPhase, detail?: string) => {
    phase = p;
    announced = p;
    o.onPhase?.(p, detail);
  };
  // Read through a call: the closure above assigns `phase`, which TypeScript's narrowing cannot see.
  const currentPhase = (): StreamPhase => phase;
  /** Enter a phase unless already announced, so a 410 reported from the catch is not said twice. */
  const enter = (p: StreamPhase) => {
    if (announced !== p) setPhase(p);
  };

  const visible = () => typeof document === 'undefined' || document.visibilityState === 'visible';

  const forget = () => {
    // The server no longer has this subscription, so there is nothing to resume from.
    sid = null;
    lastId = null;
  };

  const renewNow = async () => {
    if (closed || sid === null) return;
    try {
      const r = await client.renewSubscription(sid, leaseNs);
      if (r.expired) {
        forget();
        current?.close();
        return;
      }
      leaseDeadlineMs = now() + leaseMs;
    } catch {
      // A failed renew is not fatal on its own; the lease simply runs down and the loop reopens.
    }
  };

  const onVisibility = () => {
    if (visible()) void renewNow();
  };
  if (typeof document !== 'undefined') document.addEventListener('visibilitychange', onVisibility);

  const startRenewals = () => {
    if (renewTimer !== null) return;
    // Half the lease: one lost renew still leaves a whole half-lease of margin.
    renewTimer = setInterval(() => {
      if (closed || !visible()) return;
      void renewNow();
    }, Math.max(1000, leaseMs / 2));
  };

  const onEvent = (ev: SseEvent) => {
    if (ev.event === 'open') {
      const res = decodeOpenSubscription(JSON.parse(ev.data));
      if (res.rejectedReason) {
        setPhase('failed', res.rejectedReason);
        closed = true;
        current?.close();
        return;
      }
      if (sid !== res.subscriptionId) {
        // A different id means the server opened a new subscription rather than resuming ours,
        // so anything remembered about the old one is stale.
        sid = res.subscriptionId;
        lastId = '0';
      }
      leaseDeadlineMs = now() + leaseMs;
      attempt = 0;
      startRenewals();
      setPhase('streaming');
      return;
    }
    if (ev.event === 'update') {
      if (ev.id !== null) lastId = ev.id;
      let u: SubscriptionUpdate;
      try {
        u = decodeSubscriptionUpdate(JSON.parse(ev.data));
      } catch (e) {
        setPhase('streaming', `undecodable update: ${e instanceof Error ? e.message : String(e)}`);
        return;
      }
      enter('streaming');
      o.onUpdate(u);
      if (u.final) finished = true;
      return;
    }
    // An event name this client does not know: a newer server, not an error.
  };

  const loop = (async () => {
    let everOpened = false;
    while (!closed) {
      const resuming = sid !== null;
      if (resuming) enter('reconnecting');
      else enter(everOpened ? 'reopening' : 'opening');
      let errored = false;
      try {
        current = openStreamImpl(client.openSubscriptionUrl(req, sid ?? undefined), {
          onEvent,
          lastEventId: resuming ? (lastId ?? '0') : undefined,
          fetchImpl: client.fetchImpl,
        });
        await current.done;
        if (sid !== null) everOpened = true;
      } catch (e) {
        if (closed) break;
        errored = true;
        if (e instanceof IngressError && e.gone) {
          // The ring cannot cover the gap, or the subscription is unknown: start over.
          forget();
          setPhase('reopening', e.message);
        } else {
          setPhase('reconnecting', e instanceof Error ? e.message : String(e));
        }
      } finally {
        current = null;
      }
      if (closed) break;
      if (finished) {
        setPhase('complete');
        break;
      }
      const leaseLive = sid !== null && now() < leaseDeadlineMs;
      // A lease that ran down means the server has forgotten the subscription: reopen, not resume.
      if (!leaseLive) forget();
      if (errored || leaseLive) {
        // Back off before resuming a stream the server closed, and before retrying anything that
        // failed, so a fleet of tabs does not hammer a server that is already struggling.
        await sleep(backoffDelayMs(attempt++, rnd));
      } else {
        attempt = 0;
      }
    }
    if (renewTimer !== null) clearInterval(renewTimer);
    if (currentPhase() !== 'complete' && currentPhase() !== 'failed') setPhase('closed');
  })();

  return {
    close() {
      if (closed) return;
      closed = true;
      if (renewTimer !== null) clearInterval(renewTimer);
      if (typeof document !== 'undefined') document.removeEventListener('visibilitychange', onVisibility);
      current?.close();
      const id = sid;
      forget();
      // Best effort: a closed tab cannot be relied on to reach this, which is why the lease exists.
      if (id !== null) void client.closeSubscription(id).catch(() => undefined);
    },
    phase: () => phase,
    subscriptionId: () => sid,
    lastEventId: () => lastId,
    done: loop,
  };
}

// ---------------------------------------------------------------------------
// Scenario, workload and policy encoding
// ---------------------------------------------------------------------------

export type ScenarioValue = number | string | boolean;

/** `--set k=v` pairs as the server takes them: every value a string, exactly as on a command line. */
export type Overrides = Record<string, string>;

/**
 * `StartRunRequest.scenario` per WIRE.md: the flat `key = value` text `sim-run run` reads, plus
 * overrides applied on top of it.
 */
export interface ScenarioEnvelope {
  text: string;
  overrides: Overrides;
}

/** The engine keys a `ScenarioConfig` field encodes to, in the order `scenarios/*.txt` lists them. */
const PANEL_KEYS = [
  'name', 'seed', 'duration_s', 'warmup_s',
  'replicas', 'max_batch', 'step_base_ms', 'step_per_seq_ms', 'step_per_kv_ktoken_ms',
  'kv_capacity_tokens', 'prefill_tokens_per_s', 'step_token_budget', 'max_queue',
  'arrival_rps', 'prompt_mean', 'prompt_cv', 'output_mean', 'output_cv',
  'long_probability', 'long_prompt_mean', 'long_output_mean',
  'routing', 'p2c_choices', 'probe_live',
  'telemetry_interval_ms', 'telemetry_delay_ms',
  'client_timeout_s', 'max_attempts',
  'ttft_slo_ms', 'itl_slo_ms', 'e2e_slo_s', 'sample_interval_ms',
] as const;

/**
 * The engine keys the control panel has no field for. They travel in `ScenarioConfig.extra`,
 * verbatim, and only at StartRun: none of them is live-tunable, and the panel does not grow a
 * control per engine key. Keep this in step with the `match` in `Scenario::parse`.
 */
export const EXTRA_KEYS = [
  'load_step_at_s', 'load_step_factor', 'load_step_until_s',
  // U33 landed while this unit was in flight: `perturbation`/`perturb_amplitude`/
  // `perturb_frequency_hz` are new engine keys, but config.ts's `workload.perturbation` fields
  // still have no encoder wired to them (see `workloadToWire`'s `dropped`), so these three are
  // accepted, verbatim `extra` keys only, same as any engine key with no panel binding yet.
  'perturbation', 'perturb_amplitude', 'perturb_frequency_hz',
  'retry_budget_fraction', 'retry_backoff_s',
  'disable_decode', 'spec_draft_tokens', 'spec_accept_rate',
  'preemption', 'preemption_victim', 'scheduling', 'dram_capacity_tokens', 'swap_gbps',
  'arrival_rps_per_replica',
  'session_turns_mean', 'session_think_s',
  'prefix_roots', 'prefix_root_tokens', 'prefix_zipf_s', 'session_fork_rate', 'prefix_cache_tokens',
  'affinity_max_load_ratio', 'affinity_fallback_choices',
  'admission', 'admission_headroom', 'fair_share_burst',
  'ejection', 'ejection_ratio', 'ejection_views', 'ejection_cooldown_s',
  'tenants', 'tenant_weights', 'tenant_demand',
  'workload', 'trace_file', 'trace_sample_rate',
  'slo_classes', 'failures',
] as const;

/** Exactly the keys `Scenario::parse` in `crates/sim-scenario` accepts. An unknown key is an error there. */
export const SCENARIO_KEYS = [...PANEL_KEYS, ...EXTRA_KEYS] as const;
export type ScenarioKey = (typeof SCENARIO_KEYS)[number];

/** The subset UpdateWorkload may carry: load shape only. */
export const WORKLOAD_KEYS: readonly ScenarioKey[] = [
  'arrival_rps', 'prompt_mean', 'prompt_cv', 'output_mean', 'output_cv',
  'long_probability', 'long_prompt_mean', 'long_output_mean',
  'load_step_at_s', 'load_step_factor', 'load_step_until_s',
];

/** The subset UpdatePolicies may carry: routing only, per the proto's PolicySpec. */
export const POLICY_KEYS: readonly ScenarioKey[] = ['routing', 'p2c_choices', 'probe_live'];

/**
 * config.ts's `RoutingKind` to the engine's routing name (`crates/sim-policy`), spelled as the
 * `scenarios/*.txt` files spell it.
 *
 * `least_kv_tokens` and `least_queue_tokens` are the same policy under two names, which is a
 * mismatch worth fixing in one of the two files rather than translating forever. `prefix_affinity`
 * now has an engine implementation (`crates/sim-policy/src/prefix_affinity.rs`, registered under
 * that name), so it maps rather than dropping: see `policiesToWire` for its two knobs.
 */
export const ROUTING_TO_ENGINE: Record<RoutingKind, string | null> = {
  round_robin: 'round_robin',
  random: 'random',
  least_requests: 'least_requests',
  least_kv_tokens: 'least_queue_tokens',
  least_kv_probe: 'least_kv_probe',
  power_of_two_choices: 'p2c',
  prefix_affinity: 'prefix_affinity',
};

export interface WireEncoding {
  /** The flat key set to send, already restricted to keys the engine accepts. */
  fields: Record<string, ScenarioValue>;
  /**
   * Control-panel fields with no engine equivalent, by their config.ts path. A caller shows these
   * as "these controls do nothing against the real server yet" rather than pretending they applied.
   */
  dropped: string[];
}

/**
 * `ScenarioConfig` to the flat key set the engine accepts.
 *
 * Everything the engine has is mapped, including the SLO thresholds and the sample rate: the engine
 * does have `ttft_slo_ms`, `itl_slo_ms`, `e2e_slo_s` and `sample_interval_ms`, so dropping them
 * would report a control as dead that is not. What genuinely has no engine equivalent is the
 * accelerator label and the workload perturbation (the engine has a one-shot load step, not a
 * sinusoid, and config.ts carries no timing for a step). The two prefix-affinity knobs are sent,
 * not dropped, when `routing.kind` is `prefix_affinity`: see `policiesToWire`.
 */
export function scenarioConfigToWire(c: ScenarioConfig, overrides: Partial<Record<ScenarioKey, ScenarioValue>> = {}): WireEncoding {
  const fields: Record<string, ScenarioValue> = {
    name: c.name,
    seed: c.seed,
    duration_s: c.durationS,
    warmup_s: c.warmupS,

    replicas: c.fleet.replicas,
    max_batch: c.fleet.maxBatch,
    step_base_ms: c.fleet.stepBaseMs,
    step_per_seq_ms: c.fleet.stepPerSeqMs,
    step_per_kv_ktoken_ms: c.fleet.stepPerKvKtokenMs,
    prefill_tokens_per_s: c.fleet.prefillTokensPerS,
    kv_capacity_tokens: c.fleet.kvTokensPerReplica,
    step_token_budget: c.fleet.stepTokenBudget,
    max_queue: c.fleet.maxQueue,

    telemetry_interval_ms: c.telemetryIntervalMs,
    telemetry_delay_ms: c.telemetryDelayMs,

    client_timeout_s: c.clientTimeoutS,
    max_attempts: c.maxAttempts,

    ttft_slo_ms: c.slo.ttftMs,
    itl_slo_ms: c.slo.itlMs,
    e2e_slo_s: c.slo.e2eS,

    // The engine records at an interval; the UI thinks in points per simulated second. Exact
    // reciprocals, so this is a unit change rather than an approximation.
    sample_interval_ms: 1000 / c.samplesPerSimSecond,
  };
  const dropped: string[] = ['fleet.accelerator'];

  const w = workloadToWire(c);
  Object.assign(fields, w.fields);
  dropped.push(...w.dropped);

  const p = policiesToWire(c);
  Object.assign(fields, p.fields);
  dropped.push(...p.dropped);

  // A typo in `extra` fails here, by name, rather than as the server's "unknown keys" refusal.
  const accepted = new Set<string>(SCENARIO_KEYS);
  for (const [k, v] of Object.entries(c.extra)) {
    if (!accepted.has(k)) throw new Error(`scenario extra key ${JSON.stringify(k)} is not one the engine accepts`);
    fields[k] = v;
  }

  for (const [k, v] of Object.entries(overrides)) if (v !== undefined) fields[k] = v;
  return { fields, dropped };
}

export function workloadToWire(c: ScenarioConfig): WireEncoding {
  const w = c.workload;
  const fields: Record<string, ScenarioValue> = {
    arrival_rps: w.arrivalRps,
    prompt_mean: w.promptMean,
    prompt_cv: w.promptCv,
    output_mean: w.outputMean,
    output_cv: w.outputCv,
    long_probability: w.longProbability,
    long_prompt_mean: w.longPromptMean,
    long_output_mean: w.longOutputMean,
  };
  // A perturbation is not expressible: `load_step_*` is one step with an explicit start and end,
  // and config.ts carries an amplitude and a frequency instead. Inventing the missing timing would
  // send a load nobody asked for, so the three fields are reported dead instead.
  const dropped = ['workload.perturbation', 'workload.perturbAmplitude', 'workload.perturbFrequencyHz'];
  return { fields, dropped };
}

export function policiesToWire(c: ScenarioConfig): WireEncoding {
  const fields: Record<string, ScenarioValue> = {
    p2c_choices: c.routing.choices,
    probe_live: c.routing.probeLive,
  };
  const dropped: string[] = [];
  const engine = ROUTING_TO_ENGINE[c.routing.kind];
  if (engine === null) dropped.push('routing.kind');
  else fields.routing = engine;
  // The affinity ceiling and its fallback width are meaningless to any other policy, so they are
  // sent only alongside `prefix_affinity`; otherwise they are reported dropped like any other
  // control with nothing on the wire to receive it.
  if (c.routing.kind === 'prefix_affinity') {
    fields.affinity_max_load_ratio = c.routing.maxLoadRatio;
    fields.affinity_fallback_choices = c.routing.fallbackChoices;
  } else {
    dropped.push('routing.maxLoadRatio', 'routing.fallbackChoices');
  }
  return { fields, dropped };
}

/** Does every key belong to the accepted set? The check `Scenario::parse` performs server-side. */
export function unacceptedKeys(fields: Record<string, ScenarioValue>, accepted: readonly string[] = SCENARIO_KEYS): string[] {
  return Object.keys(fields).filter((k) => !accepted.includes(k));
}

/** One value as `Scenario::parse` reads it: numbers via `parse::<f64>`, `probe_live` as literal `true`. */
export function scenarioValueText(v: ScenarioValue): string {
  return typeof v === 'string' ? v : String(v);
}

/**
 * The flat `key = value` text, one line per field in `SCENARIO_KEYS` order so two equal configs
 * produce identical text. Exactly what `sim-run run` reads and `Scenario::to_text` writes.
 */
export function scenarioText(fields: Record<string, ScenarioValue>): string {
  const order = new Map<string, number>(SCENARIO_KEYS.map((k, i) => [k, i]));
  const keys = Object.keys(fields).sort((a, b) => (order.get(a) ?? SCENARIO_KEYS.length) - (order.get(b) ?? SCENARIO_KEYS.length) || a.localeCompare(b));
  return keys.map((k) => `${k} = ${scenarioValueText(fields[k])}\n`).join('');
}

/** The inverse, for the self-test and for reading a served `scenarios/*.txt`: comments and blanks skipped. */
export function parseScenarioText(text: string): Record<string, string> {
  const out: Record<string, string> = {};
  for (const raw of text.split('\n')) {
    const line = raw.split('#')[0].trim();
    if (line === '') continue;
    const eq = line.indexOf('=');
    if (eq === -1) throw new Error(`expected key = value, got ${JSON.stringify(raw)}`);
    out[line.slice(0, eq).trim()] = line.slice(eq + 1).trim();
  }
  return out;
}

/** Flat fields as `--set` overrides: every value a string. */
export function toOverrides(fields: Record<string, ScenarioValue>): Overrides {
  const out: Overrides = {};
  for (const [k, v] of Object.entries(fields)) out[k] = scenarioValueText(v);
  return out;
}

/** `StartRunRequest.scenario`: the text, plus any overrides layered on top. */
export function scenarioEnvelope(fields: Record<string, ScenarioValue>, overrides: Overrides = {}): ScenarioEnvelope {
  return wr({ text: scenarioText(fields), overrides }) as unknown as ScenarioEnvelope;
}
