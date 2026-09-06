// Transport client for the lbsim Ingress server.
//
// The server does not speak gRPC. It serves the message shapes of `proto/lbsim/v1/ingress.proto`
// and `subscription.proto` as **proto3-JSON over HTTP/1.1**, with the one server-streaming RPC
// mapped onto server-sent events. The protos remain the schema of record, so this file is written
// to the proto3-JSON rules rather than to whatever a particular server build happens to emit:
// lowerCamelCase field names, 64-bit integers as JSON strings, enums as their proto names, and
// absent fields meaning the proto3 default.
//
// Why `bigint` and not `number` for every uint64: simulated time is absolute Unix epoch
// nanoseconds (common.proto), around 1.79e18 for 2026, where a float64 resolves about 256 ns.
// A timestamp that passes through `number` is therefore silently wrong, and wrong in a way no
// chart reveals. Absolute instants stay `bigint` up to the point a chart needs a *relative*
// quantity, which is what `relSeconds` exists for. Durations that the wire declares as uint64
// (`leaseNs`, `simDurationNs`, latency fields on a trace) are bigint for the same reason: they
// arrive as strings and re-serialising them from a float would change the value.
//
// ---------------------------------------------------------------------------
// The one deviation from the protos
// ---------------------------------------------------------------------------
//
// `StartRunRequest.scenario` is NOT yet the nested `Scenario` message from `scenario.proto`. The
// Rust engine's scenario is a flat `key = value` set (`src/scenario.rs`, `Scenario::parse`), and
// nothing generates the nested message yet. The server therefore accepts, inside `"scenario"`,
// exactly one of:
//
//   - `{"text": "<contents of a scenarios/*.txt file>"}`, handed straight to `Scenario::parse`, or
//   - a flat object of the snake_case keys `Scenario::parse` accepts, values as JSON numbers,
//     strings or booleans (see `SCENARIO_KEYS`).
//
// `UpdateWorkloadRequest.workload` and `UpdatePoliciesRequest.policies` use the same flat
// convention, restricted to `WORKLOAD_KEYS` and `POLICY_KEYS`. When the nested messages exist this
// is the only part of the file that changes, which is why the encoding lives in one place at the
// bottom rather than being spread through the call sites.

import type { RoutingKind, Target as UiTarget, Scope as UiScope } from './types';
import type { ScenarioConfig } from './config';

// ---------------------------------------------------------------------------
// proto3-JSON scalar decoding
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

/**
 * uint64 / int64. The wire form is a string; a number is accepted only while it is still exactly
 * representable, because a large float has already lost the value and quietly carrying it on would
 * hide the loss instead of reporting it.
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

/** int32 / uint32. Genuinely a JSON number in proto3 JSON, and safely a `number`. */
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

/** subscription.proto Metric. Names are what `OpenSubscriptionRequest.metrics` carries. */
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

export const MEMORY_TIER_NUMBER = {
  MEMORY_TIER_UNSPECIFIED: 0,
  MEMORY_TIER_HBM: 1,
  MEMORY_TIER_DRAM: 2,
  MEMORY_TIER_SSD: 3,
  MEMORY_TIER_NONE: 4,
} as const;
export type MemoryTierName = keyof typeof MEMORY_TIER_NUMBER;

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

function invert<K extends string>(table: Record<K, number>): Record<number, K> {
  const out: Record<number, K> = {};
  for (const k of Object.keys(table) as K[]) out[table[k]] = k;
  return out;
}

/**
 * An enum field. Proto3 JSON sends the name; the number is accepted too, because that is what a
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
    if (!/^-?\d+$/.test(k)) throw new TypeError(`${where}: map key ${JSON.stringify(k)} is not an integer; a map<int32, ...> is JSON-encoded with stringified integer keys`);
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
  const t: WireTarget = { scope: enumName(o.scope, SCOPE_NUMBER, invert(SCOPE_NUMBER), 'SCOPE_UNSPECIFIED', `${where}.scope`) };
  if (o.clusterId !== undefined) t.clusterId = u64(o.clusterId, `${where}.clusterId`);
  if (o.poolId !== undefined) t.poolId = u64(o.poolId, `${where}.poolId`);
  if (o.tenantId !== undefined) t.tenantId = u64(o.tenantId, `${where}.tenantId`);
  if (o.replicaId !== undefined) t.replicaId = u64(o.replicaId, `${where}.replicaId`);
  if (o.sloClass !== undefined) t.sloClass = enumName(o.sloClass, SLO_CLASS_NUMBER, invert(SLO_CLASS_NUMBER), 'SLO_CLASS_UNSPECIFIED', `${where}.sloClass`);
  return t;
}

export function encodeTarget(t: WireTarget): Record<string, Json> {
  const o: Record<string, Json> = { scope: t.scope };
  if (t.clusterId !== undefined) o.clusterId = t.clusterId.toString();
  if (t.poolId !== undefined) o.poolId = t.poolId.toString();
  if (t.tenantId !== undefined) o.tenantId = t.tenantId.toString();
  if (t.replicaId !== undefined) o.replicaId = t.replicaId.toString();
  if (t.sloClass !== undefined) o.sloClass = t.sloClass;
  return o;
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
    case 'SCOPE_SLO_CLASS': return { scope, sloClass: invert(SLO_CLASS_NUMBER)[Number(t.id)] ?? 'SLO_CLASS_UNSPECIFIED' };
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
 * subscription.proto Distribution, minus the Leaf-hop histogram.
 *
 * `min` and `max` are `double` in the proto, so proto3 JSON puts them on the wire as numbers. A
 * string is accepted anyway: the transport contract this client was written against describes them
 * as strings, and tolerating both costs one branch while a mismatch would cost a run.
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
    count: u64(o.count, `${where}.count`),
    mean: dbl(o.mean, `${where}.mean`),
    min: dbl(o.min, `${where}.min`),
    max: dbl(o.max, `${where}.max`),
    percentile: arr(o.percentile, `${where}.percentile`).map((x, i) => dbl(x, `${where}.percentile[${i}]`)),
    value: arr(o.value, `${where}.value`).map((x, i) => dbl(x, `${where}.value[${i}]`)),
    fromMergedHistogram: bool(o.fromMergedHistogram, `${where}.fromMergedHistogram`),
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
  const values = enumKeyedMap(o.values, METRIC_NAME, dbl, `${where}.values`);
  const dists = enumKeyedMap(o.distributions, METRIC_NAME, decodeDistribution, `${where}.distributions`);
  return {
    target: decodeTarget(o.target, `${where}.target`),
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
    subscriptionId: str(o.subscriptionId, `${where}.subscriptionId`),
    simTimeUnixNs: u64(o.simTimeUnixNs, `${where}.simTimeUnixNs`),
    realtimeFactor: dbl(o.realtimeFactor, `${where}.realtimeFactor`),
    row: decodeMetricRow(o.row, `${where}.row`),
    final: bool(o.final, `${where}.final`),
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

const RUN_STATE_NUMBER = Object.fromEntries(RUN_STATES.map((s, i) => [s, i])) as Record<RunState, number>;

export function decodeRunStatus(v: Json, where = 'RunStatus'): RunStatus {
  const o = obj(v, where);
  return {
    runId: str(o.runId, `${where}.runId`),
    state: enumName(o.state, RUN_STATE_NUMBER, invert(RUN_STATE_NUMBER), 'STATE_UNSPECIFIED', `${where}.state`),
    simTimeUnixNs: u64(o.simTimeUnixNs, `${where}.simTimeUnixNs`),
    simEndUnixNs: u64(o.simEndUnixNs, `${where}.simEndUnixNs`),
    realtimeFactor: dbl(o.realtimeFactor, `${where}.realtimeFactor`),
    error: str(o.error, `${where}.error`),
  };
}

export interface ListRunsResult {
  runs: RunStatus[];
  nextCursor: string;
}

export function decodeListRuns(v: Json, where = 'ListRunsResponse'): ListRunsResult {
  const o = obj(v, where);
  return {
    runs: arr(o.runs, `${where}.runs`).map((r, i) => decodeRunStatus(r, `${where}.runs[${i}]`)),
    nextCursor: str(o.nextCursor, `${where}.nextCursor`),
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
    simTimeUnixNs: u64(o.simTimeUnixNs, `${where}.simTimeUnixNs`),
    fromLog: bool(o.fromLog, `${where}.fromLog`),
    restoredFromSnapshotUnixNs: u64(o.restoredFromSnapshotUnixNs, `${where}.restoredFromSnapshotUnixNs`),
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
    accepted: bool(o.accepted, `${where}.accepted`),
    requiredResimulation: bool(o.requiredResimulation, `${where}.requiredResimulation`),
    rewoundToUnixNs: u64(o.rewoundToUnixNs, `${where}.rewoundToUnixNs`),
    rejectedReason: str(o.rejectedReason, `${where}.rejectedReason`),
  };
}

export interface OpenSubscriptionResult {
  subscriptionId: string;
  leaseExpiresAtUnixNs: bigint;
  rejectedReason: string;
}

export function decodeOpenSubscription(v: Json, where = 'OpenSubscriptionResponse'): OpenSubscriptionResult {
  const o = obj(v, where);
  return {
    subscriptionId: str(o.subscriptionId, `${where}.subscriptionId`),
    leaseExpiresAtUnixNs: u64(o.leaseExpiresAtUnixNs, `${where}.leaseExpiresAtUnixNs`),
    rejectedReason: str(o.rejectedReason, `${where}.rejectedReason`),
  };
}

export interface RenewSubscriptionResult {
  leaseExpiresAtUnixNs: bigint;
  expired: boolean;
}

export function decodeRenew(v: Json, where = 'RenewSubscriptionResponse'): RenewSubscriptionResult {
  const o = obj(v, where);
  return {
    leaseExpiresAtUnixNs: u64(o.leaseExpiresAtUnixNs, `${where}.leaseExpiresAtUnixNs`),
    expired: bool(o.expired, `${where}.expired`),
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
    values: enumKeyedMap(o.values, METRIC_NAME, dbl, `${where}.values`).known,
    distributions: enumKeyedMap(o.distributions, METRIC_NAME, decodeDistribution, `${where}.distributions`).known,
    outcomeCounts: enumKeyedMap(o.outcomeCounts, OUTCOME_NAME, u64, `${where}.outcomeCounts`).known,
    declaredRatedCapacityRps: dbl(o.declaredRatedCapacityRps, `${where}.declaredRatedCapacityRps`),
    declaredRatedCapacityTokensPerS: dbl(o.declaredRatedCapacityTokensPerS, `${where}.declaredRatedCapacityTokensPerS`),
    metastableCollapse: bool(o.metastableCollapse, `${where}.metastableCollapse`),
    recoveryTimeNs: u64(o.recoveryTimeNs, `${where}.recoveryTimeNs`),
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
}

export function decodeTraceSpan(v: Json, where = 'TraceSpan'): WireTraceSpan {
  const o = obj(v, where);
  return {
    startUnixNs: u64(o.startUnixNs, `${where}.startUnixNs`),
    endUnixNs: u64(o.endUnixNs, `${where}.endUnixNs`),
    component: str(o.component, `${where}.component`),
    operation: str(o.operation, `${where}.operation`),
    replicaId: u64(o.replicaId, `${where}.replicaId`),
    concurrentSeqs: i32(o.concurrentSeqs, `${where}.concurrentSeqs`),
    kvUtilization: dbl(o.kvUtilization, `${where}.kvUtilization`),
    tokensProcessed: i32(o.tokensProcessed, `${where}.tokensProcessed`),
    kvTier: enumName(o.kvTier, MEMORY_TIER_NUMBER, invert(MEMORY_TIER_NUMBER), 'MEMORY_TIER_UNSPECIFIED', `${where}.kvTier`),
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
    id: u64(o.id, `${where}.id`),
    tenantId: u64(o.tenantId, `${where}.tenantId`),
    sloClass: enumName(o.sloClass, SLO_CLASS_NUMBER, invert(SLO_CLASS_NUMBER), 'SLO_CLASS_UNSPECIFIED', `${where}.sloClass`),
    outcome: enumName(o.outcome, OUTCOME_NUMBER, OUTCOME_NAME, 'OUTCOME_UNSPECIFIED', `${where}.outcome`),
    arrivedAtUnixNs: u64(o.arrivedAtUnixNs, `${where}.arrivedAtUnixNs`),
    admittedAtUnixNs: u64(o.admittedAtUnixNs, `${where}.admittedAtUnixNs`),
    firstTokenAtUnixNs: u64(o.firstTokenAtUnixNs, `${where}.firstTokenAtUnixNs`),
    finishedAtUnixNs: u64(o.finishedAtUnixNs, `${where}.finishedAtUnixNs`),
    promptTokens: i32(o.promptTokens, `${where}.promptTokens`),
    outputTokens: i32(o.outputTokens, `${where}.outputTokens`),
    cachedPrefixTokens: i32(o.cachedPrefixTokens, `${where}.cachedPrefixTokens`),
    clusterId: u64(o.clusterId, `${where}.clusterId`),
    replicaId: u64(o.replicaId, `${where}.replicaId`),
    replicaPathId: arr(o.replicaPathId, `${where}.replicaPathId`).map((x, i) => u64(x, `${where}.replicaPathId[${i}]`)),
    attempts: i32(o.attempts, `${where}.attempts`),
    preemptions: i32(o.preemptions, `${where}.preemptions`),
    queueWaitNs: u64(o.queueWaitNs, `${where}.queueWaitNs`),
    preemptedNs: u64(o.preemptedNs, `${where}.preemptedNs`),
    ttftNs: u64(o.ttftNs, `${where}.ttftNs`),
    e2eNs: u64(o.e2eNs, `${where}.e2eNs`),
    meanItlNs: u64(o.meanItlNs, `${where}.meanItlNs`),
    p99ItlNs: u64(o.p99ItlNs, `${where}.p99ItlNs`),
  };
}

export interface WireRequestTrace {
  record: WireRequestRecord;
  spans: WireTraceSpan[];
}

export function decodeRequestTrace(v: Json, where = 'RequestTrace'): WireRequestTrace {
  const o = obj(v, where);
  return {
    record: decodeRequestRecord(o.record, `${where}.record`),
    spans: arr(o.spans, `${where}.spans`).map((s, i) => decodeTraceSpan(s, `${where}.spans[${i}]`)),
  };
}

/** GetTracesResponse. Exported so a caller (and the self-test) can decode a captured body. */
export function decodeGetTraces(v: Json, where = 'GetTracesResponse'): WireRequestTrace[] {
  return arr(obj(v, where).traces, `${where}.traces`).map((t, i) => decodeRequestTrace(t, `${where}.traces[${i}]`));
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
    runId: str(o.runId, `${where}.runId`),
    seed: u64(o.seed, `${where}.seed`),
    eventCount: u64(o.eventCount, `${where}.eventCount`),
    stateChecksum: u64(o.stateChecksum, `${where}.stateChecksum`),
    overall: decodeScorecard(o.overall, `${where}.overall`),
    byScope: arr(o.byScope, `${where}.byScope`).map((s, i) => {
      const so = obj(s, `${where}.byScope[${i}]`);
      return {
        target: decodeTarget(so.target, `${where}.byScope[${i}].target`),
        scorecard: decodeScorecard(so.scorecard, `${where}.byScope[${i}].scorecard`),
      };
    }),
    recorded: arr(o.recorded, `${where}.recorded`).map((u, i) => decodeSubscriptionUpdate(u, `${where}.recorded[${i}]`)),
    traces: arr(o.traces, `${where}.traces`).map((t, i) => decodeRequestTrace(t, `${where}.traces[${i}]`)),
    refereeViolations: enumKeyedMap(o.refereeViolations, REFEREE_VERDICT_NAME, u64, `${where}.refereeViolations`).known,
    wallClockSeconds: dbl(o.wallClockSeconds, `${where}.wallClockSeconds`),
    realtimeFactor: dbl(o.realtimeFactor, `${where}.realtimeFactor`),
  };
}

// ---------------------------------------------------------------------------
// Errors and the HTTP layer
// ---------------------------------------------------------------------------

/** A non-2xx response, whose body is `{"error":{"code":<int>,"message":"..."}}`. */
export class IngressError extends Error {
  readonly code: number;
  readonly httpStatus: number;
  constructor(code: number, message: string, httpStatus: number) {
    super(message);
    this.name = 'IngressError';
    this.code = code;
    this.httpStatus = httpStatus;
  }
}

export type FetchLike = (input: string, init?: RequestInit) => Promise<Response>;

export interface ClientOptions {
  baseUrl: string;
  /** A seam for tests and for a caller that wants to add headers or a timeout. */
  fetchImpl?: FetchLike;
}

function joinUrl(baseUrl: string, path: string): string {
  const base = baseUrl.replace(/\/+$/, '');
  return `${base}${path}`;
}

function query(params: Record<string, string | number | bigint | undefined>): string {
  const q = new URLSearchParams();
  for (const [k, v] of Object.entries(params)) {
    if (v === undefined || v === '') continue;
    q.set(k, typeof v === 'bigint' ? v.toString() : String(v));
  }
  const s = q.toString();
  return s ? `?${s}` : '';
}

export class IngressClient {
  readonly baseUrl: string;
  private readonly fetchImpl: FetchLike;

  constructor(opts: ClientOptions) {
    this.baseUrl = opts.baseUrl.replace(/\/+$/, '');
    // Bound late so a page that installs a fetch wrapper after construction still gets it.
    this.fetchImpl = opts.fetchImpl ?? ((input, init) => fetch(input, init));
  }

  url(path: string): string {
    return joinUrl(this.baseUrl, path);
  }

  private async call<T>(method: 'GET' | 'POST', path: string, body: Json, decode: (v: Json) => T, signal?: AbortSignal): Promise<T> {
    const init: RequestInit = { method, signal };
    if (method === 'POST') {
      init.headers = { 'content-type': 'application/json' };
      init.body = JSON.stringify(body ?? {});
    }
    const res = await this.fetchImpl(this.url(path), init);
    const text = await res.text();
    let parsed: Json = undefined;
    if (text.length > 0) {
      try {
        parsed = JSON.parse(text);
      } catch {
        if (res.ok) throw new IngressError(0, `${method} ${path}: response body is not JSON: ${text.slice(0, 200)}`, res.status);
        throw new IngressError(0, `${method} ${path}: HTTP ${res.status}: ${text.slice(0, 200)}`, res.status);
      }
    }
    if (!res.ok) throw toIngressError(parsed, res.status, `${method} ${path}`);
    return decode(parsed);
  }

  // -- lifecycle ----------------------------------------------------------

  /** StartRun. `scenario` is the flat key set documented at the top of this file. */
  async startRun(req: StartRunRequest, signal?: AbortSignal): Promise<string> {
    const body: Record<string, Json> = { scenario: req.scenario };
    if (req.maxRealtimeFactor !== undefined) body.maxRealtimeFactor = req.maxRealtimeFactor;
    if (req.recordTraces !== undefined) body.recordTraces = req.recordTraces;
    return this.call('POST', '/v1/runs', body, (v) => str(obj(v, 'StartRunResponse').runId, 'StartRunResponse.runId'), signal);
  }

  stopRun(runId: string, signal?: AbortSignal): Promise<RunStatus> {
    return this.call('POST', `/v1/runs/${encodeURIComponent(runId)}:stop`, {}, decodeRunStatus, signal);
  }

  getRun(runId: string, signal?: AbortSignal): Promise<RunStatus> {
    return this.call('GET', `/v1/runs/${encodeURIComponent(runId)}`, null, decodeRunStatus, signal);
  }

  listRuns(opts: { limit?: number; cursor?: string } = {}, signal?: AbortSignal): Promise<ListRunsResult> {
    return this.call('GET', `/v1/runs${query({ limit: opts.limit, cursor: opts.cursor })}`, null, decodeListRuns, signal);
  }

  // -- interactive control ------------------------------------------------

  setSpeed(runId: string, realtimeFactor: number, paused: boolean, signal?: AbortSignal): Promise<RunStatus> {
    return this.call('POST', `/v1/runs/${encodeURIComponent(runId)}:setSpeed`, { realtimeFactor, paused }, decodeRunStatus, signal);
  }

  /** StepForward. Bounded server-side: the returned status says where it actually stopped. */
  stepForward(runId: string, simDurationNs: bigint, signal?: AbortSignal): Promise<RunStatus> {
    return this.call('POST', `/v1/runs/${encodeURIComponent(runId)}:stepForward`, { simDurationNs: simDurationNs.toString() }, decodeRunStatus, signal);
  }

  rewind(runId: string, toSimTimeUnixNs: bigint, signal?: AbortSignal): Promise<RewindResult> {
    return this.call('POST', `/v1/runs/${encodeURIComponent(runId)}:rewind`, { toSimTimeUnixNs: toSimTimeUnixNs.toString() }, decodeRewind, signal);
  }

  updateWorkload(runId: string, workload: Record<string, ScenarioValue>, signal?: AbortSignal): Promise<UpdateResult> {
    return this.call('POST', `/v1/runs/${encodeURIComponent(runId)}:updateWorkload`, { workload }, decodeUpdate, signal);
  }

  updatePolicies(runId: string, policies: Record<string, ScenarioValue>, signal?: AbortSignal): Promise<UpdateResult> {
    return this.call('POST', `/v1/runs/${encodeURIComponent(runId)}:updatePolicies`, { policies }, decodeUpdate, signal);
  }

  // -- observation --------------------------------------------------------

  openSubscription(req: OpenSubscriptionRequest, signal?: AbortSignal): Promise<OpenSubscriptionResult> {
    return this.call('POST', '/v1/subscriptions', encodeOpenSubscription(req), decodeOpenSubscription, signal);
  }

  renewSubscription(subscriptionId: string, leaseNs: bigint, signal?: AbortSignal): Promise<RenewSubscriptionResult> {
    return this.call('POST', `/v1/subscriptions/${encodeURIComponent(subscriptionId)}:renew`, { leaseNs: leaseNs.toString() }, decodeRenew, signal);
  }

  closeSubscription(subscriptionId: string, signal?: AbortSignal): Promise<void> {
    return this.call('POST', `/v1/subscriptions/${encodeURIComponent(subscriptionId)}:close`, {}, () => undefined, signal);
  }

  streamUrl(subscriptionId: string): string {
    return this.url(`/v1/subscriptions/${encodeURIComponent(subscriptionId)}/stream`);
  }

  // -- results ------------------------------------------------------------

  getResult(runId: string, signal?: AbortSignal): Promise<RunResult> {
    return this.call('GET', `/v1/runs/${encodeURIComponent(runId)}/result`, null, decodeRunResult, signal);
  }

  getTraces(runId: string, opts: { outcome?: OutcomeName; minE2eNs?: bigint; limit?: number } = {}, signal?: AbortSignal): Promise<WireRequestTrace[]> {
    const path = `/v1/runs/${encodeURIComponent(runId)}/traces${query({ outcome: opts.outcome, minE2eNs: opts.minE2eNs, limit: opts.limit })}`;
    return this.call('GET', path, null, decodeGetTraces, signal);
  }
}

export function toIngressError(body: Json, httpStatus: number, where: string): IngressError {
  if (isObject(body) && isObject(body.error)) {
    const e = body.error;
    return new IngressError(i32(e.code, 'error.code'), str(e.message, 'error.message') || `${where}: HTTP ${httpStatus}`, httpStatus);
  }
  return new IngressError(0, `${where}: HTTP ${httpStatus}`, httpStatus);
}

export interface StartRunRequest {
  scenario: Record<string, Json>;
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

export function encodeOpenSubscription(req: OpenSubscriptionRequest): Record<string, Json> {
  const o: Record<string, Json> = {
    runId: req.runId,
    target: encodeTarget(req.target),
    metrics: req.metrics,
    samplesPerSimSecond: req.samplesPerSimSecond,
  };
  if (req.percentiles && req.percentiles.length) o.percentiles = req.percentiles;
  if (req.leaseNs !== undefined) o.leaseNs = req.leaseNs.toString();
  return o;
}

// ---------------------------------------------------------------------------
// Server-sent events
// ---------------------------------------------------------------------------

export interface SseSplit {
  frames: string[];
  /** What is left over: a partial frame, to be prefixed to the next chunk. */
  rest: string;
}

/**
 * Split a buffer into complete SSE frames, returning the incomplete tail.
 *
 * Pure and exported on purpose: frame boundaries are the part of this transport most likely to be
 * wrong, and a chunk boundary in the middle of a JSON object is the failure that only shows up
 * under load. Testing it needs no network.
 */
export function parseSseFrames(buffer: string): SseSplit {
  const frames: string[] = [];
  const boundary = /\r?\n\r?\n/g;
  let start = 0;
  let m: RegExpExecArray | null;
  while ((m = boundary.exec(buffer)) !== null) {
    const block = buffer.slice(start, m.index);
    start = m.index + m[0].length;
    const data = sseFrameData(block);
    if (data !== null) frames.push(data);
  }
  return { frames, rest: buffer.slice(start) };
}

/** The `data:` payload of one frame block, or null for a block that carries none (a keepalive). */
function sseFrameData(block: string): string | null {
  const lines = block.split(/\r?\n/);
  const data: string[] = [];
  for (const line of lines) {
    if (line === '' || line.startsWith(':')) continue; // comment / keepalive
    const colon = line.indexOf(':');
    const field = colon === -1 ? line : line.slice(0, colon);
    if (field !== 'data') continue; // event:, id:, retry: are not used by this transport
    let value = colon === -1 ? '' : line.slice(colon + 1);
    if (value.startsWith(' ')) value = value.slice(1);
    data.push(value);
  }
  return data.length ? data.join('\n') : null;
}

/** Stateful wrapper over `parseSseFrames`, so a caller never has to hold the tail itself. */
export class SseBuffer {
  private rest = '';
  push(chunk: string): string[] {
    const { frames, rest } = parseSseFrames(this.rest + chunk);
    this.rest = rest;
    return frames;
  }
  /** Anything still buffered when the stream ends. A well-behaved server leaves nothing. */
  pending(): string {
    return this.rest;
  }
}

export interface StreamOptions {
  onFrame: (data: string) => void;
  signal?: AbortSignal;
  fetchImpl?: FetchLike;
}

/**
 * Read an SSE stream with `fetch` and a manual frame parser. The fallback path, and the only path
 * when a caller supplied its own `fetch` or when `EventSource` is absent (Node, a worker, a test).
 *
 * Resolves when the server closes the stream, which for this transport is expected rather than
 * exceptional: subscriptions are bounded and the caller reconnects.
 */
export async function readSseWithFetch(url: string, o: StreamOptions): Promise<void> {
  const f = o.fetchImpl ?? ((input: string, init?: RequestInit) => fetch(input, init));
  const res = await f(url, { method: 'GET', headers: { accept: 'text/event-stream' }, signal: o.signal });
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
  if (!res.body) throw new IngressError(0, `GET ${url}: response has no body to stream`, res.status);
  const reader = res.body.getReader();
  const decoder = new TextDecoder();
  const buf = new SseBuffer();
  for (;;) {
    const { done, value } = await reader.read();
    if (done) break;
    // `stream: true` because a multi-byte character can straddle a chunk just as a frame can.
    for (const frame of buf.push(decoder.decode(value, { stream: true }))) o.onFrame(frame);
  }
  for (const frame of buf.push(decoder.decode())) o.onFrame(frame);
}

export interface StreamHandle {
  /** Resolves when the server closed the stream; rejects on a transport error. */
  done: Promise<void>;
  close(): void;
}

/**
 * Open a stream, preferring `EventSource` because the browser owns the parsing, the connection
 * accounting and the back-pressure. It is only usable for a plain GET with no custom fetch, which
 * is exactly what this transport's stream endpoint is.
 */
export function openStream(url: string, o: StreamOptions): StreamHandle {
  const nativeUsable = typeof EventSource !== 'undefined' && !o.fetchImpl;
  if (!nativeUsable) {
    const ctl = new AbortController();
    const signal = o.signal ?? ctl.signal;
    const done = readSseWithFetch(url, { ...o, signal });
    return { done, close: () => ctl.abort() };
  }
  const es = new EventSource(url);
  let settle: () => void = () => {};
  let fail: (e: unknown) => void = () => {};
  const done = new Promise<void>((resolve, reject) => {
    settle = resolve;
    fail = reject;
  });
  let closed = false;
  const shut = () => {
    closed = true;
    es.close();
  };
  es.onmessage = (ev: MessageEvent) => o.onFrame(typeof ev.data === 'string' ? ev.data : String(ev.data));
  es.onerror = () => {
    // EventSource reports a closed stream and a failed connection the same way, so treat a closed
    // readyState as the ordinary end of a bounded stream and let the caller decide to reconnect.
    if (closed) return;
    shut();
    if (es.readyState === 2) settle();
    else fail(new IngressError(0, `stream ${url}: connection error`, 0));
  };
  o.signal?.addEventListener('abort', shut);
  return { done, close: shut };
}

// ---------------------------------------------------------------------------
// Subscription lifecycle: lease renewal, reconnection, reopening
// ---------------------------------------------------------------------------

export const DEFAULT_LEASE_NS = 60_000_000_000n; // 60 s, matching the mock registry's order of magnitude
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

export type StreamPhase = 'opening' | 'streaming' | 'reconnecting' | 'reopening' | 'closed' | 'failed';

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
  now?: () => number;
  openStreamImpl?: (url: string, o: StreamOptions) => StreamHandle;
}

export interface SubscriptionHandle {
  close(): void;
  phase(): StreamPhase;
  subscriptionId(): string | null;
  /** Resolves when the handle is closed and its loop has stopped. */
  done: Promise<void>;
}

function isWireTarget(t: WireTarget | UiTarget): t is WireTarget {
  return typeof t.scope === 'string' && t.scope.startsWith('SCOPE_');
}

/**
 * One subscription for one entity, kept alive for as long as the caller wants it.
 *
 * Three things the protos make the client's job, all of them here rather than in the panels:
 * the lease is renewed at half its length and not at all while the document is hidden, so a
 * backgrounded tab expires instead of costing the server forever; a stream the server closes is
 * reconnected with jittered backoff while the lease is still live; and an expired lease is a
 * reopen from scratch rather than a reconnect, because the server has forgotten the subscription.
 */
export function subscribeToTarget(client: IngressClient, o: SubscribeOptions): SubscriptionHandle {
  const leaseNs = o.leaseNs ?? DEFAULT_LEASE_NS;
  const leaseMs = Number(leaseNs) / 1e6;
  const rnd = o.rnd ?? Math.random;
  const now = o.now ?? (() => Date.now());
  const sleep = o.sleep ?? ((ms: number) => new Promise<void>((r) => setTimeout(r, ms)));
  const openStreamImpl = o.openStreamImpl ?? openStream;
  const target = isWireTarget(o.target) ? o.target : uiTargetToWire(o.target);

  let phase: StreamPhase = 'opening';
  let sid: string | null = null;
  let closed = false;
  let current: StreamHandle | null = null;
  // The lease deadline is tracked on the browser's clock. The server's absolute
  // `leaseExpiresAtUnixNs` is on the server's, and the two are not comparable; the server remains
  // the authority and says so through `expired` on renew.
  let leaseDeadlineMs = now() + leaseMs;
  let renewTimer: ReturnType<typeof setInterval> | null = null;

  const setPhase = (p: StreamPhase, detail?: string) => {
    phase = p;
    o.onPhase?.(p, detail);
  };

  const visible = () => typeof document === 'undefined' || document.visibilityState === 'visible';

  const renewNow = async () => {
    if (closed || sid === null) return;
    try {
      const r = await client.renewSubscription(sid, leaseNs);
      if (r.expired) {
        // The server dropped it: nothing to renew, so the loop must open a new subscription.
        sid = null;
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

  const loop = (async () => {
    let attempt = 0;
    // Distinguishes the first open from every later one, so the UI can say "reopening" rather than
    // "opening" when a lease was allowed to lapse: those are different stories about the same tab.
    let everOpened = false;
    while (!closed) {
      if (sid === null) {
        setPhase(everOpened ? 'reopening' : 'opening');
        try {
          const res = await client.openSubscription({
            runId: o.runId,
            target,
            metrics: o.metrics,
            samplesPerSimSecond: o.samplesPerSimSecond,
            percentiles: o.percentiles,
            leaseNs,
          });
          if (res.rejectedReason) {
            setPhase('failed', res.rejectedReason);
            return;
          }
          sid = res.subscriptionId;
          everOpened = true;
          leaseDeadlineMs = now() + leaseMs;
          attempt = 0;
          startRenewals();
        } catch (e) {
          setPhase('reopening', e instanceof Error ? e.message : String(e));
          await sleep(backoffDelayMs(attempt++, rnd));
          continue;
        }
      }
      setPhase('streaming');
      try {
        current = openStreamImpl(client.streamUrl(sid), {
          onFrame: (data) => {
            try {
              o.onUpdate(decodeSubscriptionUpdate(JSON.parse(data)));
            } catch (e) {
              setPhase('streaming', `undecodable frame: ${e instanceof Error ? e.message : String(e)}`);
            }
          },
        });
        await current.done;
      } catch (e) {
        setPhase('reconnecting', e instanceof Error ? e.message : String(e));
      } finally {
        current = null;
      }
      if (closed) break;
      if (sid !== null && now() < leaseDeadlineMs) {
        setPhase('reconnecting');
        await sleep(backoffDelayMs(attempt++, rnd));
      } else {
        // Lease gone: the subscription no longer exists server-side, so reopen rather than reconnect.
        sid = null;
        attempt = 0;
      }
    }
    setPhase('closed');
  })();

  return {
    close() {
      if (closed) return;
      closed = true;
      if (renewTimer !== null) clearInterval(renewTimer);
      if (typeof document !== 'undefined') document.removeEventListener('visibilitychange', onVisibility);
      current?.close();
      const id = sid;
      sid = null;
      // Best effort: a closed tab cannot be relied on to reach this, which is why the lease exists.
      if (id !== null) void client.closeSubscription(id).catch(() => undefined);
    },
    phase: () => phase,
    subscriptionId: () => sid,
    done: loop,
  };
}

// ---------------------------------------------------------------------------
// Scenario, workload and policy encoding
// ---------------------------------------------------------------------------

export type ScenarioValue = number | string | boolean;

/** Exactly the keys `Scenario::parse` in `src/scenario.rs` accepts. An unknown key is an error there. */
export const SCENARIO_KEYS = [
  'name', 'seed', 'duration_s', 'warmup_s',
  'replicas', 'max_batch', 'step_base_ms', 'step_per_seq_ms', 'step_per_kv_ktoken_ms',
  'kv_capacity_tokens', 'prefill_tokens_per_s', 'step_token_budget', 'max_queue',
  'arrival_rps', 'prompt_mean', 'prompt_cv', 'output_mean', 'output_cv',
  'long_probability', 'long_prompt_mean', 'long_output_mean',
  'load_step_at_s', 'load_step_factor', 'load_step_until_s',
  'routing', 'p2c_choices', 'probe_live',
  'telemetry_interval_ms', 'telemetry_delay_ms',
  'client_timeout_s', 'max_attempts', 'retry_budget_fraction', 'retry_backoff_s',
  'ttft_slo_ms', 'itl_slo_ms', 'e2e_slo_s', 'sample_interval_ms',
] as const;
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
 * config.ts's `RoutingKind` to the engine's routing name (`src/policy.rs`).
 *
 * `least_kv_tokens` and `least_queue_tokens` are the same policy under two names, which is a
 * mismatch worth fixing in one of the two files rather than translating forever. `prefix_affinity`
 * has no engine implementation at all, so it is reported as dropped: sending it would make
 * `Scenario::parse` succeed and the engine silently fall back, which is the worst of both.
 */
export const ROUTING_TO_ENGINE: Record<RoutingKind, string | null> = {
  round_robin: 'round_robin',
  random: 'random',
  least_requests: 'least_requests',
  least_kv_tokens: 'least_queue_tokens',
  power_of_two_choices: 'power_of_two_choices',
  prefix_affinity: null,
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
 * `ScenarioConfig` to the flat key set the server accepts.
 *
 * Everything the engine has is mapped, including the SLO thresholds and the sample rate: the engine
 * does have `ttft_slo_ms`, `itl_slo_ms`, `e2e_slo_s` and `sample_interval_ms`, so dropping them
 * would report a control as dead that is not. What genuinely has no engine equivalent is the
 * accelerator label, the workload perturbation (the engine has a one-shot load step, not a
 * sinusoid, and config.ts carries no timing for a step), and the two prefix-affinity knobs.
 */
export function scenarioConfigToWire(c: ScenarioConfig, extra: Partial<Record<ScenarioKey, ScenarioValue>> = {}): WireEncoding {
  const fields: Record<string, ScenarioValue> = {
    name: c.name,
    seed: c.seed,
    duration_s: c.durationS,
    warmup_s: c.warmupS,

    replicas: c.fleet.replicas,
    max_batch: c.fleet.maxBatch,
    step_base_ms: c.fleet.stepBaseMs,
    step_per_seq_ms: c.fleet.stepPerSeqMs,
    prefill_tokens_per_s: c.fleet.prefillTokensPerS,
    kv_capacity_tokens: c.fleet.kvTokensPerReplica,
    max_queue: c.fleet.maxQueue,

    telemetry_interval_ms: c.telemetryIntervalMs,
    telemetry_delay_ms: c.telemetryDelayMs,

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

  for (const [k, v] of Object.entries(extra)) if (v !== undefined) fields[k] = v;
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
  const dropped = ['routing.maxLoadRatio', 'routing.fallbackChoices'];
  const engine = ROUTING_TO_ENGINE[c.routing.kind];
  if (engine === null) dropped.unshift('routing.kind');
  else fields.routing = engine;
  return { fields, dropped };
}

/** Does every key belong to the accepted set? The check `Scenario::parse` performs server-side. */
export function unacceptedKeys(fields: Record<string, ScenarioValue>, accepted: readonly string[] = SCENARIO_KEYS): string[] {
  return Object.keys(fields).filter((k) => !accepted.includes(k));
}

/** `{"text": ...}`: the other accepted form of `scenario`, a `scenarios/*.txt` file verbatim. */
export function scenarioText(text: string): Record<string, Json> {
  return { text };
}
