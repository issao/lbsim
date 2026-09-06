// Types mirroring proto/lbsim/v1. Deliberately narrow: only what the stand-in draws.
//
// When sim-ingress exists these are replaced by generated clients from proto/. Field names here
// follow the proto names in camelCase so the swap is mechanical. See web/README.md.

/** subscription.proto Metric. Numeric values match the proto enum. */
export const Metric = {
  TTFT: 1,
  ITL: 2,
  E2E: 3,
  QUEUE_WAIT: 4,
  STEP_TIME: 8,
  KV_UTILIZATION: 20,
  KV_TOKENS_RESIDENT: 21,
  RUNNING_SEQS: 22,
  QUEUED_SEQS: 23,
  BATCH_SIZE: 25,
  PREFIX_CACHE_TOKENS: 26,
  TIER_UTILIZATION: 27,
  TIER_BANDWIDTH_UTILIZATION: 28,
  TELEMETRY_STALENESS: 29,
  OFFERED_RPS: 40,
  ADMITTED_RPS: 41,
  COMPLETED_RPS: 42,
  REJECTED_RPS: 43,
  OUTPUT_TOKENS_PER_S: 44,
  GOODPUT_TOKENS_PER_S: 45,
  PREEMPTIONS_PER_S: 46,
  PREFIX_HIT_RATE: 48,
  READY_REPLICAS: 60,
  WARMING_REPLICAS: 61,
  DRAINING_REPLICAS: 62,
  LOAD_IMBALANCE_CV: 64,
  WASTED_GPU_FRACTION: 65,
  SLO_ATTAINMENT: 66,
} as const;
export type MetricId = (typeof Metric)[keyof typeof Metric];

/** subscription.proto Scope. */
export type Scope = 'FLEET' | 'CLUSTER' | 'POOL' | 'REPLICA' | 'TENANT' | 'SLO_CLASS';

/** subscription.proto Target: exactly one entity. */
export interface Target {
  scope: Scope;
  id?: number;
}

/** common.proto Outcome. */
export type Outcome =
  | 'OK'
  | 'OK_SLO_VIOLATED'
  | 'REJECTED'
  | 'TIMEOUT_QUEUED'
  | 'TIMEOUT_RUNNING'
  | 'CANCELLED'
  | 'FAILED';

export const OUTCOMES: Outcome[] = [
  'OK',
  'OK_SLO_VIOLATED',
  'REJECTED',
  'TIMEOUT_QUEUED',
  'TIMEOUT_RUNNING',
  'CANCELLED',
  'FAILED',
];

/** common.proto MemoryTier. */
export type MemoryTier = 'HBM' | 'DRAM' | 'SSD' | 'NONE';

/** common.proto Health.Announced, plus the lifecycle states cluster health draws. */
export type ReplicaState = 'READY' | 'WARMING' | 'DRAINING' | 'EJECTED';

/** scenario.proto RoutingPolicy.kind. */
export type RoutingKind =
  | 'round_robin'
  | 'random'
  | 'least_requests'
  | 'least_kv_tokens'
  | 'power_of_two_choices'
  | 'prefix_affinity';

/** subscription.proto Distribution, minus the histogram-on-the-wire fields. */
export interface Distribution {
  count: number;
  mean: number;
  min: number;
  max: number;
  percentile: number[];
  value: number[];
  fromMergedHistogram: boolean;
}

/** metrics.proto TraceSpan. */
export interface TraceSpan {
  startMs: number;
  endMs: number;
  component: string;
  operation: string;
  replicaId: number;
  concurrentSeqs: number;
  kvUtilization: number;
  tokensProcessed: number;
  kvTier: MemoryTier;
}

/** metrics.proto RequestTrace + the RequestRecord fields the waterfall shows. */
export interface RequestTrace {
  requestId: number;
  tenant: string;
  sloClass: 'INTERACTIVE' | 'AGENT' | 'BATCH';
  outcome: Outcome;
  bucket: TraceBucket;
  arrivalSimS: number;
  promptTokens: number;
  outputTokens: number;
  ttftMs: number;
  itlMeanMs: number;
  e2eMs: number;
  replicaId: number;
  consideredIds: number[];
  predictedCacheHitTokens: number;
  actualCacheHitTokens: number;
  spans: TraceSpan[];
}

export type TraceBucket = 'p50' | 'p90' | 'p99' | 'p99.9';
export const TRACE_BUCKETS: TraceBucket[] = ['p50', 'p90', 'p99', 'p99.9'];

/** ingress.proto RewindResponse. */
export interface RewindResponse {
  simTimeS: number;
  fromLog: boolean;
  restoredFromSnapshotS: number;
}

/** ingress.proto UpdateResponse. */
export interface UpdateResponse {
  accepted: boolean;
  requiredResimulation: boolean;
  rewoundToS: number;
  rejectedReason: string;
  /** Not in the proto: what the UI changed, so the banner can name it. */
  changed: string[];
}
