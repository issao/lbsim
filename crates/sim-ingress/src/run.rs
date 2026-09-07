//! The run registry: every live run, each driven by its own thread.
//!
//! One thread per run owns the `Sim`. Nothing else touches the engine: request handlers read and
//! write a `RunState` under a mutex and wake the thread through a condvar, and the thread copies
//! each closed frame into that state as it goes. So the engine advances on exactly one thread, in
//! exactly the order `sim_leaf::run` would have advanced it, and the only thing the wall clock
//! decides is *when* the next `advance_to` is called, never *what* it advances to. That is what
//! keeps a paced run byte-identical to an unpaced one: the pacing sleep sits between two
//! `advance_to` calls whose targets are computed from simulated time alone.
//!
//! The idle guard is polled here too, on the run thread, because the run thread is the thing being
//! reaped. A run that is paused or complete has no queued work; a run advancing as fast as it can
//! is busy; a run paced for a viewer who is no longer there is idle, which is the forgotten-tab case
//! docs/execution-plan.md section 3.5 exists for.

use crate::export;
use crate::idle::{wall_now_ns, IdleDecision, IdleGuard};
use crate::lease::LeaseRegistry;
use crate::wire::{self, Distribution, MetricRow, RunStatus, State, SubscriptionUpdate, Target};
use sim_core::{Nanos, EPOCH_BASE};
use sim_leaf::Sim;
use sim_metrics::{Frame, SparseHistogram};
use sim_scenario::Scenario;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::mpsc;
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

/// `StepForward` advances at most this much simulated time per call, so the request stays bounded
/// however much it asks for; the response says where it stopped and the client repeats.
pub const STEP_CAP_NS: Nanos = 60 * 1_000_000_000;

/// How far an unpaced run advances between two visits to the shared state. Small enough that a
/// stop or a pause lands within a fraction of a second of wall time, large enough that the lock is
/// not the hot path.
const UNPACED_CHUNK_NS: Nanos = 1_000_000_000;

/// How long the run thread sleeps between polls while it has nothing to advance. This is also the
/// resolution of the idle guard and of a pacing sleep, so a speed change lands within one slice.
const POLL: Duration = Duration::from_millis(50);

/// The subscription id stamped on checkpoint rows, beside `export.rs`'s `"export"`.
const CHECKPOINT_SUBSCRIPTION_ID: &str = "checkpoint";

/// Everything a request handler may read or change about a run. The engine itself is not here; it
/// belongs to the run thread.
#[derive(Debug)]
pub struct RunState {
    pub run_id: String,
    pub scenario: Scenario,
    pub state: State,
    /// Paused by `SetSpeed`. Distinct from `idle_stopped` because a subscription opening resumes an
    /// idle stop and must not override a pause the user asked for.
    pub paused: bool,
    /// Stopped by the idle guard; `GetRun` shows `STATE_PAUSED` with `error` empty per WIRE.md.
    pub idle_stopped: bool,
    /// Simulated seconds per wall second. Zero is as fast as possible.
    pub realtime_factor: f64,
    pub sim_time: Nanos,
    pub sim_end: Nanos,
    /// Every frame closed so far, in order. What subscriptions and checkpoints read.
    pub frames: Vec<Frame>,
    pub error: String,
    /// Set once the run is complete, so `GetResult` is a lookup.
    pub result: Option<wire::RunResult>,
    /// A `StepForward` in progress: advance unpaced to here, then pause.
    pub step_target: Option<Nanos>,
    pub stop_requested: bool,
    /// How many times the idle guard checkpointed this run. Observable for tests and for the note.
    pub checkpoints: u32,
    /// The idle guard's note, kept apart from `error` because WIRE.md promises `error` stays empty.
    pub note: String,
}

impl RunState {
    pub fn is_terminal(&self) -> bool {
        matches!(self.state, State::Complete | State::Failed)
    }

    pub fn status(&self) -> RunStatus {
        RunStatus {
            run_id: self.run_id.clone(),
            state: self.state,
            sim_time_unix_ns: self.sim_time,
            sim_end_unix_ns: self.sim_end,
            realtime_factor: self.realtime_factor,
            error: self.error.clone(),
        }
    }

    pub fn sample_interval_ns(&self) -> Nanos {
        ((self.scenario.sample_interval_ms * 1e6) as Nanos).max(1)
    }
}

/// One run: its shared state and the condvar that wakes whoever is waiting on it, the run thread
/// and request handlers alike.
#[derive(Debug)]
pub struct Run {
    pub state: Mutex<RunState>,
    pub changed: Condvar,
}

/// A request that cannot be honoured, with the HTTP status WIRE.md assigns it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refused {
    pub code: u16,
    pub message: String,
}

fn refused(code: u16, message: impl Into<String>) -> Refused {
    Refused { code, message: message.into() }
}

impl Run {
    pub fn lock(&self) -> MutexGuard<'_, RunState> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn status(&self) -> RunStatus {
        self.lock().status()
    }

    /// `SetSpeed`: pause, or run at `factor` simulated seconds per wall second (zero: unpaced).
    pub fn set_speed(&self, factor: f64, paused: bool) -> Result<RunStatus, Refused> {
        let mut st = self.lock();
        if st.is_terminal() {
            return Err(refused(409, format!("run {} is {}", st.run_id, st.state.name())));
        }
        if !(factor.is_finite() && factor >= 0.0) {
            return Err(refused(400, format!("realtime_factor {factor} must be finite and non-negative")));
        }
        st.realtime_factor = factor;
        st.paused = paused;
        st.step_target = None;
        if !paused {
            // A speed change is a viewer's request; it also lifts an idle stop.
            st.idle_stopped = false;
            st.state = State::Running;
        } else {
            st.state = State::Paused;
        }
        self.changed.notify_all();
        Ok(st.status())
    }

    /// `StepForward`: advance unpaced by up to [`STEP_CAP_NS`] and pause. Blocks until the run
    /// thread gets there, which for a minute of simulated time is a fraction of a wall second.
    pub fn step(&self, sim_duration_ns: Nanos) -> Result<RunStatus, Refused> {
        let mut st = self.lock();
        if st.is_terminal() {
            return Err(refused(409, format!("run {} is {}", st.run_id, st.state.name())));
        }
        if sim_duration_ns == 0 {
            return Err(refused(400, "sim_duration_ns or barrier_windows must be positive"));
        }
        let target = st.sim_time.saturating_add(sim_duration_ns.min(STEP_CAP_NS)).min(st.sim_end);
        st.step_target = Some(target);
        st.idle_stopped = false;
        st.state = State::Running;
        self.changed.notify_all();
        while st.step_target.is_some() && !st.is_terminal() {
            st = self.changed.wait(st).unwrap_or_else(|e| e.into_inner());
        }
        Ok(st.status())
    }

    /// `StopRun`: end the run where it stands. The result is the aggregation over what ran, so
    /// `GetResult` works on a stopped run exactly as on a finished one. Blocks until the run
    /// thread has finalised.
    pub fn stop(&self) -> RunStatus {
        let mut st = self.lock();
        st.stop_requested = true;
        st.paused = false;
        st.step_target = None;
        self.changed.notify_all();
        while !st.is_terminal() {
            st = self.changed.wait(st).unwrap_or_else(|e| e.into_inner());
        }
        st.status()
    }

    /// `GetResult`, encoded. 409 until the run is complete, per WIRE.md rule 6.
    pub fn result_json(&self) -> Result<String, Refused> {
        let st = self.lock();
        match (&st.result, st.state) {
            (Some(r), _) => Ok(wire::run_result_json(r)),
            (None, State::Failed) => Err(refused(409, format!("run {} failed: {}", st.run_id, st.error))),
            (None, s) => Err(refused(409, format!("run {} is {}, no result yet", st.run_id, s.name()))),
        }
    }

    /// A subscription opened: an idle-stopped run starts advancing again (WIRE.md, "Reopening a
    /// subscription resumes it"). A pause the user asked for is left alone.
    pub fn resume_from_idle(&self) {
        let mut st = self.lock();
        if st.idle_stopped && !st.paused && !st.is_terminal() {
            st.idle_stopped = false;
            st.state = State::Running;
            self.changed.notify_all();
        }
    }
}

/// Every run this process holds, plus the lease registry the idle guard reads.
#[derive(Debug)]
pub struct Registry {
    runs: Mutex<BTreeMap<String, Arc<Run>>>,
    next_id: Mutex<u64>,
    pub leases: Mutex<LeaseRegistry>,
    idle_threshold_ns: u64,
    /// The served directory: checkpoints go to `runs/<run_id>/` under it, beside the exports.
    root: PathBuf,
}

impl Registry {
    pub fn new(root: PathBuf, idle_threshold_ns: u64) -> Self {
        Registry {
            runs: Mutex::new(BTreeMap::new()),
            next_id: Mutex::new(1),
            leases: Mutex::new(LeaseRegistry::new()),
            idle_threshold_ns,
            root,
        }
    }

    pub fn leases(&self) -> MutexGuard<'_, LeaseRegistry> {
        self.leases.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn get(&self, run_id: &str) -> Option<Arc<Run>> {
        self.runs.lock().unwrap_or_else(|e| e.into_inner()).get(run_id).cloned()
    }

    pub fn list(&self) -> Vec<RunStatus> {
        let runs = self.runs.lock().unwrap_or_else(|e| e.into_inner());
        runs.values().map(|r| r.status()).collect()
    }

    /// `StartRun`. Builds the engine on the run thread and waits for its verdict, so an invalid
    /// scenario is a 400 here rather than a run that is born failed.
    pub fn start(self: &Arc<Self>, scenario: Scenario, max_realtime_factor: f64) -> Result<String, Refused> {
        if !(max_realtime_factor.is_finite() && max_realtime_factor >= 0.0) {
            return Err(refused(400, "max_realtime_factor must be finite and non-negative"));
        }
        let run_id = {
            let mut n = self.next_id.lock().unwrap_or_else(|e| e.into_inner());
            let id = format!("r-{n}");
            *n += 1;
            id
        };
        let sim_end = EPOCH_BASE + (scenario.duration_s * 1e9) as Nanos;
        let run = Arc::new(Run {
            state: Mutex::new(RunState {
                run_id: run_id.clone(),
                scenario: scenario.clone(),
                state: State::Queued,
                paused: false,
                idle_stopped: false,
                realtime_factor: max_realtime_factor,
                sim_time: EPOCH_BASE,
                sim_end,
                frames: Vec::new(),
                error: String::new(),
                result: None,
                step_target: None,
                stop_requested: false,
                checkpoints: 0,
                note: String::new(),
            }),
            changed: Condvar::new(),
        });
        let (ready_tx, ready_rx) = mpsc::sync_channel::<Result<(), String>>(1);
        let (thread_run, registry) = (Arc::clone(&run), Arc::clone(self));
        std::thread::Builder::new()
            .name(format!("run-{run_id}"))
            .spawn(move || drive(thread_run, registry, scenario, ready_tx))
            .map_err(|e| refused(500, format!("cannot spawn run thread: {e}")))?;
        match ready_rx.recv() {
            Ok(Ok(())) => {}
            Ok(Err(why)) => return Err(refused(400, why)),
            Err(_) => return Err(refused(500, "run thread exited before reporting")),
        }
        self.runs.lock().unwrap_or_else(|e| e.into_inner()).insert(run_id.clone(), run);
        Ok(run_id)
    }
}

/// What the run thread decided to do next, under the lock, to be done outside it.
enum Next {
    /// Advance to `to`. `due` is when the pacing says that instant is allowed to happen.
    Advance { to: Nanos, due: Option<Instant> },
    /// Nothing to advance right now: paused, stepping done, idle-stopped.
    Wait,
    /// Aggregate and mark complete.
    Finish,
    /// Terminal and checkpointed, or the state is gone: leave.
    Exit,
}

/// The run thread. `ready` carries `Sim::new`'s verdict back to `StartRun`.
fn drive(run: Arc<Run>, reg: Arc<Registry>, sc: Scenario, ready: mpsc::SyncSender<Result<(), String>>) {
    let mut sim = match Sim::new(&sc) {
        Ok(sim) => Some(sim),
        Err(why) => {
            let _ = ready.send(Err(why));
            return;
        }
    };
    let _ = ready.send(Ok(()));
    let mut guard = IdleGuard::new(reg.idle_threshold_ns);
    // Pacing anchor: the wall instant at which simulated `sim` was due, at `factor`. Dropped on
    // every pause or speed change so the run does not sprint to catch up after one.
    let mut anchor: Option<(Instant, Nanos, f64)> = None;
    {
        let mut st = run.lock();
        st.state = State::Running;
        run.changed.notify_all();
    }

    loop {
        let next = {
            let mut st = run.lock();
            let sample_iv = st.sample_interval_ns();

            // The idle guard, once per visit. A run advancing as fast as it can is busy; one
            // paced for a viewer, or paused, or finished, is only as busy as its leases.
            let now_wall = wall_now_ns();
            let live = reg.leases().live_for_run(&st.run_id, now_wall);
            let advancing = st.state == State::Running && !st.paused && !st.idle_stopped;
            let queued = usize::from(st.step_target.is_some() || (advancing && st.realtime_factor == 0.0));
            let mut checkpointed_now = false;
            if guard.observe(now_wall, live, queued) == IdleDecision::Shutdown {
                checkpoint(&reg.root, &st);
                st.checkpoints += 1;
                checkpointed_now = true;
                if !st.is_terminal() {
                    st.idle_stopped = true;
                    st.state = State::Paused;
                    st.note = format!(
                        "idle for {} s with no live lease and no queued work: checkpointed and stopped advancing",
                        reg.idle_threshold_ns / 1_000_000_000
                    );
                }
                run.changed.notify_all();
            }

            if st.is_terminal() {
                // The checkpoint is the last thing a finished run's thread does.
                if checkpointed_now { Next::Exit } else { Next::Wait }
            } else if st.stop_requested {
                Next::Finish
            } else if let Some(target) = st.step_target {
                // A step runs whether or not the run is paused: it is how a paused run is moved.
                anchor = None;
                if st.sim_time >= target {
                    st.step_target = None;
                    st.paused = true;
                    st.state = State::Paused;
                    run.changed.notify_all();
                    Next::Wait
                } else {
                    Next::Advance { to: (st.sim_time + UNPACED_CHUNK_NS).min(target), due: None }
                }
            } else if st.paused || st.idle_stopped {
                anchor = None;
                Next::Wait
            } else if st.realtime_factor == 0.0 {
                anchor = None;
                Next::Advance { to: (st.sim_time + UNPACED_CHUNK_NS).min(st.sim_end), due: None }
            } else {
                let factor = st.realtime_factor;
                if anchor.map_or(true, |(_, _, f)| f != factor) {
                    anchor = Some((Instant::now(), st.sim_time, factor));
                }
                let (wall0, sim0, _) = anchor.unwrap_or((Instant::now(), st.sim_time, factor));
                let to = (st.sim_time + sample_iv).min(st.sim_end);
                let due = wall0 + Duration::from_secs_f64((to - sim0) as f64 / 1e9 / factor);
                Next::Advance { to, due: Some(due) }
            }
        };

        match next {
            Next::Exit => return,
            Next::Wait => {
                let st = run.lock();
                let _ = run.changed.wait_timeout(st, POLL);
            }
            Next::Advance { to, due } => {
                if let Some(due) = due {
                    let now = Instant::now();
                    if due > now {
                        // Sleep in slices so a pause or a speed change lands promptly; the anchor
                        // keeps the target instant stable across the re-decision.
                        let remaining = due - now;
                        std::thread::sleep(remaining.min(POLL));
                        if remaining > POLL {
                            continue;
                        }
                    }
                }
                let Some(engine) = sim.as_mut() else { return };
                let outcome = engine.advance_to(to);
                let mut st = run.lock();
                match outcome {
                    Ok(()) => {
                        let have = st.frames.len();
                        st.frames.extend_from_slice(&engine.frames()[have..]);
                        st.sim_time = engine.now();
                        if engine.finished() {
                            st.stop_requested = true;
                        }
                    }
                    Err(why) => {
                        st.state = State::Failed;
                        st.error = why;
                        st.sim_time = engine.now();
                    }
                }
                run.changed.notify_all();
            }
            Next::Finish => {
                let Some(engine) = sim.take() else { return };
                let mut st = run.lock();
                let have = st.frames.len();
                st.frames.extend_from_slice(&engine.frames()[have..]);
                st.sim_time = engine.now();
                match engine.into_result() {
                    Ok(r) => {
                        st.result = Some(export::result(&r, &st.run_id));
                        st.state = State::Complete;
                    }
                    Err(why) => {
                        st.state = State::Failed;
                        st.error = why;
                    }
                }
                run.changed.notify_all();
            }
        }
    }
}

/// The idle checkpoint: the run as it stands, in the exact documents `export.rs` writes for a
/// finished run, under `runs/<run_id>/` of the served directory. The engine cannot be snapshotted
/// yet (`Leaf::snapshot` is unimplemented), so what is saved is what a viewer could have seen: the
/// status, the scenario, every frame as a fleet-scope update, and the result once there is one.
/// A write failure is logged and not fatal: the run is still in memory.
fn checkpoint(root: &std::path::Path, st: &RunState) {
    let dir = root.join("runs").join(&st.run_id);
    let write = |name: &str, body: String| {
        if let Err(e) = std::fs::create_dir_all(&dir).and_then(|_| std::fs::write(dir.join(name), body)) {
            println!("checkpoint {}: {}: {e}", st.run_id, dir.join(name).display());
        }
    };
    write("status.json", wire::run_status_json(&st.status()) + "\n");
    write("scenario.txt", st.scenario.to_text());
    let spec = RowSpec { target: Target::Fleet, metrics: Vec::new(), percentiles: export::PERCENTILES.to_vec() };
    let n = st.frames.len();
    let mut lines = String::new();
    for (i, f) in st.frames.iter().enumerate() {
        if let Some(row) = row(f, &st.scenario, &spec) {
            let u = SubscriptionUpdate {
                subscription_id: CHECKPOINT_SUBSCRIPTION_ID.to_string(),
                sim_time_unix_ns: f.t,
                realtime_factor: st.realtime_factor,
                row,
                is_final: st.is_terminal() && i + 1 == n,
            };
            lines.push_str(&wire::subscription_update_json(&u));
            lines.push('\n');
        }
    }
    write("fleet.jsonl", lines);
    if let Some(r) = &st.result {
        write("result.json", wire::run_result_json(r) + "\n");
    }
}

// ---------------------------------------------------------------------------
// Frames to rows
// ---------------------------------------------------------------------------

/// What a subscription asked for, reduced to what a row builder needs. `metrics` empty means every
/// metric the scope supports.
#[derive(Clone, Debug)]
pub struct RowSpec {
    pub target: Target,
    pub metrics: Vec<i32>,
    pub percentiles: Vec<f64>,
}

impl RowSpec {
    fn wants(&self, metric: i32) -> bool {
        self.metrics.is_empty() || self.metrics.contains(&metric)
    }
}

/// The metrics each scope can answer from a `Frame`, the table in WIRE.md "What the first server
/// supports". A subscription naming any other metric is rejected at open.
pub const FLEET_METRICS: &[i32] = &[
    wire::METRIC_OFFERED_RPS,
    wire::METRIC_ADMITTED_RPS,
    wire::METRIC_COMPLETED_RPS,
    wire::METRIC_REJECTED_RPS,
    wire::METRIC_OUTPUT_TOKENS_PER_S,
    wire::METRIC_GOODPUT_TOKENS_PER_S,
    wire::METRIC_QUEUED_SEQS,
    wire::METRIC_RUNNING_SEQS,
    wire::METRIC_KV_UTILIZATION,
    METRIC_KV_TOKENS_RESIDENT,
    wire::METRIC_LOAD_IMBALANCE_CV,
    wire::METRIC_SLO_ATTAINMENT,
    wire::METRIC_TTFT,
    wire::METRIC_ITL,
    wire::METRIC_E2E,
    wire::METRIC_QUEUE_WAIT,
    wire::METRIC_READY_REPLICAS,
];
pub const REPLICA_METRICS: &[i32] = &[
    wire::METRIC_QUEUED_SEQS,
    wire::METRIC_RUNNING_SEQS,
    wire::METRIC_KV_UTILIZATION,
    METRIC_KV_TOKENS_RESIDENT,
    METRIC_STEP_TIME,
];

// Two metric numbers `wire.rs` does not name; the same table, and `metric_numbers_are_in_the_table`
// below holds them to it.
pub const METRIC_KV_TOKENS_RESIDENT: i32 = 21;
pub const METRIC_STEP_TIME: i32 = 8;

/// A windowed histogram as the wire carries it. The frame's histograms are bucketed, so the
/// percentiles are accurate at bucket resolution rather than exact, which is what the flag says.
fn distribution(h: &SparseHistogram, percentiles: &[f64]) -> Distribution {
    Distribution {
        count: h.count(),
        mean: h.mean(),
        min: h.min() as f64,
        max: h.max() as f64,
        percentile: percentiles.to_vec(),
        value: percentiles.iter().map(|q| h.percentile(*q) as f64).collect(),
        from_merged_histogram: true,
    }
}

/// One `MetricRow` for one frame. `None` when the target names a replica the scenario has not got.
pub fn row(f: &Frame, sc: &Scenario, spec: &RowSpec) -> Option<MetricRow> {
    let mut row = MetricRow::new(spec.target);
    let cap = sc.kv_capacity_tokens.max(1.0);
    match spec.target {
        Target::Replica(id) => {
            let r = f.replicas.get(usize::try_from(id).ok()?)?;
            let mut value = |m: i32, v: f64| {
                if spec.wants(m) {
                    row.value(m, v);
                }
            };
            value(wire::METRIC_QUEUED_SEQS, r.queued as f64);
            value(wire::METRIC_RUNNING_SEQS, r.running as f64);
            value(wire::METRIC_KV_UTILIZATION, r.kv_tokens as f64 / cap);
            value(METRIC_KV_TOKENS_RESIDENT, r.kv_tokens as f64);
            // The last step is one observation, carried as the distribution the proto types it
            // as. Zero means the replica has not stepped yet, and a duration of nothing is a gap.
            if spec.wants(METRIC_STEP_TIME) && r.last_step_ns > 0 {
                let v = r.last_step_ns as f64;
                row.distribution(
                    METRIC_STEP_TIME,
                    Distribution {
                        count: 1,
                        mean: v,
                        min: v,
                        max: v,
                        percentile: spec.percentiles.clone(),
                        value: vec![v; spec.percentiles.len()],
                        from_merged_histogram: false,
                    },
                );
            }
        }
        Target::Fleet => {
            let iv_s = (sc.sample_interval_ms / 1000.0).max(1e-9);
            let n = f.replicas.len().max(1) as f64;
            let queued: u64 = f.replicas.iter().map(|r| u64::from(r.queued)).sum();
            let running: u64 = f.replicas.iter().map(|r| u64::from(r.running)).sum();
            let kv: u64 = f.replicas.iter().map(|r| r.kv_tokens).sum();
            let ended = f.completed + f.rejected + f.timed_out;
            let mut value = |m: i32, v: f64| {
                if spec.wants(m) {
                    row.value(m, v);
                }
            };
            value(wire::METRIC_OFFERED_RPS, f.offered_rps);
            value(wire::METRIC_ADMITTED_RPS, f.admitted as f64 / iv_s);
            value(wire::METRIC_COMPLETED_RPS, f.completed as f64 / iv_s);
            value(wire::METRIC_REJECTED_RPS, f.rejected as f64 / iv_s);
            value(wire::METRIC_OUTPUT_TOKENS_PER_S, f.output_tokens as f64 / iv_s);
            value(wire::METRIC_GOODPUT_TOKENS_PER_S, f.goodput_tokens as f64 / iv_s);
            value(wire::METRIC_QUEUED_SEQS, queued as f64);
            value(wire::METRIC_RUNNING_SEQS, running as f64);
            value(wire::METRIC_KV_UTILIZATION, kv as f64 / cap / n);
            value(METRIC_KV_TOKENS_RESIDENT, kv as f64);
            value(wire::METRIC_LOAD_IMBALANCE_CV, imbalance(f));
            // Same denominator as the scorecard: everything that ended in the window, shed
            // included, so a policy cannot look good by shedding.
            value(
                wire::METRIC_SLO_ATTAINMENT,
                if ended == 0 { f64::NAN } else { f.within_slo as f64 / ended as f64 },
            );
            value(wire::METRIC_READY_REPLICAS, f.replicas.len() as f64);
            for (m, h) in [
                (wire::METRIC_TTFT, &f.ttft),
                (wire::METRIC_ITL, &f.itl_max),
                (wire::METRIC_E2E, &f.e2e),
                (wire::METRIC_QUEUE_WAIT, &f.queue_wait),
            ] {
                if spec.wants(m) {
                    row.distribution(m, distribution(h, &spec.percentiles));
                }
            }
        }
    }
    Some(row)
}

/// Coefficient of variation of queued-plus-running across replicas at the sample, the per-instant
/// form of `RunResult::load_imbalance_cv`. NaN when the fleet is idle, which the row omits.
fn imbalance(f: &Frame) -> f64 {
    let n = f.replicas.len();
    if n == 0 {
        return f64::NAN;
    }
    let loads = f.replicas.iter().map(|r| f64::from(r.queued + r.running));
    let mean = loads.clone().sum::<f64>() / n as f64;
    if mean <= 0.0 {
        return f64::NAN;
    }
    let var = loads.map(|x| (x - mean) * (x - mean)).sum::<f64>() / (n - 1).max(1) as f64;
    var.sqrt() / mean
}

/// Which recorded frame a subscription's `k`-th sample reads, `k` from 1. The client's cadence is
/// `samples_per_sim_second`; the engine's is the scenario's sample interval; the nearest recorded
/// frame is used, never an interpolation, because an interpolated batch never existed.
pub fn frame_index(k: u64, samples_per_sim_second: f64, sample_interval_ns: Nanos) -> usize {
    let t = k as f64 * 1e9 / samples_per_sim_second;
    let j = (t / sample_interval_ns.max(1) as f64).round();
    (j.max(1.0) as usize).max(1)
}

/// The simulated instant of the subscription's `k`-th sample, for reference; frames carry their
/// own `t`, which is what goes on the wire.
pub fn sample_instant(k: u64, samples_per_sim_second: f64) -> Nanos {
    EPOCH_BASE + (k as f64 * 1e9 / samples_per_sim_second) as Nanos
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metric_numbers_are_in_the_table() {
        assert_eq!(wire::metric_name(METRIC_KV_TOKENS_RESIDENT), Some("METRIC_KV_TOKENS_RESIDENT"));
        assert_eq!(wire::metric_name(METRIC_STEP_TIME), Some("METRIC_STEP_TIME"));
        for m in FLEET_METRICS.iter().chain(REPLICA_METRICS) {
            assert!(wire::metric_name(*m).is_some(), "metric {m} is not in wire::METRICS");
        }
    }

    #[test]
    fn nearest_frame_is_rounded_never_interpolated() {
        let iv = 250_000_000; // the scenarios' 250 ms
        // At the recorded cadence the k-th sample is the k-th frame.
        assert_eq!((1..=5).map(|k| frame_index(k, 4.0, iv)).collect::<Vec<_>>(), vec![1, 2, 3, 4, 5]);
        // Coarser: every fourth frame.
        assert_eq!((1..=3).map(|k| frame_index(k, 1.0, iv)).collect::<Vec<_>>(), vec![4, 8, 12]);
        // Finer: frames repeat, and the first sample never reads a frame that does not exist.
        assert_eq!((1..=6).map(|k| frame_index(k, 8.0, iv)).collect::<Vec<_>>(), vec![1, 1, 2, 2, 3, 3]);
        assert_eq!(frame_index(1, 1000.0, iv), 1);
        assert_eq!(sample_instant(4, 4.0), EPOCH_BASE + 1_000_000_000);
    }

    #[test]
    fn step_cap_is_one_minute() {
        assert_eq!(STEP_CAP_NS, 60 * 1_000_000_000);
    }
}
