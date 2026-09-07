//! `/v1/ingress/*`: the Frontend-to-Ingress API as WIRE.md maps it onto HTTP/1.1.
//!
//! Unary RPCs are `POST /v1/ingress/<RpcName>` with JSON both ways; `OpenSubscription` is a `GET`
//! answered as a server-sent event stream. The run registry in `run.rs` owns the engines; this file
//! owns the parsing, the routing, the subscriptions and the SSE framing, and nothing here touches
//! simulated time except to read it.
//!
//! The JSON reader below is the counterpart of `wire.rs`'s writer: a hundred lines rather than
//! serde, for the same reason. It reads only what the request messages need.

use crate::idle::wall_now_ns;
use crate::lease::SubscriptionId;
use crate::run::{self, Refused, Registry, RowSpec, Run};
use crate::wire::{self, Json as JsonOut, SubscriptionUpdate, Target};
use sim_scenario::{OverrideKind, Scenario};
use std::collections::{BTreeMap, VecDeque};
use std::io::Write;
use std::net::TcpStream;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

/// Per-subscription replay ring, per WIRE.md: the last 256 updates, so a reconnect with
/// `Last-Event-ID` can resume without a gap and a larger gap is a 410.
pub const RING: usize = 256;

/// How often the stream writes an SSE comment while it has nothing to send, so an idle proxy does
/// not cut a paused run's stream.
const KEEPALIVE: Duration = Duration::from_secs(10);

/// `lease_ns` when the client sends none. The registry itself treats an absent lease as zero and
/// therefore dead on arrival, which is right for a registry and wrong for a curl user.
const DEFAULT_LEASE_NS: u64 = 60 * 1_000_000_000;

pub const INGRESS_PREFIX: &str = "/v1/ingress/";

/// A parsed HTTP request, as `lib.rs` hands it over.
#[derive(Debug, Default)]
pub struct Request {
    pub method: String,
    /// Path without the query.
    pub path: String,
    /// The raw query, without the `?`.
    pub query: String,
    /// Header names lower-cased.
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Request {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str())
    }
}

/// The server: the served directory for static files, the runs, and the open subscriptions.
pub struct Server {
    pub root: PathBuf,
    pub runs: Arc<Registry>,
    subs: Mutex<BTreeMap<u64, Sub>>,
    sse_write_timeout: Duration,
    /// Connections being handled right now, kept by the accept loop in `lib.rs`.
    pub(crate) connections: AtomicUsize,
}

/// How long one SSE write may block before the stream is abandoned. A reader that stops reading
/// fills the socket buffers and then blocks the writer at the next send; without a bound that
/// writer holds a thread, a connection slot and a lease for as long as the peer stays connected.
/// Thirty seconds is longer than any proxy or browser hiccup and shorter than a forgotten tab.
pub const SSE_WRITE_TIMEOUT: Duration = Duration::from_secs(30);

/// A subscription's delivery state. The lease lives in the registry; this is the ring and the
/// cursor, which survive a reconnect while the lease does.
struct Sub {
    spec: RowSpec,
    samples_per_sim_second: f64,
    /// The next event id to allocate, from 1.
    next_seq: u64,
    /// The next sample index to deliver, from 1.
    next_k: u64,
    ring: VecDeque<(u64, String, bool)>,
    /// Bumped by every reconnect; the writer of an older generation stops, so one subscription
    /// has one writer even if the old socket has not noticed it is dead.
    generation: u64,
    /// The final update has been generated; nothing follows it.
    finished: bool,
}

impl Server {
    pub fn new(root: PathBuf, idle_threshold_ns: u64) -> Server {
        Server {
            runs: Arc::new(Registry::new(root.clone(), idle_threshold_ns)),
            root,
            subs: Mutex::new(BTreeMap::new()),
            sse_write_timeout: SSE_WRITE_TIMEOUT,
            connections: AtomicUsize::new(0),
        }
    }

    /// Connections being handled right now. What the wire cannot show and a test about a stalled or
    /// vanished client needs: that its handler thread is gone.
    pub fn connections(&self) -> usize {
        self.connections.load(Ordering::Relaxed)
    }

    /// Subscriptions holding a ring, live or waiting to be reaped.
    pub fn open_subscriptions(&self) -> usize {
        self.subs().len()
    }

    /// Tests lower the write timeout so a stalled reader is detected in well under a second.
    pub fn with_sse_write_timeout(mut self, timeout: Duration) -> Server {
        self.sse_write_timeout = timeout;
        self
    }

    fn subs(&self) -> MutexGuard<'_, BTreeMap<u64, Sub>> {
        self.subs.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Route one ingress request. Anything unknown is a 404 with an `error` body, per WIRE.md rule 6.
    pub fn handle(&self, req: Request, stream: TcpStream) -> std::io::Result<()> {
        self.reap();
        let rpc = req.path.strip_prefix(INGRESS_PREFIX).unwrap_or("");
        match (req.method.as_str(), rpc) {
            ("GET", "OpenSubscription") => self.open_subscription(&req, stream),
            ("POST", _) => match self.unary(rpc, &req.body) {
                Ok(body) => crate::respond(stream, 200, "application/json", body.as_bytes()),
                Err(r) => error(stream, r.code, &r.message),
            },
            (_, "OpenSubscription") => error(stream, 405, "OpenSubscription is a GET"),
            _ => error(stream, 405, "ingress RPCs are POST"),
        }
    }

    /// Subscriptions whose lease ran out with no writer left to notice. A stream that ended in a
    /// write error, by timeout or because the peer vanished, keeps its ring and its lease so that
    /// a reconnect within the lease resumes, as WIRE.md promises; nothing else would ever reclaim
    /// them. Done on every ingress request rather than on a timer, so the cost is one map sweep
    /// per request and there is no thread to forget.
    fn reap(&self) {
        let expired = self.runs.leases().expire(wall_now_ns());
        if !expired.is_empty() {
            let mut subs = self.subs();
            for lease in expired {
                subs.remove(&lease.id.0);
            }
        }
    }

    fn run(&self, run_id: &str) -> Result<Arc<Run>, Refused> {
        self.runs.get(run_id).ok_or_else(|| Refused { code: 404, message: format!("unknown run {run_id:?}") })
    }

    fn unary(&self, rpc: &str, body: &[u8]) -> Result<String, Refused> {
        let text = std::str::from_utf8(body).map_err(|_| bad("request body is not UTF-8"))?;
        let req = if text.trim().is_empty() { Json::Obj(Vec::new()) } else { parse_json(text).map_err(bad)? };
        let run_id = || req.str("run_id").ok_or_else(|| bad("run_id is required"));
        match rpc {
            "StartRun" => {
                let scenario = req.get("scenario").ok_or_else(|| bad("scenario is required"))?;
                let text = scenario.str("text").ok_or_else(|| bad("scenario.text is required"))?;
                let mut sc = Scenario::parse(text).map_err(bad)?;
                if let Some(Json::Obj(overrides)) = scenario.get("overrides") {
                    for (k, v) in overrides {
                        let v = v.scalar_string().ok_or_else(|| bad(format!("override {k:?} must be a scalar")))?;
                        crate::export::override_key(&mut sc, k, &v).map_err(bad)?;
                    }
                }
                let factor = req.f64("max_realtime_factor").unwrap_or(0.0);
                // `record_traces` is accepted and ignored: the engine records no traces yet.
                let id = self.runs.start(sc, factor)?;
                let mut j = JsonOut::new();
                j.begin_object().field_str("run_id", &id).end_object();
                Ok(j.finish())
            }
            "GetRun" => Ok(wire::run_status_json(&self.run(run_id()?)?.status())),
            "StopRun" => Ok(wire::run_status_json(&self.run(run_id()?)?.stop())),
            "ListRuns" => {
                let limit = req.f64("limit").unwrap_or(0.0) as usize;
                let mut j = JsonOut::new();
                j.begin_object().key("runs").begin_array();
                for s in self.runs.list().iter().take(if limit == 0 { usize::MAX } else { limit }) {
                    wire::run_status(&mut j, s);
                }
                j.end_array().field_str("next_cursor", "").end_object();
                Ok(j.finish())
            }
            "SetSpeed" => {
                let run = self.run(run_id()?)?;
                let factor = req.f64("realtime_factor").unwrap_or(0.0);
                let paused = req.bool("paused").unwrap_or(false);
                Ok(wire::run_status_json(&run.set_speed(factor, paused)?))
            }
            "StepForward" => {
                let run = self.run(run_id()?)?;
                let ns = req.u64("sim_duration_ns").unwrap_or(0);
                let windows = req.f64("barrier_windows").unwrap_or(0.0) as u64;
                if (ns == 0) == (windows == 0) {
                    return Err(bad("exactly one of sim_duration_ns and barrier_windows must be set"));
                }
                let by = if ns > 0 { ns } else { windows.saturating_mul(run.lock().sample_interval_ns()) };
                Ok(wire::run_status_json(&run.step(by)?))
            }
            "GetResult" => self.run(run_id()?)?.result_json(),
            "RenewSubscription" => {
                let id = req.str("subscription_id").and_then(parse_subscription_id);
                let lease_ns = req.u64("lease_ns").unwrap_or(DEFAULT_LEASE_NS);
                let now = wall_now_ns();
                let out = match id {
                    Some(id) => self.runs.leases().renew(&SubscriptionId(id), lease_ns, now),
                    None => crate::lease::RenewOutcome { expires_at_wall_ns: 0, expired: true },
                };
                let mut j = JsonOut::new();
                j.begin_object()
                    .field_u64("lease_expires_at_wall_ns", out.expires_at_wall_ns)
                    .field_bool("expired", out.expired)
                    .end_object();
                Ok(j.finish())
            }
            "CloseSubscription" => {
                if let Some(id) = req.str("subscription_id").and_then(parse_subscription_id) {
                    self.runs.leases().close(&SubscriptionId(id));
                    self.subs().remove(&id);
                }
                Ok("{}".to_string())
            }
            "UpdateWorkload" | "UpdatePolicies" => {
                let run = self.run(run_id()?)?;
                let kind = if rpc == "UpdateWorkload" { OverrideKind::Workload } else { OverrideKind::Policy };
                let overrides = match req.get("overrides") {
                    Some(Json::Obj(fields)) => fields,
                    Some(_) => return Err(bad("overrides must be an object")),
                    None => return Err(bad("overrides is required")),
                };
                let mut pairs = Vec::with_capacity(overrides.len());
                for (k, v) in overrides {
                    let v = v.scalar_string().ok_or_else(|| bad(format!("override {k:?} must be a scalar")))?;
                    pairs.push((k.clone(), v));
                }
                // Policed here by kind before the run thread sees the batch, so a key from the
                // other group, or a structural one, refuses the whole batch without touching the
                // engine. WIRE.md: a rejected update is 200 with `rejected_reason`, not a 4xx.
                let misfiled = pairs.iter().map(|(k, _)| k).find(|k| Scenario::override_kind(k) != kind);
                let outcome = match misfiled {
                    Some(k) => Err(match Scenario::override_kind(k) {
                        OverrideKind::Workload => format!("`{k}` is a workload key; use UpdateWorkload"),
                        OverrideKind::Policy => format!("`{k}` is a policy key; use UpdatePolicies"),
                        OverrideKind::Structural => {
                            format!("`{k}` is a structural key and requires a restart: start a new run")
                        }
                    }),
                    None => run.update(pairs)?,
                };
                let mut j = JsonOut::new();
                j.begin_object()
                    .field_bool("accepted", outcome.is_ok())
                    .field_bool("required_resimulation", false)
                    .field_u64("rewound_to_unix_ns", 0)
                    .field_str("rejected_reason", outcome.err().as_deref().unwrap_or(""))
                    .end_object();
                Ok(j.finish())
            }
            "Rewind" | "GetTraces" => Err(Refused {
                code: 501,
                message: format!("{rpc} is not implemented by this server yet"),
            }),
            _ => Err(Refused { code: 404, message: format!("unknown RPC {rpc:?}") }),
        }
    }

    // -----------------------------------------------------------------------
    // OpenSubscription
    // -----------------------------------------------------------------------

    fn open_subscription(&self, req: &Request, mut stream: TcpStream) -> std::io::Result<()> {
        stream.set_write_timeout(Some(self.sse_write_timeout))?;
        let q = parse_query(&req.query);
        let get = |k: &str| q.iter().find(|(key, _)| key == k).map(|(_, v)| v.as_str()).filter(|v| !v.is_empty());
        let Some(run_id) = get("run_id") else { return error(stream, 400, "run_id is required") };
        let run = match self.run(run_id) {
            Ok(run) => run,
            Err(r) => return error(stream, r.code, &r.message),
        };

        // A reconnect names the subscription and where it got to; the ring answers from there.
        if let Some(sid) = get("subscription_id") {
            let Some(id) = parse_subscription_id(sid) else { return error(stream, 400, "malformed subscription_id") };
            let last = match req.header("last-event-id").map(str::trim) {
                None | Some("") => 0,
                Some(v) => match v.parse::<u64>() {
                    Ok(n) => n,
                    Err(_) => return error(stream, 400, "malformed Last-Event-ID"),
                },
            };
            let now = wall_now_ns();
            let expires = {
                let leases = self.runs.leases();
                match leases.get(&SubscriptionId(id)) {
                    Some(l) if l.is_live(now) && l.run_id == run_id => l.expires_at_wall_ns,
                    _ => return error(stream, 410, "subscription is gone; open a new one"),
                }
            };
            let generation = {
                let mut subs = self.subs();
                let Some(sub) = subs.get_mut(&id) else { return error(stream, 410, "subscription is gone; open a new one") };
                let newest = sub.next_seq - 1;
                let oldest_kept = sub.ring.front().map_or(newest + 1, |(s, _, _)| *s);
                // Resumable when the next id to send is still in the ring, or nothing was missed.
                if last > newest || (last < newest && last + 1 < oldest_kept) {
                    return error(stream, 410, "the ring no longer covers that Last-Event-ID; open a new one");
                }
                sub.generation += 1;
                sub.generation
            };
            sse_head(&mut stream)?;
            sse_event(&mut stream, None, "open", &open_json(id, expires, ""))?;
            return self.stream_updates(stream, run, id, generation, last);
        }

        // A fresh open: validate everything the proto says is a rejection, then lease and stream.
        let num = |k: &str| -> Result<Option<f64>, String> {
            get(k).map(|v| v.parse::<f64>().map_err(|_| format!("{k} must be a number"))).transpose()
        };
        let rate = match num("samples_per_sim_second") {
            Ok(v) => v.unwrap_or(0.0),
            Err(e) => return error(stream, 400, &e),
        };
        let lease_ns = match get("lease_ns").map(|v| v.parse::<u64>()) {
            None => DEFAULT_LEASE_NS,
            Some(Ok(v)) => v,
            Some(Err(_)) => return error(stream, 400, "lease_ns must be a decimal string"),
        };
        let percentiles: Vec<f64> = match get("percentiles") {
            None => Vec::new(),
            Some(list) => match list.split(',').map(|p| p.trim().parse::<f64>()).collect::<Result<_, _>>() {
                Ok(v) => v,
                Err(_) => return error(stream, 400, "percentiles must be comma-separated numbers"),
            },
        };
        let spec = match subscription_spec(&run, get("scope"), get("replica_id"), get("metrics"), percentiles, rate) {
            Ok(spec) => spec,
            Err(reason) => {
                sse_head(&mut stream)?;
                return sse_event(&mut stream, None, "open", &open_json(0, 0, &reason));
            }
        };

        let now = wall_now_ns();
        let lease = self.runs.leases().open(run_id, lease_ns, now);
        let id = lease.id.0;
        self.subs().insert(
            id,
            Sub {
                spec,
                samples_per_sim_second: rate,
                next_seq: 1,
                next_k: 1,
                ring: VecDeque::new(),
                generation: 1,
                finished: false,
            },
        );
        run.resume_from_idle();
        sse_head(&mut stream)?;
        sse_event(&mut stream, None, "open", &open_json(id, lease.expires_at_wall_ns, ""))?;
        self.stream_updates(stream, run, id, 1, 0)
    }

    /// The update loop: one event per sample at the client's cadence, replayed from the ring when
    /// the ring has something newer than `last_sent`, generated from the run's frames otherwise.
    /// Ends when the lease expires, the subscription is closed or superseded, or the final update
    /// has gone out. A write error (the peer vanished, or the write timeout: a peer that stopped
    /// reading) ends it too, freeing the thread and the connection slot at once; the subscription
    /// itself stays for a reconnect until its lease runs out, and `reap` takes it then.
    fn stream_updates(
        &self,
        mut stream: TcpStream,
        run: Arc<Run>,
        id: u64,
        generation: u64,
        mut last_sent: u64,
    ) -> std::io::Result<()> {
        let mut last_write = Instant::now();
        loop {
            let now = wall_now_ns();
            {
                let mut leases = self.runs.leases();
                let live = leases.get(&SubscriptionId(id)).is_some_and(|l| l.is_live(now));
                if !live {
                    leases.expire(now);
                    drop(leases);
                    self.subs().remove(&id);
                    return Ok(());
                }
            }

            let next: Option<(u64, String, bool)> = {
                let mut subs = self.subs();
                let Some(sub) = subs.get_mut(&id) else { return Ok(()) };
                if sub.generation != generation {
                    return Ok(());
                }
                if let Some(e) = sub.ring.iter().find(|(s, _, _)| *s > last_sent) {
                    Some(e.clone())
                } else if sub.finished {
                    return self.finish_subscription(id);
                } else {
                    let st = run.lock();
                    let j = run::frame_index(sub.next_k, sub.samples_per_sim_second, st.sample_interval_ns());
                    let n = st.frames.len();
                    let ending = st.is_terminal() || st.stop_requested;
                    if n == 0 && ending {
                        drop(st);
                        return self.finish_subscription(id);
                    }
                    pick(j, n, ending).and_then(|(idx, is_final)| {
                        let frame = &st.frames[idx];
                        let row = run::row(frame, &st.scenario, &sub.spec)?;
                        let u = SubscriptionUpdate {
                            subscription_id: format!("s-{id}"),
                            sim_time_unix_ns: frame.t,
                            realtime_factor: st.realtime_factor,
                            row,
                            is_final,
                        };
                        let seq = sub.next_seq;
                        sub.next_seq += 1;
                        sub.next_k += 1;
                        sub.finished = is_final;
                        let data = wire::subscription_update_json(&u);
                        sub.ring.push_back((seq, data.clone(), is_final));
                        while sub.ring.len() > RING {
                            sub.ring.pop_front();
                        }
                        Some((seq, data, is_final))
                    })
                }
            };

            match next {
                Some((seq, data, is_final)) => {
                    sse_event(&mut stream, Some(seq), "update", &data)?;
                    last_write = Instant::now();
                    last_sent = seq;
                    if is_final {
                        return self.finish_subscription(id);
                    }
                }
                None => {
                    if last_write.elapsed() >= KEEPALIVE {
                        stream.write_all(b": keepalive\n\n")?;
                        stream.flush()?;
                        last_write = Instant::now();
                    }
                    let st = run.lock();
                    let _ = run.changed.wait_timeout(st, Duration::from_millis(100));
                }
            }
        }
    }

    /// After the final update nothing follows, so the lease is released rather than left to run
    /// down: the idle guard should see a finished viewer as gone.
    fn finish_subscription(&self, id: u64) -> std::io::Result<()> {
        self.runs.leases().close(&SubscriptionId(id));
        self.subs().remove(&id);
        Ok(())
    }
}

/// Which frame the next sample reads, as an index into `n` closed frames given the 1-based frame
/// number `j` from `run::frame_index`, and whether that update is the final one. `ending` means no
/// frame will follow these: the run is terminal, or a stop is requested, which the run thread sets
/// in the same critical section as the last frames it closes. Reading `is_terminal` alone left a
/// window between the last frame and `Complete` in which frame `n` went out once unfinal and then
/// again, final. Past the frames while the run is ending, the final update repeats the last frame,
/// so a viewer that had caught up still sees the end. The caller handles `n == 0`.
fn pick(j: usize, n: usize, ending: bool) -> Option<(usize, bool)> {
    if j <= n {
        Some((j - 1, ending && j == n))
    } else if ending {
        Some((n - 1, true))
    } else {
        None
    }
}

/// Validate a fresh open against WIRE.md "What the first server supports". The error is the
/// `rejected_reason` the client sees on a 200.
fn subscription_spec(
    run: &Run,
    scope: Option<&str>,
    replica_id: Option<&str>,
    metrics: Option<&str>,
    percentiles: Vec<f64>,
    rate: f64,
) -> Result<RowSpec, String> {
    let (target, supported): (Target, &[i32]) = match scope {
        Some("SCOPE_FLEET") => (Target::Fleet, run::FLEET_METRICS),
        Some("SCOPE_REPLICA") => {
            let id = replica_id
                .ok_or("replica_id is required for SCOPE_REPLICA")?
                .parse::<u64>()
                .map_err(|_| "replica_id must be a decimal string")?;
            let replicas = run.lock().scenario.replicas as u64;
            if id >= replicas {
                return Err(format!("replica_id {id} is not in this run's 0..{replicas}"));
            }
            (Target::Replica(id), run::REPLICA_METRICS)
        }
        Some(other) => return Err(format!("scope {other} is not served yet; SCOPE_FLEET and SCOPE_REPLICA are")),
        None => return Err("scope is required".to_string()),
    };
    let mut wanted = Vec::new();
    for name in metrics.unwrap_or("").split(',').map(str::trim).filter(|s| !s.is_empty()) {
        let Some((_, number)) = wire::METRICS.iter().find(|(n, _)| *n == name) else {
            return Err(format!("unknown metric {name}"));
        };
        if !supported.contains(number) {
            return Err(format!("{name} is not served for this scope"));
        }
        wanted.push(*number);
    }
    if !(rate.is_finite() && rate > 0.0) {
        return Err("samples_per_sim_second must be positive".to_string());
    }
    if let Some(p) = percentiles.iter().find(|p| !(p.is_finite() && **p > 0.0 && **p <= 100.0)) {
        return Err(format!("percentile {p} is outside (0, 100]"));
    }
    Ok(RowSpec { target, metrics: wanted, percentiles })
}

fn parse_subscription_id(s: &str) -> Option<u64> {
    s.strip_prefix("s-")?.parse().ok()
}

fn open_json(id: u64, expires_at_wall_ns: u64, rejected_reason: &str) -> String {
    let mut j = JsonOut::new();
    j.begin_object();
    if rejected_reason.is_empty() {
        j.field_str("subscription_id", &format!("s-{id}")).field_u64("lease_expires_at_wall_ns", expires_at_wall_ns);
    } else {
        j.field_str("rejected_reason", rejected_reason);
    }
    j.end_object();
    j.finish()
}

fn bad(message: impl Into<String>) -> Refused {
    Refused { code: 400, message: message.into() }
}

/// `{"error": "..."}` with the status, WIRE.md rule 6.
fn error(stream: TcpStream, code: u16, message: &str) -> std::io::Result<()> {
    let mut j = JsonOut::new();
    j.begin_object().field_str("error", message).end_object();
    crate::respond(stream, code, "application/json", j.finish().as_bytes())
}

// ---------------------------------------------------------------------------
// SSE framing
// ---------------------------------------------------------------------------

fn sse_head(stream: &mut TcpStream) -> std::io::Result<()> {
    stream.write_all(
        b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nCache-Control: no-cache\r\n\
          X-Content-Type-Options: nosniff\r\nConnection: close\r\n\r\n",
    )?;
    stream.flush()
}

/// One event. `data` is a single line of compact JSON, so one `data:` field carries it whole.
fn sse_event(stream: &mut TcpStream, id: Option<u64>, event: &str, data: &str) -> std::io::Result<()> {
    let mut out = String::with_capacity(data.len() + 40);
    if let Some(id) = id {
        out.push_str(&format!("id: {id}\n"));
    }
    out.push_str(&format!("event: {event}\ndata: {data}\n\n"));
    stream.write_all(out.as_bytes())?;
    stream.flush()
}

// ---------------------------------------------------------------------------
// Query strings and JSON, read
// ---------------------------------------------------------------------------

pub fn parse_query(q: &str) -> Vec<(String, String)> {
    q.split('&')
        .filter(|kv| !kv.is_empty())
        .map(|kv| {
            let (k, v) = kv.split_once('=').unwrap_or((kv, ""));
            (crate::percent_decode(&k.replace('+', " ")), crate::percent_decode(&v.replace('+', " ")))
        })
        .collect()
}

/// A JSON value, read. Numbers are `f64`; `uint64` fields arrive as decimal strings per WIRE.md
/// rule 2, and [`Json::u64`] accepts either so a hand-typed curl body still works.
#[derive(Clone, Debug, PartialEq)]
pub enum Json {
    Null,
    Bool(bool),
    Num(f64),
    Str(String),
    Arr(Vec<Json>),
    Obj(Vec<(String, Json)>),
}

impl Json {
    pub fn get(&self, key: &str) -> Option<&Json> {
        match self {
            Json::Obj(fields) => fields.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }
    pub fn str(&self, key: &str) -> Option<&str> {
        match self.get(key)? {
            Json::Str(s) => Some(s),
            _ => None,
        }
    }
    pub fn f64(&self, key: &str) -> Option<f64> {
        match self.get(key)? {
            Json::Num(n) => Some(*n),
            Json::Str(s) => s.parse().ok(),
            _ => None,
        }
    }
    pub fn bool(&self, key: &str) -> Option<bool> {
        match self.get(key)? {
            Json::Bool(b) => Some(*b),
            _ => None,
        }
    }
    pub fn u64(&self, key: &str) -> Option<u64> {
        match self.get(key)? {
            Json::Str(s) => s.parse().ok(),
            Json::Num(n) if *n >= 0.0 && n.fract() == 0.0 => Some(*n as u64),
            _ => None,
        }
    }
    /// A scalar as the text a scenario override wants.
    fn scalar_string(&self) -> Option<String> {
        match self {
            Json::Str(s) => Some(s.clone()),
            Json::Num(n) => Some(format!("{n}")),
            Json::Bool(b) => Some(b.to_string()),
            _ => None,
        }
    }
}

/// Deepest JSON nesting a request may carry. A `StartRun` body is three levels deep.
const MAX_DEPTH: usize = 64;

pub fn parse_json(text: &str) -> Result<Json, String> {
    let mut p = Parser { s: text.as_bytes(), i: 0 };
    let v = p.value(0)?;
    p.ws();
    if p.i != p.s.len() {
        return Err(format!("trailing characters at byte {}", p.i));
    }
    Ok(v)
}

struct Parser<'a> {
    s: &'a [u8],
    i: usize,
}

impl Parser<'_> {
    fn ws(&mut self) {
        while self.i < self.s.len() && matches!(self.s[self.i], b' ' | b'\t' | b'\n' | b'\r') {
            self.i += 1;
        }
    }
    fn peek(&self) -> Option<u8> {
        self.s.get(self.i).copied()
    }
    fn expect(&mut self, c: u8) -> Result<(), String> {
        if self.peek() == Some(c) {
            self.i += 1;
            Ok(())
        } else {
            Err(format!("expected {:?} at byte {}", c as char, self.i))
        }
    }
    /// `depth` is the nesting level. The parser recurses once per level, so a body of nothing but
    /// open brackets would otherwise be a stack overflow, which aborts the whole process and every
    /// run in it rather than failing one request.
    fn value(&mut self, depth: usize) -> Result<Json, String> {
        if depth > MAX_DEPTH {
            return Err(format!("nesting deeper than {MAX_DEPTH} levels at byte {}", self.i));
        }
        self.ws();
        match self.peek() {
            Some(b'{') => {
                self.i += 1;
                let mut fields = Vec::new();
                self.ws();
                if self.peek() == Some(b'}') {
                    self.i += 1;
                    return Ok(Json::Obj(fields));
                }
                loop {
                    self.ws();
                    let k = self.string()?;
                    self.ws();
                    self.expect(b':')?;
                    let v = self.value(depth + 1)?;
                    fields.push((k, v));
                    self.ws();
                    match self.peek() {
                        Some(b',') => self.i += 1,
                        Some(b'}') => {
                            self.i += 1;
                            return Ok(Json::Obj(fields));
                        }
                        _ => return Err(format!("expected ',' or '}}' at byte {}", self.i)),
                    }
                }
            }
            Some(b'[') => {
                self.i += 1;
                let mut items = Vec::new();
                self.ws();
                if self.peek() == Some(b']') {
                    self.i += 1;
                    return Ok(Json::Arr(items));
                }
                loop {
                    items.push(self.value(depth + 1)?);
                    self.ws();
                    match self.peek() {
                        Some(b',') => self.i += 1,
                        Some(b']') => {
                            self.i += 1;
                            return Ok(Json::Arr(items));
                        }
                        _ => return Err(format!("expected ',' or ']' at byte {}", self.i)),
                    }
                }
            }
            Some(b'"') => Ok(Json::Str(self.string()?)),
            Some(b't') => self.literal("true", Json::Bool(true)),
            Some(b'f') => self.literal("false", Json::Bool(false)),
            Some(b'n') => self.literal("null", Json::Null),
            Some(c) if c == b'-' || c.is_ascii_digit() => {
                let start = self.i;
                while self.i < self.s.len() && matches!(self.s[self.i], b'0'..=b'9' | b'-' | b'+' | b'.' | b'e' | b'E') {
                    self.i += 1;
                }
                let text = std::str::from_utf8(&self.s[start..self.i]).map_err(|e| e.to_string())?;
                text.parse::<f64>().map(Json::Num).map_err(|_| format!("bad number {text:?} at byte {start}"))
            }
            Some(c) => Err(format!("unexpected {:?} at byte {}", c as char, self.i)),
            None => Err("unexpected end of input".to_string()),
        }
    }
    fn literal(&mut self, word: &str, v: Json) -> Result<Json, String> {
        if self.s[self.i..].starts_with(word.as_bytes()) {
            self.i += word.len();
            Ok(v)
        } else {
            Err(format!("bad literal at byte {}", self.i))
        }
    }
    /// The inverse of `wire.rs`'s `push_escaped`, plus the `\/` and `\uXXXX` forms any client may send.
    fn string(&mut self) -> Result<String, String> {
        self.expect(b'"')?;
        let mut out = Vec::new();
        loop {
            let Some(c) = self.peek() else { return Err("unterminated string".to_string()) };
            self.i += 1;
            match c {
                b'"' => break,
                b'\\' => {
                    let Some(e) = self.peek() else { return Err("unterminated escape".to_string()) };
                    self.i += 1;
                    match e {
                        b'"' => out.push(b'"'),
                        b'\\' => out.push(b'\\'),
                        b'/' => out.push(b'/'),
                        b'n' => out.push(b'\n'),
                        b'r' => out.push(b'\r'),
                        b't' => out.push(b'\t'),
                        b'b' => out.push(8),
                        b'f' => out.push(12),
                        b'u' => {
                            let hex = self.s.get(self.i..self.i + 4).ok_or("short \\u escape")?;
                            self.i += 4;
                            let code = u32::from_str_radix(std::str::from_utf8(hex).map_err(|e| e.to_string())?, 16)
                                .map_err(|e| e.to_string())?;
                            // Surrogate pairs are not needed by any request message; a lone one
                            // becomes the replacement character rather than an error.
                            let ch = char::from_u32(code).unwrap_or('\u{fffd}');
                            let mut buf = [0u8; 4];
                            out.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
                        }
                        other => return Err(format!("bad escape \\{}", other as char)),
                    }
                }
                c => out.push(c),
            }
        }
        String::from_utf8(out).map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn the_final_update_is_the_last_frame_exactly_once() {
        assert_eq!(pick(3, 5, false), Some((2, false)));
        assert_eq!(pick(5, 5, false), Some((4, false)), "the last frame so far, with more to come");
        assert_eq!(pick(5, 5, true), Some((4, true)), "the last frame there will be");
        assert_eq!(pick(6, 5, false), None, "ahead of the run: wait");
        assert_eq!(pick(6, 5, true), Some((4, true)), "caught up when the run ended: the last frame, final");
    }

    #[test]
    fn nesting_is_bounded() {
        assert!(parse_json(&"[".repeat(MAX_DEPTH + 1)).unwrap_err().contains("nesting"));
        let ok = format!("{}{}", "[".repeat(MAX_DEPTH), "]".repeat(MAX_DEPTH));
        assert!(parse_json(&ok).is_ok());
        assert!(parse_json(&"[".repeat(20 * 1024)).is_err(), "no stack overflow");
    }

    /// Every request field this file reads. WIRE.md's rule holds both ways: what the server emits
    /// is checked in `tests/wire_export.rs` and `tests/ingress_http.rs`; what it *reads* is checked
    /// here, so a request field the protos do not have cannot appear.
    const REQUEST_FIELDS: &[&str] = &[
        "scenario",
        "max_realtime_factor",
        "record_traces",
        "run_id",
        "limit",
        "cursor",
        "realtime_factor",
        "paused",
        "sim_duration_ns",
        "barrier_windows",
        "subscription_id",
        "lease_ns",
        "scope",
        "replica_id",
        "metrics",
        "samples_per_sim_second",
        "percentiles",
    ];
    /// The one documented deviation: the scenario travels as text plus overrides until codegen.
    const WIRE_MD_DEVIATIONS: &[&str] = &["text", "overrides"];

    fn read(rel: &str) -> String {
        let p = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(rel);
        std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
    }

    #[test]
    fn request_fields_are_proto_fields_or_documented_deviations() {
        let protos = read("../../proto/lbsim/v1/ingress.proto") + &read("../../proto/lbsim/v1/subscription.proto");
        for f in REQUEST_FIELDS {
            assert!(protos.contains(&format!(" {f} = ")), "{f} is not a field in ingress.proto or subscription.proto");
        }
        let wire_md = read("WIRE.md");
        for f in WIRE_MD_DEVIATIONS {
            assert!(wire_md.contains(&format!("\"{f}\"")), "{f} is not documented in WIRE.md");
        }
    }

    #[test]
    fn json_reader_handles_the_request_shapes() {
        let v = parse_json(
            r#" {"scenario": {"text": "name = a\nseed = 1", "overrides": {"arrival_rps": "90", "replicas": 4}},
                 "max_realtime_factor": 2.5, "record_traces": false, "sim_duration_ns": "60000000000",
                 "list": [1, -2.5e1, "x", null, true], "esc": "a\"b\\c\/dé"} "#,
        )
        .unwrap();
        assert_eq!(v.get("scenario").unwrap().str("text"), Some("name = a\nseed = 1"));
        let o = v.get("scenario").unwrap().get("overrides").unwrap();
        assert_eq!(o.get("arrival_rps").unwrap().scalar_string().as_deref(), Some("90"));
        assert_eq!(o.get("replicas").unwrap().scalar_string().as_deref(), Some("4"));
        assert_eq!(v.f64("max_realtime_factor"), Some(2.5));
        assert_eq!(v.bool("record_traces"), Some(false));
        assert_eq!(v.u64("sim_duration_ns"), Some(60_000_000_000));
        assert_eq!(
            v.get("list"),
            Some(&Json::Arr(vec![Json::Num(1.0), Json::Num(-25.0), Json::Str("x".into()), Json::Null, Json::Bool(true)]))
        );
        assert_eq!(v.str("esc"), Some("a\"b\\c/dé"));
        assert!(parse_json("{").is_err());
        assert!(parse_json("{} x").is_err());
        assert!(parse_json(r#"{"a": tru}"#).is_err());
        assert_eq!(parse_json("{}").unwrap(), Json::Obj(Vec::new()));
    }

    #[test]
    fn query_strings_decode() {
        let q = parse_query("run_id=r-1&scope=SCOPE_FLEET&metrics=METRIC_TTFT%2CMETRIC_E2E&percentiles=50,99&x=a+b&flag");
        assert_eq!(q.iter().find(|(k, _)| k == "metrics").unwrap().1, "METRIC_TTFT,METRIC_E2E");
        assert_eq!(q.iter().find(|(k, _)| k == "x").unwrap().1, "a b");
        assert_eq!(q.iter().find(|(k, _)| k == "flag").unwrap().1, "");
        assert_eq!(parse_subscription_id("s-12"), Some(12));
        assert_eq!(parse_subscription_id("12"), None);
    }

    #[test]
    fn open_response_has_exactly_the_proto_fields() {
        assert_eq!(open_json(7, 5, ""), r#"{"subscription_id":"s-7","lease_expires_at_wall_ns":"5"}"#);
        assert_eq!(open_json(0, 0, "no"), r#"{"rejected_reason":"no"}"#);
    }
}
