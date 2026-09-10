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
use sim_leaf::{Applied, Sim};
use sim_metrics::trace::RequestTrace;
use sim_metrics::{Frame, SparseHistogram};
use sim_scenario::Scenario;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
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

/// How long a finished run stays in memory after its checkpoint is on disk, with nobody leased to
/// it. Long enough for the dashboard's end-of-run `GetResult` and a reload; short enough that a
/// site whose default run is ten minutes never holds more than a couple of finished runs at once.
/// Before this every finished run stayed until the instance died: ninety-odd MB each, and the
/// engine's memory budget in about ten runs.
pub const DEFAULT_COMPLETED_RETENTION_SECONDS: u64 = 120;

/// Reads `LBSIM_COMPLETED_RETENTION_S`. Default 120; a value that does not parse falls back to the
/// default rather than to zero, for the same reason `IDLE_SHUTDOWN_SECONDS` does. Zero is allowed
/// and means "release as soon as the checkpoint is written".
pub fn completed_retention_ns_from_env() -> u64 {
    std::env::var("LBSIM_COMPLETED_RETENTION_S")
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .unwrap_or(DEFAULT_COMPLETED_RETENTION_SECONDS)
        .saturating_mul(1_000_000_000)
}

/// Resident set size of this process, from `/proc/self/statm`, so a memory report is a number
/// rather than a guess. `None` off Linux.
pub fn process_rss_bytes() -> Option<u64> {
    let statm = std::fs::read_to_string("/proc/self/statm").ok()?;
    let pages: u64 = statm.split_whitespace().nth(1)?.parse().ok()?;
    Some(pages * 4096)
}

/// The trace ring: the newest sampled journeys a run keeps for `GetTraces`, bounded by count and
/// by encoded size, whichever trips first. Two thousand is twenty pages of the dashboard's table;
/// five MiB is `export::DEFAULT_TRACE_BUDGET_BYTES`, the same ceiling a finished run's
/// `traces.jsonl` is held to, so a live run costs no more memory than its export would take on disk.
pub const TRACE_RING_LEN: usize = 2_000;
pub const TRACE_RING_BYTES: usize = 5 * 1024 * 1024;

/// One retained trace and its wire form. Encoded once, at drain, on the run thread: `GetTraces`
/// then filters on the struct and concatenates the strings, and a busy dashboard polling every two
/// seconds never re-encodes the same journey.
#[derive(Debug)]
pub struct TraceEntry {
    pub trace: RequestTrace,
    pub json: String,
}

/// `TRACE_RING_LEN` / `TRACE_RING_BYTES`, kept exact: the oldest entries leave as the newest arrive.
#[derive(Debug, Default)]
pub struct TraceRing {
    entries: std::collections::VecDeque<TraceEntry>,
    bytes: usize,
}

impl TraceRing {
    pub fn push(&mut self, trace: RequestTrace) {
        let json = crate::trace_wire::request_trace_json(&trace);
        self.bytes += json.len();
        self.entries.push_back(TraceEntry { trace, json });
        while self.entries.len() > TRACE_RING_LEN || self.bytes > TRACE_RING_BYTES {
            let Some(old) = self.entries.pop_front() else { break };
            self.bytes -= old.json.len();
        }
    }
    pub fn len(&self) -> usize {
        self.entries.len()
    }
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
    /// Newest first: the tail of the ring is what a viewer polling a live run wants to see move.
    pub fn newest_first(&self) -> impl Iterator<Item = &TraceEntry> {
        self.entries.iter().rev()
    }
}

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
    /// When the idle guard stopped it, so the run thread can reap a run nobody came back for.
    /// Cleared with `idle_stopped`.
    pub idle_stopped_at_wall_ns: Option<u64>,
    /// Unregistered, by `StartRun` at the cap or by the reap: the run thread exits on sight and
    /// nothing more is written. Whoever still holds the `Arc<Run>` reads why in `note`.
    pub evicted: bool,
    /// Finished and let go: the frames and traces are gone from memory and the registry answers
    /// for it from a status stub and the checkpoint on disk. Set with `evicted`'s effect on the run
    /// thread, and kept distinct because a released run is still listed.
    pub released: bool,
    /// When the run went terminal, which is when the retention clock starts.
    pub finished_at_wall_ns: Option<u64>,
    /// The last time a `GetRun`, `GetTraces`, `GetResult` or `OpenSubscription` landed on this
    /// run, wall time. Set at construction so a run that finishes and is never read again still
    /// anchors its retention at `finished_at_wall_ns` (the old behaviour); a read after that pushes
    /// the anchor forward, so a viewer who is still looking is never the reason a run is released
    /// out from under them. `ListRuns` and the idle guard's own polling do not touch this: a listing
    /// is not a read of the run, and the retention clock exists to answer "is anyone still reading
    /// this one", not "did the process do a map sweep".
    pub last_read_wall_ns: u64,
    /// The terminal checkpoint is on disk: `GetResult` and replay can answer without this process.
    /// A run is never released before this is set.
    pub terminal_checkpoint: bool,
    /// Registration order for the eviction: `r-10` sorts before `r-2`, so the map's order is no use.
    pub started_at_wall_ns: u64,
    /// Simulated seconds per wall second. Zero is as fast as possible.
    pub realtime_factor: f64,
    pub sim_time: Nanos,
    pub sim_end: Nanos,
    /// Every frame closed so far, in order. What subscriptions and checkpoints read.
    pub frames: Vec<Frame>,
    pub error: String,
    /// Set once the run is complete, so `GetResult` is a lookup.
    pub result: Option<wire::RunResult>,
    /// The newest sampled journeys, drained from the engine after every chunk. What `GetTraces`
    /// answers from; empty for a run started without `record_traces` and a scenario at rate zero.
    pub traces: TraceRing,
    /// A `StepForward` in progress: advance unpaced to here, then pause.
    pub step_target: Option<Nanos>,
    pub stop_requested: bool,
    /// How many times the idle guard checkpointed this run. Observable for tests and for the note.
    pub checkpoints: u32,
    /// The idle guard's note, kept apart from `error` because WIRE.md promises `error` stays empty.
    pub note: String,
    /// An `UpdateWorkload` or `UpdatePolicies` waiting for the run thread, the only place the
    /// engine lives. One at a time: a second caller waits for the slot.
    pub pending_update: Option<PendingUpdate>,
    /// The run thread's verdict on the last pending update, until its caller takes it.
    pub update_result: Option<UpdateOutcome>,
}

/// Overrides bound for `Sim::apply_overrides`, already policed by kind at the server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingUpdate {
    pub overrides: Vec<(String, String)>,
}

/// What the engine said to an update: which keys changed, or why the whole batch was refused.
pub type UpdateOutcome = Result<Applied, String>;

impl RunState {
    pub fn new(run_id: String, scenario: Scenario, max_realtime_factor: f64) -> RunState {
        let sim_end = EPOCH_BASE + (scenario.duration_s * 1e9) as Nanos;
        RunState {
            run_id,
            scenario,
            state: State::Queued,
            paused: false,
            idle_stopped: false,
            idle_stopped_at_wall_ns: None,
            evicted: false,
            released: false,
            finished_at_wall_ns: None,
            last_read_wall_ns: wall_now_ns(),
            terminal_checkpoint: false,
            started_at_wall_ns: wall_now_ns(),
            realtime_factor: max_realtime_factor,
            sim_time: EPOCH_BASE,
            sim_end,
            frames: Vec::new(),
            error: String::new(),
            result: None,
            traces: TraceRing::default(),
            step_target: None,
            stop_requested: false,
            checkpoints: 0,
            note: String::new(),
            pending_update: None,
            update_result: None,
        }
    }

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
            st.idle_stopped_at_wall_ns = None;
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
        st.idle_stopped_at_wall_ns = None;
        st.state = State::Running;
        self.changed.notify_all();
        while st.step_target.is_some() && !st.is_terminal() {
            st = self.changed.wait(st).unwrap_or_else(|e| e.into_inner());
        }
        Ok(st.status())
    }

    /// `UpdateWorkload` / `UpdatePolicies`: hand the overrides to the run thread and wait for its
    /// verdict, the way `step` waits for its target. Forward only: nothing is rewound, and a
    /// refused batch leaves the run exactly as it was. A paused run applies it too, since the run
    /// thread looks for one on every visit rather than only after an advance.
    pub fn update(&self, overrides: Vec<(String, String)>) -> Result<UpdateOutcome, Refused> {
        let mut st = self.lock();
        while (st.pending_update.is_some() || st.update_result.is_some()) && !st.is_terminal() {
            st = self.changed.wait(st).unwrap_or_else(|e| e.into_inner());
        }
        if st.is_terminal() {
            return Err(refused(409, format!("run {} is {}", st.run_id, st.state.name())));
        }
        st.pending_update = Some(PendingUpdate { overrides });
        self.changed.notify_all();
        loop {
            if let Some(outcome) = st.update_result.take() {
                // Free the slot for the next caller.
                self.changed.notify_all();
                return Ok(outcome);
            }
            if st.is_terminal() {
                st.pending_update = None;
                return Err(refused(409, format!("run {} ended before the update was applied", st.run_id)));
            }
            st = self.changed.wait(st).unwrap_or_else(|e| e.into_inner());
        }
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

    /// `GetTraces`, encoded: the entries that pass `q`, newest first by the instant the request
    /// ended, at most `q.limit`. Sorted rather than read off the ring's tail because the engine
    /// settles a rejection at its arrival and a completion at its last step, so the ring's order is
    /// not the order a viewer means by "newest". Answers at any state, since a live run's traces
    /// are the point; a run that recorded none gives an empty list rather than an error, because
    /// "nothing sampled" is an answer and not a fault.
    pub fn traces_json(&self, q: &crate::trace_wire::TraceQuery) -> String {
        let st = self.lock();
        let mut hits: Vec<&TraceEntry> = st.traces.newest_first().filter(|e| q.matches(&e.trace)).collect();
        // Stable, so two journeys ending in the same instant keep the engine's own order.
        hits.sort_by_key(|e| std::cmp::Reverse(e.trace.record.finished_at));
        let mut out = String::from("{\"traces\":[");
        for (i, e) in hits.iter().take(q.limit).enumerate() {
            if i > 0 {
                out.push(',');
            }
            out.push_str(&e.json);
        }
        out.push_str("]}");
        out
    }

    /// A `GetRun`, `GetTraces`, `GetResult` or a successful `OpenSubscription` lookup landed on
    /// this run: refresh the retention clock. The caller decides what counts as a read; this just
    /// stamps the instant, under the same lock every other mutation of `RunState` takes.
    pub fn touch_read(&self) {
        self.lock().last_read_wall_ns = wall_now_ns();
    }

    /// A subscription opened: an idle-stopped run starts advancing again (WIRE.md, "Reopening a
    /// subscription resumes it"). A pause the user asked for is left alone.
    pub fn resume_from_idle(&self) {
        let mut st = self.lock();
        if st.idle_stopped && !st.paused && !st.is_terminal() {
            st.idle_stopped = false;
            st.idle_stopped_at_wall_ns = None;
            st.state = State::Running;
            self.changed.notify_all();
        }
    }
}

/// Every run this process holds, plus the lease registry the idle guard reads.
#[derive(Debug)]
pub struct Registry {
    runs: Mutex<BTreeMap<String, Arc<Run>>>,
    /// Finished runs let go of: the status each had when released, so `ListRuns` and `GetRun`
    /// keep answering for the process's lifetime at a hundred bytes a run; the result and the
    /// frames are on disk under `runs/<run_id>/`.
    released: Mutex<BTreeMap<String, RunStatus>>,
    next_id: Mutex<u64>,
    pub leases: Mutex<LeaseRegistry>,
    idle_threshold_ns: u64,
    completed_retention_ns: AtomicU64,
    /// The served directory: checkpoints go to `runs/<run_id>/` under it, beside the exports.
    root: PathBuf,
}

/// A held run, for the memory report.
#[derive(Clone, Debug, PartialEq)]
pub struct Held {
    pub run_id: String,
    pub state: State,
    pub frames: usize,
    pub traces: usize,
}

impl Registry {
    pub fn new(root: PathBuf, idle_threshold_ns: u64) -> Self {
        Registry {
            runs: Mutex::new(BTreeMap::new()),
            released: Mutex::new(BTreeMap::new()),
            next_id: Mutex::new(1),
            leases: Mutex::new(LeaseRegistry::new()),
            idle_threshold_ns,
            completed_retention_ns: AtomicU64::new(DEFAULT_COMPLETED_RETENTION_SECONDS * 1_000_000_000),
            root,
        }
    }

    /// How long a finished run stays in memory once its checkpoint is written and nobody holds a
    /// lease on it. Atomic so the server can set it after construction, and a test can shorten it.
    pub fn set_completed_retention_ns(&self, ns: u64) {
        self.completed_retention_ns.store(ns, Ordering::Relaxed);
    }

    pub fn completed_retention_ns(&self) -> u64 {
        self.completed_retention_ns.load(Ordering::Relaxed)
    }

    // Lock order: runs → leases → released → run.state, each optional, never reversed. `leases`
    // then `subs` then `run.state` is the server's order; nothing takes `leases` before `runs`.
    pub fn leases(&self) -> MutexGuard<'_, LeaseRegistry> {
        self.leases.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn released_map(&self) -> MutexGuard<'_, BTreeMap<String, RunStatus>> {
        self.released.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn get(&self, run_id: &str) -> Option<Arc<Run>> {
        self.runs.lock().unwrap_or_else(|e| e.into_inner()).get(run_id).cloned()
    }

    /// The status a released run had when it was let go, if `run_id` is one.
    pub fn released_status(&self, run_id: &str) -> Option<RunStatus> {
        self.released_map().get(run_id).cloned()
    }

    /// Every run in memory, then every released one, each in id order.
    pub fn list(&self) -> Vec<RunStatus> {
        let runs = self.runs.lock().unwrap_or_else(|e| e.into_inner());
        let mut out: Vec<RunStatus> = runs.values().map(|r| r.status()).collect();
        out.extend(self.released_map().values().cloned());
        out
    }

    /// The runs in memory, with what each holds: the memory report's per-run line.
    pub fn held(&self) -> Vec<Held> {
        let runs = self.runs.lock().unwrap_or_else(|e| e.into_inner());
        runs.iter()
            .map(|(id, r)| {
                let st = r.lock();
                Held { run_id: id.clone(), state: st.state, frames: st.frames.len(), traces: st.traces.len() }
            })
            .collect()
    }

    pub fn released_count(&self) -> usize {
        self.released_map().len()
    }

    /// The reap's half of an eviction: takes `runs` alone, so the run thread calls it after it
    /// has let go of its own state.
    fn remove(&self, run_id: &str) {
        self.runs.lock().unwrap_or_else(|e| e.into_inner()).remove(run_id);
    }

    /// Let a finished run go: out of `runs`, its frames and traces dropped, a status stub kept.
    /// The run thread exits on `released`. The caller holds `runs`; `released` and `run.state` are
    /// taken here, in that order.
    fn release_held(runs: &mut BTreeMap<String, Arc<Run>>, released: &mut BTreeMap<String, RunStatus>, run_id: &str, why: &str) {
        let Some(run) = runs.remove(run_id) else { return };
        let mut st = run.lock();
        st.released = true;
        st.frames = Vec::new();
        st.traces = TraceRing::default();
        st.note = format!("released: {why}; the checkpoint under runs/{run_id}/ answers from here");
        released.insert(run_id.to_string(), st.status());
        run.changed.notify_all();
    }

    /// The run thread's release, after the retention: `runs` then `released` then the run's state.
    fn release(&self, run_id: &str, why: &str) {
        let mut runs = self.runs.lock().unwrap_or_else(|e| e.into_inner());
        let mut released = self.released_map();
        Self::release_held(&mut runs, &mut released, run_id, why);
    }

    /// `StartRun`. Builds the engine on the run thread and waits for its verdict, so an invalid
    /// scenario is a 400 here rather than a run that is born failed.
    pub fn start(self: &Arc<Self>, scenario: Scenario, max_realtime_factor: f64) -> Result<String, Refused> {
        if !(max_realtime_factor.is_finite() && max_realtime_factor >= 0.0) {
            return Err(refused(400, "max_realtime_factor must be finite and non-negative"));
        }
        // Held until the run is registered, so the cap is exact under concurrent starts.
        let mut runs = self.runs.lock().unwrap_or_else(|e| e.into_inner());
        // A StartRun at the cap is the moment memory matters: every finished run whose checkpoint
        // is on disk and that nobody is leased to goes now rather than at the end of its retention,
        // so the new engine is not built beside eight finished runs' frames.
        if runs.len() >= MAX_LIVE_RUNS {
            let now = wall_now_ns();
            // Every finished, checkpointed, unleased run is a candidate; a read within the last
            // `MIN_RELEASE_READ_AGE_NS` takes it off the table regardless of memory pressure, and
            // among what is left the least-recently-read goes first (the sort is why this is a
            // `Vec` and not left as `filter`, even though every candidate past the floor is
            // released here: the order is what a test, and a log line, can hold this to).
            let mut candidates: Vec<(u64, String)> = {
                let leases = self.leases();
                runs.iter()
                    .filter_map(|(id, r)| {
                        let st = r.lock();
                        if !(st.is_terminal() && st.terminal_checkpoint && leases.live_for_run(id, now) == 0) {
                            return None;
                        }
                        let anchor = st.last_read_wall_ns.max(st.finished_at_wall_ns.unwrap_or(0));
                        (now.saturating_sub(anchor) >= MIN_RELEASE_READ_AGE_NS).then(|| (anchor, id.clone()))
                    })
                    .collect()
            };
            candidates.sort();
            let mut released = self.released_map();
            for (_, id) in &candidates {
                Self::release_held(&mut runs, &mut released, id, &format!("{MAX_LIVE_RUNS} runs held and a StartRun arrived"));
            }
        }
        // The cap bounds CPU and memory for live work. A parked checkpoint is neither, so an
        // idle-stopped run gives up its slot, oldest first, and the 503 is for a box whose eight
        // are all busy or watched; a crashed tab must never be able to lock the public site.
        loop {
            let held = runs.values().filter(|r| !r.lock().is_terminal()).count();
            if held < MAX_LIVE_RUNS {
                break;
            }
            let oldest_idle = runs
                .iter()
                .filter_map(|(id, r)| {
                    let st = r.lock();
                    (st.idle_stopped && !st.is_terminal()).then(|| (st.started_at_wall_ns, id.clone()))
                })
                .min();
            let Some((_, id)) = oldest_idle else {
                return Err(refused(503, format!("{held} runs are live, the most this server holds; stop one first")));
            };
            let run = runs.remove(&id).expect("the id came from this map");
            let mut st = run.lock();
            st.evicted = true;
            st.note = format!(
                "evicted: {MAX_LIVE_RUNS} runs held and a StartRun arrived; the checkpoint under runs/{id}/ stays"
            );
            run.changed.notify_all();
        }
        let run_id = {
            let mut n = self.next_id.lock().unwrap_or_else(|e| e.into_inner());
            let id = format!("r-{n}");
            *n += 1;
            id
        };
        let run = Arc::new(Run {
            state: Mutex::new(RunState::new(run_id.clone(), scenario.clone(), max_realtime_factor)),
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
        runs.insert(run_id.clone(), run);
        Ok(run_id)
    }
}

/// How many non-terminal runs one server holds. Each is a thread and an engine, and an unpaced run
/// is never reaped by the idle guard, so without a bound a loop of `StartRun`s is a way to fill the
/// box. Eight is more than a handful of browser tabs and less than the reserved container CPU can
/// keep paced; a 503 past it says "stop one" rather than slowing every run down.
pub const MAX_LIVE_RUNS: usize = 8;

/// At the cap, a terminal run read within this long is left alone even though it is otherwise
/// eligible for release: a `StartRun` needing memory back must never be the reason a `GetTraces`
/// or a `GetRun` that landed moments ago gets cut off from under it. Ten seconds is longer than
/// any request this server answers.
const MIN_RELEASE_READ_AGE_NS: u64 = 10 * 1_000_000_000;

/// What the run thread decided to do next, under the lock, to be done outside it.
enum Next {
    /// Advance to `to`. `due` is when the pacing says that instant is allowed to happen.
    Advance { to: Nanos, due: Option<Instant> },
    /// Nothing to advance right now: paused, stepping done, idle-stopped.
    Wait,
    /// Aggregate and mark complete.
    Finish,
    /// Terminal and checkpointed, or evicted: leave.
    Exit,
    /// Idle-stopped for twice the threshold and nobody came back: unregister, then leave.
    Reap,
    /// Finished, checkpointed, unleased, and the retention has run out: let go, then leave.
    Release,
}

/// The run thread. `ready` carries `Sim::new`'s verdict back to `StartRun`.
fn drive(run: Arc<Run>, reg: Arc<Registry>, sc: Scenario, ready: mpsc::SyncSender<Result<(), String>>) {
    let mut sim = match Sim::new(&sc) {
        Ok(mut sim) => {
            // Nothing on this path reads the per-request records: the scorecard reads the
            // engine's tally and histograms, the traces are cloned at settle, and the frames are
            // what every subscription and the checkpoint read. Folding them saves ~100 bytes a
            // request for the run's whole length.
            sim.fold_records();
            Some(sim)
        }
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
    let run_id = {
        let mut st = run.lock();
        st.state = State::Running;
        run.changed.notify_all();
        st.run_id.clone()
    };

    loop {
        // Built under the lock, written after it: every subscription's writer waits on this lock,
        // so a file write inside it stalls every viewer for the duration of the write.
        let mut pending: Option<CheckpointInputs> = None;
        // Leases before the run state, never inside it: the writers take `subs` then `run.state`,
        // and `reap` takes `leases` then `subs`, so `leases` under `run.state` would be a cycle.
        let now_wall = wall_now_ns();
        let live = reg.leases().live_for_run(&run_id, now_wall);
        let next = {
            let mut st = run.lock();
            let sample_iv = st.sample_interval_ns();

            // A pending update lands here, between two chunks, whatever the pacing: a paused run
            // still visits this block every `POLL`, so a viewer's change never waits for a resume.
            if let (Some(update), Some(engine)) = (st.pending_update.take(), sim.as_mut()) {
                let pairs: Vec<(&str, &str)> =
                    update.overrides.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
                st.update_result = Some(engine.apply_overrides(&pairs));
                run.changed.notify_all();
            }

            // The idle guard, once per visit. A run advancing as fast as it can is busy; one
            // paced for a viewer, or paused, or finished, is only as busy as its leases.
            let advancing = st.state == State::Running && !st.paused && !st.idle_stopped;
            let queued = usize::from(st.step_target.is_some() || (advancing && st.realtime_factor == 0.0));
            // A terminal run's checkpoint is the retention rule's, below, not the idle guard's.
            if guard.observe(now_wall, live, queued) == IdleDecision::Shutdown && !st.is_terminal() {
                pending = Some(Checkpoint::from_state(&st));
                st.checkpoints += 1;
                st.idle_stopped = true;
                st.idle_stopped_at_wall_ns = Some(now_wall);
                st.state = State::Paused;
                st.note = format!(
                    "idle for {} s with no live lease and no queued work: checkpointed and stopped advancing",
                    reg.idle_threshold_ns / 1_000_000_000
                );
                run.changed.notify_all();
            }
            if st.is_terminal() && st.finished_at_wall_ns.is_none() {
                st.finished_at_wall_ns = Some(now_wall);
            }

            let reap_due = st
                .idle_stopped_at_wall_ns
                .is_some_and(|at| now_wall.saturating_sub(at) >= 2 * reg.idle_threshold_ns);
            if st.evicted || st.released {
                Next::Exit
            } else if reap_due && !st.is_terminal() {
                // Bounds memory with no new arrivals to evict it: the checkpoint from the idle
                // stop is already on disk, so nothing is lost but the in-memory engine.
                st.evicted = true;
                st.note = format!(
                    "reaped: idle-stopped for {} s, twice the idle threshold; the checkpoint under runs/{}/ stays",
                    2 * reg.idle_threshold_ns / 1_000_000_000,
                    st.run_id
                );
                run.changed.notify_all();
                Next::Reap
            } else if st.is_terminal() {
                // A run that failed mid-chunk has no result and never passed through `Finish`,
                // which is where a completed run's checkpoint is rendered; it gets the same
                // documents minus the result here, once. Then the retention: the run stays while
                // someone holds a lease on it (a viewer reading its final frames) and for the
                // retention after the checkpoint landed, and is let go after that.
                if !st.terminal_checkpoint {
                    pending = Some(Checkpoint::from_state(&st));
                    st.terminal_checkpoint = true;
                }
                // The retention clock is anchored at whichever is later, when the run finished or
                // the last time somebody read it: a `GetRun` every few hundred milliseconds must
                // hold a finished run open for as long as it keeps landing, not just for the
                // retention after the run ended.
                let due = st.finished_at_wall_ns.is_some_and(|at| {
                    let anchor = st.last_read_wall_ns.max(at);
                    now_wall.saturating_sub(anchor) >= reg.completed_retention_ns()
                });
                if due && live == 0 && pending.is_none() { Next::Release } else { Next::Wait }
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

        if let Some(inputs) = pending.take() {
            inputs.render().write(&reg.root);
        }

        match next {
            Next::Exit => return,
            Next::Reap => {
                reg.remove(&run_id);
                return;
            }
            Next::Release => {
                reg.release(
                    &run_id,
                    &format!("finished {} s ago, unleased", reg.completed_retention_ns() / 1_000_000_000),
                );
                return;
            }
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
                absorb(&mut run.lock(), engine, outcome);
                run.changed.notify_all();
            }
            Next::Finish => {
                let Some(mut engine) = sim.take() else { return };
                // The terminal checkpoint's inputs, gathered under the lock and rendered after it:
                // rendering every frame's fleet row and the replica rows of a 256-replica run is
                // seconds of work, and every viewer's writer waits on this lock.
                let checkpoint = {
                    let mut st = run.lock();
                    st.frames.extend(engine.drain_frames());
                    drain_traces(&mut st, &mut engine);
                    st.sim_time = engine.now();
                    let checkpoint = match engine.into_result() {
                        Ok(mut r) => {
                            // `into_result` only sees the frames since the last drain (the one just
                            // above left `Sim`'s copy empty); `st.frames` is the accumulated record
                            // of the whole run, so `GetResult` reattaches it here rather than off a
                            // truncated `r.frames`. One clone, once, at the end of a run, and the
                            // checkpoint below takes it over rather than cloning again.
                            r.frames = st.frames.clone();
                            st.result = Some(export::result(&r, &st.run_id));
                            st.state = State::Complete;
                            st.finished_at_wall_ns = Some(wall_now_ns());
                            // Everything `runs/index.json` needs but the stride, captured now while
                            // `r` and `st` are still borrowed; the stride itself is not known until
                            // the render below has thinned `replicas.jsonl`, which happens off this
                            // lock. Issao, 2026-09-10: a released run this checkpoint answers for is
                            // not in the replay picker's index, and cannot be opened as a recording.
                            let index_fields = export::IndexFields {
                                run_id: st.run_id.clone(),
                                name: r.scenario.name.clone(),
                                routing: r.routing_label.clone(),
                                scenario_file: None,
                                sim_start_unix_ns: export::sim_start(&r),
                                sim_end_unix_ns: r.measured_to,
                                sample_interval_ms: r.scenario.sample_interval_ms,
                                replicas: r.scenario.replicas,
                                replica_sample_stride: 0,
                            };
                            Some((Checkpoint::inputs(&st, std::mem::take(&mut r.frames)), index_fields))
                        }
                        Err(why) => {
                            st.state = State::Failed;
                            st.error = why;
                            st.finished_at_wall_ns = Some(wall_now_ns());
                            None
                        }
                    };
                    run.changed.notify_all();
                    checkpoint
                };
                if let Some((inputs, mut index_fields)) = checkpoint {
                    let rendered = inputs.render();
                    let stride = rendered.replica_sample_stride;
                    rendered.write(&reg.root);
                    if let Some(stride) = stride {
                        index_fields.replica_sample_stride = stride;
                        if let Err(e) = export::append_to_index(&reg.root, &index_fields) {
                            println!("checkpoint {}: index.json: {e}", index_fields.run_id);
                        }
                    }
                    let mut st = run.lock();
                    st.terminal_checkpoint = true;
                    run.changed.notify_all();
                }
            }
        }
    }
}

/// What the run thread folds into the state after one `advance_to`, whatever its verdict. The
/// frames the engine closed in that chunk are kept even when the chunk failed: they are history a
/// viewer was promised, and a failure that erases the last second before it is harder to diagnose
/// than one that keeps it. A finished engine requests its own stop in the same critical section as
/// its last frames, which is what lets a subscription mark the last frame final on first sight.
fn absorb(st: &mut RunState, engine: &mut Sim, outcome: Result<(), String>) {
    st.frames.extend(engine.drain_frames());
    drain_traces(st, engine);
    st.sim_time = engine.now();
    match outcome {
        Ok(()) => {
            if engine.finished() {
                st.stop_requested = true;
            }
        }
        Err(why) => {
            st.state = State::Failed;
            st.error = why;
        }
    }
}

/// The traces the chunk retained, into the ring. Encoding happens here, on the run thread and
/// under the lock; at a 5 % sample a one-second chunk of a 70 rps run is three or four journeys,
/// a few KB, which is cheaper than the frame copy beside it.
fn drain_traces(st: &mut RunState, engine: &mut Sim) {
    for t in engine.drain_traces() {
        st.traces.push(t);
    }
}

/// The checkpoint: the run as it stands, in the documents `export.rs` writes for a finished run,
/// under `runs/<run_id>/` of the served directory. The engine cannot be snapshotted yet
/// (`Leaf::snapshot` is unimplemented), so what is saved is what a viewer could have seen: the
/// status, the scenario, every frame as a fleet-scope update, the per-replica rows of a finished
/// run within `export::REPLICA_ROWS_BUDGET_BYTES`, the result once there is one, and the sampled
/// traces if any. The inputs are gathered under the run lock and everything is rendered and written
/// outside it. A write failure is logged and not fatal: the run is still in memory.
struct Checkpoint {
    run_id: String,
    docs: Vec<(&'static str, String)>,
    traces: Vec<RequestTrace>,
    /// The stride chosen for this checkpoint's `replicas.jsonl`, the same number `checkpoint.json`
    /// carries — `None` when there were no frames to thin, in which case there is nothing worth
    /// listing in `runs/index.json` either.
    replica_sample_stride: Option<usize>,
}

/// What a checkpoint is rendered from, owned, so the rendering happens off the lock.
struct CheckpointInputs {
    run_id: String,
    status: RunStatus,
    scenario: Scenario,
    frames: Vec<Frame>,
    result: Option<wire::RunResult>,
    /// Measured traces only, like `sim-run export`'s `traces.jsonl`.
    traces: Vec<RequestTrace>,
    terminal: bool,
}

impl Checkpoint {
    /// The inputs from the state, with the frames cloned: the idle checkpoint and a run that
    /// failed mid-chunk take this route; a completed run hands over the clone it made anyway.
    fn from_state(st: &RunState) -> CheckpointInputs {
        Checkpoint::inputs(st, st.frames.clone())
    }

    fn inputs(st: &RunState, frames: Vec<Frame>) -> CheckpointInputs {
        let measured_from = EPOCH_BASE + (st.scenario.warmup_s * 1e9) as Nanos;
        CheckpointInputs {
            run_id: st.run_id.clone(),
            status: st.status(),
            scenario: st.scenario.clone(),
            frames,
            result: st.result.clone(),
            traces: {
                // Oldest first, the order `sim-run export` writes them in.
                let mut traces: Vec<RequestTrace> = st
                    .traces
                    .newest_first()
                    .filter(|e| e.trace.record.arrived_at >= measured_from)
                    .map(|e| e.trace.clone())
                    .collect();
                traces.reverse();
                traces
            },
            terminal: st.is_terminal(),
        }
    }

    fn write(&self, root: &std::path::Path) {
        let dir = root.join("runs").join(&self.run_id);
        for (name, body) in &self.docs {
            if let Err(e) = std::fs::create_dir_all(&dir).and_then(|_| std::fs::write(dir.join(name), body)) {
                println!("checkpoint {}: {}: {e}", self.run_id, dir.join(name).display());
            }
        }
        if !self.traces.is_empty() {
            if let Err(e) = export::export_traces(&self.traces, &dir, export::DEFAULT_TRACE_BUDGET_BYTES) {
                println!("checkpoint {}: traces: {e}", self.run_id);
            }
        }
    }
}

impl CheckpointInputs {
    fn render(self) -> Checkpoint {
        let sc = &self.scenario;
        let mut replica_sample_stride = None;
        let mut docs = vec![
            ("status.json", wire::run_status_json(&self.status) + "\n"),
            ("scenario.txt", sc.to_text()),
        ];
        let spec = RowSpec { target: Target::Fleet, metrics: Vec::new(), percentiles: export::PERCENTILES.to_vec() };
        let n = self.frames.len();
        let mut lines = String::new();
        for (i, f) in self.frames.iter().enumerate() {
            if let Some(row) = row(f, sc, &spec) {
                let u = SubscriptionUpdate {
                    subscription_id: CHECKPOINT_SUBSCRIPTION_ID.to_string(),
                    sim_time_unix_ns: f.t,
                    realtime_factor: self.status.realtime_factor,
                    row,
                    is_final: self.terminal && i + 1 == n,
                };
                lines.push_str(&wire::subscription_update_json(&u));
                lines.push('\n');
            }
        }
        docs.push(("fleet.jsonl", lines));
        if self.terminal && n > 0 {
            // The per-replica rows, thinned to the export's budget. The stride is sized from the
            // last sample's rows rather than by rendering every sample to measure it, which for a
            // ten-thousand-replica run is the difference between seconds and minutes; a busy
            // sample is a fair proxy for the rest, and the budget is a download size, not a
            // contract. `checkpoint.json` carries the stride, as `index.json` does for an export.
            let replica_lines = |s: usize| -> String {
                let f = &self.frames[s];
                let mut out = String::new();
                for id in 0..f.replicas.len() {
                    let rspec = RowSpec { target: Target::Replica(id as u64), metrics: Vec::new(), percentiles: Vec::new() };
                    if let Some(row) = row(f, sc, &rspec) {
                        let u = SubscriptionUpdate {
                            subscription_id: CHECKPOINT_SUBSCRIPTION_ID.to_string(),
                            sim_time_unix_ns: f.t,
                            realtime_factor: self.status.realtime_factor,
                            row,
                            is_final: s + 1 == n && id + 1 == f.replicas.len(),
                        };
                        out.push_str(&wire::subscription_update_json(&u));
                        out.push('\n');
                    }
                }
                out
            };
            let last = replica_lines(n - 1);
            let estimate = vec![last.len() as u64; n];
            let stride = export::replica_sample_stride(&estimate, export::REPLICA_ROWS_BUDGET_BYTES);
            let mut lines = String::new();
            for s in (0..n).filter(|&s| export::is_replica_sample(s, n, stride)) {
                if s + 1 == n {
                    lines.push_str(&last);
                } else {
                    lines.push_str(&replica_lines(s));
                }
            }
            docs.push(("replicas.jsonl", lines));
            docs.push(("checkpoint.json", format!("{{\"replica_sample_stride\":{stride},\"frames\":{n}}}\n")));
            replica_sample_stride = Some(stride);
        }
        if let Some(r) = &self.result {
            docs.push(("result.json", wire::run_result_json(r) + "\n"));
        }
        Checkpoint { run_id: self.run_id, docs, traces: self.traces, replica_sample_stride }
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
    wire::METRIC_PREEMPTIONS_PER_S,
    wire::METRIC_RETRIES_PER_S,
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
    wire::METRIC_WARMING_REPLICAS,
    wire::METRIC_DRAINING_REPLICAS,
    wire::METRIC_GPU_UTILIZATION,
    wire::METRIC_GPU_USEFUL_FRACTION,
    wire::METRIC_GPU_COMPUTE_BOUND_FRACTION,
    wire::METRIC_TRUE_SPEED_MULTIPLIER,
    wire::METRIC_PREFIX_HIT_RATE,
    wire::METRIC_TIER_UTILIZATION,
    wire::METRIC_TIER_BANDWIDTH_UTILIZATION,
];
pub const REPLICA_METRICS: &[i32] = &[
    wire::METRIC_QUEUED_SEQS,
    wire::METRIC_RUNNING_SEQS,
    wire::METRIC_KV_UTILIZATION,
    METRIC_KV_TOKENS_RESIDENT,
    METRIC_STEP_TIME,
    wire::METRIC_GPU_UTILIZATION,
    wire::METRIC_GPU_USEFUL_FRACTION,
    wire::METRIC_GPU_COMPUTE_BOUND_FRACTION,
    wire::METRIC_REPLICA_STATE,
    wire::METRIC_TRUE_SPEED_MULTIPLIER,
    wire::METRIC_TTFT,
    wire::METRIC_PREFIX_HIT_RATE,
    wire::METRIC_PREEMPTIONS_PER_S,
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

/// Exact percentiles of a per-replica scalar across the fleet at one instant, nearest-rank like
/// `Distribution::exact`'s windowed sample. The question this answers is never "what is the average"
/// but "how many replicas sit idle while others saturate", so it is a distribution over replicas
/// rather than over time. `None` when no replica has a finite reading.
/// The memory-tier gauges of a frame at fleet scope, `(METRIC_TIER_UTILIZATION,
/// METRIC_TIER_BANDWIDTH_UTILIZATION)`, each a value and a per-tier distribution.
///
/// The encoding, WIRE.md-style: the value is the headline tier, DRAM's fill for 27 and the shared
/// fabric's busy fraction for 28 (both paths' transfer time over the window, since every migration
/// crosses the one fabric). The distribution carries the per-tier reading, and its `percentile` slots
/// are not percentiles but tier ids: 1 the cluster DRAM pool, 2 the cluster SSD pool, section 7.2's
/// numbering, with `value[k]` that tier's fill (27) or path busy fraction (28). Tier 2 is present
/// only when the scenario has an SSD pool. `count` is the number of tiers, `min`/`max`/`mean` are
/// over them, and nothing is merged from a histogram. Every reading is clamped to [0, 1]: a fill can
/// exceed one only through a per-replica cap larger than the pool, and a busy fraction only when
/// more than one transfer is in flight on an unlimited fabric.
pub fn tier_gauges(f: &Frame, sc: &Scenario) -> (Distribution, Distribution) {
    let n = f.replicas.len().max(1) as f64;
    let dram_cap = if sc.dram_pool_tokens > 0.0 { sc.dram_pool_tokens } else { sc.dram_capacity_tokens() * n };
    let window_ns = (sc.sample_interval_ms * 1e6).max(1e-9);
    let unit = |v: f64| v.clamp(0.0, 1.0);
    let mut fill = vec![(1.0, unit(f.tier_dram_used as f64 / dram_cap.max(1.0)))];
    let mut busy = vec![(1.0, unit(f.tier_dram_busy_ns as f64 / window_ns))];
    if sc.ssd_pool_tokens > 0.0 {
        fill.push((2.0, unit(f.tier_ssd_used as f64 / sc.ssd_pool_tokens)));
        busy.push((2.0, unit(f.tier_ssd_busy_ns as f64 / window_ns)));
    }
    let over_tiers = |slots: Vec<(f64, f64)>| {
        let values: Vec<f64> = slots.iter().map(|(_, v)| *v).collect();
        Distribution {
            count: values.len() as u64,
            mean: values.iter().sum::<f64>() / values.len() as f64,
            min: values.iter().cloned().fold(f64::INFINITY, f64::min),
            max: values.iter().cloned().fold(f64::NEG_INFINITY, f64::max),
            percentile: slots.iter().map(|(id, _)| *id).collect(),
            value: values,
            from_merged_histogram: false,
        }
    };
    (over_tiers(fill), over_tiers(busy))
}

/// The scalar the fleet row carries beside each tier distribution: DRAM's fill, and the fabric's
/// busy fraction, the two paths' transfer time together over the window.
pub fn tier_values(f: &Frame, sc: &Scenario) -> (f64, f64) {
    let (fill, _) = tier_gauges(f, sc);
    let window_ns = (sc.sample_interval_ms * 1e6).max(1e-9);
    let fabric = ((f.tier_dram_busy_ns + f.tier_ssd_busy_ns) as f64 / window_ns).clamp(0.0, 1.0);
    (fill.value[0], fabric)
}

pub fn distribution_over_replicas(values: &[f64], percentiles: &[f64]) -> Option<Distribution> {
    let mut finite: Vec<f64> = values.iter().copied().filter(|v| v.is_finite()).collect();
    if finite.is_empty() {
        return None;
    }
    finite.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let n = finite.len();
    let sum: f64 = finite.iter().sum();
    let value = percentiles
        .iter()
        .map(|q| {
            let rank = ((q / 100.0) * n as f64).ceil().max(1.0) as usize;
            finite[rank.min(n) - 1]
        })
        .collect();
    Some(Distribution {
        count: n as u64,
        mean: sum / n as f64,
        min: finite[0],
        max: finite[n - 1],
        percentile: percentiles.to_vec(),
        value,
        from_merged_histogram: false,
    })
}

/// One `MetricRow` for one frame: the raw sample cadence, `row_over` on a window of one.
pub fn row(f: &Frame, sc: &Scenario, spec: &RowSpec) -> Option<MetricRow> {
    row_over(std::slice::from_ref(f), sc, spec)
}

/// How many recorded frames a smoothing window covers: the frames whose instant lies in
/// `(t - window, t]` at the engine's cadence, which is `ceil(window / interval)`, never fewer than
/// one. Zero is the raw cadence, and so is any window shorter than one sample.
pub fn frames_in_window(smoothing_window_ns: u64, sample_interval_ns: Nanos) -> usize {
    let iv = sample_interval_ns.max(1);
    (smoothing_window_ns.div_ceil(iv)).max(1) as usize
}

/// The mean of the finite values in `it`, NaN when there are none. `MetricRow::value` drops NaN,
/// so a gauge undefined in every frame of the window stays absent, as it is in a raw row.
fn mean_finite(it: impl Iterator<Item = f64>) -> f64 {
    let (mut sum, mut n) = (0.0, 0usize);
    for v in it.filter(|v| v.is_finite()) {
        sum += v;
        n += 1;
    }
    if n == 0 { f64::NAN } else { sum / n as f64 }
}

/// One `MetricRow` over a trailing window of frames, the last of which is the sample's own:
/// `OpenSubscriptionRequest.smoothing_window_ns` as WIRE.md defines it. `None` when the target
/// names a replica the scenario has not got.
///
/// The rules, chosen so that a window of one frame is bit-for-bit the raw row:
///
/// - a gauge or a rate is the mean of its per-frame values, each computed exactly as the raw row
///   computes it (every frame covers one sample interval, so the mean of per-frame rates is the
///   rate over the window);
/// - a fraction of requests (SLO attainment) or of time (compute-bound share, prefix hit rate) is
///   the ratio of the window's sums, so a frame that ended two requests does not weigh as much as
///   one that ended two hundred;
/// - a latency distribution is the merge of the frames' histograms, so its p99 is the p99 of every
///   request that finished in the window;
/// - a distribution over replicas (GPU, KV) is taken over each replica's mean across the window,
///   the question being how many replicas sat idle over the window while others saturated;
/// - a count of replicas by state, and a replica's own state, are read at the sample: a fraction
///   of a replica is not a count, and the Machines page enumerates ids from the ready count.
pub fn row_over(window: &[Frame], sc: &Scenario, spec: &RowSpec) -> Option<MetricRow> {
    let last = window.last()?;
    let mut row = MetricRow::new(spec.target);
    let cap = sc.kv_capacity_tokens.max(1.0);
    let iv_s = (sc.sample_interval_ms / 1000.0).max(1e-9);
    let window_ns = (sc.sample_interval_ms * 1e6).max(1e-9);
    let avg = |per_frame: &dyn Fn(&Frame) -> f64| mean_finite(window.iter().map(per_frame));
    let sum = |per_frame: &dyn Fn(&Frame) -> u64| window.iter().map(per_frame).sum::<u64>();
    match spec.target {
        Target::Replica(id) => {
            let idx = usize::try_from(id).ok()?;
            let r = last.replicas.get(idx)?;
            // The replica's sample in every frame of the window that has one.
            let samples: Vec<&sim_metrics::ReplicaSample> = window.iter().filter_map(|f| f.replicas.get(idx)).collect();
            let avg_r = |g: &dyn Fn(&sim_metrics::ReplicaSample) -> f64| mean_finite(samples.iter().map(|s| g(s)));
            let sum_r = |g: &dyn Fn(&sim_metrics::ReplicaSample) -> u64| samples.iter().map(|s| g(s)).sum::<u64>();
            let mut value = |m: i32, v: f64| {
                if spec.wants(m) {
                    row.value(m, v);
                }
            };
            value(wire::METRIC_QUEUED_SEQS, avg_r(&|r| r.queued as f64));
            value(wire::METRIC_RUNNING_SEQS, avg_r(&|r| r.running as f64));
            value(wire::METRIC_KV_UTILIZATION, avg_r(&|r| r.kv_tokens as f64 / cap));
            value(METRIC_KV_TOKENS_RESIDENT, avg_r(&|r| r.kv_tokens as f64));
            // Utilization is the time-in-step share (what a GPU counter reports), useful the share of
            // the maximum possible work; both are ratios of the window's sums, so a frame in which
            // the replica stepped little weighs what it did, not a full frame's worth. Neither sum
            // exceeds the window by construction, but the ratio is clamped anyway rather than trust
            // an upstream invariant. Compute-bound is busy's complement of `WASTED_GPU_FRACTION`;
            // 0/0 (never stepped) is NaN, which `value` drops.
            let span_ns = samples.len() as f64 * window_ns;
            value(wire::METRIC_GPU_UTILIZATION, (sum_r(&|r| r.busy_ns) as f64 / span_ns).min(1.0));
            value(wire::METRIC_GPU_USEFUL_FRACTION, (sum_r(&|r| r.useful_ns) as f64 / span_ns).min(1.0));
            value(
                wire::METRIC_GPU_COMPUTE_BOUND_FRACTION,
                sum_r(&|r| r.compute_ns) as f64 / sum_r(&|r| r.busy_ns) as f64,
            );
            value(wire::METRIC_REPLICA_STATE, r.state as f64);
            value(wire::METRIC_TRUE_SPEED_MULTIPLIER, avg_r(&|r| r.speed));
            value(wire::METRIC_PREEMPTIONS_PER_S, avg_r(&|r| r.preemptions as f64 / (window_ns / 1e9)));
            // `prompt_tokens` accumulates on every admission regardless of a prefix model, so gating
            // on it alone would put a permanent, meaningless 0% reading on every scenario that never
            // asked for prefix caching. NaN when there is no prefix model, which `value` drops; a
            // real cache with nothing admitted this window is also NaN (0/0) for the same reason.
            value(
                wire::METRIC_PREFIX_HIT_RATE,
                if sc.prefix_roots > 0 {
                    sum_r(&|r| r.prefix_hit_tokens) as f64 / sum_r(&|r| r.prompt_tokens) as f64
                } else {
                    f64::NAN
                },
            );
            // Seconds as a double, like every other duration gauge and like export.rs's replica
            // row — not a distribution. Zero means the replica has not stepped yet, and a
            // duration of nothing is a gap, so only the frames in which it stepped count. Raw
            // `row.value` rather than the closure: this is the closure's last use above, and a
            // borrow of `row` through it must not still be live when the direct calls below
            // borrow `row` again.
            let stepped: Vec<f64> = samples.iter().filter(|s| s.last_step_ns > 0).map(|s| s.last_step_ns as f64).collect();
            if spec.wants(METRIC_STEP_TIME) && !stepped.is_empty() {
                row.value(METRIC_STEP_TIME, mean_finite(stepped.into_iter()) / 1e9);
            }
            // A distribution rather than a value so the client's histogram code path serves both;
            // a mean of one bucket, no percentiles, `from_merged_histogram: false` because it was
            // never a histogram. Omitted, not zero, when nothing in the replica got a first token
            // in the window: a mean of nothing is a gap, not 0 ns.
            let ttft_count = sum_r(&|r| r.ttft_count);
            if spec.wants(wire::METRIC_TTFT) && ttft_count > 0 {
                row.distribution(
                    wire::METRIC_TTFT,
                    Distribution {
                        count: ttft_count,
                        mean: sum_r(&|r| r.ttft_sum_ns) as f64 / ttft_count as f64,
                        min: 0.0,
                        max: 0.0,
                        percentile: Vec::new(),
                        value: Vec::new(),
                        from_merged_histogram: false,
                    },
                );
            }
        }
        Target::Fleet => {
            // The fleet's means are over the replicas that are there to serve: an absent slot (0) or
            // one inside its cold start (4) has no device behind it yet. Every slot in a fixed fleet.
            // Membership is read at the sample, like the state counts below.
            let is_serving = |r: &sim_metrics::ReplicaSample| !matches!(r.state, 0 | 4);
            let serving: Vec<usize> = (0..last.replicas.len()).filter(|&i| is_serving(&last.replicas[i])).collect();
            let n = serving.len().max(1) as f64;
            let ended = sum(&|f| f.completed + f.rejected + f.timed_out);
            let mut value = |m: i32, v: f64| {
                if spec.wants(m) {
                    row.value(m, v);
                }
            };
            value(wire::METRIC_OFFERED_RPS, avg(&|f| f.offered_rps));
            value(wire::METRIC_ADMITTED_RPS, avg(&|f| f.admitted as f64 / iv_s));
            value(wire::METRIC_COMPLETED_RPS, avg(&|f| f.completed as f64 / iv_s));
            value(wire::METRIC_REJECTED_RPS, avg(&|f| f.rejected as f64 / iv_s));
            value(wire::METRIC_OUTPUT_TOKENS_PER_S, avg(&|f| f.output_tokens as f64 / iv_s));
            value(wire::METRIC_GOODPUT_TOKENS_PER_S, avg(&|f| f.goodput_tokens as f64 / iv_s));
            // Rates, and explicit zeros: a fleet with headroom preempts nothing, and the panel must
            // read that as 0/s rather than as a metric nobody serves (U115).
            value(wire::METRIC_PREEMPTIONS_PER_S, avg(&|f| f.preemptions as f64 / iv_s));
            value(wire::METRIC_RETRIES_PER_S, avg(&|f| f.retries as f64 / iv_s));
            value(wire::METRIC_QUEUED_SEQS, avg(&|f| f.replicas.iter().map(|r| u64::from(r.queued)).sum::<u64>() as f64));
            value(wire::METRIC_RUNNING_SEQS, avg(&|f| f.replicas.iter().map(|r| u64::from(r.running)).sum::<u64>() as f64));
            value(wire::METRIC_KV_UTILIZATION, avg(&|f| f.replicas.iter().map(|r| r.kv_tokens).sum::<u64>() as f64 / cap / n));
            value(METRIC_KV_TOKENS_RESIDENT, avg(&|f| f.replicas.iter().map(|r| r.kv_tokens).sum::<u64>() as f64));
            value(wire::METRIC_LOAD_IMBALANCE_CV, avg(&imbalance));
            // Same denominator as the scorecard: everything that ended in the window, shed
            // included, so a policy cannot look good by shedding.
            value(
                wire::METRIC_SLO_ATTAINMENT,
                if ended == 0 { f64::NAN } else { sum(&|f| f.within_slo) as f64 / ended as f64 },
            );
            // `ReplicaSample::state`'s doc comment: READY is 1 or 2, a crashed replica (3) is still
            // in `f.replicas` so the frame keeps a slot per replica id but it is not ready, and a
            // client deriving "how many are down" as fleet size minus this must see it drop. 4 and 5
            // are the autoscaler's in-between states, 0 a slot it has not filled.
            let live: Vec<&sim_metrics::ReplicaSample> =
                last.replicas.iter().filter(|r| matches!(r.state, 1 | 2)).collect();
            value(wire::METRIC_READY_REPLICAS, live.len() as f64);
            value(wire::METRIC_WARMING_REPLICAS, last.replicas.iter().filter(|r| r.state == 4).count() as f64);
            value(wire::METRIC_DRAINING_REPLICAS, last.replicas.iter().filter(|r| r.state == 5).count() as f64);
            value(
                wire::METRIC_TRUE_SPEED_MULTIPLIER,
                avg(&|f| {
                    let live: Vec<&sim_metrics::ReplicaSample> = f.replicas.iter().filter(|r| matches!(r.state, 1 | 2)).collect();
                    if live.is_empty() { 0.0 } else { live.iter().map(|r| r.speed).sum::<f64>() / live.len() as f64 }
                }),
            );
            // GPU utilization, GPU useful and the KV-utilization band: the mean is never the
            // interesting number, it is how many replicas sit idle while others saturate. Same
            // percentiles as the latency distributions below, falling back to 50/90/99 when the spec
            // asked for none, because "no percentiles requested" means "give me the defaults", not
            // "give me none". Each replica's reading is its mean over the window; for the two time
            // shares that is the ratio of its window sums to the span it was sampled over.
            let per_replica = |i: usize, g: &dyn Fn(&sim_metrics::ReplicaSample) -> f64| {
                mean_finite(window.iter().filter_map(|f| f.replicas.get(i)).map(g))
            };
            let share = |i: usize, g: &dyn Fn(&sim_metrics::ReplicaSample) -> u64| {
                let samples: Vec<u64> = window.iter().filter_map(|f| f.replicas.get(i)).map(g).collect();
                (samples.iter().sum::<u64>() as f64 / (samples.len() as f64 * window_ns)).min(1.0)
            };
            let gpu: Vec<f64> = serving.iter().map(|&i| share(i, &|r| r.busy_ns)).collect();
            let gpu_useful: Vec<f64> = serving.iter().map(|&i| share(i, &|r| r.useful_ns)).collect();
            let kv_ratios: Vec<f64> = serving.iter().map(|&i| per_replica(i, &|r| r.kv_tokens as f64 / cap)).collect();
            let busy_sum = sum(&|f| f.replicas.iter().map(|r| r.busy_ns).sum::<u64>());
            let compute_sum = sum(&|f| f.replicas.iter().map(|r| r.compute_ns).sum::<u64>());
            value(wire::METRIC_GPU_UTILIZATION, gpu.iter().sum::<f64>() / n);
            value(wire::METRIC_GPU_USEFUL_FRACTION, gpu_useful.iter().sum::<f64>() / n);
            value(wire::METRIC_GPU_COMPUTE_BOUND_FRACTION, compute_sum as f64 / busy_sum as f64);
            // Ratio of sums, not a mean of per-replica ratios, so a replica with no prompt tokens
            // this window does not skew the fleet number. Gated on `prefix_roots` for the same
            // reason as the replica branch above: `prompt_tokens` is nonzero on every scenario, so
            // the ratio alone can't tell "no cache" from "cache, nothing admitted" (both NaN-free
            // zero would be misleading; NaN, which `value` drops, is correct for the former).
            let prefix_hit_rate = if sc.prefix_roots > 0 {
                let prompt_sum = sum(&|f| f.replicas.iter().map(|r| r.prompt_tokens).sum::<u64>());
                let hit_sum = sum(&|f| f.replicas.iter().map(|r| r.prefix_hit_tokens).sum::<u64>());
                hit_sum as f64 / prompt_sum as f64
            } else {
                f64::NAN
            };
            value(wire::METRIC_PREFIX_HIT_RATE, prefix_hit_rate);
            let default_percentiles = [50.0, 90.0, 99.0];
            let percentiles =
                if spec.percentiles.is_empty() { &default_percentiles[..] } else { &spec.percentiles[..] };
            if spec.wants(wire::METRIC_GPU_UTILIZATION) {
                if let Some(d) = distribution_over_replicas(&gpu, percentiles) {
                    row.distribution(wire::METRIC_GPU_UTILIZATION, d);
                }
            }
            if spec.wants(wire::METRIC_GPU_USEFUL_FRACTION) {
                if let Some(d) = distribution_over_replicas(&gpu_useful, percentiles) {
                    row.distribution(wire::METRIC_GPU_USEFUL_FRACTION, d);
                }
            }
            if spec.wants(wire::METRIC_KV_UTILIZATION) {
                if let Some(d) = distribution_over_replicas(&kv_ratios, percentiles) {
                    row.distribution(wire::METRIC_KV_UTILIZATION, d);
                }
            }
            // The memory tiers are cluster-scoped, so they are fleet gauges and no replica row
            // carries them; see `tier_gauges` for the per-tier encoding. Over a window each tier's
            // reading is its mean, slot by slot: the slots are the scenario's tiers, the same in
            // every frame.
            if spec.wants(wire::METRIC_TIER_UTILIZATION) || spec.wants(wire::METRIC_TIER_BANDWIDTH_UTILIZATION) {
                let (fill, busy) = tier_gauges_over(window, sc);
                let fill_v = avg(&|f| tier_values(f, sc).0);
                let fabric_v = avg(&|f| tier_values(f, sc).1);
                if spec.wants(wire::METRIC_TIER_UTILIZATION) {
                    row.value(wire::METRIC_TIER_UTILIZATION, fill_v);
                    row.distribution(wire::METRIC_TIER_UTILIZATION, fill);
                }
                if spec.wants(wire::METRIC_TIER_BANDWIDTH_UTILIZATION) {
                    row.value(wire::METRIC_TIER_BANDWIDTH_UTILIZATION, fabric_v);
                    row.distribution(wire::METRIC_TIER_BANDWIDTH_UTILIZATION, busy);
                }
            }
            for m in [wire::METRIC_TTFT, wire::METRIC_ITL, wire::METRIC_E2E, wire::METRIC_QUEUE_WAIT] {
                if spec.wants(m) {
                    row.distribution(m, distribution(&merged_histogram(window, m), &spec.percentiles));
                }
            }
        }
    }
    Some(row)
}

/// The frame's windowed histogram for one of the four latency metrics.
fn latency_histogram(f: &Frame, metric: i32) -> &SparseHistogram {
    match metric {
        wire::METRIC_TTFT => &f.ttft,
        wire::METRIC_ITL => &f.itl_max,
        wire::METRIC_E2E => &f.e2e,
        _ => &f.queue_wait,
    }
}

/// The frames' histograms of one latency merged bucket-wise; a window of one is that frame's own.
fn merged_histogram(window: &[Frame], metric: i32) -> SparseHistogram {
    let mut it = window.iter().map(|f| latency_histogram(f, metric));
    let mut h = it.next().cloned().unwrap_or_default();
    for other in it {
        h.merge(other);
    }
    h
}

/// `tier_gauges` over a window: each tier's fill and busy fraction averaged across the frames. The
/// tier ids and their order are the scenario's, identical in every frame, so the slots line up.
fn tier_gauges_over(window: &[Frame], sc: &Scenario) -> (Distribution, Distribution) {
    let per_frame: Vec<(Distribution, Distribution)> = window.iter().map(|f| tier_gauges(f, sc)).collect();
    let (last_fill, last_busy) = per_frame.last().cloned().expect("a window has at least one frame");
    let mean_slots = |busy: bool, template: &Distribution| {
        let values: Vec<f64> = (0..template.value.len())
            .map(|k| mean_finite(per_frame.iter().map(|p| if busy { p.1.value[k] } else { p.0.value[k] })))
            .collect();
        Distribution {
            count: values.len() as u64,
            mean: values.iter().sum::<f64>() / values.len() as f64,
            min: values.iter().cloned().fold(f64::INFINITY, f64::min),
            max: values.iter().cloned().fold(f64::NEG_INFINITY, f64::max),
            percentile: template.percentile.clone(),
            value: values,
            from_merged_histogram: false,
        }
    };
    (mean_slots(false, &last_fill), mean_slots(true, &last_busy))
}

/// Coefficient of variation of queued-plus-running across replicas at the sample, the per-instant
/// form of `RunResult::load_imbalance_cv`. NaN when the fleet is idle, which the row omits.
fn imbalance(f: &Frame) -> f64 {
    // An absent or warming slot holds nothing by construction; its zero is not the router's doing.
    let serving = f.replicas.iter().filter(|r| !matches!(r.state, 0 | 4));
    let n = serving.clone().count();
    if n == 0 {
        return f64::NAN;
    }
    let loads = serving.map(|r| f64::from(r.queued + r.running));
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
    fn live_replica_step_time_is_a_value_in_seconds() {
        // U55: the live server used to carry replica STEP_TIME as a one-sample distribution while
        // export.rs and adapter.ts both treat it as a plain value in seconds, so the live heatmap's
        // step time read NaN. `row` must match export.rs's replica row.
        let frame = Frame {
            t: 0,
            offered_rps: 0.0,
            admitted: 0,
            completed: 0,
            rejected: 0,
            timed_out: 0,
            within_slo: 0,
            output_tokens: 0,
            goodput_tokens: 0,
            ttft: SparseHistogram::default(),
            itl_max: SparseHistogram::default(),
            e2e: SparseHistogram::default(),
            queue_wait: SparseHistogram::default(),
            preemptions: 0,
            retries: 0,
            tier_dram_used: 0,
            tier_ssd_used: 0,
            tier_dram_busy_ns: 0,
            tier_ssd_busy_ns: 0,
            replicas: vec![sim_metrics::ReplicaSample { last_step_ns: 2_000_000, ..Default::default() }],
        };
        let sc = Scenario::default();
        let spec = RowSpec { target: Target::Replica(0), metrics: Vec::new(), percentiles: Vec::new() };
        let r = row(&frame, &sc, &spec).expect("replica 0 exists");
        assert_eq!(r.values.iter().find(|(m, _)| *m == METRIC_STEP_TIME).map(|(_, v)| *v), Some(0.002));
        assert!(r.distributions.iter().all(|(m, _)| *m != METRIC_STEP_TIME));
    }

    #[test]
    fn fleet_gpu_utilization_is_the_mean_and_a_distribution_over_replicas() {
        // U94b: one idle replica and one fully busy one for a whole window, so the mean is exactly
        // 0.5 and every percentile lands in [0, 1] rather than collapsing to a single value.
        let sc = Scenario::default();
        let window_ns = (sc.sample_interval_ms * 1e6) as Nanos;
        let frame = Frame {
            t: 0,
            offered_rps: 0.0,
            admitted: 0,
            completed: 0,
            rejected: 0,
            timed_out: 0,
            within_slo: 0,
            output_tokens: 0,
            goodput_tokens: 0,
            ttft: SparseHistogram::default(),
            itl_max: SparseHistogram::default(),
            e2e: SparseHistogram::default(),
            queue_wait: SparseHistogram::default(),
            preemptions: 0,
            retries: 0,
            tier_dram_used: 0,
            tier_ssd_used: 0,
            tier_dram_busy_ns: 0,
            tier_ssd_busy_ns: 0,
            // `state: 1`, READY: the default 0 is ABSENT, a slot with no device, which the fleet
            // means leave out, as the engine's samples always carry a real state.
            // The second replica stepped the whole window at a quarter of the work it could do:
            // utilization (busy) 1.0, useful 0.25. Fleet means over the two are 0.5 and 0.125.
            replicas: vec![
                sim_metrics::ReplicaSample { busy_ns: 0, useful_ns: 0, state: 1, ..Default::default() },
                sim_metrics::ReplicaSample { busy_ns: window_ns, useful_ns: window_ns / 4, state: 1, ..Default::default() },
            ],
        };
        let spec = RowSpec { target: Target::Fleet, metrics: Vec::new(), percentiles: Vec::new() };
        let r = row(&frame, &sc, &spec).expect("fleet row");
        let mean_of = |m: i32| r.values.iter().find(|(k, _)| *k == m).map(|(_, v)| *v);
        assert_eq!(mean_of(wire::METRIC_GPU_UTILIZATION), Some(0.5));
        assert_eq!(mean_of(wire::METRIC_GPU_USEFUL_FRACTION), Some(0.125));
        for m in [wire::METRIC_GPU_UTILIZATION, wire::METRIC_GPU_USEFUL_FRACTION] {
            let d = r
                .distributions
                .iter()
                .find(|(k, _)| *k == m)
                .map(|(_, d)| d)
                .unwrap_or_else(|| panic!("metric {m}: distribution over replicas"));
            assert_eq!(d.percentile, vec![50.0, 90.0, 99.0]);
            assert_eq!(d.count, 2);
            assert!(d.value.iter().all(|v| (0.0..=1.0).contains(v)), "{:?}", d.value);
        }
    }

    /// A frame with nothing in it at `t`, for windows built by hand.
    fn blank_frame(t: Nanos) -> Frame {
        Frame {
            t,
            offered_rps: 0.0,
            admitted: 0,
            completed: 0,
            rejected: 0,
            timed_out: 0,
            within_slo: 0,
            output_tokens: 0,
            goodput_tokens: 0,
            ttft: SparseHistogram::default(),
            itl_max: SparseHistogram::default(),
            e2e: SparseHistogram::default(),
            queue_wait: SparseHistogram::default(),
            preemptions: 0,
            retries: 0,
            tier_dram_used: 0,
            tier_ssd_used: 0,
            tier_dram_busy_ns: 0,
            tier_ssd_busy_ns: 0,
            replicas: vec![sim_metrics::ReplicaSample { state: 1, speed: 1.0, ..Default::default() }],
        }
    }

    fn fleet_value(r: &MetricRow, m: i32) -> Option<f64> {
        r.values.iter().find(|(k, _)| *k == m).map(|(_, v)| *v)
    }

    #[test]
    fn a_smoothing_window_covers_ceil_window_over_interval_frames_never_fewer_than_one() {
        let iv = 250_000_000;
        assert_eq!(frames_in_window(0, iv), 1, "zero is the raw cadence");
        assert_eq!(frames_in_window(100_000_000, iv), 1, "shorter than one sample is raw too");
        assert_eq!(frames_in_window(250_000_000, iv), 1);
        assert_eq!(frames_in_window(1_000_000_000, iv), 4);
        assert_eq!(frames_in_window(1_100_000_000, iv), 5, "(t - 1.1 s, t] holds five frames at 250 ms");
        assert_eq!(frames_in_window(30_000_000_000, iv), 120);
        assert_eq!(frames_in_window(120_000_000_000, iv), 480);
    }

    #[test]
    fn an_offered_rate_alternating_0_and_100_smooths_to_50_over_any_even_window() {
        // Issao: "a global selector of a window average ... to make it easier to smooth out
        // variation/oscilatory patterns". The oscillation is one sample long; every window of an
        // even number of samples sees half of each, and a window of one is the raw sample.
        let sc = Scenario::default();
        let iv = ((sc.sample_interval_ms * 1e6) as Nanos).max(1);
        let frames: Vec<Frame> = (0..16)
            .map(|i| {
                let mut f = blank_frame(EPOCH_BASE + (i + 1) as Nanos * iv);
                f.offered_rps = if i % 2 == 0 { 0.0 } else { 100.0 };
                f.replicas[0].queued = if i % 2 == 0 { 0 } else { 10 };
                f
            })
            .collect();
        let spec = RowSpec { target: Target::Fleet, metrics: Vec::new(), percentiles: Vec::new() };
        let last = frames.len() - 1;
        for m in [2usize, 4, 8, 16] {
            let r = row_over(&frames[last + 1 - m..], &sc, &spec).unwrap();
            assert_eq!(fleet_value(&r, wire::METRIC_OFFERED_RPS), Some(50.0), "window of {m}");
            assert_eq!(fleet_value(&r, wire::METRIC_QUEUED_SEQS), Some(5.0), "window of {m}");
        }
        let raw = row(&frames[last], &sc, &spec).unwrap();
        assert_eq!(fleet_value(&raw, wire::METRIC_OFFERED_RPS), Some(100.0));
        let raw = row_over(&frames[last..], &sc, &spec).unwrap();
        assert_eq!(fleet_value(&raw, wire::METRIC_OFFERED_RPS), Some(100.0), "a window of one is the raw row");
        // The replica scope smooths the same way, and its state is the sample's, not a mean.
        let spec = RowSpec { target: Target::Replica(0), metrics: Vec::new(), percentiles: Vec::new() };
        let r = row_over(&frames[last + 1 - 4..], &sc, &spec).unwrap();
        assert_eq!(fleet_value(&r, wire::METRIC_QUEUED_SEQS), Some(5.0));
        assert_eq!(fleet_value(&r, wire::METRIC_REPLICA_STATE), Some(1.0));
    }

    #[test]
    fn a_smoothed_p99_is_the_p99_of_every_request_in_the_window() {
        // Three frames of very different tails. The window's p99 must be the p99 of the
        // concatenated records, not an average of the three p99s, which is a number that is not a
        // percentile of anything.
        let sc = Scenario::default();
        let iv = ((sc.sample_interval_ms * 1e6) as Nanos).max(1);
        let sets: [Vec<u64>; 3] = [
            (1..=100).map(|i| i * 1_000_000).collect(),
            (1..=20).map(|i| i * 50_000_000).collect(),
            vec![3_000_000_000, 4_000_000_000, 400_000_000],
        ];
        let mut all = sim_metrics::Histogram::new();
        let frames: Vec<Frame> = sets
            .iter()
            .enumerate()
            .map(|(i, records)| {
                let mut f = blank_frame(EPOCH_BASE + (i + 1) as Nanos * iv);
                let mut h = sim_metrics::Histogram::new();
                for r in records {
                    h.record(*r);
                    all.record(*r);
                }
                f.ttft = h.to_sparse();
                f.completed = records.len() as u64;
                f
            })
            .collect();
        let spec = RowSpec { target: Target::Fleet, metrics: Vec::new(), percentiles: vec![50.0, 99.0] };
        let r = row_over(&frames, &sc, &spec).unwrap();
        let d = r.distributions.iter().find(|(m, _)| *m == wire::METRIC_TTFT).map(|(_, d)| d).unwrap();
        assert_eq!(d.count, 123);
        assert_eq!(d.value, vec![all.percentile(50.0) as f64, all.percentile(99.0) as f64]);
        assert_eq!(d.min, all.min() as f64);
        assert_eq!(d.max, all.max() as f64);
        assert_eq!(d.mean, all.mean());
        assert!(d.from_merged_histogram);
        let mean_of_p99s: f64 = frames.iter().map(|f| f.ttft.percentile(99.0) as f64).sum::<f64>() / 3.0;
        assert_ne!(d.value[1], mean_of_p99s, "merged, not averaged");
        // The rate over the window is the mean of the per-frame rates.
        let iv_s = sc.sample_interval_ms / 1000.0;
        assert_eq!(fleet_value(&r, wire::METRIC_COMPLETED_RPS), Some(123.0 / 3.0 / iv_s));
    }

    #[test]
    fn step_cap_is_one_minute() {
        assert_eq!(STEP_CAP_NS, 60 * 1_000_000_000);
    }

    fn scenario(duration_s: &str) -> Scenario {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../scenarios/route_p2c.txt");
        let mut sc = Scenario::parse(&std::fs::read_to_string(path).unwrap()).unwrap();
        export::override_key(&mut sc, "duration_s", duration_s).unwrap();
        sc
    }

    fn temp_dir(name: &str) -> crate::test_scratch::ScratchDir {
        crate::test_scratch::scratch(&format!("run-{name}"))
    }

    /// One second of engine: enough closed frames to tell "kept" from "dropped".
    fn advanced_sim() -> (Scenario, Sim) {
        let sc = scenario("100");
        let mut sim = Sim::new(&sc).unwrap();
        sim.advance_to(EPOCH_BASE + 1_000_000_000).unwrap();
        assert!(sim.frames().len() >= 4, "{}", sim.frames().len());
        (sc, sim)
    }

    #[test]
    fn a_failed_chunk_keeps_the_frames_it_closed() {
        let (sc, mut sim) = advanced_sim();
        let closed = sim.frames().len();
        let mut st = RunState::new("r-1".into(), sc, 0.0);
        absorb(&mut st, &mut sim, Err("the leaf refused".into()));
        assert_eq!(st.state, State::Failed);
        assert_eq!(st.error, "the leaf refused");
        assert_eq!(st.frames.len(), closed);
        assert!(sim.frames().is_empty(), "drained, not copied");
        assert_eq!(st.sim_time, sim.now());
        assert!(!st.stop_requested);
    }

    #[test]
    fn a_finished_engine_requests_the_stop_with_its_last_frames() {
        let sc = scenario("20");
        let mut sim = Sim::new(&sc).unwrap();
        let mut st = RunState::new("r-1".into(), sc, 0.0);
        let outcome = sim.advance_to(st.sim_end);
        let closed = sim.frames().len();
        absorb(&mut st, &mut sim, outcome);
        assert!(sim.finished());
        assert!(st.stop_requested);
        assert_eq!(st.state, State::Queued, "the stop is the run thread's to finish");
        assert_eq!(st.frames.len(), closed);
    }

    #[test]
    fn a_checkpoint_is_gathered_under_the_lock_and_rendered_after_it() {
        let (sc, mut sim) = advanced_sim();
        let mut st = RunState::new("r-7".into(), sc, 1.0);
        absorb(&mut st, &mut sim, Ok(()));
        let cp = Checkpoint::from_state(&st).render();
        let names: Vec<&str> = cp.docs.iter().map(|(n, _)| *n).collect();
        assert_eq!(names, ["status.json", "scenario.txt", "fleet.jsonl"], "no result and no replica rows until the run ends");
        let fleet = &cp.docs[2].1;
        assert_eq!(fleet.lines().count(), st.frames.len());
        assert!(!fleet.contains("\"final\":true"), "a live run's checkpoint is not final");

        st.state = State::Failed;
        let cp = Checkpoint::from_state(&st).render();
        let names: Vec<&str> = cp.docs.iter().map(|(n, _)| *n).collect();
        assert_eq!(names, ["status.json", "scenario.txt", "fleet.jsonl", "replicas.jsonl", "checkpoint.json"]);
        let finals: Vec<usize> =
            cp.docs[2].1.lines().enumerate().filter(|(_, l)| l.contains("\"final\":true")).map(|(i, _)| i).collect();
        assert_eq!(finals, [st.frames.len() - 1], "a terminal run's last frame is final");
        // Every sample of a run this small fits the budget: one row per replica per frame.
        assert_eq!(cp.docs[3].1.lines().count(), st.frames.len() * st.scenario.replicas);
        assert!(cp.docs[4].1.contains("\"replica_sample_stride\":1"), "{}", cp.docs[4].1);

        // Written where `export.rs` puts a finished run, from a value that owns no lock.
        let root = temp_dir("checkpoint");
        cp.write(&root);
        let dir = root.join("runs").join("r-7");
        for (name, body) in &cp.docs {
            assert_eq!(std::fs::read_to_string(dir.join(name)).unwrap(), *body);
        }
    }

    #[test]
    fn the_ninth_live_run_is_refused_until_one_ends() {
        let scratch = temp_dir("cap");
        let reg = Arc::new(Registry::new(scratch.to_path_buf(), 3600 * 1_000_000_000));
        // Paced, so none of them finishes during the test.
        let ids: Vec<String> = (0..MAX_LIVE_RUNS).map(|_| reg.start(scenario("100"), 1.0).unwrap()).collect();
        let refused = reg.start(scenario("100"), 1.0).unwrap_err();
        assert_eq!(refused.code, 503, "{}", refused.message);
        assert_eq!(reg.list().len(), MAX_LIVE_RUNS, "the refused run was never registered");

        let first = reg.get(&ids[0]).unwrap();
        first.stop();
        {
            let mut st = first.lock();
            let deadline = Instant::now() + Duration::from_secs(10);
            while !st.is_terminal() {
                assert!(Instant::now() < deadline, "the stopped run never ended: {:?}", st.state);
                st = first.changed.wait_timeout(st, Duration::from_millis(50)).unwrap_or_else(|e| e.into_inner()).0;
            }
        }
        let ninth = reg.start(scenario("100"), 1.0).unwrap();
        assert_eq!(reg.list().len(), MAX_LIVE_RUNS + 1, "a finished run stays listed; only live ones count");
        for id in ids.iter().skip(1).chain([&ninth]) {
            reg.get(id).unwrap().stop();
        }
    }

    /// Polls every few milliseconds, for up to `secs`; the run thread's own cadence is `POLL`.
    fn wait_for(what: &str, secs: u64, mut done: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(secs);
        while !done() {
            assert!(Instant::now() < deadline, "{what} did not happen within {secs} s");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn an_idle_stopped_run_does_not_count_and_is_evicted_at_the_cap() {
        let scratch = temp_dir("cap-idle");
        let reg = Arc::new(Registry::new(scratch.to_path_buf(), 50_000_000));
        // Paced with no lease: idle from birth, stopped by the guard within a few polls.
        let first_id = reg.start(scenario("100"), 1.0).unwrap();
        let first = reg.get(&first_id).unwrap();
        // The seven that must stay live are leased before they start: a 50 ms threshold leaves
        // no honest window to lease them afterwards, and ids are handed out in order.
        let mut live = Vec::new();
        for n in 0..MAX_LIVE_RUNS - 1 {
            let expected = format!("r-{}", n + 2);
            reg.leases().open(&expected, 60_000_000_000, wall_now_ns());
            let id = reg.start(scenario("100"), 1.0).unwrap();
            assert_eq!(id, expected);
            live.push(id);
        }
        wait_for("the idle stop", 2, || first.lock().idle_stopped);
        assert!(reg.get(&first_id).is_some(), "an idle-stopped run stays registered until the cap needs its slot");

        // Eight held, one of them parked: the parked one gives up its slot.
        let expected = format!("r-{}", MAX_LIVE_RUNS + 1);
        reg.leases().open(&expected, 60_000_000_000, wall_now_ns());
        let last = reg.start(scenario("100"), 1.0).expect("the idle-stopped run did not count");
        assert_eq!(last, expected);
        live.push(last);
        assert!(reg.get(&first_id).is_none(), "evicted at the cap");
        assert_eq!(reg.list().len(), MAX_LIVE_RUNS);
        assert!(first.lock().evicted);
        assert!(first.lock().note.starts_with("evicted:"), "{}", first.lock().note);
        let status = reg.root.join("runs").join(&first_id).join("status.json");
        wait_for("the checkpoint", 2, || status.exists());

        // Every slot busy or watched: nothing to evict, so the cap answers.
        let refused = reg.start(scenario("100"), 1.0).unwrap_err();
        assert_eq!(refused.code, 503, "{}", refused.message);
        assert_eq!(reg.list().len(), MAX_LIVE_RUNS);
        for id in &live {
            reg.get(id).unwrap().stop();
        }
    }

    #[test]
    fn an_idle_stopped_run_is_reaped_after_twice_the_threshold() {
        let scratch = temp_dir("reap-idle");
        let reg = Arc::new(Registry::new(scratch.to_path_buf(), 50_000_000));
        let id = reg.start(scenario("100"), 1.0).unwrap();
        let run = reg.get(&id).unwrap();
        wait_for("the reap", 2, || reg.get(&id).is_none());
        assert!(reg.list().is_empty());
        let st = run.lock();
        assert!(st.evicted && st.idle_stopped);
        assert!(st.note.starts_with("reaped:"), "{}", st.note);
        assert!(reg.root.join("runs").join(&id).join("status.json").exists(), "the checkpoint outlives the run");
    }

    /// A finished run whose checkpoint is on disk, ready for the retention tests below: past
    /// `terminal_checkpoint` without waiting on the retention itself.
    fn finished_run(reg: &Arc<Registry>) -> (String, Arc<Run>) {
        let id = reg.start(scenario("20"), 0.0).unwrap();
        let run = reg.get(&id).unwrap();
        wait_for("completion", 10, || run.lock().is_terminal());
        wait_for("the checkpoint", 10, || run.lock().terminal_checkpoint);
        (id, run)
    }

    #[test]
    fn a_read_every_500ms_keeps_a_finished_run_held_and_it_is_released_once_reads_stop() {
        // Follow-up to 38c36cf: a GetRun landing every few hundred milliseconds must hold the run
        // open for as long as it keeps landing, not just for the retention after the run ended.
        let scratch = temp_dir("retention-reads");
        let reg = Arc::new(Registry::new(scratch.to_path_buf(), 3600 * 1_000_000_000));
        reg.set_completed_retention_ns(1_000_000_000); // 1 s
        let (id, run) = finished_run(&reg);

        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            run.touch_read();
            std::thread::sleep(Duration::from_millis(500));
            assert!(reg.get(&id).is_some(), "a read landed under a second ago; a 1 s retention must still hold it");
        }
        // No more reads past this point: released a bit more than a second after the last one.
        wait_for("the release once the reads stop", 5, || reg.get(&id).is_none());
    }

    #[test]
    fn result_and_traces_json_do_not_touch_the_clock_on_their_own_only_touch_read_does() {
        // `result_json` and `traces_json` stay pure reads of the state; it is the server's explicit
        // `run.touch_read()` beside each RPC (GetRun, GetResult, GetTraces, OpenSubscription) that
        // is the read signal. `tests/run_memory.rs` exercises that wiring through the real RPCs.
        let scratch = temp_dir("retention-other-reads");
        let reg = Arc::new(Registry::new(scratch.to_path_buf(), 3600 * 1_000_000_000));
        reg.set_completed_retention_ns(1_000_000_000);
        let (id, run) = finished_run(&reg);
        let old = wall_now_ns().saturating_sub(900_000_000);
        {
            let mut st = run.lock();
            st.last_read_wall_ns = old;
            st.finished_at_wall_ns = Some(old); // as if it had finished 900 ms ago too
        }
        let _ = run.result_json();
        let _ = run.traces_json(&crate::trace_wire::TraceQuery { outcome: Default::default(), min_e2e_ns: 0, tenant_id: None, limit: 10 });
        std::thread::sleep(Duration::from_millis(300));
        assert!(reg.get(&id).is_none(), "neither call touched the clock; the 1 s retention from 900 ms ago ran out");

        let (id2, run2) = finished_run(&reg);
        {
            let mut st = run2.lock();
            st.last_read_wall_ns = wall_now_ns().saturating_sub(900_000_000);
            st.finished_at_wall_ns = Some(wall_now_ns().saturating_sub(900_000_000));
        }
        run2.touch_read();
        std::thread::sleep(Duration::from_millis(600));
        assert!(reg.get(&id2).is_some(), "touch_read just now must hold the run past 600 ms later");
    }

    #[test]
    fn the_cap_releases_terminal_runs_but_never_one_read_in_the_last_ten_seconds() {
        let scratch = temp_dir("cap-read-floor");
        let reg = Arc::new(Registry::new(scratch.to_path_buf(), 3600 * 1_000_000_000));
        let old = wall_now_ns().saturating_sub(20 * 1_000_000_000);
        // Seven long-idle terminal runs, well past the ten-second floor.
        let mut old_ids = Vec::new();
        for _ in 0..MAX_LIVE_RUNS - 1 {
            let (id, run) = finished_run(&reg);
            let mut st = run.lock();
            st.last_read_wall_ns = old;
            st.finished_at_wall_ns = Some(old);
            drop(st);
            old_ids.push(id);
        }
        // An eighth, terminal too, but read just now: it must survive the cap regardless.
        let (fresh_id, fresh) = finished_run(&reg);
        fresh.touch_read();

        // The ninth `start` finds eight runs held and releases what it can, sparing the fresh one.
        let ninth = reg.start(scenario("100"), 1.0).unwrap();
        for id in &old_ids {
            assert!(reg.get(id).is_none(), "run {id} was last read 20 s ago: eligible under cap pressure");
        }
        assert!(reg.get(&fresh_id).is_some(), "a run read moments ago must survive the cap");
        reg.get(&ninth).unwrap().stop();
    }
}
