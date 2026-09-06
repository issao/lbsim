//! The simulation loop.
//!
//! One replica step is one engine iteration, as in continuous batching: admit from the queue if
//! there is room, do a bounded amount of prefill work, emit one token for every decoding sequence,
//! retire whatever finished. Batch composition therefore changes constantly.
//!
//! **Deviation worth naming.** `docs/ARCHITECTURE.md` section 3 advances a replica by solving a
//! closed form over a whole epoch rather than stepping. That matters when long decode runs dominate,
//! and it needs the KV growth term, which today's scope excludes. The result itself is not in doubt:
//! `bench/validate_epochs.py` proves the closed form exactly equivalent to per-step iteration,
//! including the compute branch and speculation. Stepping here is the simple thing that is correct
//! at today's scale, and the epoch advance lands with the KV model.

use crate::metrics::{Histogram, Outcome, RequestRecord, Series};
use crate::policy::{ReplicaView, Routing};
use crate::queue::{EventQueue, PRIO_OBSERVE};
use crate::rng::{Rng, Streams};
use crate::scenario::Scenario;
use crate::workload::{Request, Workload};
use crate::{Nanos, EPOCH_BASE, MILLI};
use std::collections::{HashMap, VecDeque};

/// A modelled router-to-replica round trip, paid when a policy probes for fresh state instead of
/// reading the delayed snapshot.
const PROBE_COST: Nanos = MILLI;

struct Seq {
    req: Request,
    prefill_left: u32,
    output_left: u32,
    admitted_at: Nanos,
    first_token_at: Nanos,
    last_token_at: Nanos,
    max_itl: Nanos,
    itl_sum: Nanos,
    itl_count: u32,
}

#[derive(Default)]
struct Replica {
    queue: VecDeque<Request>,
    running: Vec<Seq>,
    queued_tokens: u64,
    /// Resident key-value tokens: for every running sequence, its prompt plus what it has generated.
    /// This is the real capacity constraint, and it is denominated in tokens rather than requests.
    kv_tokens: u64,
    next_step_at: Nanos,
    scheduled: bool,
    last_step_ns: Nanos,
    completed: u64,
}

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
    pub fn slo_attainment(&self) -> f64 {
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

pub fn run(sc: &Scenario) -> Result<RunResult, String> {
    let routing = Routing::parse(&sc.routing, sc.p2c_choices)?;
    let streams = Streams::new(sc.seed);
    let mut route_rng: Rng = streams.stream("route");
    let mut workload = Workload::new(&streams);

    let start = EPOCH_BASE;
    let end = start + (sc.duration_s * 1e9) as Nanos;
    let measured_from = start + (sc.warmup_s * 1e9) as Nanos;

    let mut q: EventQueue<Ev> = EventQueue::new(start);
    let mut replicas: Vec<Replica> = (0..sc.replicas).map(|_| Replica::default()).collect();
    let mut views: Vec<ReplicaView> = vec![ReplicaView::default(); sc.replicas];
    let mut placed: HashMap<u64, usize> = HashMap::new();
    let mut done: HashMap<u64, bool> = HashMap::new();
    let mut records: Vec<RequestRecord> = Vec::new();
    let mut rr_cursor = 0usize;

    let step_budget = sc.step_token_budget;
    let prefill_rate = sc.prefill_tokens_per_s;
    let sample_iv = (sc.sample_interval_ms * 1e6) as Nanos;
    let tele_iv = (sc.telemetry_interval_ms * 1e6) as Nanos;
    let tele_delay = (sc.telemetry_delay_ms * 1e6) as Nanos;

    let mut replica_load: Vec<Series> = (0..sc.replicas)
        .map(|i| Series::new(&format!("replica_{}", i)))
        .collect();
    let mut fleet_queue = Series::new("fleet_queue");
    let mut fleet_running = Series::new("fleet_running");
    let mut fleet_kv = Series::new("fleet_kv_utilization");
    let mut offered = Series::new("offered_rps");

    let mut ttft = Histogram::new();
    let mut itl_max = Histogram::new();
    let mut e2e = Histogram::new();
    let mut queue_wait = Histogram::new();
    let mut outcomes: HashMap<&'static str, u64> = HashMap::new();
    let mut fingerprint: u64 = 0;
    let mut retries: u64 = 0;
    let mut first_attempts: u64 = 0;

    q.schedule(start, Ev::Arrival);
    q.schedule_prio(start + sample_iv, PRIO_OBSERVE, Ev::Sample);
    for i in 0..sc.replicas {
        q.schedule(start + (i as Nanos * tele_iv) / sc.replicas.max(1) as Nanos, Ev::TelemetryPublish(i));
    }

    while let Some((now, ev)) = q.pop() {
        if now > end {
            break;
        }
        match ev {
            Ev::Arrival => {
                let elapsed = (now - start) as f64 / 1e9;
                let req = workload.make(sc, now);
                first_attempts += 1;
                dispatch(
                    &mut q, &routing, &views, &mut rr_cursor, &mut route_rng, sc, now, req,
                );
                let gap = workload.next_gap_ns(sc, elapsed);
                q.schedule(now + gap.max(1), Ev::Arrival);
            }

            Ev::Admit(target, req) => {
                let r = &mut replicas[target];
                if r.queue.len() >= sc.max_queue {
                    // Shed before consuming device time: the cheap failure, and deliberately
                    // distinct in the outcome from one that fails after burning work.
                    finish(
                        &mut records, &mut outcomes, &mut done, Outcome::Rejected, &req, now,
                        target, 0, 0, 0, 0,
                    );
                    continue;
                }
                placed.insert(req.id, target);
                q.schedule(req.deadline, Ev::Timeout(req.id));
                r.queued_tokens += req.prompt as u64;
                r.queue.push_back(req);
                if !r.scheduled {
                    r.scheduled = true;
                    r.next_step_at = now;
                    q.schedule(now, Ev::Step(target));
                }
            }

            Ev::Step(i) => {
                let r = &mut replicas[i];
                // Admit while there is room. The replica is itself a scheduler, so this is the second
                // scheduling layer and it can disagree with the router.
                //
                // Two limits, and which one binds is the point: a sequence-count cap, and the
                // key-value token budget. A 24,000-token prompt consumes what eight chat turns
                // consume, so a queue of long requests blocks admission that a request count would
                // have allowed. Nothing here preempts; a blocked request waits, which is what
                // produces the queueing this scenario is about.
                let kv_cap = sc.kv_capacity_tokens as u64;
                while r.running.len() < sc.max_batch {
                    let next_cost = match r.queue.front() {
                        Some(req) => req.prompt as u64,
                        None => break,
                    };
                    if r.kv_tokens + next_cost > kv_cap && !r.running.is_empty() {
                        break;
                    }
                    match r.queue.pop_front() {
                        Some(req) => {
                            r.queued_tokens = r.queued_tokens.saturating_sub(req.prompt as u64);
                            r.kv_tokens += req.prompt as u64;
                            r.running.push(Seq {
                                prefill_left: req.prompt,
                                output_left: req.output,
                                admitted_at: now,
                                first_token_at: 0,
                                last_token_at: 0,
                                max_itl: 0,
                                itl_sum: 0,
                                itl_count: 0,
                                req,
                            });
                        }
                        None => break,
                    }
                }
                if r.running.is_empty() {
                    r.scheduled = false;
                    continue;
                }

                // Chunked prefill: a bounded token budget per step, taken in admission order. This
                // is what stops one long prompt from inserting a multi-second stall into everyone
                // else's token stream.
                let mut budget = step_budget;
                let mut prefill_tokens = 0u32;
                for s in r.running.iter_mut() {
                    if budget == 0 {
                        break;
                    }
                    if s.prefill_left > 0 {
                        let take = s.prefill_left.min(budget);
                        s.prefill_left -= take;
                        budget -= take;
                        prefill_tokens += take;
                    }
                }
                let decoding = r.running.iter().filter(|s| s.prefill_left == 0).count();

                // Step time: fixed overhead, a marginal cost per decoding sequence, and the prefill
                // work done this step. Prefill and decode contend for one device, which is why the
                // terms add and why a big prefill shows up in everyone's inter-token latency.
                //
                // The bandwidth term is why step time grows as contexts lengthen rather than only as
                // the batch widens: the engine re-reads every resident key-value token every step.
                // At batch 256 and 4,000 tokens of context it is the dominant term, larger than the
                // weight read.
                let step_ns = (sc.step_base_ms * 1e6) as Nanos
                    + (sc.step_per_seq_ms * 1e6) as Nanos * decoding as Nanos
                    + ((sc.step_per_kv_ktoken_ms * r.kv_tokens as f64 / 1000.0) * 1e6) as Nanos
                    + ((prefill_tokens as f64 / prefill_rate) * 1e9) as Nanos;
                let step_ns = step_ns.max(1);
                let token_at = now + step_ns;
                r.last_step_ns = step_ns;

                let mut finished: Vec<usize> = Vec::new();
                for (idx, s) in r.running.iter_mut().enumerate() {
                    if s.prefill_left > 0 {
                        continue;
                    }
                    if s.first_token_at == 0 {
                        s.first_token_at = token_at;
                    } else {
                        let gap = token_at - s.last_token_at;
                        s.max_itl = s.max_itl.max(gap);
                        s.itl_sum += gap;
                        s.itl_count += 1;
                    }
                    s.last_token_at = token_at;
                    s.output_left = s.output_left.saturating_sub(1);
                    if s.output_left == 0 {
                        finished.push(idx);
                    }
                }
                r.kv_tokens += decoding as u64;

                for idx in finished.iter().rev() {
                    let s = r.running.swap_remove(*idx);
                    r.kv_tokens = r
                        .kv_tokens
                        .saturating_sub(s.req.prompt as u64 + s.req.output as u64);
                    r.completed += 1;
                    let mean_itl = if s.itl_count > 0 { s.itl_sum / s.itl_count as Nanos } else { 0 };
                    let within = s.first_token_at - s.req.arrived_at
                        <= (sc.ttft_slo_ms * 1e6) as Nanos
                        && s.max_itl <= (sc.itl_slo_ms * 1e6) as Nanos
                        && token_at - s.req.arrived_at <= (sc.e2e_slo_s * 1e9) as Nanos;
                    let outcome = if within { Outcome::Ok } else { Outcome::OkSloViolated };
                    finish(
                        &mut records, &mut outcomes, &mut done, outcome, &s.req, token_at, i,
                        s.admitted_at, s.first_token_at, s.max_itl, mean_itl,
                    );
                }

                if r.running.is_empty() && r.queue.is_empty() {
                    r.scheduled = false;
                } else {
                    r.next_step_at = token_at;
                    q.schedule(token_at, Ev::Step(i));
                }
                fingerprint = fingerprint
                    .wrapping_mul(0x100_0000_01b3)
                    .wrapping_add(step_ns ^ (i as u64));
            }

            Ev::TelemetryPublish(i) => {
                let r = &replicas[i];
                let view = ReplicaView {
                    sampled_at: now,
                    queued: r.queue.len() as u32,
                    running: r.running.len() as u32,
                    queued_tokens: r.queued_tokens
                        + r.running.iter().map(|s| s.prefill_left as u64 + s.output_left as u64).sum::<u64>(),
                    kv_tokens: r.kv_tokens,
                    last_step_ns: r.last_step_ns,
                    ejected: false,
                };
                // Delayed delivery. This one line is the whole staleness mechanism: a policy cannot
                // see the fleet as it is, only as it was.
                q.schedule(now + tele_delay, Ev::TelemetryDeliver(i, view));
                q.schedule(now + tele_iv, Ev::TelemetryPublish(i));
            }

            Ev::TelemetryDeliver(i, view) => {
                views[i] = view;
            }

            Ev::Timeout(id) => {
                if done.contains_key(&id) {
                    continue;
                }
                let Some(&i) = placed.get(&id) else { continue };
                let r = &mut replicas[i];
                // Where it died decides how expensive the failure was.
                let mut victim: Option<(Request, bool)> = None;
                if let Some(pos) = r.queue.iter().position(|x| x.id == id) {
                    let req = r.queue.remove(pos).unwrap();
                    r.queued_tokens = r.queued_tokens.saturating_sub(req.prompt as u64);
                    victim = Some((req, false));
                } else if let Some(pos) = r.running.iter().position(|s| s.req.id == id) {
                    let s = r.running.swap_remove(pos);
                    let generated = s.req.output.saturating_sub(s.output_left) as u64;
                    r.kv_tokens = r.kv_tokens.saturating_sub(s.req.prompt as u64 + generated);
                    victim = Some((s.req, true));
                }
                if let Some((req, was_running)) = victim {
                    let outcome = if was_running {
                        Outcome::TimeoutRunning
                    } else {
                        Outcome::TimeoutQueued
                    };
                    finish(
                        &mut records, &mut outcomes, &mut done, outcome, &req, now, i, 0, 0, 0, 0,
                    );
                    // Retry, under a budget. Retries are what turn a slowdown into a collapse, and
                    // they cost far more here than in a stateless service because a timeout after
                    // thirty seconds has already burned thirty seconds of device work.
                    let budget_ok = retries as f64
                        <= sc.retry_budget_fraction * first_attempts.max(1) as f64;
                    if req.attempts < sc.max_attempts && budget_ok {
                        retries += 1;
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
                        dispatch(
                            &mut q, &routing, &views, &mut rr_cursor, &mut route_rng, sc, at, again,
                        );
                    }
                }
            }

            Ev::Sample => {
                let mut tq = 0.0;
                let mut tr = 0.0;
                let mut tkv = 0.0;
                for (i, r) in replicas.iter().enumerate() {
                    let load = (r.queue.len() + r.running.len()) as f64;
                    replica_load[i].push(now, load);
                    tq += r.queue.len() as f64;
                    tr += r.running.len() as f64;
                    tkv += r.kv_tokens as f64 / sc.kv_capacity_tokens;
                }
                fleet_queue.push(now, tq);
                fleet_running.push(now, tr);
                fleet_kv.push(now, 100.0 * tkv / sc.replicas as f64);
                offered.push(now, Workload::rate_at(sc, (now - start) as f64 / 1e9));
                q.schedule_prio(now + sample_iv, PRIO_OBSERVE, Ev::Sample);
            }
        }
    }

    // Statistics come from the measured window only, so a run measures steady state rather than the
    // transient of an empty fleet filling up.
    for rec in records.iter().filter(|r| r.arrived_at >= measured_from) {
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
    let measured: Vec<RequestRecord> = records
        .into_iter()
        .filter(|r| r.arrived_at >= measured_from)
        .collect();
    let mut measured_outcomes: HashMap<&'static str, u64> = HashMap::new();
    for r in &measured {
        *measured_outcomes.entry(r.outcome.label()).or_insert(0) += 1;
    }
    let _ = outcomes;

    Ok(RunResult {
        scenario: sc.clone(),
        routing_label: routing.label(),
        records: measured,
        ttft,
        itl_max,
        e2e,
        queue_wait,
        replica_load,
        fleet_queue,
        fleet_running,
        fleet_kv_utilization: fleet_kv,
        offered_rps: offered,
        outcomes: measured_outcomes,
        events: q.dispatched,
        fingerprint,
        measured_from,
        measured_to: end,
        rated_rps: sc.rated_rps(),
        replicas_inspected_per_decision: routing.inspected(sc.replicas),
        retries,
        first_attempts,
    })
}

#[allow(clippy::too_many_arguments)]
fn dispatch(
    q: &mut EventQueue<Ev>,
    routing: &Routing,
    views: &[ReplicaView],
    rr_cursor: &mut usize,
    rng: &mut Rng,
    sc: &Scenario,
    now: Nanos,
    req: Request,
) {
    match routing.choose(views, rr_cursor, rng) {
        Some(target) => {
            // A probe buys fresh state and costs a round trip, so the price of freshness is visible
            // rather than free.
            let delay = if sc.probe_live { PROBE_COST } else { 0 };
            q.schedule(now + delay, Ev::Admit(target, req));
        }
        None => {}
    }
}

#[allow(clippy::too_many_arguments)]
fn finish(
    records: &mut Vec<RequestRecord>,
    outcomes: &mut HashMap<&'static str, u64>,
    done: &mut HashMap<u64, bool>,
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
    records.push(RequestRecord {
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
    });
}
