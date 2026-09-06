//! The simulation loop.
//!
//! Arrivals, routing, admission at the replica, timeouts and retries, telemetry delay and sampling:
//! everything that happens *between* replicas. What happens *inside* a replica during one step is
//! `sim_model::Replica`'s, and this loop only decides when to call it and what to do with what it
//! retired. The split follows the Leaf seam in `docs/ARCHITECTURE.md` section 10.3, so the loop can
//! later be cut along it without touching the physics.

use sim_metrics::{Frame, Histogram, Outcome, ReplicaSample, RequestRecord, Series};
use sim_policy::{Admission, AdmissionContext, AdmissionPolicy, ReplicaView, RequestView, RouteContext, RoutingPolicy};
use sim_core::queue::{EventQueue, PRIO_OBSERVE};
use sim_core::rng::{Rng, Streams};
use sim_model::Replica;
use sim_scenario::Scenario;
use sim_workload::{Request, Workload};
use sim_core::{Nanos, EPOCH_BASE, MILLI};
use std::collections::HashMap;

/// A modelled router-to-replica round trip, paid when a policy probes for fresh state instead of
/// reading the delayed snapshot.
const PROBE_COST: Nanos = MILLI;

enum Ev {
    Arrival,
    Step(usize),
    TelemetryPublish(usize),
    TelemetryDeliver(usize, ReplicaView),
    Timeout(u64),
    Sample,
    Admit(usize, Request),
}

pub struct RunResult {
    pub scenario: Scenario,
    pub routing_label: String,
    pub records: Vec<RequestRecord>,
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
    if sc.max_attempts == 0 {
        bad.push("max_attempts must be at least 1".into());
    }
    if !(sc.load_step_factor.is_finite() && sc.load_step_factor >= 0.0 && sc.load_step_factor <= 1000.0) {
        bad.push(format!("load_step_factor = {} (need 0..=1000)", sc.load_step_factor));
    }

    // The estimate that would actually have caught the runaway: how much state this run implies.
    let peak_rps = sc.arrival_rps * sc.load_step_factor.max(1.0);
    let expected_arrivals = peak_rps * sc.duration_s * sc.max_attempts as f64;
    if expected_arrivals > MAX_RECORDS as f64 {
        bad.push(format!(
            "this scenario implies about {:.0} requests ({:.0} rps x {:.0} s x {} attempts), \
             above the {} record ceiling. Shorten the run, lower the rate, or raise MAX_RECORDS \
             deliberately",
            expected_arrivals, peak_rps, sc.duration_s, sc.max_attempts, MAX_RECORDS
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
        let w = std::mem::take(self);
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
            replicas: replicas
                .iter()
                .map(|r| ReplicaSample {
                    queued: r.queued() as u32,
                    running: r.running() as u32,
                    kv_tokens: r.kv_tokens(),
                    last_step_ns: r.last_step_ns(),
                })
                .collect(),
        }
    }
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

    cost: sim_physics::CostModel,
    sample_iv: Nanos,
    tele_iv: Nanos,
    tele_delay: Nanos,

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

    /// Set by a tripwire; the run is over and `into_result` reports why.
    tripped: Option<String>,
    /// Set once the loop would have exited: an event past the end, an empty queue, or a trip.
    finished: bool,
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
        let workload = Workload::new(&streams);

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

        Ok(Sim {
            sc: sc.clone(),
            router,
            admission,
            tenant_shares,
            route_rng,
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
            cost,
            sample_iv,
            tele_iv,
            tele_delay,
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
            tripped: None,
            finished: false,
        })
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
    /// Every replica as it is right now, not as the delayed telemetry shows it: live telemetry for a
    /// viewer, never for a policy.
    pub fn latest_views(&self) -> Vec<ReplicaView> {
        self.replicas.iter().map(|r| view_of(r, self.now)).collect()
    }

    /// Dispatch every event at or before `t`, in the order the run always dispatched them, and stop.
    ///
    /// An event beyond `t` is left in the queue untouched, so `now()` never passes `t`. The one
    /// exception is the end of the run: the first event past `end` is popped and discarded, as the
    /// loop always did, because the dispatched count that the report prints includes it.
    pub fn advance_to(&mut self, t: Nanos) -> Result<(), String> {
        let sc = &self.sc;
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
            if self.q.dispatched > MAX_EVENTS {
                self.tripped = Some(format!(
                    "event ceiling: {} events dispatched with {:.0}% of the run remaining. Something is \
                     scheduling work faster than it retires; suspect a feedback loop in the arrival or \
                     retry path",
                    self.q.dispatched,
                    100.0 * (end.saturating_sub(now)) as f64 / (end - start).max(1) as f64
                ));
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
            match ev {
                Ev::Arrival => {
                    let elapsed = (now - start) as f64 / 1e9;
                    let req = self.workload.make(sc, now);
                    self.first_attempts += 1;
                    let d = dispatch(
                        &mut *self.router, &mut *self.admission, &self.views, &self.replicas,
                        &self.tenant_shares, &mut self.route_rng, sc, now, &req,
                    );
                    place(
                        &mut self.q, &mut self.records, &mut self.outcomes, &mut self.done,
                        &mut self.window, d, req, now,
                    );
                    let gap = self.workload.next_gap_ns(sc, elapsed);
                    self.q.schedule(now + gap.max(1), Ev::Arrival);
                }

                Ev::Admit(target, req) => {
                    let r = &mut self.replicas[target];
                    let (id, deadline) = (req.id, req.deadline);
                    if let Err(req) = r.enqueue(req, sc.max_queue) {
                        // Shed before consuming device time: the cheap failure, and deliberately
                        // distinct in the outcome from one that fails after burning work.
                        finish(
                            &mut self.records, &mut self.outcomes, &mut self.done, &mut self.window,
                            Outcome::Rejected, &req, now, target, 0, 0, 0, 0,
                        );
                        continue;
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
                        let within = s.first_token_at - s.req.arrived_at
                            <= (sc.ttft_slo_ms * 1e6) as Nanos
                            && s.max_itl <= (sc.itl_slo_ms * 1e6) as Nanos
                            && token_at - s.req.arrived_at <= (sc.e2e_slo_s * 1e9) as Nanos;
                        let outcome = if within { Outcome::Ok } else { Outcome::OkSloViolated };
                        self.admission.on_complete(s.req.tenant, s.req.output, token_at);
                        finish(
                            &mut self.records, &mut self.outcomes, &mut self.done, &mut self.window,
                            outcome, &s.req, token_at, i, s.admitted_at, s.first_token_at, s.max_itl,
                            s.mean_itl,
                        );
                    }

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
                        finish(
                            &mut self.records, &mut self.outcomes, &mut self.done, &mut self.window,
                            outcome, &req, now, i, 0, 0, 0, 0,
                        );
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
                            // Re-routed rather than pinned, so a retry does not land on the same
                            // struggling replica by construction.
                            let d = dispatch(
                                &mut *self.router, &mut *self.admission, &self.views, &self.replicas,
                                &self.tenant_shares, &mut self.route_rng, sc, at, &again,
                            );
                            place(
                                &mut self.q, &mut self.records, &mut self.outcomes, &mut self.done,
                                &mut self.window, d, again, at,
                            );
                        }
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
                    let rate = Workload::rate_at(sc, (now - start) as f64 / 1e9);
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
            return Err(format!("run {:?} aborted, {}", sc.name, why));
        }
        Ok(())
    }

    /// The post-run aggregation. Callable before `end` for a partial result, which is the same
    /// computation over the records so far.
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

        Ok(RunResult {
            scenario: sc.clone(),
            routing_label: self.router.label(),
            records: measured,
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
        ejected: false,
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
) -> Dispatch {
    let request = RequestView {
        id: req.id,
        prompt_tokens: req.prompt,
        arrived_at: req.arrived_at,
        deadline: req.deadline,
        tenant: req.tenant,
        attempts: req.attempts,
    };
    let actx = AdmissionContext { now, views, request: &request, tenant_weights: tenant_shares };
    if admission.admit(&actx) == Admission::Reject {
        return Dispatch::Rejected;
    }
    let live = |i: usize| view_of(&replicas[i], now);
    let mut ctx = RouteContext::new(now, views, &request, rng, &live);
    match router.choose(&mut ctx) {
        Some(target) => {
            let paid = ctx.probes() as Nanos + if sc.probe_live { 1 } else { 0 };
            Dispatch::Route { target, delay: paid * PROBE_COST }
        }
        None => Dispatch::Dropped,
    }
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
    };
    window.record(&rec);
    records.push(rec);
}
