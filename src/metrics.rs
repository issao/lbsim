//! Histograms, series and the per-request record.
//!
//! At target scale a simulated hour completes hundreds of millions of requests, so distributions are
//! aggregated online into mergeable histograms and full records exist only for a sample. Today's
//! runs are small enough to keep every record, but the shapes are the ones that scale.

use crate::Nanos;

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
        self.v.iter().cloned().fold(f64::MIN, f64::max)
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
