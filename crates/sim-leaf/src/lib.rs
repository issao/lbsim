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
use sim_policy::{Admission, AdmissionContext, AdmissionPolicy, ReplicaView, RequestView, RouteContext, RoutingPolicy};
use sim_core::queue::{EventQueue, PRIO_OBSERVE};
use sim_core::rng::{Rng, Streams};
use sim_model::trace::{ResourceSnapshot, StepEvent};
use sim_model::Replica;
use sim_scenario::{FailureEvent, FailureKind, OverrideKind, Scenario};
use sim_workload::{Request, Workload};
use sim_core::{Nanos, EPOCH_BASE, MILLI};
use std::cell::RefCell;
use std::collections::HashMap;

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
            let vals: Vec<f64> = self.replica_load.iter().map(|r| r.v[s]).collect();
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
/// The fix for that bug is in. These caps are the second line of defence, because the next bug of
/// this shape should cost one confusing error message rather than a machine.
const MAX_EVENTS: u64 = 50_000_000;
const MAX_IN_FLIGHT: usize = 2_000_000;
const MAX_RECORDS: usize = 20_000_000;
const MAX_QUEUE_LEN: usize = 5_000_000;

/// The peak offered rate a scenario implies, once a configured load step is in effect. Shared by
/// `validate`'s record-count estimate and `event_ceiling` below, so the two stay consistent.
fn peak_rps(sc: &Scenario) -> f64 {
    sc.arrival_rps * sc.load_step_factor.max(1.0)
}

/// `MAX_EVENTS` is a floor, not the number actually enforced: a large, legitimate run can
/// dispatch far more than 50M events without anything having gone wrong, since the retry-fork
/// bug this ceiling exists to catch is a runaway *rate*, not a runaway total. `event_ceiling`
/// scales the tripwire with the run instead of holding it fixed.
///
/// Coefficients are from a release-build, one-core measurement at `route_p2c`, ~0.3 offered/
/// capacity: 1,000 replicas @ 2,500 rps ran 120 sim-s in 2.1 s wall at 9.0M events (~30 events per
/// completed request); 10,000 replicas @ 25,000 rps ran 60 sim-s in 38.8 s at 1.14M events/s,
/// i.e. ~114 step events per replica-second. 40 events/request and 200 step-events/replica-second
/// (sized to a 5 ms step) give both headroom.
pub fn event_ceiling(sc: &Scenario) -> u64 {
    let by_requests = peak_rps(sc) * sc.duration_s * 40.0;
    let by_steps = sc.replicas as f64 * sc.duration_s * 200.0;
    (MAX_EVENTS as f64).max(by_requests + by_steps) as u64
}

/// Guard resident memory directly: the ceilings above bound the state this simulator tracks, not
/// memory a future bug of a different shape allocates outside it. Checked only every 1M
/// dispatched events, so the cost of the guard itself stays negligible.
#[cfg(target_os = "linux")]
fn rss_over_guard(dispatched: u64) -> Option<String> {
    const GUARD_BYTES: u64 = 8 * 1024 * 1024 * 1024;
    if dispatched == 0 || dispatched % 1_000_000 != 0 {
        return None;
    }
    let statm = std::fs::read_to_string("/proc/self/statm").ok()?;
    let pages: u64 = statm.split_whitespace().nth(1)?.parse().ok()?;
    let bytes = pages * 4096;
    (bytes > GUARD_BYTES).then(|| {
        format!(
            "memory ceiling: {bytes} bytes resident after {dispatched} events, above the 8 GiB \
             guard. Shorten the run, lower the rate, or raise the guard deliberately"
        )
    })
}
#[cfg(not(target_os = "linux"))]
fn rss_over_guard(_dispatched: u64) -> Option<String> {
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
    if expected_arrivals > MAX_RECORDS as f64 {
        bad.push(format!(
            "this scenario implies about {:.0} requests ({:.0} rps x {:.0} s x {} attempts), \
             above the {} record ceiling. Shorten the run, lower the rate, or raise MAX_RECORDS \
             deliberately",
            expected_arrivals, peak, sc.duration_s, sc.max_attempts, MAX_RECORDS
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
    /// Each replica's step clock as of the previous close, so a frame carries the window's share
    /// of busy time and not the run's. Indexed by position and grown with zeros, so a fleet that
    /// changes size mid-run reads as new replicas that were idle until now.
    prev_busy: Vec<Nanos>,
    prev_compute: Vec<Nanos>,
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
    fn close(&mut self, t: Nanos, offered_rps: f64, replicas: &[Replica]) -> Frame {
        let mut w = std::mem::take(self);
        w.prev_busy.resize(replicas.len(), 0);
        w.prev_compute.resize(replicas.len(), 0);
        let samples: Vec<ReplicaSample> = replicas
            .iter()
            .zip(w.prev_busy.iter_mut().zip(w.prev_compute.iter_mut()))
            .map(|(r, (prev_busy, prev_compute))| {
                let busy = r.busy_ns_through(t);
                let compute = r.compute_ns_through(t);
                let sample = ReplicaSample {
                    queued: r.queued() as u32,
                    running: r.running() as u32,
                    kv_tokens: r.kv_tokens(),
                    last_step_ns: r.last_step_ns(),
                    busy_ns: busy - *prev_busy,
                    compute_ns: compute - *prev_compute,
                };
                *prev_busy = busy;
                *prev_compute = compute;
                sample
            })
            .collect();
        // The clocks outlive the window they were read in.
        self.prev_busy = w.prev_busy;
        self.prev_compute = w.prev_compute;
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
pub struct Sim {
    sc: Scenario,
    router: Box<dyn RoutingPolicy>,
    admission: Box<dyn AdmissionPolicy>,
    tenant_shares: Vec<f64>,
    route_rng: Rng,
    /// Its own stream, so turning sessions on cannot perturb arrivals, shapes or routing.
    session_rng: Rng,
    sessions_spawned: u64,
    workload: Workload,

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

    /// This run's event ceiling, computed from `sc` by `event_ceiling` in `Sim::new` rather than
    /// held as a constant. See `event_ceiling`'s doc comment for the formula.
    event_ceiling: u64,
    /// Set by a tripwire; the run is over and `into_result` reports why.
    tripped: Option<String>,
    /// Set once the loop would have exited: an event past the end, an empty queue, or a trip.
    finished: bool,
}

/// Ids for session turns, above the first-attempt and retry ranges.
const SESSION_ID_BASE: u64 = 1 << 40;

/// Perhaps schedule the session's next turn after a completed one. The number of turns is geometric
/// with mean `session_turns_mean`, the next turn carries the whole context so far plus a short new
/// prompt, and it goes back to the replica that holds that context, which parks it in the meantime.
/// The turn arrives at the gateway like any other request and admission may shed it, but routing
/// may not move it: the point of keeping context resident is lost on any other replica, and a router
/// that knew that would do the same.
fn follow_up(
    sc: &Scenario,
    rng: &mut Rng,
    spawned: &mut u64,
    i: usize,
    replica: &mut Replica,
    q: &mut EventQueue<Ev>,
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
        let router = sim_policy::make_routing(sc)?;
        let admission = sim_policy::make_admission(sc)?;
        let tenant_shares = sc.tenant_shares();
        let streams = Streams::new(sc.seed);
        let route_rng: Rng = streams.stream("route");
        let session_rng: Rng = streams.stream("session");
        let workload = Workload::new(&streams);
        let tracing = Tracing::new(&streams, sc);

        let start = EPOCH_BASE;
        let end = start + (sc.duration_s * 1e9) as Nanos;
        let measured_from = start + (sc.warmup_s * 1e9) as Nanos;

        let mut q: EventQueue<Ev> = EventQueue::new(start);
        let replicas: Vec<Replica> = (0..sc.replicas).map(|_| Replica::default()).collect();
        let views: Vec<ReplicaView> = vec![ReplicaView::default(); sc.replicas];

        let cost = sc.cost_model();
        let sample_iv = (sc.sample_interval_ms * 1e6) as Nanos;
        let tele_iv = (sc.telemetry_interval_ms * 1e6) as Nanos;
        let tele_delay = (sc.telemetry_delay_ms * 1e6) as Nanos;

        let replica_load: Vec<Series> = (0..sc.replicas)
            .map(|i| Series::new(&format!("replica_{}", i)))
            .collect();

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
            tenant_shares,
            route_rng,
            session_rng,
            sessions_spawned: 0,
            workload,
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
            event_ceiling: event_ceiling(sc),
            tripped: None,
            finished: false,
        })
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
                &mut *self.router, &mut *self.admission, &self.views, &self.replicas,
                &self.tenant_shares, &mut self.route_rng, sc, at, &again, traced.then_some(&mut probed),
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
    /// The frames closed so far. Available while the run is in progress, which is the point.
    pub fn frames(&self) -> &[Frame] {
        &self.frames
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
            self.router = router;
            self.admission = admission;
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
            if self.q.dispatched > self.event_ceiling {
                self.tripped = Some(format!(
                    "event ceiling: {} events dispatched against a ceiling of {} with {:.0}% of the \
                     run remaining. Something is scheduling work faster than it retires; suspect a \
                     feedback loop in the arrival or retry path. Shorten the run, lower the rate, or \
                     raise MAX_EVENTS deliberately",
                    self.q.dispatched,
                    self.event_ceiling,
                    100.0 * (end.saturating_sub(now)) as f64 / (end - start).max(1) as f64
                ));
                self.finished = true;
                break;
            }
            if let Some(msg) = rss_over_guard(self.q.dispatched) {
                self.tripped = Some(msg);
                self.finished = true;
                break;
            }
            if self.records.len() > MAX_RECORDS || self.placed.len() > MAX_IN_FLIGHT {
                self.tripped = Some(format!(
                    "state ceiling: {} records and {} tracked requests. Offered load is far above what \
                     this fleet retires, or requests are never completing",
                    self.records.len(),
                    self.placed.len()
                ));
                self.finished = true;
                break;
            }
            if self.q.len() > MAX_QUEUE_LEN {
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
                    let req = self.workload.make(sc, now);
                    self.first_attempts += 1;
                    let traced = self.tracing.sample();
                    let mut probed = Vec::new();
                    let d = dispatch(
                        &mut *self.router, &mut *self.admission, &self.views, &self.replicas,
                        &self.tenant_shares, &mut self.route_rng, sc, now, &req, traced.then_some(&mut probed),
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
                        &mut *self.admission, &self.views, &self.tenant_shares, now, &req, i,
                    );
                    if matches!(d, Dispatch::Rejected) {
                        // The turn that would have reused the parked context is not coming.
                        self.replicas[i].remove(req.id);
                    }
                    place(
                        &mut self.q, &mut self.records, &mut self.outcomes, &mut self.done,
                        &mut self.window, d, req, now,
                    );
                }

                Ev::Admit(target, req) => {
                    if self.replicas[target].is_down() {
                        // Sent here on a stale view: the router will not learn of the crash until the
                        // next telemetry delivery. Refused before any device time, and retried.
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
                    let Some(out) = self.replicas[i].step(sc, &self.cost, now) else { continue };
                    let token_at = out.token_at;
                    for s in out.finished {
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
                            &mut self.replicas[i], &mut self.q, &s.req, token_at,
                        );
                    }
                    self.window.preemptions += out.preempted as u64;

                    if !out.idle {
                        self.q.schedule(token_at, Ev::Step(i));
                    }
                    self.fingerprint = self
                        .fingerprint
                        .wrapping_mul(0x100_0000_01b3)
                        .wrapping_add(out.step_ns ^ (i as u64));
                }

                Ev::TelemetryPublish(i) => {
                    let view = view_of(&self.replicas[i], now);
                    // Delayed delivery. This one line is the whole staleness mechanism: a policy cannot
                    // see the fleet as it is, only as it was.
                    self.q.schedule(now + self.tele_delay, Ev::TelemetryDeliver(i, view));
                    self.q.schedule(now + self.tele_iv, Ev::TelemetryPublish(i));
                }

                Ev::TelemetryDeliver(i, view) => {
                    self.views[i] = view;
                }

                Ev::Timeout(id) => {
                    if self.done.contains_key(&id) {
                        continue;
                    }
                    let Some(&i) = self.placed.get(&id) else { continue };
                    if let Some((req, was_running)) = self.replicas[i].remove(id) {
                        let outcome = if was_running {
                            Outcome::TimeoutRunning
                        } else {
                            Outcome::TimeoutQueued
                        };
                        self.abort(&req, i, outcome, now);
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
                    self.fleet_kv.push(now, 100.0 * tkv / sc.replicas as f64);
                    let window_end_ns = now - start;
                    let window_start_ns = window_end_ns.saturating_sub(self.sample_iv);
                    let rate = self.workload.offered_rps(sc, window_start_ns, window_end_ns);
                    self.offered.push(now, rate);
                    self.frames.push(self.window.close(now, rate, &self.replicas));
                    self.q.schedule_prio(now + self.sample_iv, PRIO_OBSERVE, Ev::Sample);
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
        // growing `last_step_ns` is the one tell a policy has.
        ejected: r.is_down(),
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
    replicas: &[Replica],
    tenant_shares: &[f64],
    rng: &mut Rng,
    sc: &Scenario,
    now: Nanos,
    req: &Request,
    probed: Option<&mut Vec<usize>>,
) -> Dispatch {
    let request = request_view(req);
    if shed(admission, views, tenant_shares, now, &request) {
        return Dispatch::Rejected;
    }
    // A traced request also learns which replicas the policy paid to look at; the stale views it
    // read for free are not observable from here.
    let probed = RefCell::new(probed);
    let live = |i: usize| {
        if let Some(p) = probed.borrow_mut().as_mut() {
            p.push(i);
        }
        view_of(&replicas[i], now)
    };
    let mut ctx = RouteContext::new(now, views, &request, rng, &live);
    match router.choose(&mut ctx) {
        Some(target) => {
            let paid = ctx.probes() as Nanos + if sc.probe_live { 1 } else { 0 };
            Dispatch::Route { target, delay: paid * PROBE_COST }
        }
        None => Dispatch::Dropped,
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
