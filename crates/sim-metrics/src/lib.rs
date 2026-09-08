//! Histograms, series and the per-request record.
//!
//! At target scale a simulated hour completes hundreds of millions of requests, so distributions are
//! aggregated online into mergeable histograms and full records exist only for a sample. Today's
//! runs are small enough to keep every record, but the shapes are the ones that scale.

pub mod trace;

use sim_core::Nanos;

/// Log-linear bucketed histogram, HDR style.
///
/// Mergeable by bucket-wise addition, which is the property that matters: percentiles are *not*
/// mergeable, so averaging per-shard p99s would produce a number that is not a percentile of
/// anything. Two significant digits gives about 1% error in the tail, which is far below the
/// uncertainty in the cost model.
#[derive(Clone)]
pub struct Histogram {
    buckets: Vec<u64>,
    count: u64,
    sum: u128,
    min: u64,
    max: u64,
}

const SUB_BITS: u32 = 6; // 64 sub-buckets per power of two, so ~1.6% resolution
const SUB: usize = 1 << SUB_BITS;
const BUCKETS: usize = SUB * 48; // up to 2^48 ns, about three days

impl Default for Histogram {
    fn default() -> Self {
        Self::new()
    }
}

impl Histogram {
    pub fn new() -> Self {
        Histogram {
            buckets: vec![0; BUCKETS],
            count: 0,
            sum: 0,
            min: u64::MAX,
            max: 0,
        }
    }

    #[inline]
    fn index(v: u64) -> usize {
        if v < SUB as u64 {
            return v as usize;
        }
        let msb = 63 - v.leading_zeros();
        let shift = msb - SUB_BITS;
        let sub = (v >> shift) as usize & (SUB - 1);
        let idx = ((shift as usize + 1) << SUB_BITS) + sub;
        idx.min(BUCKETS - 1)
    }

    #[inline]
    fn value_of(idx: usize) -> u64 {
        if idx < SUB {
            return idx as u64;
        }
        let shift = (idx >> SUB_BITS) as u32 - 1;
        let sub = (idx & (SUB - 1)) as u64;
        (sub | SUB as u64) << shift
    }

    pub fn record(&mut self, v: u64) {
        self.buckets[Self::index(v)] += 1;
        self.count += 1;
        self.sum += v as u128;
        self.min = self.min.min(v);
        self.max = self.max.max(v);
    }

    pub fn merge(&mut self, other: &Histogram) {
        for (a, b) in self.buckets.iter_mut().zip(other.buckets.iter()) {
            *a += *b;
        }
        self.count += other.count;
        self.sum += other.sum;
        if other.count > 0 {
            self.min = self.min.min(other.min);
            self.max = self.max.max(other.max);
        }
    }

    pub fn count(&self) -> u64 {
        self.count
    }

    pub fn mean(&self) -> f64 {
        if self.count == 0 {
            return f64::NAN;
        }
        self.sum as f64 / self.count as f64
    }

    pub fn min(&self) -> u64 {
        if self.count == 0 {
            0
        } else {
            self.min
        }
    }

    pub fn max(&self) -> u64 {
        self.max
    }

    /// `q` in [0, 100].
    pub fn percentile(&self, q: f64) -> u64 {
        if self.count == 0 {
            return 0;
        }
        let target = ((q / 100.0) * self.count as f64).ceil().max(1.0) as u64;
        let mut seen = 0u64;
        for (i, c) in self.buckets.iter().enumerate() {
            seen += *c;
            if seen >= target {
                return Self::value_of(i);
            }
        }
        self.max
    }

    /// Fraction of samples at or below `v`. Used for SLO attainment.
    pub fn fraction_below(&self, v: u64) -> f64 {
        if self.count == 0 {
            return f64::NAN;
        }
        let limit = Self::index(v);
        let below: u64 = self.buckets[..=limit].iter().sum();
        below as f64 / self.count as f64
    }
}

/// A histogram with only its occupied buckets, for keeping many of them.
///
/// The dense form is 3,072 buckets, 24 KB, right for a run-wide aggregate and wrong for one per
/// sample: a 120 s run at four samples a second holds four per frame, close to 50 MB of mostly
/// zeros. This keeps `(bucket, count)` pairs in bucket order and answers the same questions the
/// dense form does, to the bit, because it walks the same buckets in the same order.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct SparseHistogram {
    pub pairs: Vec<(u16, u64)>,
    pub count: u64,
    pub sum: u128,
    pub min: u64,
    pub max: u64,
}

impl SparseHistogram {
    pub fn count(&self) -> u64 {
        self.count
    }

    pub fn mean(&self) -> f64 {
        if self.count == 0 {
            return f64::NAN;
        }
        self.sum as f64 / self.count as f64
    }

    pub fn min(&self) -> u64 {
        if self.count == 0 {
            0
        } else {
            self.min
        }
    }

    pub fn max(&self) -> u64 {
        self.max
    }

    /// `q` in [0, 100]. The dense algorithm over the occupied buckets only; zeros never move the
    /// running total, so skipping them cannot change where it crosses the target.
    pub fn percentile(&self, q: f64) -> u64 {
        if self.count == 0 {
            return 0;
        }
        let target = ((q / 100.0) * self.count as f64).ceil().max(1.0) as u64;
        let mut seen = 0u64;
        for (i, c) in self.pairs.iter() {
            seen += *c;
            if seen >= target {
                return Histogram::value_of(*i as usize);
            }
        }
        self.max
    }

    /// Fraction of samples at or below `v`.
    pub fn fraction_below(&self, v: u64) -> f64 {
        if self.count == 0 {
            return f64::NAN;
        }
        let limit = Histogram::index(v);
        let below: u64 = self.pairs.iter().filter(|(i, _)| *i as usize <= limit).map(|(_, c)| *c).sum();
        below as f64 / self.count as f64
    }

    /// Bucket-wise addition, the dense form's `merge` on the sparse form: the result answers every
    /// question exactly as the dense merge of the two would, because the pairs stay in bucket order.
    /// This is what a smoothing window over frames needs, and why p99 over a window is the p99 of
    /// every request in it rather than an average of per-frame p99s.
    pub fn merge(&mut self, other: &SparseHistogram) {
        if other.count == 0 {
            return;
        }
        let mut merged = Vec::with_capacity(self.pairs.len() + other.pairs.len());
        let (mut a, mut b) = (self.pairs.iter().peekable(), other.pairs.iter().peekable());
        loop {
            match (a.peek(), b.peek()) {
                (Some(&&(i, x)), Some(&&(j, y))) if i == j => {
                    merged.push((i, x + y));
                    a.next();
                    b.next();
                }
                (Some(&&(i, x)), Some(&&(j, _))) if i < j => {
                    merged.push((i, x));
                    a.next();
                }
                (Some(_), Some(&&(j, y))) => {
                    merged.push((j, y));
                    b.next();
                }
                (Some(&&pair), None) => {
                    merged.push(pair);
                    a.next();
                }
                (None, Some(&&pair)) => {
                    merged.push(pair);
                    b.next();
                }
                (None, None) => break,
            }
        }
        self.min = if self.count == 0 { other.min } else { self.min.min(other.min) };
        self.max = self.max.max(other.max);
        self.pairs = merged;
        self.count += other.count;
        self.sum += other.sum;
    }
}

impl Histogram {
    pub fn to_sparse(&self) -> SparseHistogram {
        SparseHistogram {
            pairs: self
                .buckets
                .iter()
                .enumerate()
                .filter(|(_, c)| **c > 0)
                .map(|(i, c)| (i as u16, *c))
                .collect(),
            count: self.count,
            sum: self.sum,
            min: self.min,
            max: self.max,
        }
    }
}

/// One replica as it stood at a sample instant. The per-replica part of a frame, and what the
/// dashboard's fleet heat map is drawn from.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct ReplicaSample {
    pub queued: u32,
    pub running: u32,
    pub kv_tokens: u64,
    pub last_step_ns: Nanos,
    /// Nanoseconds of the window `(previous sample, t]` the replica spent inside a step, as opposed
    /// to idle with nothing to run. A step that straddles a sample instant is split at it, so this is
    /// never more than the window. The consumer divides by the window length for
    /// `METRIC_GPU_UTILIZATION`; no ratio is stored because the window length is the scenario's.
    pub busy_ns: Nanos,
    /// Of `busy_ns`, the part the cost model priced at the compute roofline: prefill and speculative
    /// verification. The rest was the weight read and key-value re-read, bandwidth-bound decode.
    /// `METRIC_GPU_COMPUTE_BOUND_FRACTION` is this over `busy_ns`.
    pub compute_ns: Nanos,
    /// Prompt tokens admitted in the window, and of those the tokens found already resident, in a
    /// prefix cache or as a session's parked context, so their prefill was skipped. The prefix hit
    /// rate is the ratio, and it is what affinity routing is trying to raise.
    pub prompt_tokens: u64,
    pub prefix_hit_tokens: u64,
    /// `METRIC_REPLICA_STATE`'s number: 1 READY, 2 DEGRADED, 3 EJECTED, 4 WARMING (turned up, inside
    /// its cold start), 5 DRAINING (turned down, finishing what it holds), 0 ABSENT (a slot the
    /// autoscaler has not filled; kept so replica ids are stable). A sample built by the engine
    /// always carries `Replica::state()`; a fleet with no autoscaler never shows 4, 5 or 0.
    pub state: u8,
    /// The engine's modelled speed fraction, 1.0 healthy. `Default`'s 0.0 is not a real reading, for
    /// the same reason as `state`: a sample the engine produced always carries `Replica::speed()`.
    pub speed: f64,
    /// Nanoseconds to first token summed over the window `(previous sample, t]`, paired with
    /// `ttft_count` for `METRIC_TTFT`'s windowed mean at replica scope. Zero when no sequence in the
    /// replica got its first token this window.
    pub ttft_sum_ns: u64,
    pub ttft_count: u64,
    /// Contexts this replica evicted in the window, running or parked. The engine's counter is
    /// cumulative; the sampler takes the delta, like `busy_ns`.
    pub preemptions: u64,
}

/// Everything observable about one sample interval, closed at `t`.
///
/// A run-wide histogram answers "how did the run go"; a frame answers "what is happening now",
/// which is the question a live dashboard and the Leaf-to-Ingress metrics flow of
/// `docs/ARCHITECTURE.md` 10.3 both ask. Counters and histograms cover the window `(previous
/// sample, t]`, keyed by when a request *finished*, so a spike shows up in the frame where it hurt.
/// The gauges that the fleet-wide `Series` already carry, queue depth and the like, are not repeated
/// here; the per-replica breakdown is.
#[derive(Clone, Debug, PartialEq)]
pub struct Frame {
    pub t: Nanos,
    pub offered_rps: f64,
    /// Requests that entered a replica queue in the window.
    pub admitted: u64,
    /// Requests that finished in the window, by how: successes of either kind, sheds, timeouts of
    /// either kind, and the successes that met every SLO.
    pub completed: u64,
    pub rejected: u64,
    pub timed_out: u64,
    pub within_slo: u64,
    /// Tokens delivered by the completions in the window, all of them and those within SLO.
    pub output_tokens: u64,
    pub goodput_tokens: u64,
    /// Latency distributions of the completions in the window, recorded by the same rules as the
    /// run-wide ones: a time to first token only when there was one, a worst gap only when there was
    /// more than one token.
    pub ttft: SparseHistogram,
    pub itl_max: SparseHistogram,
    pub e2e: SparseHistogram,
    pub queue_wait: SparseHistogram,
    /// Contexts evicted from a cache in the window, running or parked; the dashboard's preemption
    /// rate.
    pub preemptions: u64,
    /// Retry attempts scheduled in the window: the client-side amplification a slowdown produces,
    /// `METRIC_RETRIES_PER_S` at fleet scope. `RunResult::retries` is the same count run-wide.
    pub retries: u64,
    /// Context held in the cluster memory tiers at `t`, in tokens: `METRIC_TIER_UTILIZATION` over
    /// the pool sizes. Fleet-scope because the pools are (architecture section 7.2).
    pub tier_dram_used: u64,
    pub tier_ssd_used: u64,
    /// Transfer time debited to each tier's path in the window, charged when a migration is
    /// scheduled: `METRIC_TIER_BANDWIDTH_UTILIZATION` over the window, their sum being the shared
    /// fabric's busy time.
    pub tier_dram_busy_ns: Nanos,
    pub tier_ssd_busy_ns: Nanos,
    pub replicas: Vec<ReplicaSample>,
}

/// A sampled gauge. Values are step functions, so the meaningful average is time-weighted: the
/// arithmetic mean of samples is wrong whenever the value is bursty, which is the normal case.
#[derive(Clone, Default)]
pub struct Series {
    pub name: String,
    pub t: Vec<Nanos>,
    pub v: Vec<f64>,
}

impl Series {
    pub fn new(name: &str) -> Self {
        Series {
            name: name.to_string(),
            t: Vec::new(),
            v: Vec::new(),
        }
    }

    pub fn push(&mut self, t: Nanos, v: f64) {
        self.t.push(t);
        self.v.push(v);
    }

    pub fn mean(&self) -> f64 {
        if self.v.is_empty() {
            return f64::NAN;
        }
        self.v.iter().sum::<f64>() / self.v.len() as f64
    }

    pub fn max(&self) -> f64 {
        // NaN on empty, matching `mean`. Folding from `f64::MIN` returned -1.8e308, which a report
        // would happily print as a peak queue depth. Reachable for a run shorter than one sample
        // interval, and a wrong number that looks like a number is worse than an obvious absence.
        if self.v.is_empty() {
            return f64::NAN;
        }
        self.v.iter().cloned().fold(f64::NEG_INFINITY, f64::max)
    }

    /// Coefficient of variation. For per-replica load this is the direct measure of whether the load
    /// balancer is doing its job.
    pub fn cv(&self) -> f64 {
        let n = self.v.len();
        if n < 2 {
            return f64::NAN;
        }
        let m = self.mean();
        if m == 0.0 {
            return 0.0;
        }
        let var = self.v.iter().map(|x| (x - m) * (x - m)).sum::<f64>() / (n - 1) as f64;
        var.sqrt() / m
    }

    /// Dominant oscillation frequency and its relative amplitude, by scanning a coarse
    /// discrete Fourier transform over the mean-removed signal.
    ///
    /// This is how telemetry-induced instability becomes a number instead of an anecdote: a peak at
    /// a frequency related to the telemetry period is the signature of the control loop ringing.
    pub fn dominant_frequency(&self, sample_interval_s: f64) -> (f64, f64) {
        let n = self.v.len();
        if n < 16 || sample_interval_s <= 0.0 {
            return (0.0, 0.0);
        }
        let mean = self.mean();
        let centred: Vec<f64> = self.v.iter().map(|x| x - mean).collect();
        let energy: f64 = centred.iter().map(|x| x * x).sum::<f64>().max(1e-12);
        let mut best = (0.0f64, 0.0f64);
        // Skip k=0 (the mean) and stop at Nyquist.
        for k in 1..(n / 2) {
            let w = std::f64::consts::TAU * k as f64 / n as f64;
            let (mut re, mut im) = (0.0f64, 0.0f64);
            for (i, x) in centred.iter().enumerate() {
                let a = w * i as f64;
                re += x * a.cos();
                im -= x * a.sin();
            }
            let power = (re * re + im * im) / (n as f64 / 2.0).max(1.0);
            if power > best.1 {
                best = (k as f64 / (n as f64 * sample_interval_s), power);
            }
        }
        // Report amplitude relative to the signal's own energy, so it is comparable across runs.
        let rel = (best.1 / energy).sqrt();
        (best.0, rel)
    }
}

/// How a request ended. The distinction that earns its place is *when* it failed, because shedding
/// early is cheap and failing late has already burned device time.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Outcome {
    Ok,
    OkSloViolated,
    Rejected,
    TimeoutQueued,
    TimeoutRunning,
}

impl Outcome {
    pub fn label(&self) -> &'static str {
        match self {
            Outcome::Ok => "ok",
            Outcome::OkSloViolated => "ok_slo_violated",
            Outcome::Rejected => "rejected",
            Outcome::TimeoutQueued => "timeout_queued",
            Outcome::TimeoutRunning => "timeout_running",
        }
    }
    pub fn is_success(&self) -> bool {
        matches!(self, Outcome::Ok | Outcome::OkSloViolated)
    }
}

#[derive(Clone, Debug)]
pub struct RequestRecord {
    pub id: u64,
    pub arrived_at: Nanos,
    pub admitted_at: Nanos,
    pub first_token_at: Nanos,
    pub finished_at: Nanos,
    pub prompt_tokens: u32,
    pub output_tokens: u32,
    pub replica: usize,
    pub attempts: u32,
    pub outcome: Outcome,
    /// The worst gap between consecutive tokens. A single long stall is what a user perceives, and a
    /// mean hides it entirely.
    pub max_itl: Nanos,
    pub mean_itl: Nanos,
    /// SLO class the outcome was judged against; zero when the scenario has no classes.
    pub class: u8,
}

impl RequestRecord {
    pub fn ttft(&self) -> Option<Nanos> {
        if self.first_token_at > 0 && self.first_token_at >= self.arrived_at {
            Some(self.first_token_at - self.arrived_at)
        } else {
            None
        }
    }
    pub fn e2e(&self) -> Option<Nanos> {
        if self.finished_at > 0 && self.outcome.is_success() {
            Some(self.finished_at - self.arrived_at)
        } else {
            None
        }
    }
    pub fn queue_wait(&self) -> Nanos {
        self.admitted_at.saturating_sub(self.arrived_at)
    }
}
