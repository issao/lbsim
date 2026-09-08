//! The simulation loop.
//!
//! Arrivals, routing, admission at the replica, timeouts and retries, telemetry delay and sampling:
//! everything that happens *between* replicas. What happens *inside* a replica during one step is
//! `sim_model::Replica`'s, and this loop only decides when to call it and what to do with what it
//! retired. The split follows the Leaf seam in `docs/ARCHITECTURE.md` section 10.3, so the loop can
//! later be cut along it without touching the physics.

use sim_leaf_api::{AdvanceRequest, AdvanceResponse, ConfigureShardResponse, Leaf};
use sim_metrics::trace::{BandwidthOrCompute, MemoryTier, RequestTrace, ResourceState, SpanKind, TraceSampler, TraceSpan};
use sim_metrics::{Frame, Histogram, Outcome, ReplicaSample, RequestRecord, Series};
use sim_policy::{
    Admission, AdmissionContext, AdmissionPolicy, AutoscalingPolicy, FleetView, HealthPolicy, PrefixIndex,
    ReplicaView, RequestView, RouteContext, RoutingPolicy, SchedulingPolicy,
};
use sim_core::queue::{EventQueue, PRIO_OBSERVE};
use sim_core::rng::{Rng, Streams};
use sim_model::trace::{ResourceSnapshot, StepEvent};
use sim_model::{Lifecycle, PrefixTree, Replica, Tiers};
use sim_scenario::{FailureEvent, FailureKind, OverrideKind, Scenario};
use sim_workload::{Request, Workload};
use sim_core::{Nanos, EPOCH_BASE, MILLI};
use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap, VecDeque};

/// A modelled router-to-replica round trip, paid when a policy probes for fresh state instead of
/// reading the delayed snapshot.
const PROBE_COST: Nanos = MILLI;

/// The sampler's quotas per window of `1 / trace_sample_rate` completions: the first this many of
/// every latency bucket and every outcome are retained whatever the draw says, so the tail and the
/// failures are present in the retained set rather than merely probable.
const TRACE_PER_BUCKET: u32 = 2;
const TRACE_PER_OUTCOME: u32 = 2;

enum Ev {
    Arrival,
    Step(usize),
    TelemetryPublish(usize),
    TelemetryDeliver(usize, ReplicaView),
    Timeout(u64),
    Sample,
    Admit(usize, Request),
    /// A session's next turn arriving at the gateway, bound for the replica that parked its context.
    SessionTurn(usize, Request),
    /// A scheduled failure begins or ends; the index is into `Sim::failures`.
    Fail(usize),
    Recover(usize),
    /// The autoscaler's decision tick. Never scheduled with `autoscaling = none`.
    Autoscale,
    /// A turned-up slot's cold start ends; a turned-down slot's drain runs out. Each carries the
    /// slot's lifecycle generation, so a slot that moved on since the event was scheduled ignores it.
    WarmupDone(usize, u64),
    DrainTimeout(usize, u64),
}

pub struct RunResult {
    pub scenario: Scenario,
    pub routing_label: String,
    pub records: Vec<RequestRecord>,
    /// Span-by-span journeys of the requests the trace sample kept, measured window only.
    pub traces: Vec<RequestTrace>,
    pub ttft: Histogram,
    pub itl_max: Histogram,
    pub e2e: Histogram,
    pub queue_wait: Histogram,
    /// Per-replica queued+running over time. The rolling hotspot is visible directly in this.
    pub replica_load: Vec<Series>,
    pub fleet_queue: Series,
    pub fleet_running: Series,
    pub fleet_kv_utilization: Series,
    pub offered_rps: Series,
    /// One per sample: the counts, distributions and per-replica state of that interval. The series
    /// above are the run-wide view; these are the live one.
    pub frames: Vec<Frame>,
    pub outcomes: HashMap<&'static str, u64>,
    pub events: u64,
    pub fingerprint: u64,
    pub measured_from: Nanos,
    pub measured_to: Nanos,
    pub rated_rps: f64,
    pub replicas_inspected_per_decision: usize,
    pub retries: u64,
    pub first_attempts: u64,
}

impl RunResult {
    fn measured_s(&self) -> f64 {
        (self.measured_to.saturating_sub(self.measured_from)) as f64 / 1e9
    }
    pub fn completed(&self) -> u64 {
        self.records.iter().filter(|r| r.outcome.is_success()).count() as u64
    }
    pub fn throughput_tokens_s(&self) -> f64 {
        let toks: u64 = self
            .records
            .iter()
            .filter(|r| r.outcome.is_success())
            .map(|r| r.output_tokens as u64)
            .sum();
        toks as f64 / self.measured_s()
    }
    /// Tokens per second delivered *within SLO*. The headline number, because a fleet can have
    /// excellent throughput and near-zero goodput by making everyone slightly too slow.
    pub fn goodput_tokens_s(&self) -> f64 {
        let toks: u64 = self
            .records
            .iter()
            .filter(|r| r.outcome == Outcome::Ok)
            .map(|r| r.output_tokens as u64)
            .sum();
        toks as f64 / self.measured_s()
    }
    /// Fraction of **all** measured requests that received acceptable service.
    ///
    /// The denominator is every request, not every *successful* request. Dividing by successes was a
    /// real defect, caught by the arena harness: a policy that shed nine requests in ten and served
    /// the tenth well would have reported perfect attainment. That is exactly the trade the SLO gate
    /// exists to forbid, and the metric was blind to it.
    ///
    /// A shed request is a request that did not get service. Whether shedding it early was the right
    /// call is a separate question, answered by comparing goodput, and `served_attainment` below keeps
    /// the old view for when the question really is "of what we served, how much was good".
    pub fn slo_attainment(&self) -> f64 {
        if self.records.is_empty() {
            return f64::NAN;
        }
        let ok = self.records.iter().filter(|r| r.outcome == Outcome::Ok).count();
        ok as f64 / self.records.len() as f64
    }

    /// Of the requests that completed, the fraction within SLO. Diagnostic rather than a score:
    /// it cannot distinguish good service from aggressive shedding, which is why it is not the
    /// headline.
    pub fn served_attainment(&self) -> f64 {
        let n = self.records.iter().filter(|r| r.outcome.is_success()).count();
        if n == 0 {
            return f64::NAN;
        }
        let ok = self.records.iter().filter(|r| r.outcome == Outcome::Ok).count();
        ok as f64 / n as f64
    }
    /// The SLO classes present in the measured records, ascending. Empty-string scenarios give `[0]`.
    pub fn classes(&self) -> Vec<u8> {
        let mut cs: Vec<u8> = self.records.iter().map(|r| r.class).collect();
        cs.sort_unstable();
        cs.dedup();
        cs
    }
    /// `goodput_tokens_s` restricted to one class.
    pub fn class_goodput_tokens_s(&self, class: u8) -> f64 {
        let toks: u64 = self
            .records
            .iter()
            .filter(|r| r.class == class && r.outcome == Outcome::Ok)
            .map(|r| r.output_tokens as u64)
            .sum();
        toks as f64 / self.measured_s()
    }
    /// `slo_attainment` restricted to one class: Ok over every measured request of that class.
    pub fn class_attainment(&self, class: u8) -> f64 {
        let n = self.records.iter().filter(|r| r.class == class).count();
        if n == 0 {
            return f64::NAN;
        }
        let ok = self.records.iter().filter(|r| r.class == class && r.outcome == Outcome::Ok).count();
        ok as f64 / n as f64
    }
    pub fn completed_rps(&self) -> f64 {
        self.completed() as f64 / self.measured_s()
    }
    /// Time-averaged coefficient of variation of per-replica load: the direct measure of whether the
    /// load balancer is doing its job.
    pub fn load_imbalance_cv(&self) -> f64 {
        if self.replica_load.is_empty() {
            return f64::NAN;
        }
        let samples = self.replica_load[0].v.len();
        if samples == 0 {
            return f64::NAN;
        }
        let mut acc = 0.0;
        let mut n = 0usize;
        for s in 0..samples {
            // A slot that is absent or warming holds no load by construction, and counting its zero
            // would report an imbalance the router never made. Nothing is filtered in a fixed fleet.
            let frame = self.frames.get(s);
            let vals: Vec<f64> = self
                .replica_load
                .iter()
                .enumerate()
                .filter(|(i, _)| frame.map_or(true, |f| f.replicas.get(*i).map_or(true, |r| !matches!(r.state, 0 | 4))))
                .map(|(_, r)| r.v[s])
                .collect();
            if vals.is_empty() {
                continue;
            }
            let mean = vals.iter().sum::<f64>() / vals.len() as f64;
            if mean <= 0.0 {
                continue;
            }
            let var = vals.iter().map(|x| (x - mean) * (x - mean)).sum::<f64>()
                / (vals.len() - 1).max(1) as f64;
            acc += var.sqrt() / mean;
            n += 1;
        }
        if n == 0 {
            f64::NAN
        } else {
            acc / n as f64
        }
    }
    /// Did the fleet come back after the spike ended?
    ///
    /// Returns (mean queue before the spike, mean queue in the final quarter, recovered). This is the
    /// question that separates a bad few minutes from an outage: a metastable collapse is one the
    /// fleet stays in after offered load returns to normal, so comparing during-spike numbers proves
    /// nothing. Only the tail of the run does.
    pub fn recovery(&self) -> Option<(f64, f64, bool)> {
        let sc = &self.scenario;
        if sc.load_step_at_s < 0.0 || sc.load_step_until_s < 0.0 {
            return None;
        }
        let start = self.measured_from.saturating_sub((sc.warmup_s * 1e9) as Nanos);
        let at = |secs: f64| start + (secs * 1e9) as Nanos;
        let pre_from = at(sc.warmup_s);
        let pre_to = at(sc.load_step_at_s);
        // The final quarter, and at least 30 s after the spike ended, so recovery has had a chance.
        let post_from = at((sc.duration_s * 0.75).max(sc.load_step_until_s + 30.0));
        let window = |lo: Nanos, hi: Nanos| -> f64 {
            let vals: Vec<f64> = self
                .fleet_queue
                .t
                .iter()
                .zip(self.fleet_queue.v.iter())
                .filter(|(t, _)| **t >= lo && **t < hi)
                .map(|(_, v)| *v)
                .collect();
            if vals.is_empty() {
                f64::NAN
            } else {
                vals.iter().sum::<f64>() / vals.len() as f64
            }
        };
        let pre = window(pre_from, pre_to);
        let post = window(post_from, self.measured_to);
        if pre.is_nan() || post.is_nan() {
            return None;
        }
        // Recovered if the queue came back to within 50% of where it was. A loose threshold on
        // purpose: the distinction being drawn is "returned to normal" against "stayed collapsed",
        // not a precise steady state.
        Some((pre, post, post <= pre * 1.5 + 1.0))
    }

    pub fn outcome(&self, label: &str) -> u64 {
        *self.outcomes.get(label).unwrap_or(&0)
    }
}

/// Ceilings that turn a runaway into an error instead of an out-of-memory kill.
///
/// These exist because a real bug did exactly that. The retry path scheduled a fresh arrival
/// alongside re-dispatching a retry, and since every arrival schedules its own successor, each retry
/// permanently *forked* the arrival chain. Growth was exponential in the number of retries, and the
/// process consumed all memory and swap on the machine it was running on before anyone could read an
/// error message.
///
/// The fix for that bug is in, and the retry path is tested now, so the fixed *total-event* ceiling
/// that used to guard against it is gone (it tripped on legitimate large runs well before anything
/// was actually wrong -- a 10,000-replica, 25,000 rps, 120 s run used only 56% of it). What is left is
/// a direct memory guard (below) plus these three state ceilings, each now scaled to the run instead
/// of held fixed, with the old fixed values kept as floors so a small scenario is exactly as protected
/// as it always was.
const MAX_IN_FLIGHT_FLOOR: usize = 2_000_000;
const MAX_RECORDS_FLOOR: usize = 20_000_000;
const MAX_QUEUE_LEN_FLOOR: usize = 5_000_000;

/// The peak offered rate a scenario implies, once a configured load step is in effect. Shared by
/// `validate`'s and `max_records`'s record-count estimate.
fn peak_rps(sc: &Scenario) -> f64 {
    sc.arrival_rps * sc.load_step_factor.max(1.0)
}

/// How many requests a run can plausibly have in flight or queued at once, scaled with replica count
/// and duration so a legitimately large, long fleet does not trip a ceiling meant for a runaway.
fn max_in_flight(sc: &Scenario) -> usize {
    (MAX_IN_FLIGHT_FLOOR as f64).max(sc.replicas as f64 * sc.duration_s * 20.0) as usize
}
fn max_queue_len(sc: &Scenario) -> usize {
    (MAX_QUEUE_LEN_FLOOR as f64).max(sc.replicas as f64 * sc.duration_s * 20.0) as usize
}

/// How many `RequestRecord`s a run can plausibly produce, scaled with offered load, duration and the
/// retry budget. Shared by `validate` (rejecting a scenario that implies more than this before any
/// work starts) and `Sim::new` (the ceiling actually enforced while running), so the two stay
/// consistent.
fn max_records(sc: &Scenario) -> usize {
    (MAX_RECORDS_FLOOR as f64).max(peak_rps(sc) * sc.duration_s * sc.max_attempts as f64 * 2.0) as usize
}

/// The memory budget in MiB: `LBSIM_MEMORY_BUDGET_MB`, read once at run start. Absent or malformed
/// falls back to 20000 (this project's 24 GB machine ceiling, minus headroom); the Dockerfile sets
/// this to ~80% of the container's memory for a cloud backend.
pub fn memory_budget_mb() -> u64 {
    std::env::var("LBSIM_MEMORY_BUDGET_MB")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(20_000)
}

/// Guard resident memory directly against a budget: the ceilings above bound the state this
/// simulator tracks, not memory a future bug of a different shape allocates outside it. Checked only
/// every `interval` dispatched events (1,000,000 in a real run; a test shrinks this so a tiny budget
/// trips inside a short run), so the cost of the guard itself stays negligible.
#[cfg(target_os = "linux")]
fn rss_over_budget(dispatched: u64, interval: u64, budget_bytes: u64) -> Option<String> {
    if interval == 0 || dispatched == 0 || dispatched % interval != 0 {
        return None;
    }
    let statm = std::fs::read_to_string("/proc/self/statm").ok()?;
    let pages: u64 = statm.split_whitespace().nth(1)?.parse().ok()?;
    let bytes = pages * 4096;
    (bytes > budget_bytes).then(|| {
        format!(
            "resident memory {} MB exceeds the LBSIM_MEMORY_BUDGET_MB budget of {} MB; lower \
             replicas, duration or arrival, or raise the budget",
            bytes / (1024 * 1024),
            budget_bytes / (1024 * 1024)
        )
    })
}
#[cfg(not(target_os = "linux"))]
fn rss_over_budget(_dispatched: u64, _interval: u64, _budget_bytes: u64) -> Option<String> {
    None
}

/// Reject a scenario that cannot be simulated inside the resource ceilings above.
///
/// Checked before any work starts, so an implausible parameter is a message rather than an hour of
/// swapping. The bounds are deliberately generous: the point is to catch a slipped decimal place or a
/// unit confusion, not to second-guess a legitimate experiment.
fn validate(sc: &Scenario) -> Result<(), String> {
    let mut bad: Vec<String> = Vec::new();
    if !(sc.arrival_rps.is_finite() && sc.arrival_rps > 0.0 && sc.arrival_rps <= 5.0e6) {
        bad.push(format!("arrival_rps = {} (need 0 < r <= 5e6)", sc.arrival_rps));
    }
    if !(sc.duration_s.is_finite() && sc.duration_s > 0.0 && sc.duration_s <= 86_400.0) {
        bad.push(format!("duration_s = {} (need 0 < d <= 86400)", sc.duration_s));
    }
    if sc.warmup_s < 0.0 || sc.warmup_s >= sc.duration_s {
        bad.push(format!(
            "warmup_s = {} must be non-negative and less than duration_s = {}",
            sc.warmup_s, sc.duration_s
        ));
    }
    if sc.replicas == 0 || sc.replicas > 200_000 {
        bad.push(format!("replicas = {} (need 1..=200000)", sc.replicas));
    }
    if sc.fleet_max() < sc.replicas || sc.fleet_max() > 200_000 {
        bad.push(format!("max_replicas = {} (need replicas = {} ..= 200000)", sc.fleet_max(), sc.replicas));
    }
    if sc.fleet_min() > sc.fleet_max() {
        bad.push(format!("min_replicas = {} exceeds max_replicas = {}", sc.fleet_min(), sc.fleet_max()));
    }
    if !(sc.autoscale_interval_s.is_finite() && sc.autoscale_interval_s > 0.0) {
        bad.push(format!("autoscale_interval_s = {} (need > 0)", sc.autoscale_interval_s));
    }
    if !(sc.autoscale_target.is_finite() && sc.autoscale_target > 0.0 && sc.autoscale_target <= 1.0) {
        bad.push(format!("autoscale_target = {} (need 0 < t <= 1)", sc.autoscale_target));
    }
    if sc.autoscale_step == 0 {
        bad.push("autoscale_step = 0 (need >= 1)".into());
    }
    for (k, v) in [
        ("autoscale_cooldown_s", sc.autoscale_cooldown_s),
        ("warmup_delay_s", sc.warmup_delay_s),
        ("drain_timeout_s", sc.drain_timeout_s),
    ] {
        if !(v.is_finite() && v >= 0.0) {
            bad.push(format!("{k} = {v} (need >= 0)"));
        }
    }
    if sc.max_batch == 0 || sc.max_batch > 100_000 {
        bad.push(format!("max_batch = {} (need 1..=100000)", sc.max_batch));
    }
    if !(sc.prefill_tokens_per_s.is_finite() && sc.prefill_tokens_per_s > 0.0) {
        bad.push(format!("prefill_tokens_per_s = {} must be positive", sc.prefill_tokens_per_s));
    }
    if sc.step_token_budget == 0 {
        bad.push("step_token_budget must be at least 1".into());
    }
    if !(sc.step_base_ms.is_finite() && sc.step_base_ms > 0.0) {
        bad.push(format!("step_base_ms = {} must be positive", sc.step_base_ms));
    }
    if !(sc.sample_interval_ms.is_finite() && sc.sample_interval_ms > 0.0) {
        bad.push(format!("sample_interval_ms = {} must be positive", sc.sample_interval_ms));
    }
    match sc.failure_events() {
        Ok(evs) => {
            for f in evs.iter().filter(|f| f.replica >= sc.replicas) {
                bad.push(format!("failures: replica {} is not in a fleet of {}", f.replica, sc.replicas));
            }
        }
        Err(why) => bad.push(format!("failures: {why}")),
    }
    if sc.max_attempts == 0 {
        bad.push("max_attempts must be at least 1".into());
    }
    if !(sc.load_step_factor.is_finite() && sc.load_step_factor >= 0.0 && sc.load_step_factor <= 1000.0) {
        bad.push(format!("load_step_factor = {} (need 0..=1000)", sc.load_step_factor));
    }

    // The estimate that would actually have caught the runaway: how much state this run implies.
    let peak = peak_rps(sc);
    let expected_arrivals = peak * sc.duration_s * sc.max_attempts as f64;
    let cap = max_records(sc);
    if expected_arrivals > cap as f64 {
        bad.push(format!(
            "this scenario implies about {:.0} requests ({:.0} rps x {:.0} s x {} attempts), \
             above the {} record ceiling this run implies. Shorten the run, lower the rate, or \
             lower max_attempts",
            expected_arrivals, peak, sc.duration_s, sc.max_attempts, cap
        ));
    }
    let samples = sc.duration_s * 1000.0 / sc.sample_interval_ms * sc.replicas as f64;
    if samples > 200_000_000.0 {
        bad.push(format!(
            "this scenario implies about {:.0} per-replica samples; raise sample_interval_ms or \
             lower replicas",
            samples
        ));
    }

    if bad.is_empty() {
        Ok(())
    } else {
        Err(format!("scenario {:?} is not runnable:\n  - {}", sc.name, bad.join("\n  - ")))
    }
}

/// The frame under construction: what has happened since the last sample.
///
/// Fed by `finish` and by the admit path, closed into a `Frame` at every sample and reset. The
/// histograms are dense while accumulating, because recording into a dense histogram is one index
/// and one add, and sparse only once frozen.
#[derive(Default)]
struct Window {
    admitted: u64,
    completed: u64,
    rejected: u64,
    timed_out: u64,
    within_slo: u64,
    output_tokens: u64,
    goodput_tokens: u64,
    ttft: Histogram,
    itl_max: Histogram,
    e2e: Histogram,
    queue_wait: Histogram,
    preemptions: u64,
    retries: u64,
    /// Each replica's step clock as of the previous close, so a frame carries the window's share
    /// of busy time and not the run's. Indexed by position and grown with zeros, so a fleet that
    /// changes size mid-run reads as new replicas that were idle until now.
    prev_busy: Vec<Nanos>,
    prev_compute: Vec<Nanos>,
    prev_prompt: Vec<u64>,
    prev_hit: Vec<u64>,
    /// Same pattern as `prev_busy`, for the cumulative TTFT counters: a frame's share is this
    /// window's delta, not the run's.
    prev_ttft_sum: Vec<u64>,
    prev_ttft_count: Vec<u64>,
    prev_preemptions: Vec<u64>,
    /// The tier clocks as of the previous close, same pattern.
    prev_dram_busy: Nanos,
    prev_ssd_busy: Nanos,
}

impl Window {
    fn record(&mut self, rec: &RequestRecord) {
        match rec.outcome {
            Outcome::Ok | Outcome::OkSloViolated => {
                self.completed += 1;
                self.output_tokens += rec.output_tokens as u64;
                if rec.outcome == Outcome::Ok {
                    self.within_slo += 1;
                    self.goodput_tokens += rec.output_tokens as u64;
                }
                if let Some(t) = rec.ttft() {
                    self.ttft.record(t);
                }
                if rec.max_itl > 0 {
                    self.itl_max.record(rec.max_itl);
                }
                if let Some(t) = rec.e2e() {
                    self.e2e.record(t);
                }
                self.queue_wait.record(rec.queue_wait());
            }
            Outcome::Rejected => self.rejected += 1,
            Outcome::TimeoutQueued | Outcome::TimeoutRunning => self.timed_out += 1,
        }
    }

    /// Freeze the window into a frame at `t` and start the next one.
    fn close(&mut self, t: Nanos, offered_rps: f64, replicas: &[Replica], tiers: &Tiers) -> Frame {
        let mut w = std::mem::take(self);
        w.prev_busy.resize(replicas.len(), 0);
        w.prev_compute.resize(replicas.len(), 0);
        w.prev_prompt.resize(replicas.len(), 0);
        w.prev_hit.resize(replicas.len(), 0);
        w.prev_ttft_sum.resize(replicas.len(), 0);
        w.prev_ttft_count.resize(replicas.len(), 0);
        w.prev_preemptions.resize(replicas.len(), 0);
        let samples: Vec<ReplicaSample> = replicas
            .iter()
            .enumerate()
            .map(|(i, r)| {
                let busy = r.busy_ns_through(t);
                let compute = r.compute_ns_through(t);
                let prompt = r.prompt_tokens_total();
                let hit = r.prefix_hit_tokens_total();
                let ttft_sum = r.ttft_sum_ns();
                let ttft_count = r.ttft_count();
                let preemptions = r.preemptions();
                let sample = ReplicaSample {
                    queued: r.queued() as u32,
                    running: r.running() as u32,
                    kv_tokens: r.kv_tokens(),
                    last_step_ns: r.last_step_ns(),
                    busy_ns: busy - w.prev_busy[i],
                    compute_ns: compute - w.prev_compute[i],
                    prompt_tokens: prompt - w.prev_prompt[i],
                    prefix_hit_tokens: hit - w.prev_hit[i],
                    state: r.state(),
                    speed: r.speed(),
                    ttft_sum_ns: ttft_sum - w.prev_ttft_sum[i],
                    ttft_count: ttft_count - w.prev_ttft_count[i],
                    preemptions: preemptions - w.prev_preemptions[i],
                };
                w.prev_busy[i] = busy;
                w.prev_compute[i] = compute;
                w.prev_prompt[i] = prompt;
                w.prev_hit[i] = hit;
                w.prev_ttft_sum[i] = ttft_sum;
                w.prev_ttft_count[i] = ttft_count;
                w.prev_preemptions[i] = preemptions;
                sample
            })
            .collect();
        // The clocks outlive the window they were read in.
        self.prev_busy = w.prev_busy;
        self.prev_compute = w.prev_compute;
        self.prev_prompt = w.prev_prompt;
        self.prev_hit = w.prev_hit;
        self.prev_ttft_sum = w.prev_ttft_sum;
        self.prev_ttft_count = w.prev_ttft_count;
        self.prev_preemptions = w.prev_preemptions;
        let (dram_busy, ssd_busy) = tiers.busy_ns();
        let tier_dram_busy_ns = dram_busy - w.prev_dram_busy;
        let tier_ssd_busy_ns = ssd_busy - w.prev_ssd_busy;
        self.prev_dram_busy = dram_busy;
        self.prev_ssd_busy = ssd_busy;
        Frame {
            t,
            offered_rps,
            admitted: w.admitted,
            completed: w.completed,
            rejected: w.rejected,
            timed_out: w.timed_out,
            within_slo: w.within_slo,
            output_tokens: w.output_tokens,
            goodput_tokens: w.goodput_tokens,
            ttft: w.ttft.to_sparse(),
            itl_max: w.itl_max.to_sparse(),
            e2e: w.e2e.to_sparse(),
            queue_wait: w.queue_wait.to_sparse(),
            preemptions: w.preemptions,
            retries: w.retries,
            // Summed over replicas rather than read from the pools, so the gauge is right with or
            // without cluster pools: each replica always knows what it holds in each tier.
            tier_dram_used: replicas.iter().map(|r| r.dram_tokens()).sum(),
            tier_ssd_used: replicas.iter().map(|r| r.ssd_tokens()).sum(),
            tier_dram_busy_ns,
            tier_ssd_busy_ns,
            replicas: samples,
        }
    }
}

/// What `Sim::apply_overrides` did: the keys whose value changed, and the instant from which the
/// change holds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Applied {
    pub changed: Vec<String>,
    pub at: Nanos,
}

/// A run that can stop and continue.
///
/// Everything `run` used to keep on its stack lives here, so the loop can be driven a window at a
/// time: `StepForward`, `SetSpeed` and the Leaf's `Advance` all need an engine that dispatches every
/// event up to an instant and then hands control back. `run` is `new`, `advance_to(end)` and
/// `into_result`, and produces the byte-identical output it always did; the golden fingerprints are
/// the proof.
/// One scheduler per replica, from the scenario's `scheduling` name.
fn make_schedulers(sc: &Scenario) -> Result<Vec<Box<dyn SchedulingPolicy>>, String> {
    (0..sc.fleet_max()).map(|_| sim_policy::make_scheduling(sc)).collect()
}

pub struct Sim {
    sc: Scenario,
    router: Box<dyn RoutingPolicy>,
    admission: Box<dyn AdmissionPolicy>,
    /// Decides ejection from the delayed views, and writes its verdict back into them, so routing and
    /// admission see a gray replica leave the rotation through the seam they already read.
    health: Box<dyn HealthPolicy>,
    /// Sizes the fleet from the delayed views on every `Ev::Autoscale`. The engine owns what a
    /// decision costs: the cold start, the drain, and which replica goes.
    autoscaler: Box<dyn AutoscalingPolicy>,
    autoscale_iv: Nanos,
    /// Whether a tick is in the queue, so a policy override that turns autoscaling on mid-run starts
    /// exactly one cycle.
    autoscale_pending: bool,
    /// Per slot: bumped on every lifecycle move, so a `WarmupDone` or `DrainTimeout` scheduled for an
    /// earlier life of the slot is ignored; and whether its telemetry cycle is in the queue, so a slot
    /// that returns keeps one cycle rather than two.
    lifecycle_gen: Vec<u64>,
    tele_pending: Vec<bool>,
    /// What the router and admission sample from: the views of the slots that are up, dense and in
    /// slot order, with `route_slot` mapping a choice back to its slot and `route_pos` the inverse
    /// (`NO_REPLICA` for a slot that is not up). A sampling policy draws over `views.len()` and falls
    /// back to the first usable replica when every draw lands on an ejected one; with a third of the
    /// slots absent that fallback funnels a ninth of all traffic to slot 0. So absent, warming and
    /// draining slots are not in this array at all. A fixed fleet has every slot up and this is
    /// `views` itself, in the same order, so nothing about a fixed fleet's routing moves.
    route_views: Vec<ReplicaView>,
    route_slot: Vec<usize>,
    route_pos: Vec<usize>,
    /// One scheduler per replica, since the seam is replica-scoped and a policy may keep state.
    schedulers: Vec<Box<dyn SchedulingPolicy>>,
    tenant_shares: Vec<f64>,
    route_rng: Rng,
    /// Its own stream, so turning sessions on cannot perturb arrivals, shapes or routing.
    session_rng: Rng,
    sessions_spawned: u64,
    workload: Workload,
    /// The prefix topology every request's `prefix_node` indexes; grown by session turns.
    tree: PrefixTree,
    /// Which replicas hold which prefix, fed from each replica's cache after every step.
    holders: PrefixHolders,
    /// Its own stream, so a fork rate of zero draws nothing and perturbs nothing.
    fork_rng: Rng,
    /// Nodes of recently completed requests, the pool a fork picks its parent from. Bounded, so
    /// the pool is the recent past and not the whole run.
    recent_nodes: VecDeque<u64>,

    start: Nanos,
    end: Nanos,
    measured_from: Nanos,
    /// How far the run has been advanced: every event at or before this instant has been dispatched.
    now: Nanos,

    q: EventQueue<Ev>,
    replicas: Vec<Replica>,
    views: Vec<ReplicaView>,
    placed: HashMap<u64, usize>,
    done: HashMap<u64, bool>,
    records: Vec<RequestRecord>,

    tracing: Tracing,

    cost: sim_physics::CostModel,
    /// The cluster's memory tiers and their shared fabric: the one piece of simulated state every
    /// replica's step touches, because the pools are cluster-wide.
    tiers: Tiers,
    sample_iv: Nanos,
    tele_iv: Nanos,
    tele_delay: Nanos,
    failures: Vec<FailureEvent>,

    replica_load: Vec<Series>,
    fleet_queue: Series,
    fleet_running: Series,
    fleet_kv: Series,
    offered: Series,

    outcomes: HashMap<&'static str, u64>,
    fingerprint: u64,
    retries: u64,
    first_attempts: u64,
    window: Window,
    frames: Vec<Frame>,

    /// This run's state ceilings, computed from `sc` in `Sim::new` rather than held as constants.
    /// See `max_in_flight`, `max_queue_len` and `max_records` for the formulas.
    max_in_flight: usize,
    max_queue_len: usize,
    max_records: usize,
    /// The memory guard: a budget in bytes (from `memory_budget_mb`, or overridden by
    /// `set_memory_budget_bytes` for a test) and how often, in dispatched events, to check
    /// resident memory against it (overridden by `set_memory_check_interval` for a test).
    memory_budget_bytes: u64,
    memory_check_interval: u64,
    /// Set by a tripwire; the run is over and `into_result` reports why.
    tripped: Option<String>,
    /// Set once the loop would have exited: an event past the end, an empty queue, or a trip.
    finished: bool,
}

/// Ids for session turns, above the first-attempt and retry ranges.
const SESSION_ID_BASE: u64 = 1 << 40;

/// How many recently completed nodes a fork can pick from.
const FORK_POOL: usize = 256;

/// Fleet-wide prefix residency, the gateway's view: node to the replicas whose cache holds it. Kept
/// from what each replica reports after every step rather than by asking replicas at route time, so
/// a lookup is a walk up the tree, O(depth), and never a scan of the fleet.
#[derive(Default, Debug)]
pub struct PrefixHolders {
    by_node: BTreeMap<u64, Vec<usize>>,
}

impl PrefixHolders {
    /// Fold one replica's cache changes in. Evictions are applied after insertions, since a node
    /// inserted and evicted within one step is not resident.
    pub fn apply(&mut self, replica: usize, inserted: &[u64], evicted: &[u64]) {
        for &n in inserted {
            let v = self.by_node.entry(n).or_default();
            if !v.contains(&replica) {
                v.push(replica);
                v.sort_unstable();
            }
        }
        for &n in evicted {
            if let Some(v) = self.by_node.get_mut(&n) {
                v.retain(|&r| r != replica);
                if v.is_empty() {
                    self.by_node.remove(&n);
                }
            }
        }
    }

    /// See `sim_policy::PrefixIndex::holders`: the replicas holding `node` or its deepest held
    /// ancestor, with the hit in tokens, best first, at most four.
    pub fn holders(&self, tree: &PrefixTree, node: u64) -> Vec<(usize, u32)> {
        let mut n = node;
        while n != 0 {
            if let Some(v) = self.by_node.get(&n) {
                let hit = tree.path_tokens(n);
                return v.iter().take(4).map(|&r| (r, hit)).collect();
            }
            n = tree.parent(n);
        }
        Vec::new()
    }

    /// The index as the routing seam sees it.
    pub fn index<'a>(&'a self, tree: &'a PrefixTree) -> LeafPrefixIndex<'a> {
        LeafPrefixIndex { tree, holders: self }
    }
}

/// `PrefixHolders` bound to its tree, the shape the routing seam takes.
pub struct LeafPrefixIndex<'a> {
    tree: &'a PrefixTree,
    holders: &'a PrefixHolders,
}

impl PrefixIndex for LeafPrefixIndex<'_> {
    fn holders(&self, node: u64) -> Vec<(usize, u32)> {
        self.holders.holders(self.tree, node)
    }
}

/// Perhaps schedule the session's next turn after a completed one. The number of turns is geometric
/// with mean `session_turns_mean`, the next turn carries the whole context so far plus a short new
/// prompt, and it goes back to the replica that holds that context, which parks it in the meantime.
/// The turn arrives at the gateway like any other request and admission may shed it, but routing
/// may not move it: the point of keeping context resident is lost on any other replica, and a router
/// that knew that would do the same.
#[allow(clippy::too_many_arguments)]
fn follow_up(
    sc: &Scenario,
    rng: &mut Rng,
    spawned: &mut u64,
    i: usize,
    replica: &mut Replica,
    q: &mut EventQueue<Ev>,
    tree: &mut PrefixTree,
    prev: &Request,
    now: Nanos,
) {
    if sc.session_turns_mean <= 1.0 {
        return;
    }
    if rng.f64() >= 1.0 - 1.0 / sc.session_turns_mean {
        return;
    }
    *spawned += 1;
    let context = prev.prompt as u64 + prev.output as u64;
    let new_prompt = 1 + rng.below(sc.prompt_mean.max(1.0) as u64) as u32;
    let output = 1 + rng.below((2.0 * sc.output_mean).max(1.0) as u64) as u32;
    let at = now + (sc.session_think_s * 1e9) as Nanos;
    // The whole previous context is what the next turn shares, a node under the previous turn's;
    // with no prefix model the tree stays empty and the turn carries node 0 as it always did.
    let (prefix_node, prefix_tokens) = if sc.prefix_roots > 0 {
        let own = (prev.prompt as u64 + prev.output as u64 - prev.prefix_tokens as u64) as u32;
        let node = tree.child(prev.prefix_node, own);
        (node, tree.path_tokens(node))
    } else {
        (0, 0)
    };
    let req = Request {
        id: SESSION_ID_BASE + *spawned,
        arrived_at: at,
        attempt_at: at,
        prompt: (context + new_prompt as u64).min(u32::MAX as u64) as u32,
        output,
        attempts: 1,
        deadline: at + (sc.client_timeout_s * 1e9) as Nanos,
        is_long: prev.is_long,
        tenant: prev.tenant,
        class: prev.class,
        prefix_node,
        prefix_tokens,
    };
    replica.park(req.id, context, req.deadline, now);
    q.schedule(at, Ev::SessionTurn(i, req));
}

pub fn run(sc: &Scenario) -> Result<RunResult, String> {
    let mut sim = Sim::new(sc)?;
    let end = sim.end();
    sim.advance_to(end)?;
    sim.into_result()
}

impl Sim {
    pub fn new(sc: &Scenario) -> Result<Sim, String> {
        validate(sc)?;
        // An unknown name is an error here, before anything runs, never a panic mid-run.
        sim_policy::make_scheduling(sc)?;
        let router = sim_policy::make_routing(sc)?;
        let admission = sim_policy::make_admission(sc)?;
        let health = sim_policy::make_health(sc)?;
        let autoscaler = sim_policy::make_autoscaling(sc)?;
        let schedulers = make_schedulers(sc)?;
        let tenant_shares = sc.tenant_shares();
        let streams = Streams::new(sc.seed);
        let route_rng: Rng = streams.stream("route");
        let session_rng: Rng = streams.stream("session");
        let fork_rng: Rng = streams.stream("fork");
        let workload = Workload::new(&streams);
        let tree = PrefixTree::new(sc, &mut streams.stream("prefix_tree"));
        let tracing = Tracing::new(&streams, sc);

        let start = EPOCH_BASE;
        let end = start + (sc.duration_s * 1e9) as Nanos;
        let measured_from = start + (sc.warmup_s * 1e9) as Nanos;

        let mut q: EventQueue<Ev> = EventQueue::new(start);
        // Every slot the autoscaler may ever fill exists from the start, so replica ids are stable
        // for the whole run; the ones past `replicas` begin absent, unrouted and silent. Without an
        // autoscaler `fleet_max() == replicas` and this is the fleet it always was.
        let slots = sc.fleet_max();
        let replicas: Vec<Replica> = (0..slots)
            .map(|i| {
                let mut r = Replica::default();
                if i >= sc.replicas {
                    r.set_lifecycle(Lifecycle::Absent);
                }
                r
            })
            .collect();
        let views: Vec<ReplicaView> = (0..slots)
            .map(|i| ReplicaView { ejected: i >= sc.replicas, ..ReplicaView::default() })
            .collect();

        let cost = sc.cost_model();
        let tiers = Tiers::new(sc);
        let sample_iv = (sc.sample_interval_ms * 1e6) as Nanos;
        let tele_iv = (sc.telemetry_interval_ms * 1e6) as Nanos;
        let tele_delay = (sc.telemetry_delay_ms * 1e6) as Nanos;

        let replica_load: Vec<Series> = (0..slots)
            .map(|i| Series::new(&format!("replica_{}", i)))
            .collect();
        let autoscale_iv = (sc.autoscale_interval_s * 1e9) as Nanos;
        let autoscale_pending = sc.autoscaling != "none";
        if autoscale_pending {
            q.schedule(start + autoscale_iv, Ev::Autoscale);
        }

        q.schedule(start, Ev::Arrival);
        q.schedule_prio(start + sample_iv, PRIO_OBSERVE, Ev::Sample);
        for i in 0..sc.replicas {
            q.schedule(start + (i as Nanos * tele_iv) / sc.replicas.max(1) as Nanos, Ev::TelemetryPublish(i));
        }
        // Validated above, so this cannot fail. A scenario without failures schedules nothing here,
        // which is what keeps every existing run byte-identical.
        let failures = sc.failure_events().unwrap_or_default();
        for (k, f) in failures.iter().enumerate() {
            q.schedule(start + (f.at * 1e9) as Nanos, Ev::Fail(k));
            if let Some(until) = f.until {
                q.schedule(start + (until * 1e9) as Nanos, Ev::Recover(k));
            }
        }

        Ok(Sim {
            sc: sc.clone(),
            router,
            admission,
            health,
            autoscaler,
            autoscale_iv,
            autoscale_pending,
            lifecycle_gen: vec![0; slots],
            tele_pending: (0..slots).map(|i| i < sc.replicas).collect(),
            route_views: views[..sc.replicas].to_vec(),
            route_slot: (0..sc.replicas).collect(),
            route_pos: (0..slots).map(|i| if i < sc.replicas { i } else { NO_REPLICA }).collect(),
            schedulers,
            tenant_shares,
            route_rng,
            session_rng,
            sessions_spawned: 0,
            workload,
            tree,
            holders: PrefixHolders::default(),
            fork_rng,
            recent_nodes: VecDeque::new(),
            start,
            end,
            measured_from,
            now: start,
            q,
            replicas,
            views,
            placed: HashMap::new(),
            done: HashMap::new(),
            records: Vec::new(),
            tracing,
            cost,
            tiers,
            sample_iv,
            tele_iv,
            tele_delay,
            failures,
            replica_load,
            fleet_queue: Series::new("fleet_queue"),
            fleet_running: Series::new("fleet_running"),
            fleet_kv: Series::new("fleet_kv_utilization"),
            offered: Series::new("offered_rps"),
            outcomes: HashMap::new(),
            fingerprint: 0,
            retries: 0,
            first_attempts: 0,
            window: Window::default(),
            frames: Vec::new(),
            max_in_flight: max_in_flight(sc),
            max_queue_len: max_queue_len(sc),
            max_records: max_records(sc),
            memory_budget_bytes: memory_budget_mb() * 1024 * 1024,
            memory_check_interval: 1_000_000,
            tripped: None,
            finished: false,
        })
    }

    /// One autoscaling decision, from what a controller can see: the lifecycle counts it set itself
    /// and the delayed views. Up fills the lowest absent slots; down cancels warming slots first,
    /// since they hold nothing, then drains the ready replicas the view shows lightest.
    fn autoscale(&mut self, now: Nanos) {
        let (mut ready, mut warming, mut draining) = (0, 0, 0);
        for r in &self.replicas {
            match r.lifecycle() {
                Lifecycle::Active => ready += 1,
                Lifecycle::Warming => warming += 1,
                Lifecycle::Draining => draining += 1,
                Lifecycle::Absent => {}
            }
        }
        let (min, max) = (self.sc.fleet_min(), self.sc.fleet_max());
        let fv = FleetView { now, ready, warming, draining, max, min, views: &self.views };
        let want = self.autoscaler.desired(&fv).clamp(min, max);
        let current = ready + warming;
        if want > current {
            let absent: Vec<usize> = (0..self.replicas.len())
                .filter(|&i| self.replicas[i].lifecycle() == Lifecycle::Absent)
                .take(want - current)
                .collect();
            for i in absent {
                self.turn_up(i, now);
            }
        } else if want < current {
            let mut n = current - want;
            let mut warming: Vec<usize> = (0..self.replicas.len())
                .filter(|&i| self.replicas[i].lifecycle() == Lifecycle::Warming)
                .collect();
            while n > 0 {
                let Some(i) = warming.pop() else { break };
                self.lifecycle_gen[i] += 1;
                self.replicas[i].set_lifecycle(Lifecycle::Absent);
                n -= 1;
            }
            let mut ready: Vec<usize> = (0..self.replicas.len())
                .filter(|&i| self.replicas[i].lifecycle() == Lifecycle::Active)
                .collect();
            ready.sort_by_key(|&i| (self.views[i].queued + self.views[i].running, i));
            for i in ready.into_iter().take(n) {
                self.drain(i, now);
            }
        }
    }

    /// The routable set changed: rebuild the dense views and both maps. O(slots), on a lifecycle
    /// move only, never per request.
    fn rebuild_route(&mut self) {
        self.route_slot.clear();
        self.route_views.clear();
        for (i, r) in self.replicas.iter().enumerate() {
            self.route_pos[i] = if r.lifecycle() == Lifecycle::Active {
                self.route_slot.push(i);
                self.route_views.push(self.views[i]);
                self.route_slot.len() - 1
            } else {
                NO_REPLICA
            };
        }
    }

    /// A delivery or a verdict changed `views`: the dense copy follows.
    fn sync_route_views(&mut self) {
        for (d, &slot) in self.route_slot.iter().enumerate() {
            self.route_views[d] = self.views[slot];
        }
    }

    /// An absent slot begins its cold start.
    fn turn_up(&mut self, i: usize, now: Nanos) {
        self.lifecycle_gen[i] += 1;
        self.replicas[i].set_lifecycle(Lifecycle::Warming);
        let warmup = (self.sc.warmup_delay_s * 1e9) as Nanos;
        self.q.schedule(now + warmup, Ev::WarmupDone(i, self.lifecycle_gen[i]));
    }

    /// A ready replica stops taking work. The router sees it leave at once, since the controller
    /// decided it; what it holds finishes, or is lost at the drain timeout.
    fn drain(&mut self, i: usize, now: Nanos) {
        self.lifecycle_gen[i] += 1;
        self.replicas[i].set_lifecycle(Lifecycle::Draining);
        self.views[i].ejected = true;
        self.rebuild_route();
        if self.replicas[i].load() == 0 {
            self.retire(i, now);
            return;
        }
        let timeout = (self.sc.drain_timeout_s * 1e9) as Nanos;
        self.q.schedule(now + timeout, Ev::DrainTimeout(i, self.lifecycle_gen[i]));
    }

    /// The slot leaves the fleet. Whatever it still holds is lost exactly as a crash loses it, and
    /// its cache goes with it, so a slot that returns later returns cold.
    fn retire(&mut self, i: usize, now: Nanos) {
        for req in self.replicas[i].crash() {
            self.abort(&req, i, Outcome::TimeoutRunning, now);
        }
        let (inserted, evicted) = self.replicas[i].drain_prefix_changes();
        self.holders.apply(i, &inserted, &evicted);
        self.replicas[i].recover();
        self.lifecycle_gen[i] += 1;
        self.replicas[i].set_lifecycle(Lifecycle::Absent);
        self.views[i].ejected = true;
    }

    /// Record a failed attempt and, under the retry budget, dispatch the next one. Timeouts, crashes
    /// and refused dispatches all end here, so a retry means the same thing whatever caused it.
    fn abort(&mut self, req: &Request, i: usize, outcome: Outcome, now: Nanos) {
        let sc = &self.sc;
        let start = self.start;
        finish(
            &mut self.records, &mut self.outcomes, &mut self.done, &mut self.window,
            outcome, req, now, i, 0, 0, 0, 0,
        );
        self.tracing.settle(req.id, &self.records, &mut self.replicas, i, &self.cost);
        // Retry, under a budget. Retries are what turn a slowdown into a collapse, and
        // they cost far more here than in a stateless service because a timeout after
        // thirty seconds has already burned thirty seconds of device work.
        let budget_ok = self.retries as f64
            <= sc.retry_budget_fraction * self.first_attempts.max(1) as f64;
        if req.attempts < sc.max_attempts && budget_ok {
            self.retries += 1;
            self.window.retries += 1;
            let mut again = req.clone();
            again.attempts += 1;
            again.attempt_at = now + (sc.retry_backoff_s * 1e9) as Nanos;
            // The deadline runs from *this* attempt, since a client that retries gives
            // itself a fresh timeout. Latency, however, is still measured from the
            // original arrival below: from the user's point of view the wait started when
            // they first asked.
            again.deadline = again.attempt_at + (sc.client_timeout_s * 1e9) as Nanos;
            again.id = 1_000_000_000 + again.id * 8 + again.attempts as u64;
            again.arrived_at = req.arrived_at;
            let at = again.attempt_at;
            // A retry is a fresh draw: its journey is its own, from its own arrival.
            let traced = self.tracing.sample();
            let mut probed = Vec::new();
            // Re-routed rather than pinned, so a retry does not land on the same
            // struggling replica by construction.
            let d = dispatch(
                &mut *self.router, &mut *self.admission, &self.route_views, &self.route_slot, &self.replicas,
                &self.tenant_shares, &mut self.route_rng, sc, at, &again, traced.then_some(&mut probed),
                &RoutableIndex { inner: self.holders.index(&self.tree), pos: &self.route_pos },
            );
            let (again_id, routed) = (again.id, matches!(d, Dispatch::Route { .. }));
            if traced {
                self.tracing.draft(&again, at, &d, probed, &self.views, start, self.placed.len());
            }
            place(
                &mut self.q, &mut self.records, &mut self.outcomes, &mut self.done,
                &mut self.window, d, again, at,
            );
            if traced && !routed {
                self.tracing.settle(again_id, &self.records, &mut self.replicas, NO_REPLICA, &self.cost);
            }
        }
    }

    pub fn now(&self) -> Nanos {
        self.now
    }
    pub fn end(&self) -> Nanos {
        self.end
    }
    pub fn finished(&self) -> bool {
        self.finished
    }
    /// Override the memory guard's budget, in bytes. For a test that wants to exercise the guard
    /// without touching `LBSIM_MEMORY_BUDGET_MB` in the environment.
    pub fn set_memory_budget_bytes(&mut self, bytes: u64) {
        self.memory_budget_bytes = bytes;
    }
    /// Override how often, in dispatched events, the memory guard checks resident memory. For a
    /// test: 1,000,000 dispatched events is too many for a short run to reach.
    pub fn set_memory_check_interval(&mut self, events: u64) {
        self.memory_check_interval = events;
    }
    /// The frames closed so far. Available while the run is in progress, which is the point.
    pub fn frames(&self) -> &[Frame] {
        &self.frames
    }
    pub fn prefix_tree(&self) -> &PrefixTree {
        &self.tree
    }
    pub fn prefix_holders(&self) -> &PrefixHolders {
        &self.holders
    }
    /// The frames closed since the last `drain_frames` call (or the start of the run, for the
    /// first), handing ownership to the caller instead of leaving a second copy behind. For a
    /// long-lived run whose driver copies each new frame into its own state as it advances, so
    /// that state does not also grow the engine's own copy without bound. A `Sim` that drains is
    /// then a `Sim` whose `frames()` and `into_result` see only what nothing has drained yet;
    /// `frames()` and `into_result` alone, never mixed with this, are what a caller that keeps the
    /// engine's copy as the record of the whole run should use instead.
    pub fn drain_frames(&mut self) -> Vec<Frame> {
        std::mem::take(&mut self.frames)
    }
    /// The traces the sampler has retained since the last drain, warm-up included, in completion
    /// order. `drain_frames`'s contract, for traces: a long-lived driver takes them as it goes so
    /// the engine's own copy does not grow for the run's whole length, and `into_result` then sees
    /// only what nothing has drained. The sampler's window quotas are untouched, so what is kept
    /// is exactly what `sim_leaf::run` would have kept.
    pub fn drain_traces(&mut self) -> Vec<RequestTrace> {
        std::mem::take(&mut self.tracing.traces)
    }
    /// Every record so far, warmup included; the measured selection is `into_result`'s.
    pub fn records(&self) -> &[RequestRecord] {
        &self.records
    }
    /// The instant of the next pending event, if any. What a barrier loop sizes its window by.
    pub fn next_event(&self) -> Option<Nanos> {
        self.q.peek_time()
    }
    /// Every replica as it is right now, not as the delayed telemetry shows it: live telemetry for a
    /// viewer, never for a policy.
    pub fn latest_views(&self) -> Vec<ReplicaView> {
        self.replicas.iter().map(|r| view_of(r, self.now)).collect()
    }
    /// The fleet as the policies see it: the delayed telemetry, with the health policy's ejections
    /// written in. This is the view a test of detection latency has to read, because a policy's
    /// ejection is nowhere else.
    pub fn policy_views(&self) -> &[ReplicaView] {
        &self.views
    }

    /// Change workload or policy settings in a run that is under way, forward only.
    ///
    /// The whole batch is checked before anything moves: an unknown key, a value that does not parse,
    /// or a structural key (`replicas`, `seed`, `duration_s`, the physics, the telemetry cadence)
    /// refuses the batch and leaves the run exactly as it was, so a caller never has to undo half an
    /// update. A workload key takes effect at the next arrival, because the workload reads the
    /// scenario on every draw. A policy key rebuilds the router and the admission policy through the
    /// registries, and **a rebuilt policy starts with empty state**: a round-robin cursor returns to
    /// zero, a probe cache is cold, a fair-share ledger is blank. The random streams are untouched,
    /// so a run with no overrides is byte-identical to `run`; the golden fingerprints prove it.
    ///
    /// `Applied::changed` lists the keys whose value actually changed; a key set to the value it
    /// already had is accepted and rebuilds nothing.
    pub fn apply_overrides(&mut self, overrides: &[(&str, &str)]) -> Result<Applied, String> {
        let mut sc = self.sc.clone();
        let mut changed = Vec::new();
        let mut rebuild_policies = false;
        for &(key, value) in overrides {
            // The round trip first, so an unknown key is reported as unknown rather than as structural.
            let next = sc.with_override(key, value)?;
            let kind = match Scenario::override_kind(key) {
                OverrideKind::Structural => return Err(format!("`{key}` requires a restart")),
                kind => kind,
            };
            if next.to_text() != sc.to_text() {
                changed.push(key.to_string());
                rebuild_policies |= kind == OverrideKind::Policy;
            }
            sc = next;
        }
        validate(&sc)?;
        if rebuild_policies {
            let router = sim_policy::make_routing(&sc)?;
            let admission = sim_policy::make_admission(&sc)?;
            let health = sim_policy::make_health(&sc)?;
            let autoscaler = sim_policy::make_autoscaling(&sc)?;
            let schedulers = make_schedulers(&sc)?;
            self.router = router;
            self.admission = admission;
            self.health = health;
            self.autoscaler = autoscaler;
            self.schedulers = schedulers;
            self.autoscale_iv = (sc.autoscale_interval_s * 1e9) as Nanos;
            // Turned on mid-run: one cycle starts now. A cycle already running keeps its cadence and
            // reads the new interval at its next tick; `none` lets the running cycle lapse.
            if sc.autoscaling != "none" && !self.autoscale_pending {
                self.autoscale_pending = true;
                self.q.schedule(self.now + self.autoscale_iv, Ev::Autoscale);
            }
        }
        self.sc = sc;
        Ok(Applied { changed, at: self.now })
    }

    /// Dispatch every event at or before `t`, in the order the run always dispatched them, and stop.
    ///
    /// An event beyond `t` is left in the queue untouched, so `now()` never passes `t`. The one
    /// exception is the end of the run: the first event past `end` is popped and discarded, as the
    /// loop always did, because the dispatched count that the report prints includes it.
    pub fn advance_to(&mut self, t: Nanos) -> Result<(), String> {
        let start = self.start;
        let end = self.end;
        // Tripwires. Each one names what to look at, because the failure mode being guarded against is
        // exponential growth in scheduled work, which looks like a hang and then an out-of-memory kill.
        while !self.finished {
            let Some(at) = self.q.peek_time() else {
                self.finished = true;
                break;
            };
            if at > t && at <= end {
                break;
            }
            let Some((now, ev)) = self.q.pop() else { break };
            if now > end {
                self.finished = true;
                break;
            }
            if let Some(msg) = rss_over_budget(
                self.q.dispatched, self.memory_check_interval, self.memory_budget_bytes,
            ) {
                self.tripped = Some(msg);
                self.finished = true;
                break;
            }
            if self.records.len() > self.max_records || self.placed.len() > self.max_in_flight {
                self.tripped = Some(format!(
                    "state ceiling: {} records and {} tracked requests. Offered load is far above what \
                     this fleet retires, or requests are never completing",
                    self.records.len(),
                    self.placed.len()
                ));
                self.finished = true;
                break;
            }
            if self.q.len() > self.max_queue_len {
                self.tripped = Some(format!(
                    "queue ceiling: {} pending events. Almost always a self-scheduling event that never \
                     terminates",
                    self.q.len()
                ));
                self.finished = true;
                break;
            }
            self.now = now;
            // Borrowed per event rather than per call, so an arm that ends in `abort` can take the
            // whole of `self`.
            let sc = &self.sc;
            match ev {
                Ev::Arrival => {
                    let elapsed = (now - start) as f64 / 1e9;
                    let mut req = self.workload.make_with_prefixes(sc, now, &self.tree);
                    // A fork: an agent spawned from a recently finished request's context shares
                    // that whole prefix rather than starting at a root. Drawn before the pool is
                    // consulted, so the stream advances the same way whether or not one is there.
                    if sc.session_fork_rate > 0.0 && self.fork_rng.f64() < sc.session_fork_rate {
                        if let Some(node) = self.recent_nodes.pop_back() {
                            req.prefix_node = node;
                            req.prefix_tokens = self.tree.path_tokens(node);
                            req.prompt = req.prompt.max(req.prefix_tokens.saturating_add(1));
                        }
                    }
                    self.first_attempts += 1;
                    let traced = self.tracing.sample();
                    let mut probed = Vec::new();
                    let d = dispatch(
                        &mut *self.router, &mut *self.admission, &self.route_views, &self.route_slot, &self.replicas,
                        &self.tenant_shares, &mut self.route_rng, sc, now, &req, traced.then_some(&mut probed),
                        &RoutableIndex { inner: self.holders.index(&self.tree), pos: &self.route_pos },
                    );
                    let (id, routed) = (req.id, matches!(d, Dispatch::Route { .. }));
                    if traced {
                        self.tracing.draft(&req, now, &d, probed, &self.views, start, self.placed.len());
                    }
                    place(
                        &mut self.q, &mut self.records, &mut self.outcomes, &mut self.done,
                        &mut self.window, d, req, now,
                    );
                    if traced && !routed {
                        self.tracing.settle(id, &self.records, &mut self.replicas, NO_REPLICA, &self.cost);
                    }
                    let gap = self.workload.next_gap_ns(sc, elapsed);
                    self.q.schedule(now + gap.max(1), Ev::Arrival);
                }

                Ev::SessionTurn(i, req) => {
                    // A first attempt for the retry budget as much as a fresh arrival is.
                    self.first_attempts += 1;
                    let d = dispatch_pinned(
                        &mut *self.admission, &self.route_views, &self.tenant_shares, now, &req, i,
                    );
                    if matches!(d, Dispatch::Rejected) {
                        // The turn that would have reused the parked context is not coming.
                        self.replicas[i].remove(req.id);
                        self.tiers.reclaim(&mut self.replicas[i]);
                    }
                    place(
                        &mut self.q, &mut self.records, &mut self.outcomes, &mut self.done,
                        &mut self.window, d, req, now,
                    );
                }

                Ev::Admit(target, req) => {
                    if !self.replicas[target].accepts() && self.replicas[target].lifecycle() != Lifecycle::Draining {
                        // Sent here on a stale view: the router will not learn of the crash until the
                        // next telemetry delivery. Refused before any device time, and retried. A
                        // draining replica is the exception: it was routable when the request left,
                        // and finishing a straggler is cheaper than bouncing it.
                        self.abort(&req, target, Outcome::TimeoutQueued, now);
                        continue;
                    }
                    let r = &mut self.replicas[target];
                    let (id, deadline) = (req.id, req.deadline);
                    if let Err(req) = r.enqueue(req, sc.max_queue) {
                        // Shed before consuming device time: the cheap failure, and deliberately
                        // distinct in the outcome from one that fails after burning work. A session
                        // turn shed here releases the context parked for it.
                        r.remove(id);
                        self.tiers.reclaim(r);
                        finish(
                            &mut self.records, &mut self.outcomes, &mut self.done, &mut self.window,
                            Outcome::Rejected, &req, now, target, 0, 0, 0, 0,
                        );
                        self.tracing.settle(id, &self.records, &mut self.replicas, target, &self.cost);
                        continue;
                    }
                    if let Some(d) = self.tracing.drafts.get_mut(&id) {
                        d.enqueued_at = now;
                        d.queue_ahead = r.queued().saturating_sub(1) as u32;
                        r.tracer_mut().track(id);
                    }
                    self.window.admitted += 1;
                    self.placed.insert(id, target);
                    self.q.schedule(deadline, Ev::Timeout(id));
                    if r.wake(now) {
                        self.q.schedule(now, Ev::Step(target));
                    }
                }

                Ev::Step(i) => {
                    let Some(out) = self.replicas[i].step_tiered(
                        sc,
                        &self.cost,
                        now,
                        &self.tree,
                        &mut *self.schedulers[i],
                        &mut self.tiers,
                    ) else {
                        continue
                    };
                    let (inserted, evicted) = self.replicas[i].drain_prefix_changes();
                    self.holders.apply(i, &inserted, &evicted);
                    let token_at = out.token_at;
                    for s in out.finished {
                        if s.req.prefix_node != 0 {
                            if self.recent_nodes.len() == FORK_POOL {
                                self.recent_nodes.pop_front();
                            }
                            self.recent_nodes.push_back(s.req.prefix_node);
                        }
                        // An infinite ITL target saturates to Nanos::MAX in the cast, which is the
                        // intended "never fails on ITL".
                        let (ttft_ms, itl_ms, e2e_s) = sc.slo_for(s.req.class);
                        let within = s.first_token_at - s.req.arrived_at <= (ttft_ms * 1e6) as Nanos
                            && s.max_itl <= (itl_ms * 1e6) as Nanos
                            && token_at - s.req.arrived_at <= (e2e_s * 1e9) as Nanos;
                        let outcome = if within { Outcome::Ok } else { Outcome::OkSloViolated };
                        self.admission.on_complete(s.req.tenant, s.req.output, token_at);
                        finish(
                            &mut self.records, &mut self.outcomes, &mut self.done, &mut self.window,
                            outcome, &s.req, token_at, i, s.admitted_at, s.first_token_at, s.max_itl,
                            s.mean_itl,
                        );
                        self.tracing.settle(s.req.id, &self.records, &mut self.replicas, i, &self.cost);
                        follow_up(
                            sc, &mut self.session_rng, &mut self.sessions_spawned, i,
                            &mut self.replicas[i], &mut self.q, &mut self.tree, &s.req, token_at,
                        );
                    }
                    self.window.preemptions += out.preempted as u64;

                    if !out.idle {
                        self.q.schedule(token_at, Ev::Step(i));
                    } else if self.replicas[i].lifecycle() == Lifecycle::Draining {
                        // Nothing left to run: the drain is complete and the slot leaves the fleet.
                        self.retire(i, now);
                    }
                    self.fingerprint = self
                        .fingerprint
                        .wrapping_mul(0x100_0000_01b3)
                        .wrapping_add(out.step_ns ^ (i as u64));
                }

                Ev::TelemetryPublish(i) => {
                    if self.replicas[i].lifecycle() == Lifecycle::Absent {
                        // A slot that left the fleet stops announcing; `turn_up` restarts the cycle.
                        self.tele_pending[i] = false;
                        continue;
                    }
                    let view = view_of(&self.replicas[i], now);
                    // Delayed delivery. This one line is the whole staleness mechanism: a policy cannot
                    // see the fleet as it is, only as it was.
                    self.q.schedule(now + self.tele_delay, Ev::TelemetryDeliver(i, view));
                    self.q.schedule(now + self.tele_iv, Ev::TelemetryPublish(i));
                }

                Ev::TelemetryDeliver(i, view) => {
                    self.views[i] = view;
                    // A slot the controller moved since this view was published is not routable
                    // whatever the view says: the controller made that decision itself, so it does
                    // not wait for telemetry to hear of it. A fixed fleet never enters here.
                    if self.replicas[i].lifecycle() != Lifecycle::Active {
                        self.views[i].ejected = true;
                    }
                    // Assessed on every delivery, over the delayed views, and written back into them:
                    // an ejection reaches the router by the same stale path a crash does. With
                    // `ejection = none` this returns the flags already there, byte for byte.
                    let verdict = self.health.assess(now, &self.views);
                    for (v, ejected) in self.views.iter_mut().zip(verdict) {
                        v.ejected = ejected;
                    }
                    self.sync_route_views();
                }

                Ev::Timeout(id) => {
                    if self.done.contains_key(&id) {
                        continue;
                    }
                    let Some(&i) = self.placed.get(&id) else { continue };
                    let removed = self.replicas[i].remove(id);
                    self.tiers.reclaim(&mut self.replicas[i]);
                    if let Some((req, was_running)) = removed {
                        let outcome = if was_running {
                            Outcome::TimeoutRunning
                        } else {
                            Outcome::TimeoutQueued
                        };
                        self.abort(&req, i, outcome, now);
                        if self.replicas[i].lifecycle() == Lifecycle::Draining && self.replicas[i].load() == 0 {
                            self.retire(i, now);
                        }
                    }
                }

                Ev::Fail(k) => {
                    let f = self.failures[k].clone();
                    match f.kind {
                        // Everything it held is lost at once, running or not: the work in flight
                        // is what a crash costs, and every client of it sees a timeout.
                        FailureKind::Crash => {
                            for req in self.replicas[f.replica].crash() {
                                self.abort(&req, f.replica, Outcome::TimeoutRunning, now);
                            }
                            self.tiers.reclaim(&mut self.replicas[f.replica]);
                            let (inserted, evicted) = self.replicas[f.replica].drain_prefix_changes();
                            self.holders.apply(f.replica, &inserted, &evicted);
                        }
                        FailureKind::Slow(mult) => self.replicas[f.replica].set_speed(mult),
                        FailureKind::Hang => self.replicas[f.replica].set_speed(0.0),
                    }
                }

                Ev::Recover(k) => {
                    let i = self.failures[k].replica;
                    let r = &mut self.replicas[i];
                    r.recover();
                    // A hang leaves whatever has not timed out still queued; it resumes now.
                    if r.wake(now) {
                        self.q.schedule(now, Ev::Step(i));
                    }
                }

                Ev::Sample => {
                    let mut tq = 0.0;
                    let mut tr = 0.0;
                    let mut tkv = 0.0;
                    for (i, r) in self.replicas.iter().enumerate() {
                        let load = r.load() as f64;
                        self.replica_load[i].push(now, load);
                        tq += r.queued() as f64;
                        tr += r.running() as f64;
                        tkv += r.kv_tokens() as f64 / sc.kv_capacity_tokens;
                    }
                    self.fleet_queue.push(now, tq);
                    self.fleet_running.push(now, tr);
                    // Over the replicas that can hold context, so an absent slot's empty cache does
                    // not read as headroom. Exactly `replicas` for a fixed fleet.
                    let holding = self
                        .replicas
                        .iter()
                        .filter(|r| matches!(r.lifecycle(), Lifecycle::Active | Lifecycle::Draining))
                        .count()
                        .max(1);
                    self.fleet_kv.push(now, 100.0 * tkv / holding as f64);
                    let window_end_ns = now - start;
                    let window_start_ns = window_end_ns.saturating_sub(self.sample_iv);
                    let rate = self.workload.offered_rps(sc, window_start_ns, window_end_ns);
                    self.offered.push(now, rate);
                    self.frames.push(self.window.close(now, rate, &self.replicas, &self.tiers));
                    self.q.schedule_prio(now + self.sample_iv, PRIO_OBSERVE, Ev::Sample);
                }

                Ev::Autoscale => {
                    if sc.autoscaling == "none" {
                        self.autoscale_pending = false;
                        continue;
                    }
                    self.autoscale(now);
                    self.q.schedule(now + self.autoscale_iv, Ev::Autoscale);
                }

                Ev::WarmupDone(i, gen) => {
                    if self.lifecycle_gen[i] == gen && self.replicas[i].lifecycle() == Lifecycle::Warming {
                        self.lifecycle_gen[i] += 1;
                        self.replicas[i].set_lifecycle(Lifecycle::Active);
                        self.rebuild_route();
                        // Its telemetry starts with readiness, so the router hears of it one delay
                        // later, the way it hears of everything else.
                        if !self.tele_pending[i] {
                            self.tele_pending[i] = true;
                            self.q.schedule(now, Ev::TelemetryPublish(i));
                        }
                    }
                }

                Ev::DrainTimeout(i, gen) => {
                    if self.lifecycle_gen[i] == gen && self.replicas[i].lifecycle() == Lifecycle::Draining {
                        self.retire(i, now);
                    }
                }
            }
        }
        // Everything at or before the target has run, so time stands at the target; at the run's
        // end it stands at the end whatever was asked for.
        self.now = self.now.max(t.min(end));
        if self.finished {
            self.now = end;
        }
        if let Some(why) = &self.tripped {
            return Err(format!("run {:?} aborted, {}", self.sc.name, why));
        }
        Ok(())
    }

    /// The post-run aggregation. Callable before `end` for a partial result, which is the same
    /// computation over the records so far.
    ///
    /// `RunResult.frames` here is whatever `self.frames` still holds: the complete run for a
    /// caller that never called `drain_frames`, or only the frames closed since the last drain
    /// for one that did. A caller that drains and still wants `into_result` to carry the full
    /// sequence must reattach its own accumulated copy after this returns.
    pub fn into_result(self) -> Result<RunResult, String> {
        let sc = &self.sc;
        let measured_from = self.measured_from;
        if let Some(why) = self.tripped {
            return Err(format!("run {:?} aborted, {}", sc.name, why));
        }

        let mut ttft = Histogram::new();
        let mut itl_max = Histogram::new();
        let mut e2e = Histogram::new();
        let mut queue_wait = Histogram::new();

        // Statistics come from the measured window only, so a run measures steady state rather than the
        // transient of an empty fleet filling up.
        for rec in self.records.iter().filter(|r| r.arrived_at >= measured_from) {
            if let Some(t) = rec.ttft() {
                ttft.record(t);
            }
            if rec.max_itl > 0 {
                itl_max.record(rec.max_itl);
            }
            if let Some(t) = rec.e2e() {
                e2e.record(t);
            }
            queue_wait.record(rec.queue_wait());
        }
        let measured: Vec<RequestRecord> = self
            .records
            .into_iter()
            .filter(|r| r.arrived_at >= measured_from)
            .collect();
        let mut measured_outcomes: HashMap<&'static str, u64> = HashMap::new();
        for r in &measured {
            *measured_outcomes.entry(r.outcome.label()).or_insert(0) += 1;
        }
        let _ = self.outcomes;
        let traces: Vec<RequestTrace> =
            self.tracing.traces.into_iter().filter(|t| t.record.arrived_at >= measured_from).collect();

        Ok(RunResult {
            scenario: sc.clone(),
            routing_label: self.router.label(),
            records: measured,
            traces,
            ttft,
            itl_max,
            e2e,
            queue_wait,
            replica_load: self.replica_load,
            fleet_queue: self.fleet_queue,
            fleet_running: self.fleet_running,
            fleet_kv_utilization: self.fleet_kv,
            offered_rps: self.offered,
            frames: self.frames,
            outcomes: measured_outcomes,
            events: self.q.dispatched,
            fingerprint: self.fingerprint,
            measured_from,
            measured_to: self.end,
            rated_rps: sc.rated_rps(),
            replicas_inspected_per_decision: self.router.inspected(sc.replicas),
            retries: self.retries,
            first_attempts: self.first_attempts,
        })
    }
}

/// The Leaf service, in-process, as one shard that owns every replica.
///
/// Wraps `Sim` whole: arrivals, routing and retries still happen inside the loop, so this shard
/// generates its own work and accepts none from outside. That is deliberate. The point of this type
/// is that `sim_leaf_api::Leaf` is real, has the proto's shape, and `sim-ingress` can be written
/// against it now; moving the ingress-side events out of the loop is the split described in
/// `sim_leaf_api`'s module docs and is its own unit of work. `advance` is `Sim::advance_to` plus
/// what completed and what was sampled since the previous call.
#[derive(Default)]
pub struct LocalLeaf {
    sim: Option<Sim>,
    shard_id: u32,
    /// How much of `Sim::records` and `Sim::frames` earlier windows already reported.
    records_reported: usize,
    frames_reported: usize,
    /// The scenario's telemetry period, for saying whether a publish fell inside a window.
    tele_iv: Nanos,
}

impl LocalLeaf {
    pub fn new() -> Self {
        Self::default()
    }

    /// The run-wide aggregation, for a driver that owns this shard in-process and wants the same
    /// `RunResult` that `run` produces. A process transport will not have this; it will merge the
    /// per-window responses instead.
    pub fn into_result(self) -> Result<RunResult, String> {
        self.sim.ok_or_else(|| "LocalLeaf was never configured".to_string())?.into_result()
    }

    fn sim_mut(&mut self) -> Result<&mut Sim, String> {
        self.sim.as_mut().ok_or_else(|| "LocalLeaf: advance before configure".to_string())
    }
}

impl Leaf for LocalLeaf {
    fn configure(
        &mut self,
        shard_id: u32,
        sc: &Scenario,
        replica_ids: &[u64],
        shard_seed: u64,
    ) -> Result<ConfigureShardResponse, String> {
        // One shard, every replica, the scenario's own seed: anything else is the sharded server
        // this type explicitly is not, and saying so beats silently ignoring the arguments.
        let all: Vec<u64> = (0..sc.replicas as u64).collect();
        if !replica_ids.is_empty() && replica_ids != all.as_slice() {
            return Ok(ConfigureShardResponse {
                accepted: false,
                rejected_reason: format!(
                    "LocalLeaf owns every replica of the scenario; asked for {} of {}",
                    replica_ids.len(),
                    sc.replicas
                ),
                next_event_unix_ns: 0,
            });
        }
        if shard_seed != sc.seed {
            return Ok(ConfigureShardResponse {
                accepted: false,
                rejected_reason: format!(
                    "LocalLeaf runs on the scenario seed {}; a derived shard seed {} would change the run",
                    sc.seed, shard_seed
                ),
                next_event_unix_ns: 0,
            });
        }
        let sim = match Sim::new(sc) {
            Ok(sim) => sim,
            Err(why) => {
                return Ok(ConfigureShardResponse {
                    accepted: false,
                    rejected_reason: why,
                    next_event_unix_ns: 0,
                })
            }
        };
        let next = sim.next_event().unwrap_or(0);
        self.shard_id = shard_id;
        self.records_reported = 0;
        self.frames_reported = 0;
        self.tele_iv = (sc.telemetry_interval_ms * 1e6) as Nanos;
        self.sim = Some(sim);
        Ok(ConfigureShardResponse { accepted: true, rejected_reason: String::new(), next_event_unix_ns: next })
    }

    fn advance(&mut self, req: AdvanceRequest) -> Result<AdvanceResponse, String> {
        if req.shard_id != self.shard_id {
            return Err(format!("LocalLeaf is shard {}, asked to advance shard {}", self.shard_id, req.shard_id));
        }
        if !req.work.is_empty() || !req.control.is_empty() {
            return Err(
                "LocalLeaf generates its own arrivals and timeouts; dispatched work and control \
                 actions arrive with the ingress/leaf split"
                    .to_string(),
            );
        }
        let tele_iv = self.tele_iv;
        let (records_reported, frames_reported) = (self.records_reported, self.frames_reported);
        let sim = self.sim_mut()?;
        let from = sim.now();
        sim.advance_to(req.advance_until_unix_ns)?;
        let to = sim.now();
        let completed = sim.records()[records_reported..].to_vec();
        let metrics = sim.frames()[frames_reported..].to_vec();
        // A publish instant is a multiple of the period from the start; one fell in (from, to] when
        // the count of them changed. Replica offsets stagger the publishes inside the period, so
        // this says a round of them began, which is what Ingress needs to know.
        let start = EPOCH_BASE;
        let telemetry_due =
            tele_iv > 0 && (to.saturating_sub(start)) / tele_iv != (from.saturating_sub(start)) / tele_iv;
        let out = AdvanceResponse {
            shard_id: req.shard_id,
            advanced_to_unix_ns: to,
            next_event_unix_ns: if sim.finished() { 0 } else { sim.next_event().unwrap_or(0) },
            completed,
            // The engine does not yet raise a first-token event apart from the record; the split
            // adds that hook when Ingress has a client that measures it.
            first_tokens: Vec::new(),
            telemetry: sim.latest_views(),
            telemetry_due,
            metrics,
        };
        self.records_reported = self.sim.as_ref().map_or(0, |s| s.records().len());
        self.frames_reported = self.sim.as_ref().map_or(0, |s| s.frames().len());
        Ok(out)
    }

    fn snapshot(&self, _at: Nanos) -> Result<Vec<u8>, String> {
        Err("not implemented".to_string())
    }

    fn restore(&mut self, _at: Nanos, _state: &[u8]) -> Result<(), String> {
        Err("not implemented".to_string())
    }
}

/// What a replica reports about itself, as a policy will see it after the telemetry delay, or right
/// now if a policy pays for a probe. One function so the two views cannot disagree.
fn view_of(r: &Replica, now: Nanos) -> ReplicaView {
    ReplicaView {
        sampled_at: now,
        queued: r.queued() as u32,
        running: r.running() as u32,
        queued_tokens: r.outstanding_tokens(),
        kv_tokens: r.kv_tokens(),
        last_step_ns: r.last_step_ns(),
        // Only a crash is announced. A slow or hung replica reports itself as healthy, and the
        // growing `last_step_ns` is the one tell a policy has. A slot the autoscaler has not made
        // ready is not there to be routed to.
        ejected: !r.accepts(),
    }
}

/// The router's decision for one request, before it has cost anything.
enum Dispatch {
    /// Send to this replica after this much modelled delay.
    Route { target: usize, delay: Nanos },
    /// Admission shed it. The cheap failure, and it never touched a replica.
    Rejected,
    /// No usable replica at all.
    Dropped,
}

/// Admission, then routing, from the stale view only. Probes are the exception and are charged: one
/// modelled round trip each, on top of the flat `probe_live` charge a scenario can ask for.
#[allow(clippy::too_many_arguments)]
fn dispatch(
    router: &mut dyn RoutingPolicy,
    admission: &mut dyn AdmissionPolicy,
    views: &[ReplicaView],
    route_slot: &[usize],
    replicas: &[Replica],
    tenant_shares: &[f64],
    rng: &mut Rng,
    sc: &Scenario,
    now: Nanos,
    req: &Request,
    probed: Option<&mut Vec<usize>>,
    prefix: &dyn PrefixIndex,
) -> Dispatch {
    let request = request_view(req);
    if shed(admission, views, tenant_shares, now, &request) {
        return Dispatch::Rejected;
    }
    // A traced request also learns which replicas the policy paid to look at; the stale views it
    // read for free are not observable from here.
    let probed = RefCell::new(probed);
    // The router's indices are into `views`, the dense array of slots that are up; `route_slot`
    // turns each back into a slot, for the probe, the trace and the destination alike.
    let live = |i: usize| {
        if let Some(p) = probed.borrow_mut().as_mut() {
            p.push(route_slot[i]);
        }
        view_of(&replicas[route_slot[i]], now)
    };
    let mut ctx = RouteContext::new(now, views, &request, rng, &live, prefix);
    match router.choose(&mut ctx) {
        Some(target) => {
            let paid = ctx.probes() as Nanos + if sc.probe_live { 1 } else { 0 };
            Dispatch::Route { target: route_slot[target], delay: paid * PROBE_COST }
        }
        None => Dispatch::Dropped,
    }
}

/// The prefix index as the router sees it: holders named by their dense routing index rather than
/// their slot, and a holder that is not up (its cache went with it) left out.
struct RoutableIndex<'a> {
    inner: LeafPrefixIndex<'a>,
    pos: &'a [usize],
}

impl PrefixIndex for RoutableIndex<'_> {
    fn holders(&self, node: u64) -> Vec<(usize, u32)> {
        self.inner
            .holders(node)
            .into_iter()
            .filter_map(|(slot, hit)| (self.pos[slot] != NO_REPLICA).then_some((self.pos[slot], hit)))
            .collect()
    }
}

/// A session's follow-up turn: admission exactly as for any other request, then the replica holding
/// its context, unrouted and with nothing paid for a view. Admission may shed a turn; routing may not
/// move it, because the context it reuses is resident on that replica and nowhere else.
fn dispatch_pinned(
    admission: &mut dyn AdmissionPolicy,
    views: &[ReplicaView],
    tenant_shares: &[f64],
    now: Nanos,
    req: &Request,
    target: usize,
) -> Dispatch {
    if shed(admission, views, tenant_shares, now, &request_view(req)) {
        Dispatch::Rejected
    } else {
        Dispatch::Route { target, delay: 0 }
    }
}

fn request_view(req: &Request) -> RequestView {
    RequestView {
        id: req.id,
        prompt_tokens: req.prompt,
        arrived_at: req.arrived_at,
        deadline: req.deadline,
        tenant: req.tenant,
        attempts: req.attempts,
        prefix_node: req.prefix_node,
        prefix_tokens: req.prefix_tokens,
    }
}

/// The admission step alone, from the stale view: true when the policy shed the request.
fn shed(
    admission: &mut dyn AdmissionPolicy,
    views: &[ReplicaView],
    tenant_shares: &[f64],
    now: Nanos,
    request: &RequestView,
) -> bool {
    let actx = AdmissionContext { now, views, request, tenant_weights: tenant_shares };
    admission.admit(&actx) == Admission::Reject
}

/// Apply a dispatch: schedule the admission at the replica, or record the shed.
fn place(
    q: &mut EventQueue<Ev>,
    records: &mut Vec<RequestRecord>,
    outcomes: &mut HashMap<&'static str, u64>,
    done: &mut HashMap<u64, bool>,
    window: &mut Window,
    d: Dispatch,
    req: Request,
    now: Nanos,
) {
    match d {
        Dispatch::Route { target, delay } => q.schedule(now + delay, Ev::Admit(target, req)),
        Dispatch::Rejected => finish(
            records, outcomes, done, window, Outcome::Rejected, &req, now, NO_REPLICA, 0, 0, 0, 0,
        ),
        Dispatch::Dropped => {}
    }
}

/// The replica index recorded for a request that admission shed before routing.
pub const NO_REPLICA: usize = usize::MAX;

/// What the loop knows about a traced attempt before and beside the replica: the gateway and router
/// spans, and when it joined the replica's queue. The replica records the rest.
struct Draft {
    tenant: u32,
    attempt_at: Nanos,
    routed_until: Nanos,
    candidates: Vec<u64>,
    stale_view_age: Nanos,
    /// Requests placed and not yet finished when this one arrived: the gateway's in-flight count.
    in_flight: u32,
    enqueued_at: Nanos,
    queue_ahead: u32,
}

/// The trace sample: its own stream, the journeys in flight, and what the sampler retained.
///
/// Held apart from the rest of `Sim` so the loop can call it while it holds the scenario borrowed:
/// every method takes the other fields it needs explicitly.
struct Tracing {
    /// Tracing draws from its own stream, so turning it on moves nothing else; the sampler at
    /// completion then decides which of the drawn journeys are retained.
    rng: Rng,
    rate: f64,
    sampler: TraceSampler,
    /// Journeys in flight, by attempt id: what the loop saw before the replica took over.
    drafts: HashMap<u64, Draft>,
    traces: Vec<RequestTrace>,
    kv_capacity: u64,
}

impl Tracing {
    fn new(streams: &Streams, sc: &Scenario) -> Tracing {
        Tracing {
            rng: streams.stream("trace"),
            rate: sc.trace_sample_rate,
            sampler: TraceSampler::new(streams.stream("trace_keep"), sc.trace_sample_rate, TRACE_PER_BUCKET, TRACE_PER_OUTCOME),
            drafts: HashMap::new(),
            traces: Vec::new(),
            kv_capacity: sc.kv_capacity_tokens as u64,
        }
    }

    /// Whether this attempt's journey is recorded. Nothing else reads the `trace` stream, which is
    /// why a run is byte-identical with tracing on or off; at rate zero it is not even touched.
    fn sample(&mut self) -> bool {
        self.rate > 0.0 && self.rng.f64() < self.rate
    }

    /// Open the journey of a traced attempt at the moment the router has decided. `in_flight` is
    /// the gateway's count of placed, unfinished requests.
    fn draft(&mut self, req: &Request, now: Nanos, d: &Dispatch, probed: Vec<usize>, views: &[ReplicaView], start: Nanos, in_flight: usize) {
        let (routed_until, target) = match d {
            Dispatch::Route { target, delay } => (now + delay, Some(*target)),
            _ => (now, None),
        };
        let mut candidates: Vec<u64> = probed.into_iter().map(|i| i as u64).collect();
        if let Some(t) = target {
            if !candidates.contains(&(t as u64)) {
                candidates.push(t as u64);
            }
        }
        // A view never delivered is as old as the run.
        let stale_view_age = target.map_or(0, |t| match views[t].sampled_at {
            0 => now.saturating_sub(start),
            at => now.saturating_sub(at),
        });
        self.drafts.insert(
            req.id,
            Draft {
                tenant: req.tenant,
                attempt_at: now,
                routed_until,
                candidates,
                stale_view_age,
                in_flight: in_flight as u32,
                enqueued_at: 0,
                queue_ahead: 0,
            },
        );
    }

    /// Close the journey of `id`, whose record was just pushed, and let the sampler decide whether it
    /// is retained. Called after every `finish`; a request that was never drawn returns at once.
    fn settle(&mut self, id: u64, records: &[RequestRecord], replicas: &mut [Replica], replica: usize, cost: &sim_physics::CostModel) {
        let Some(draft) = self.drafts.remove(&id) else { return };
        let rec = match records.last() {
            Some(r) if r.id == id => r.clone(),
            _ => return,
        };
        // Always read the replica back, kept or not, so it stops recording for this id.
        let events = match replicas.get_mut(replica) {
            Some(r) => r.tracer_mut().take(id),
            None => Vec::new(),
        };
        let Some(bucket) = self.sampler.keep(&rec) else { return };
        let spans = spans_of(&draft, &rec, &events, replica, cost, self.kv_capacity);
        self.traces.push(RequestTrace { record: rec, tenant_id: draft.tenant as u64, bucket, spans });
    }
}

/// The journey as spans: gateway queue, routing decision, then whatever the replica recorded. A
/// request that timed out in the queue never reached a step, so its replica-queue span is closed by
/// the record instead.
fn spans_of(
    d: &Draft,
    rec: &RequestRecord,
    events: &[(StepEvent, ResourceSnapshot)],
    replica: usize,
    cost: &sim_physics::CostModel,
    kv_capacity: u64,
) -> Vec<TraceSpan> {
    let mut spans = Vec::with_capacity(events.len() + 3);
    let loop_state = ResourceState { running: d.in_flight, ..Default::default() };
    spans.push(TraceSpan {
        start_unix_ns: d.attempt_at,
        end_unix_ns: d.attempt_at,
        kind: SpanKind::IngressQueue,
        replica_id: 0,
        resource: loop_state,
        kv_tier: MemoryTier::None,
    });
    spans.push(TraceSpan {
        start_unix_ns: d.attempt_at,
        end_unix_ns: d.routed_until,
        kind: SpanKind::RoutingDecision { candidates: d.candidates.clone(), stale_view_age: d.stale_view_age },
        replica_id: 0,
        resource: loop_state,
        kv_tier: MemoryTier::None,
    });
    let replica_id = replica as u64;
    let mut admitted = false;
    for (ev, snap) in events {
        let (start, end, kind) = match *ev {
            StepEvent::Admitted { at, .. } => {
                admitted = true;
                (d.enqueued_at, at, SpanKind::ReplicaQueue)
            }
            StepEvent::PrefillChunk { tokens, start, end, .. } => (start, end, SpanKind::PrefillChunk { tokens }),
            StepEvent::DecodeStep { start, end, .. } => (start, end, SpanKind::DecodeStep),
            StepEvent::Retired { .. } => continue,
        };
        spans.push(TraceSpan {
            start_unix_ns: start,
            end_unix_ns: end,
            kind,
            replica_id,
            resource: resource_of(snap, cost, kv_capacity),
            kv_tier: MemoryTier::Hbm,
        });
    }
    if d.enqueued_at > 0 && !admitted {
        spans.push(TraceSpan {
            start_unix_ns: d.enqueued_at,
            end_unix_ns: rec.finished_at,
            kind: SpanKind::ReplicaQueue,
            replica_id,
            resource: ResourceState { queued: d.queue_ahead, kv_capacity, ..Default::default() },
            kv_tier: MemoryTier::None,
        });
    }
    spans
}

/// The step's conditions in the wire's terms. The step is compute-bound when its prefill term
/// outweighs its weight-and-cache read, which is the comparison the cost model makes implicitly.
fn resource_of(snap: &ResourceSnapshot, cost: &sim_physics::CostModel, kv_capacity: u64) -> ResourceState {
    let prefill_term = cost.step_ns(0, 0, snap.prefill_tokens);
    let bandwidth_term = cost.step_ns(0, snap.kv_tokens, 0);
    ResourceState {
        batch_size: snap.batch_size,
        running: snap.running,
        queued: snap.queued,
        kv_tokens_resident: snap.kv_tokens,
        kv_capacity,
        step_ns: snap.step_ns,
        bound: if prefill_term > bandwidth_term { BandwidthOrCompute::Compute } else { BandwidthOrCompute::Bandwidth },
    }
}

#[allow(clippy::too_many_arguments)]
fn finish(
    records: &mut Vec<RequestRecord>,
    outcomes: &mut HashMap<&'static str, u64>,
    done: &mut HashMap<u64, bool>,
    window: &mut Window,
    outcome: Outcome,
    req: &Request,
    now: Nanos,
    replica: usize,
    admitted_at: Nanos,
    first_token_at: Nanos,
    max_itl: Nanos,
    mean_itl: Nanos,
) {
    done.insert(req.id, true);
    *outcomes.entry(outcome.label()).or_insert(0) += 1;
    let rec = RequestRecord {
        id: req.id,
        arrived_at: req.arrived_at,
        admitted_at: if admitted_at == 0 { now } else { admitted_at },
        first_token_at,
        finished_at: now,
        prompt_tokens: req.prompt,
        output_tokens: if outcome.is_success() { req.output } else { 0 },
        replica,
        attempts: req.attempts,
        outcome,
        max_itl,
        mean_itl,
        class: req.class,
    };
    window.record(&rec);
    records.push(rec);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A small, deterministic fleet: only the structural property under test matters, not the
    /// statistics, so there is no reason for this to be bigger than it is.
    fn fixture() -> Scenario {
        let mut s = Scenario::default();
        s.name = "test_drain".into();
        s.seed = 42;
        s.replicas = 8;
        s.max_batch = 16;
        s.duration_s = 30.0;
        s.warmup_s = 3.0;
        s.client_timeout_s = 8.0;
        s.routing = "p2c".into();
        s.arrival_rps = 0.5 * s.rated_rps();
        s
    }

    /// `drain_frames` must hand out exactly the frames `frames()` would have shown, and nothing
    /// more or less, whatever the chunk boundaries: concatenating three drains equals one
    /// undrained run's final `frames()`, and each drain leaves the buffer empty behind it.
    #[test]
    fn draining_yields_the_same_frames_as_not_draining() {
        let sc = fixture();
        let mut whole = Sim::new(&sc).unwrap();
        let mut drained = Sim::new(&sc).unwrap();

        let end = whole.end();
        let chunks = [end / 3, 2 * end / 3, end];

        let mut collected = Vec::new();
        for &t in &chunks {
            whole.advance_to(t).unwrap();
            drained.advance_to(t).unwrap();
            collected.extend(drained.drain_frames());
            assert!(drained.frames().is_empty(), "drain_frames must empty the buffer behind it");
        }

        assert_eq!(collected, whole.frames());
        assert!(!collected.is_empty(), "the fixture must produce at least one sample");
    }
}
