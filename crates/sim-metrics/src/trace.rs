//! Request traces: one sampled request's journey through the fleet, span by span, with what each
//! resource was doing while the span ran.
//!
//! Issao: "We should have a way to sample requests to see execution traces and what was busy in each
//! resource as it executed." The shapes here are `metrics.proto` `RequestTrace` and `TraceSpan`, with
//! more detail than the proto carries today (`ResourceState`), because the engine knows it and the
//! wire can grow into it. Everything downstream, the JSON encoder, `GetTraces` filters, the export and
//! the dashboard panel, is built against this file; the engine fills it in the step (`SpanKind` per
//! span) and the only call it makes at completion is `TraceSampler::keep`.
//!
//! Until the engine records spans, `fixtures::sample_traces` produces deterministic traces shaped like
//! the real thing, so the downstream can be tested now.

use crate::{Histogram, Outcome, RequestRecord};
use sim_core::rng::Rng;
use sim_core::Nanos;

/// `common.proto` `MemoryTier`: where a request's KV cache lived during a span.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum MemoryTier {
    #[default]
    Hbm,
    Dram,
    Ssd,
    /// Discarded; resuming means recomputing prefill.
    None,
}

impl MemoryTier {
    pub fn name(self) -> &'static str {
        match self {
            MemoryTier::Hbm => "MEMORY_TIER_HBM",
            MemoryTier::Dram => "MEMORY_TIER_DRAM",
            MemoryTier::Ssd => "MEMORY_TIER_SSD",
            MemoryTier::None => "MEMORY_TIER_NONE",
        }
    }
}

/// Which roofline the step sat under. Decode at small batch is bandwidth-bound (weights are re-read
/// per step); prefill and large batches are compute-bound. Which one explains why a span took as long
/// as it did, so it travels with the span rather than being re-derived from batch size.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum BandwidthOrCompute {
    #[default]
    Bandwidth,
    Compute,
}

/// What the resource was doing while the span ran. For a replica span it is the replica; for the
/// gateway and router spans it is that component's own queue and in-flight count, with the KV fields
/// zero because those components hold no cache.
#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub struct ResourceState {
    /// Sequences in the step this span was part of.
    pub batch_size: u32,
    /// Sequences in flight on the resource, including those not in this step.
    pub running: u32,
    /// Sequences waiting behind this one.
    pub queued: u32,
    pub kv_tokens_resident: u64,
    pub kv_capacity: u64,
    /// The step's duration, which for a decode span is the inter-token gap this request saw.
    pub step_ns: Nanos,
    pub bound: BandwidthOrCompute,
}

impl ResourceState {
    /// Fraction of the KV budget in use, the proto's `kv_utilization`. Zero when there is no
    /// cache, which is the gateway and router case.
    pub fn kv_utilization(&self) -> f64 {
        if self.kv_capacity == 0 {
            0.0
        } else {
            self.kv_tokens_resident as f64 / self.kv_capacity as f64
        }
    }
}

/// What the span was. The variants carry the detail specific to that kind of span; the resource
/// conditions common to every span live in `ResourceState`.
#[derive(Clone, PartialEq, Debug)]
pub enum SpanKind {
    /// Waiting at the gateway for a routing decision.
    IngressQueue,
    /// The router choosing a replica: which ones it looked at and how old its view of them was.
    RoutingDecision { candidates: Vec<u64>, stale_view_age: Nanos },
    /// Waiting in the replica's queue for a slot in a batch.
    ReplicaQueue,
    /// One chunk of prefill, of `tokens` prompt tokens.
    PrefillChunk { tokens: u32 },
    /// Admitted to the batch with prompt left, and served nothing this step: the step's prefill
    /// budget, `others_prefill` tokens of it, went to the sequences ahead. One span per step waited,
    /// the granularity of every other replica span, so a journey on a replica is contiguous.
    PrefillWait { others_prefill: u32 },
    /// One decode step, producing one token.
    DecodeStep,
    /// Bringing the KV cache back from a lower tier.
    KvFetch,
    /// Evicted from the batch, waiting to resume.
    Preempted,
}

impl SpanKind {
    /// The proto's `operation`. `queue` for both queues, distinguished by `component`.
    pub fn operation(&self) -> &'static str {
        match self {
            SpanKind::IngressQueue | SpanKind::ReplicaQueue => "queue",
            SpanKind::RoutingDecision { .. } => "route",
            SpanKind::PrefillChunk { .. } => "prefill",
            SpanKind::PrefillWait { .. } => "prefill_wait",
            SpanKind::DecodeStep => "decode",
            SpanKind::KvFetch => "kv_fetch",
            SpanKind::Preempted => "preempted",
        }
    }

    /// The proto's `tokens_processed`: prompt tokens for a prefill chunk, one for a decode step, and
    /// for a wait in the batch the prefill tokens the step spent on the other sequences, which is
    /// what the wait is made of; nothing for a queue.
    pub fn tokens_processed(&self) -> u32 {
        match self {
            SpanKind::PrefillChunk { tokens } => *tokens,
            SpanKind::PrefillWait { others_prefill } => *others_prefill,
            SpanKind::DecodeStep => 1,
            _ => 0,
        }
    }

    pub fn is_replica_span(&self) -> bool {
        !matches!(self, SpanKind::IngressQueue | SpanKind::RoutingDecision { .. })
    }
}

/// `metrics.proto` `TraceSpan`. `component` is derived: `gateway`, `router`, or `replica:<id>`.
#[derive(Clone, PartialEq, Debug)]
pub struct TraceSpan {
    pub start_unix_ns: Nanos,
    pub end_unix_ns: Nanos,
    pub kind: SpanKind,
    /// The replica the span ran on; zero for the gateway and router spans, which is the proto's
    /// absent value.
    pub replica_id: u64,
    pub resource: ResourceState,
    pub kv_tier: MemoryTier,
}

impl TraceSpan {
    pub fn component(&self) -> String {
        match self.kind {
            SpanKind::IngressQueue => "gateway".to_string(),
            SpanKind::RoutingDecision { .. } => "router".to_string(),
            _ => format!("replica:{}", self.replica_id),
        }
    }

    pub fn duration_ns(&self) -> Nanos {
        self.end_unix_ns.saturating_sub(self.start_unix_ns)
    }
}

/// Which part of the latency distribution a request landed in. The strata of the sampler, and the
/// dashboard's way of asking for "a typical request" or "one from the far tail".
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum TraceBucket {
    /// At or below the median.
    #[default]
    P50,
    /// Between the median and p90.
    P90,
    /// Between p90 and p99.
    P99,
    /// Above p99: the tail worth reading.
    P999,
}

impl TraceBucket {
    pub const ALL: [TraceBucket; 4] = [TraceBucket::P50, TraceBucket::P90, TraceBucket::P99, TraceBucket::P999];

    /// The dashboard's labels (`web/src/lib/types.ts` `TraceBucket`).
    pub fn label(self) -> &'static str {
        match self {
            TraceBucket::P50 => "p50",
            TraceBucket::P90 => "p90",
            TraceBucket::P99 => "p99",
            TraceBucket::P999 => "p99.9",
        }
    }

    pub fn index(self) -> usize {
        self as usize
    }

    /// Place a latency against the p50, p90 and p99 thresholds of its population.
    pub fn of(latency_ns: Nanos, thresholds: [Nanos; 3]) -> TraceBucket {
        let [p50, p90, p99] = thresholds;
        if latency_ns <= p50 {
            TraceBucket::P50
        } else if latency_ns <= p90 {
            TraceBucket::P90
        } else if latency_ns <= p99 {
            TraceBucket::P99
        } else {
            TraceBucket::P999
        }
    }
}

/// `metrics.proto` `RequestTrace`: the record plus its spans. `tenant_id` and `bucket` sit beside the
/// engine's record because that record does not carry them yet; the encoder folds `tenant_id` into
/// the proto's `RequestRecord`.
#[derive(Clone, Debug)]
pub struct RequestTrace {
    pub record: RequestRecord,
    pub tenant_id: u64,
    pub bucket: TraceBucket,
    pub spans: Vec<TraceSpan>,
}

impl RequestTrace {
    /// Wall time from arrival to whatever ended the request, success or not. `RequestRecord::e2e`
    /// is `None` for a failure; the sampler still needs to place a timeout in a bucket.
    pub fn latency_ns(&self) -> Nanos {
        latency_of(&self.record)
    }
}

pub fn latency_of(r: &RequestRecord) -> Nanos {
    if r.finished_at > 0 {
        r.finished_at.saturating_sub(r.arrived_at)
    } else {
        0
    }
}

/// Number of engine outcomes, for the per-outcome quota table.
const OUTCOMES: usize = 5;

fn outcome_index(o: Outcome) -> usize {
    match o {
        Outcome::Ok => 0,
        Outcome::OkSloViolated => 1,
        Outcome::Rejected => 2,
        Outcome::TimeoutQueued => 3,
        Outcome::TimeoutRunning => 4,
    }
}

/// The stratified keep/drop decision, made once per completed request.
///
/// Uniform sampling of hundreds of millions of requests contains almost no examples of the tail,
/// which is the part worth reading. So the decision has two parts. A quota: for every `1 / rate`
/// requests seen, the first `per_bucket` in each latency bucket and the first `per_outcome` of each
/// outcome are kept regardless of the draw. Then a uniform draw at `rate` for everything else. The
/// quotas refill every `1 / rate` requests rather than once per run, because a once-per-run quota
/// is spent on the first few requests, when the running percentiles are least representative, and
/// the true tail arrives later. The expected number of traces stays proportional to `rate`, which is
/// what a byte budget needs.
///
/// The latency bucket comes from a running histogram of latencies seen so far, so the sampler needs
/// nothing but the record. The decision is a pure function of the seeded stream and the sequence of
/// records: the same records in the same order with the same seed keep the same set, and exactly one
/// draw is consumed per call whether or not the quota already decided.
pub struct TraceSampler {
    rng: Rng,
    rate: f64,
    per_bucket: u32,
    per_outcome: u32,
    /// Requests per quota window, `1 / rate`; the whole run when the rate is zero.
    window: u64,
    seen: u64,
    kept_by_bucket: [u32; 4],
    kept_by_outcome: [u32; OUTCOMES],
    latencies: Histogram,
}

impl TraceSampler {
    pub fn new(seed_stream: Rng, rate: f64, per_bucket: u32, per_outcome: u32) -> Self {
        let rate = if rate.is_finite() { rate.clamp(0.0, 1.0) } else { 0.0 };
        let window = if rate > 0.0 { (1.0 / rate).ceil() as u64 } else { u64::MAX };
        TraceSampler {
            rng: seed_stream,
            rate,
            per_bucket,
            per_outcome,
            window: window.max(1),
            seen: 0,
            kept_by_bucket: [0; 4],
            kept_by_outcome: [0; OUTCOMES],
            latencies: Histogram::new(),
        }
    }

    /// The bucket this record falls in against the latencies seen so far, not counting itself.
    pub fn bucket_of(&self, record: &RequestRecord) -> TraceBucket {
        if self.latencies.count() == 0 {
            return TraceBucket::P50;
        }
        TraceBucket::of(
            latency_of(record),
            [self.latencies.percentile(50.0), self.latencies.percentile(90.0), self.latencies.percentile(99.0)],
        )
    }

    /// Decide whether this request's spans are kept. Called once per completed request, in
    /// completion order, and returns the bucket the request was placed in so the caller can stamp
    /// the trace with it.
    pub fn keep(&mut self, record: &RequestRecord) -> Option<TraceBucket> {
        let bucket = self.bucket_of(record);
        let kept = self.keep_in(bucket, record.outcome);
        self.latencies.record(latency_of(record));
        if kept {
            Some(bucket)
        } else {
            None
        }
    }

    /// The decision with the bucket already known. One draw per call, always, so the stream's
    /// position depends only on how many requests have been seen.
    pub fn keep_in(&mut self, bucket: TraceBucket, outcome: Outcome) -> bool {
        if self.seen % self.window == 0 {
            self.kept_by_bucket = [0; 4];
            self.kept_by_outcome = [0; OUTCOMES];
        }
        self.seen += 1;
        let draw = self.rng.f64();

        let b = &mut self.kept_by_bucket[bucket.index()];
        let o = &mut self.kept_by_outcome[outcome_index(outcome)];
        let by_quota = *b < self.per_bucket || *o < self.per_outcome;
        let kept = by_quota || draw < self.rate;
        if kept {
            *b = b.saturating_add(1);
            *o = o.saturating_add(1);
        }
        kept
    }

    pub fn seen(&self) -> u64 {
        self.seen
    }
}

/// Deterministic traces shaped like the engine's, for building and testing everything downstream
/// before the engine records spans.
pub mod fixtures {
    use super::*;
    use sim_core::EPOCH_BASE;

    const MS: Nanos = 1_000_000;

    /// `n` traces, byte-identical on every call. Each request queues at the gateway, is routed over
    /// two candidates, queues at its replica, prefills in three chunks and decodes for forty steps.
    /// Roughly every seventh request ends badly (a late success, a shed, or a timeout mid-decode)
    /// so the failure paths downstream have something to filter for, and latencies are spread over
    /// two decades so every bucket is occupied. Buckets are assigned against the fixture set's own
    /// percentiles, as the engine will assign them against the run's.
    pub fn sample_traces(n: usize) -> Vec<RequestTrace> {
        let mut out: Vec<RequestTrace> = (0..n as u64).map(one).collect();
        let mut hist = Histogram::new();
        for t in &out {
            hist.record(t.latency_ns());
        }
        let thresholds = [hist.percentile(50.0), hist.percentile(90.0), hist.percentile(99.0)];
        for t in &mut out {
            t.bucket = TraceBucket::of(t.latency_ns(), thresholds);
        }
        out
    }

    fn one(i: u64) -> RequestTrace {
        let mut rng = Rng::from_seed(0x7ace_0000 + i);
        let id = 1 + i;
        let tenant_id = 1 + i % 3;
        let replica_id = 1 + i % 8;
        let other = 1 + (i + 3) % 8;
        let arrived = EPOCH_BASE + i * 250 * MS + rng.below(200) * MS;
        let prompt_tokens = 384 + rng.below(1024) as u32;
        let output_tokens = 40u32;
        // A heavy tail: most steps are quick, a few requests sit behind a long queue or a slow
        // batch, so the fixture's own p99.9 is a decade above its median.
        let load = rng.lognormal(1.0, 1.2);
        let outcome = match i % 7 {
            3 if i % 14 == 3 => Outcome::TimeoutRunning,
            3 => Outcome::OkSloViolated,
            6 if i % 21 == 6 => Outcome::Rejected,
            _ => Outcome::Ok,
        };

        let mut tl = Timeline { spans: Vec::new(), t: arrived };

        let gateway = ResourceState { running: 12 + (load * 20.0) as u32, queued: (load * 3.0) as u32, ..Default::default() };
        tl.push(SpanKind::IngressQueue, (2.0 * MS as f64 * load) as Nanos, 0, gateway, MemoryTier::None);
        let router = ResourceState { running: 1, ..Default::default() };
        tl.push(
            SpanKind::RoutingDecision { candidates: vec![replica_id, other], stale_view_age: 250 * MS },
            MS / 2,
            0,
            router,
            MemoryTier::None,
        );

        if outcome == Outcome::Rejected {
            let finished = tl.t;
            return RequestTrace {
                record: RequestRecord {
                    id,
                    arrived_at: arrived,
                    admitted_at: 0,
                    first_token_at: 0,
                    finished_at: finished,
                    prompt_tokens,
                    output_tokens: 0,
                    replica: 0,
                    attempts: 1,
                    outcome,
                    max_itl: 0,
                    mean_itl: 0,
                    class: 0,
                },
                tenant_id,
                bucket: TraceBucket::P50,
                spans: tl.spans,
            };
        }

        let kv_capacity = 262_144u64;
        let batch = 8 + (load * 8.0).min(24.0) as u32;
        let kv_resident = (kv_capacity as f64 * (0.45 + 0.05 * load).min(0.98)) as u64;
        let replica = |batch: u32, step_ns: Nanos, bound: BandwidthOrCompute| ResourceState {
            batch_size: batch,
            running: batch,
            queued: (load * 4.0) as u32,
            kv_tokens_resident: kv_resident,
            kv_capacity,
            step_ns,
            bound,
        };

        tl.push(SpanKind::ReplicaQueue, (30.0 * MS as f64 * load) as Nanos, replica_id, replica(batch, 0, BandwidthOrCompute::Compute), MemoryTier::None);
        let admitted = tl.t;

        let chunk = prompt_tokens.div_ceil(3);
        let mut left = prompt_tokens;
        for _ in 0..3 {
            let tokens = left.min(chunk);
            left -= tokens;
            let dur = 18 * MS + tokens as Nanos * 40_000;
            tl.push(SpanKind::PrefillChunk { tokens }, dur, replica_id, replica(batch, dur, BandwidthOrCompute::Compute), MemoryTier::Hbm);
        }
        let first_token = tl.t;

        let steps = if outcome == Outcome::TimeoutRunning { 25 } else { output_tokens };
        let mut max_itl = 0;
        let mut sum_itl = 0;
        for k in 0..steps {
            // One stall mid-stream, so the per-request worst gap is a real number.
            let stall = if k == 17 { (60.0 * MS as f64 * load) as Nanos } else { 0 };
            let dur = 28 * MS + rng.below(6) * MS + stall;
            let bound = if batch >= 16 { BandwidthOrCompute::Compute } else { BandwidthOrCompute::Bandwidth };
            tl.push(SpanKind::DecodeStep, dur, replica_id, replica(batch, dur, bound), MemoryTier::Hbm);
            max_itl = max_itl.max(dur);
            sum_itl += dur;
        }
        if outcome == Outcome::TimeoutRunning {
            tl.push(SpanKind::Preempted, 400 * MS, replica_id, replica(batch, 0, BandwidthOrCompute::Bandwidth), MemoryTier::Dram);
        }
        let finished = tl.t;
        let produced = if outcome == Outcome::TimeoutRunning { steps } else { output_tokens };

        RequestTrace {
            record: RequestRecord {
                id,
                arrived_at: arrived,
                admitted_at: admitted,
                first_token_at: first_token,
                finished_at: finished,
                prompt_tokens,
                output_tokens: produced,
                replica: replica_id as usize,
                attempts: 1,
                outcome,
                max_itl,
                mean_itl: if steps > 1 { sum_itl / (steps as Nanos - 1).max(1) } else { 0 },
                class: 0,
            },
            tenant_id,
            bucket: TraceBucket::P50,
            spans: tl.spans,
        }
    }

    /// Spans laid end to end: each new one starts where the last ended.
    struct Timeline {
        spans: Vec<TraceSpan>,
        t: Nanos,
    }

    impl Timeline {
        fn push(&mut self, kind: SpanKind, dur: Nanos, replica_id: u64, resource: ResourceState, kv_tier: MemoryTier) {
            self.spans.push(TraceSpan {
                start_unix_ns: self.t,
                end_unix_ns: self.t + dur,
                kind,
                replica_id,
                resource,
                kv_tier,
            });
            self.t += dur;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixture_traces_are_shaped_like_the_engine_will_produce() {
        let traces = fixtures::sample_traces(20);
        assert_eq!(traces.len(), 20);
        let ok = traces.iter().find(|t| t.record.outcome == Outcome::Ok).unwrap();
        let ops: Vec<&str> = ok.spans.iter().map(|s| s.kind.operation()).collect();
        assert_eq!(&ops[..4], &["queue", "route", "queue", "prefill"]);
        assert_eq!(ops.iter().filter(|o| **o == "prefill").count(), 3);
        assert_eq!(ops.iter().filter(|o| **o == "decode").count(), 40);
        assert_eq!(ok.spans[0].component(), "gateway");
        assert_eq!(ok.spans[1].component(), "router");
        assert!(ok.spans[3].component().starts_with("replica:"));
        for w in ok.spans.windows(2) {
            assert_eq!(w[0].end_unix_ns, w[1].start_unix_ns, "spans are contiguous");
        }
        assert_eq!(ok.spans.last().unwrap().end_unix_ns, ok.record.finished_at);
        assert_eq!(format!("{:?}", fixtures::sample_traces(20)), format!("{traces:?}"), "fixtures are deterministic");
    }
}
