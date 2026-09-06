// Sampled request traces, stratified by latency bucket.
//
// Uniform sampling of hundreds of millions of requests contains almost no examples above the
// 99.9th percentile, and those are the only ones worth reading. So the recorder keeps a handful in
// each bucket and each outcome, and `GetTraces` is a query against that: opening a trace does not
// pause the run.

import type { Frame } from './engine';
import type { ScenarioConfig } from './config';
import type { MemoryTier, Outcome, RequestTrace, TraceBucket, TraceSpan } from './types';
import { TRACE_BUCKETS } from './types';
import { quantile } from './hist';
import { clamp, lognormalQuantile, uniform } from './rng';

const BUCKET_P: Record<TraceBucket, number> = { p50: 50, p90: 90, p99: 99, 'p99.9': 99.9 };
const TENANTS = ['chat-eu', 'chat-us', 'agent-batch', 'doc-summarize'];

function pickOutcome(c: ScenarioConfig, bucket: TraceBucket, e2eMs: number, ttftMs: number, r: number): Outcome {
  if (e2eMs > c.slo.e2eS * 1000) return r < 0.6 ? 'TIMEOUT_RUNNING' : 'CANCELLED';
  if (bucket === 'p99.9' && r < 0.18) return 'TIMEOUT_QUEUED';
  if (bucket === 'p99.9' && r < 0.26) return 'FAILED';
  if (ttftMs > c.slo.ttftMs) return 'OK_SLO_VIOLATED';
  return 'OK';
}

/** ingress.proto GetTraces, served from the recording. */
export function getTraces(
  c: ScenarioConfig,
  frames: Frame[],
  opts: { bucket?: TraceBucket; outcome?: Outcome | 'any'; limit: number }
): RequestTrace[] {
  if (frames.length === 0) return [];
  const buckets = opts.bucket ? [opts.bucket] : TRACE_BUCKETS;
  const out: RequestTrace[] = [];

  for (const bucket of buckets) {
    const p = BUCKET_P[bucket];
    let made = 0;
    for (let n = 0; made < opts.limit && n < opts.limit * 8; n++) {
      const f = frames[Math.floor(uniform(c.seed, n, 7, p) * frames.length)];
      if (!f) continue;
      const trace = buildTrace(c, f, bucket, n);
      if (opts.outcome && opts.outcome !== 'any' && trace.outcome !== opts.outcome) continue;
      out.push(trace);
      made++;
    }
  }
  return out;
}

function buildTrace(
  c: ScenarioConfig,
  f: Frame,
  bucket: TraceBucket,
  n: number
): RequestTrace {
  const p = BUCKET_P[bucket];
  const seed = c.seed;
  const rnd = (k: number) => uniform(seed, n, k, p, f.tick);

  const targetE2e = Math.max(quantile(f.e2e, p), 5);
  const targetTtft = Math.max(quantile(f.ttft, p), 2);
  const itlMs = Math.max(quantile(f.itl, clamp(p - 5, 1, 99.9)), 0.5);

  // Pick a replica, biased toward the loaded ones for the slow buckets: that is the whole story of
  // the hotspot dynamic, and a trace tab that hid it would be useless.
  const live = f.replicas.filter((r) => r.present && r.state !== 'EJECTED');
  const sorted = [...live].sort((a, b) => b.queuedSeqs + b.batchSize - (a.queuedSeqs + a.batchSize));
  const skew = p >= 99 ? 0.12 : p >= 90 ? 0.35 : 1;
  const replica = sorted[Math.floor(rnd(1) * Math.max(1, Math.floor(sorted.length * skew)))] ?? live[0];

  const isLong = rnd(2) < (p >= 99 ? 0.55 : c.workload.longProbability);
  const promptTokens = Math.round(
    isLong
      ? lognormalQuantile(c.workload.longPromptMean, 0.5, rnd(3))
      : lognormalQuantile(c.workload.promptMean, c.workload.promptCv, rnd(3))
  );
  const outputTokens = Math.max(
    1,
    Math.round(isLong ? lognormalQuantile(c.workload.longOutputMean, 0.6, rnd(4)) : lognormalQuantile(c.workload.outputMean, c.workload.outputCv, rnd(4)))
  );

  const hit = replica ? replica.prefixHitRate : 0.08;
  const cachedTokens = Math.round(promptTokens * hit * (0.6 + 0.6 * rnd(5)));
  const considered = candidateSet(c, f, replica ? replica.id : 0, rnd(6));

  const spans: TraceSpan[] = [];
  let cursor = 0;
  const push = (
    component: string,
    operation: string,
    durMs: number,
    over: Partial<TraceSpan> = {}
  ) => {
    const span: TraceSpan = {
      startMs: cursor,
      endMs: cursor + durMs,
      component,
      operation,
      replicaId: replica ? replica.id : 0,
      concurrentSeqs: Math.round(replica ? replica.batchSize : 0),
      kvUtilization: replica ? replica.kvUtilization : 0,
      tokensProcessed: 0,
      kvTier: 'HBM',
      ...over,
    };
    spans.push(span);
    cursor = span.endMs;
  };

  push('gateway', 'admit', 0.25 + 0.4 * rnd(7), { concurrentSeqs: 0, kvUtilization: 0 });
  push('router', 'decide', 0.4 + 0.6 * rnd(8), { concurrentSeqs: 0, kvUtilization: 0 });
  push('router', 'rpc', 0.8 + 1.4 * rnd(9), { concurrentSeqs: 0, kvUtilization: 0 });

  const fixed = cursor;
  const prefillMs = Math.max(
    1,
    ((promptTokens - cachedTokens) / c.fleet.prefillTokensPerS) * 1000 * (1 + 0.6 * (replica ? replica.batchSize / c.fleet.maxBatch : 0))
  );
  const queueMs = Math.max(0, targetTtft - fixed - prefillMs);
  if (queueMs > 0.05) push(`replica:${replica ? replica.id : 0}`, 'queue', queueMs, { tokensProcessed: 0 });

  // Chunked prefill: one span per chunk, so the step budget is visible.
  const chunks = Math.max(1, Math.ceil((promptTokens - cachedTokens) / Math.max(c.fleet.kvTokensPerReplica / 200, 512)));
  const nChunks = Math.min(chunks, 4);
  for (let k = 0; k < nChunks; k++) {
    push(`replica:${replica ? replica.id : 0}`, 'prefill', prefillMs / nChunks, {
      tokensProcessed: Math.round((promptTokens - cachedTokens) / nChunks),
    });
  }

  // Decode, interrupted by preemption stalls when KV was tight at this moment. The segment
  // durations are solved for the bucket's end-to-end target rather than stretched afterwards, so
  // no span can come out with a negative duration.
  const tight = (replica ? replica.kvUtilization : 0) > 0.9 || p >= 99;
  const stalls = tight ? 1 + Math.floor(rnd(10) * 2) : 0;
  const segs = stalls + 1;
  const stallMs: number[] = [];
  const stallTier: MemoryTier[] = [];
  for (let k = 0; k < stalls; k++) {
    const tier: MemoryTier = rnd(11 + k) < 0.6 ? 'DRAM' : 'NONE';
    stallTier.push(tier);
    stallMs.push(tier === 'DRAM' ? 20 + 18 * rnd(20 + k) : 240 + 90 * rnd(20 + k));
  }
  const stallTotal = stallMs.reduce((a, b) => a + b, 0);
  const decodeTotal = Math.max(targetE2e - cursor - stallTotal, itlMs * outputTokens * 0.25, 1);
  for (let k = 0; k < segs; k++) {
    push(`replica:${replica ? replica.id : 0}`, 'decode', decodeTotal / segs, {
      tokensProcessed: Math.round(outputTokens / segs),
    });
    if (k < stalls) {
      push(
        `replica:${replica ? replica.id : 0}`,
        stallTier[k] === 'DRAM' ? 'preempted-swap' : 'preempted-recompute',
        stallMs[k],
        {
          kvTier: stallTier[k],
          tokensProcessed: stallTier[k] === 'NONE' ? Math.round(promptTokens * 0.9) : 0,
        }
      );
    }
  }

  const total = spans[spans.length - 1].endMs;
  const outcome = pickOutcome(c, bucket, total, targetTtft, rnd(30));
  const tenantIdx = Math.floor(rnd(31) * TENANTS.length);

  return {
    requestId: 100000 + Math.floor(rnd(32) * 899999),
    tenant: TENANTS[tenantIdx],
    sloClass: tenantIdx === 2 ? 'BATCH' : tenantIdx === 3 ? 'AGENT' : 'INTERACTIVE',
    outcome,
    bucket,
    arrivalSimS: f.simS,
    promptTokens,
    outputTokens,
    ttftMs: targetTtft,
    itlMeanMs: itlMs,
    e2eMs: total,
    replicaId: replica ? replica.id : 0,
    consideredIds: considered,
    predictedCacheHitTokens: Math.round(cachedTokens * (0.7 + 0.9 * rnd(33))),
    actualCacheHitTokens: cachedTokens,
    spans,
  };
}

/** Which replicas the router looked at, recorded so herding is measurable. */
function candidateSet(c: ScenarioConfig, f: Frame, chosen: number, r: number): number[] {
  const ids = f.replicas.filter((x) => x.present && x.state === 'READY').map((x) => x.id);
  switch (c.routing.kind) {
    case 'round_robin':
      return [chosen];
    case 'random':
      return [chosen];
    case 'power_of_two_choices': {
      const out = new Set<number>([chosen]);
      for (let k = 0; out.size < Math.min(c.routing.choices, ids.length); k++) {
        out.add(ids[Math.floor(uniform(c.seed, chosen, k, r * 1e6) * ids.length)]);
      }
      return [...out];
    }
    case 'prefix_affinity': {
      const out = new Set<number>([chosen]);
      for (let k = 0; out.size < Math.min(c.routing.fallbackChoices + 1, ids.length); k++) {
        out.add(ids[Math.floor(uniform(c.seed, chosen, k + 40, r * 1e6) * ids.length)]);
      }
      return [...out];
    }
    default:
      // least_requests and least_kv_tokens rank the whole delayed snapshot.
      return ids;
  }
}
