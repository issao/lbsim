//! The live Ingress endpoint, driven over TCP exactly as a browser or curl would drive it.
//!
//! The server is the real one from `sim_ingress::serve_on`, bound to an ephemeral port and given
//! the idle threshold directly rather than through the environment, so the tests in this binary
//! can run in parallel without sharing a process-wide setting. The client below is a few dozen
//! lines of `std::net`: no HTTP library, because the point is that the wire is plain HTTP/1.1 and
//! plain server-sent events, readable with nothing else.

mod common;

use sim_ingress::server::{parse_json, Json};
use sim_ingress::Server;
use std::collections::BTreeSet;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

const S: u64 = 1_000_000_000;

fn workspace() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

/// A server on an ephemeral port serving a fresh directory. Returns the address, the server (for
/// the one thing the wire does not expose, a run's frame count) and the directory guard, which is
/// where an idle checkpoint lands and which must stay bound at the call site for the server's
/// whole lifetime: it removes the directory on drop.
fn start_server(name: &str, idle_threshold_ns: u64) -> (SocketAddr, Arc<Server>, common::ScratchDir) {
    start_server_with(name, idle_threshold_ns, sim_ingress::server::SSE_WRITE_TIMEOUT)
}

/// As `start_server`, with the SSE write timeout chosen by the test: the production 30 s is right
/// for a browser behind a slow proxy and wrong for a test that wants to see the timeout fire.
fn start_server_with(
    name: &str,
    idle_threshold_ns: u64,
    sse_write_timeout: Duration,
) -> (SocketAddr, Arc<Server>, common::ScratchDir) {
    let dir = common::scratch(&format!("ingress-http-{name}"));
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let server = Arc::new(Server::new(dir.to_path_buf(), idle_threshold_ns).with_sse_write_timeout(sse_write_timeout));
    let handle = Arc::clone(&server);
    std::thread::spawn(move || sim_ingress::serve_on(handle, listener));
    (addr, server, dir)
}

/// One HTTP response, body fully read.
struct Response {
    status: u16,
    body: String,
}

impl Response {
    fn json(&self) -> Json {
        parse_json(&self.body).unwrap_or_else(|e| panic!("{e} in {:?}", self.body))
    }
}

fn read_head(reader: &mut BufReader<TcpStream>) -> (u16, Vec<(String, String)>) {
    let mut line = String::new();
    reader.read_line(&mut line).unwrap();
    let status: u16 = line.split_whitespace().nth(1).expect("status line").parse().unwrap();
    let mut headers = Vec::new();
    loop {
        line.clear();
        reader.read_line(&mut line).unwrap();
        if line == "\r\n" || line.is_empty() {
            break;
        }
        if let Some((k, v)) = line.split_once(':') {
            headers.push((k.trim().to_ascii_lowercase(), v.trim().to_string()));
        }
    }
    (status, headers)
}

fn request(addr: SocketAddr, method: &str, target: &str, extra_headers: &[(&str, &str)], body: &str) -> Response {
    let mut stream = TcpStream::connect(addr).unwrap();
    let mut head = format!("{method} {target} HTTP/1.1\r\nHost: localhost\r\nContent-Length: {}\r\n", body.len());
    for (k, v) in extra_headers {
        head.push_str(&format!("{k}: {v}\r\n"));
    }
    head.push_str("\r\n");
    stream.write_all(head.as_bytes()).unwrap();
    stream.write_all(body.as_bytes()).unwrap();
    let mut reader = BufReader::new(stream);
    let (status, headers) = read_head(&mut reader);
    let length: usize = headers.iter().find(|(k, _)| k == "content-length").map(|(_, v)| v.parse().unwrap()).unwrap_or(0);
    let mut body = vec![0u8; length];
    reader.read_exact(&mut body).unwrap();
    Response { status, body: String::from_utf8(body).unwrap() }
}

fn post(addr: SocketAddr, rpc: &str, body: &str) -> Response {
    request(addr, "POST", &format!("/v1/ingress/{rpc}"), &[], body)
}

/// `StartRun` of a scenario file with overrides; returns the run id.
fn start_run(addr: SocketAddr, file: &str, overrides: &[(&str, &str)], max_realtime_factor: f64) -> String {
    let text = std::fs::read_to_string(workspace().join("scenarios").join(file)).unwrap();
    let escaped = text.replace('\\', "\\\\").replace('"', "\\\"").replace('\n', "\\n");
    let ov: Vec<String> = overrides.iter().map(|(k, v)| format!("\"{k}\":\"{v}\"")).collect();
    let body = format!(
        "{{\"scenario\":{{\"text\":\"{escaped}\",\"overrides\":{{{}}}}},\"max_realtime_factor\":{max_realtime_factor},\"record_traces\":false}}",
        ov.join(",")
    );
    let r = post(addr, "StartRun", &body);
    assert_eq!(r.status, 200, "{}", r.body);
    r.json().str("run_id").unwrap().to_string()
}

fn get_run(addr: SocketAddr, run_id: &str) -> Json {
    let r = post(addr, "GetRun", &format!("{{\"run_id\":\"{run_id}\"}}"));
    assert_eq!(r.status, 200, "{}", r.body);
    r.json()
}

fn wait_for_state(addr: SocketAddr, run_id: &str, state: &str, within: Duration) -> Json {
    let deadline = Instant::now() + within;
    loop {
        let s = get_run(addr, run_id);
        if s.str("state") == Some(state) {
            return s;
        }
        assert!(Instant::now() < deadline, "run {run_id} did not reach {state}: {s:?}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// The value under a numeric metric key in a `values` map.
fn metric_value(row: &Json, metric: i32) -> Option<f64> {
    row.get("values")?.f64(&metric.to_string())
}

/// Every `"key":` in a JSON document, minus the enum-number map keys, which are numbers by rule 4.
fn json_keys(text: &str) -> BTreeSet<String> {
    let mut keys = BTreeSet::new();
    let b = text.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'"' {
            let start = i + 1;
            let mut j = start;
            while j < b.len() && b[j] != b'"' {
                if b[j] == b'\\' {
                    j += 1;
                }
                j += 1;
            }
            if j + 1 < b.len() && b[j + 1] == b':' {
                let key = &text[start..j];
                if key.parse::<i64>().is_err() {
                    keys.insert(key.to_string());
                }
            }
            i = j + 1;
        } else {
            i += 1;
        }
    }
    keys
}

/// Field names declared in the protos the wire is built from.
fn proto_fields() -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for name in ["ingress.proto", "subscription.proto", "metrics.proto", "common.proto"] {
        let text = std::fs::read_to_string(workspace().join("proto/lbsim/v1").join(name)).unwrap();
        // One statement per `;`, since the protos put small messages on one line.
        for stmt in text.lines().flat_map(|l| l.split("//").next().unwrap_or("").split(';')) {
            if let Some((decl, _)) = stmt.split_once('=') {
                if let Some(field) = decl.split_whitespace().last() {
                    out.insert(field.to_string());
                }
            }
        }
    }
    out
}

fn assert_proto_keys(doc: &str) {
    let fields = proto_fields();
    for k in json_keys(doc) {
        assert!(fields.contains(&k), "{k:?} is not a proto field; document: {doc}");
    }
}

// ---------------------------------------------------------------------------
// Server-sent events, read
// ---------------------------------------------------------------------------

struct Event {
    id: Option<u64>,
    event: String,
    data: String,
}

/// An open SSE stream. `next` returns `None` at end of stream; a read that stalls for longer than
/// the timeout is a failure, never a hang.
#[derive(Debug)]
struct Sse {
    reader: BufReader<TcpStream>,
}

impl Sse {
    fn next(&mut self) -> Option<Event> {
        let mut ev = Event { id: None, event: String::new(), data: String::new() };
        let mut line = String::new();
        loop {
            line.clear();
            match self.reader.read_line(&mut line) {
                Ok(0) => return None,
                Ok(_) => {}
                Err(e) => panic!("reading the stream: {e}"),
            }
            let l = line.trim_end_matches(['\r', '\n']);
            if l.is_empty() {
                if ev.event.is_empty() && ev.data.is_empty() {
                    continue; // a keepalive comment's blank line
                }
                return Some(ev);
            }
            if l.starts_with(':') {
                continue;
            }
            let (field, value) = l.split_once(':').unwrap_or((l, ""));
            let value = value.strip_prefix(' ').unwrap_or(value);
            match field {
                "id" => ev.id = Some(value.parse().unwrap()),
                "event" => ev.event = value.to_string(),
                "data" => ev.data = value.to_string(),
                _ => {}
            }
        }
    }

    fn update(&mut self) -> Event {
        let ev = self.next().expect("stream ended early");
        assert_eq!(ev.event, "update", "{}", ev.data);
        ev
    }
}

/// `GET /v1/ingress/OpenSubscription?query`. On 200 the stream is returned; otherwise the status
/// and body are.
fn open_subscription(addr: SocketAddr, query: &str, headers: &[(&str, &str)]) -> Result<Sse, (u16, String)> {
    let mut stream = TcpStream::connect(addr).unwrap();
    stream.set_read_timeout(Some(Duration::from_secs(30))).unwrap();
    let mut head = format!("GET /v1/ingress/OpenSubscription?{query} HTTP/1.1\r\nHost: localhost\r\nAccept: text/event-stream\r\n");
    for (k, v) in headers {
        head.push_str(&format!("{k}: {v}\r\n"));
    }
    head.push_str("\r\n");
    stream.write_all(head.as_bytes()).unwrap();
    let mut reader = BufReader::new(stream);
    let (status, hs) = read_head(&mut reader);
    if status != 200 {
        let length: usize = hs.iter().find(|(k, _)| k == "content-length").map(|(_, v)| v.parse().unwrap()).unwrap_or(0);
        let mut body = vec![0u8; length];
        reader.read_exact(&mut body).unwrap();
        return Err((status, String::from_utf8(body).unwrap()));
    }
    assert!(hs.iter().any(|(k, v)| k == "content-type" && v.starts_with("text/event-stream")), "{hs:?}");
    Ok(Sse { reader })
}

/// Open and read the `open` event, returning the stream and the subscription id (or the rejection).
fn subscribe(addr: SocketAddr, query: &str) -> (Sse, Json) {
    let mut sse = open_subscription(addr, query, &[]).unwrap_or_else(|(s, b)| panic!("{s}: {b}"));
    let first = sse.next().expect("an open event");
    assert_eq!(first.event, "open", "{}", first.data);
    assert_eq!(first.id, None, "the open event carries no id; ids number the updates");
    assert_proto_keys(&first.data);
    let open = parse_json(&first.data).unwrap();
    (sse, open)
}

fn renew(addr: SocketAddr, sid: &str, lease_ns: u64) -> Json {
    let r = post(addr, "RenewSubscription", &format!("{{\"subscription_id\":\"{sid}\",\"lease_ns\":\"{lease_ns}\"}}"));
    assert_eq!(r.status, 200, "{}", r.body);
    assert_proto_keys(&r.body);
    r.json()
}

fn close(addr: SocketAddr, sid: &str) {
    let r = post(addr, "CloseSubscription", &format!("{{\"subscription_id\":\"{sid}\"}}"));
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(r.body, "{}");
}

// ---------------------------------------------------------------------------
// Lifecycle
// ---------------------------------------------------------------------------

#[test]
fn a_run_starts_completes_and_has_a_positive_goodput() {
    let (addr, _server, _dir) = start_server("lifecycle", 3600 * S);
    let run_id = start_run(addr, "route_p2c.txt", &[("duration_s", "20")], 0.0);
    assert_eq!(run_id, "r-1");

    let done = wait_for_state(addr, &run_id, "STATE_COMPLETE", Duration::from_secs(120));
    assert_eq!(done.str("error"), Some(""));
    assert_eq!(done.str("sim_time_unix_ns"), done.str("sim_end_unix_ns"));
    assert_proto_keys(&post(addr, "GetRun", &format!("{{\"run_id\":\"{run_id}\"}}")).body);

    let listed = post(addr, "ListRuns", "{}");
    assert_eq!(listed.status, 200);
    assert_proto_keys(&listed.body);
    let runs = match listed.json().get("runs") {
        Some(Json::Arr(items)) => items.clone(),
        other => panic!("runs: {other:?}"),
    };
    assert!(runs.iter().any(|r| r.str("run_id") == Some(run_id.as_str())));

    let result = post(addr, "GetResult", &format!("{{\"run_id\":\"{run_id}\"}}"));
    assert_eq!(result.status, 200, "{}", result.body);
    assert_proto_keys(&result.body);
    let doc = result.json();
    assert_eq!(doc.str("run_id"), Some(run_id.as_str()));
    let goodput = metric_value(doc.get("overall").unwrap(), 45).expect("goodput in the scorecard");
    assert!(goodput > 0.0, "goodput {goodput}");
    assert!(doc.u64("event_count").unwrap() > 0);

    // Errors, per WIRE.md rule 6.
    let missing = post(addr, "GetRun", "{\"run_id\":\"r-999\"}");
    assert_eq!(missing.status, 404);
    assert!(missing.json().str("error").is_some());
    assert_eq!(post(addr, "SetSpeed", &format!("{{\"run_id\":\"{run_id}\",\"paused\":true}}")).status, 409);
    assert_eq!(post(addr, "StartRun", "{\"scenario\":{\"text\":\"nonsense = 1\"}}").status, 400);
    assert_eq!(post(addr, "StartRun", "not json").status, 400);
    assert_eq!(post(addr, "NoSuchRpc", "{}").status, 404);
    assert_eq!(request(addr, "GET", "/v1/ingress/GetRun", &[], "").status, 405);
}

#[test]
fn the_same_scenario_started_twice_has_the_same_fingerprint() {
    let (addr, _server, _dir) = start_server("determinism", 3600 * S);
    let a = start_run(addr, "route_p2c.txt", &[("duration_s", "20")], 0.0);
    let b = start_run(addr, "route_p2c.txt", &[("duration_s", "20")], 0.0);
    // The second is paced, so its wall-clock schedule differs from the first's; the result must not.
    let c = start_run(addr, "route_p2c.txt", &[("duration_s", "20")], 40.0);
    let mut checksums = Vec::new();
    for id in [&a, &b, &c] {
        wait_for_state(addr, id, "STATE_COMPLETE", Duration::from_secs(120));
        let r = post(addr, "GetResult", &format!("{{\"run_id\":\"{id}\"}}")).json();
        checksums.push((r.str("state_checksum").unwrap().to_string(), r.str("event_count").unwrap().to_string()));
    }
    assert_eq!(checksums[0], checksums[1]);
    assert_eq!(checksums[0], checksums[2], "a paced run must produce the unpaced run's numbers");
    assert_ne!(checksums[0].0, "0");
}

#[test]
fn speed_step_and_stop_are_honoured_and_bounded() {
    let (addr, _server, _dir) = start_server("control", 3600 * S);
    // Paced at one simulated second per wall second, so the run is nowhere near done when the
    // controls arrive.
    let run_id = start_run(addr, "route_p2c.txt", &[("duration_s", "100")], 1.0);
    let paused = post(addr, "SetSpeed", &format!("{{\"run_id\":\"{run_id}\",\"paused\":true,\"realtime_factor\":1}}"));
    assert_eq!(paused.status, 200, "{}", paused.body);
    assert_eq!(paused.json().str("state"), Some("STATE_PAUSED"));
    let no_result = post(addr, "GetResult", &format!("{{\"run_id\":\"{run_id}\"}}"));
    assert_eq!(no_result.status, 409, "{}", no_result.body);

    // A step asks for two minutes and gets one: the cap, and the response says where it stopped.
    let before: u64 = get_run(addr, &run_id).str("sim_time_unix_ns").unwrap().parse().unwrap();
    let stepped = post(addr, "StepForward", &format!("{{\"run_id\":\"{run_id}\",\"sim_duration_ns\":\"{}\"}}", 120 * S));
    assert_eq!(stepped.status, 200, "{}", stepped.body);
    let s = stepped.json();
    assert_eq!(s.str("state"), Some("STATE_PAUSED"));
    let after: u64 = s.str("sim_time_unix_ns").unwrap().parse().unwrap();
    assert_eq!(after - before, 60 * S, "stepped by the cap, not by what was asked");

    // Windows are the scenario's sample interval, 250 ms here.
    let stepped = post(addr, "StepForward", &format!("{{\"run_id\":\"{run_id}\",\"barrier_windows\":4}}"));
    assert_eq!(stepped.status, 200, "{}", stepped.body);
    let t: u64 = stepped.json().str("sim_time_unix_ns").unwrap().parse().unwrap();
    assert_eq!(t - after, S);
    assert_eq!(post(addr, "StepForward", &format!("{{\"run_id\":\"{run_id}\"}}")).status, 400);

    // Stop ends the run where it stands, and the result is the aggregation over what ran.
    let stopped = post(addr, "StopRun", &format!("{{\"run_id\":\"{run_id}\"}}"));
    assert_eq!(stopped.status, 200, "{}", stopped.body);
    let s = stopped.json();
    assert_eq!(s.str("state"), Some("STATE_COMPLETE"));
    assert_eq!(s.str("sim_time_unix_ns").unwrap().parse::<u64>().unwrap(), t);
    let result = post(addr, "GetResult", &format!("{{\"run_id\":\"{run_id}\"}}"));
    assert_eq!(result.status, 200, "{}", result.body);
    assert_eq!(post(addr, "StepForward", &format!("{{\"run_id\":\"{run_id}\",\"barrier_windows\":1}}")).status, 409);
}


// ---------------------------------------------------------------------------
// Subscriptions
// ---------------------------------------------------------------------------

#[test]
fn a_subscription_streams_at_the_asked_cadence_renews_and_closes() {
    let (addr, _server, _dir) = start_server("subscribe", 3600 * S);
    // Paced at 4 simulated seconds per wall second: a 40 s run takes 10 s, long enough to renew
    // and close mid-stream.
    let run_id = start_run(addr, "route_p2c.txt", &[("duration_s", "40")], 4.0);
    let (mut sse, open) = subscribe(
        addr,
        &format!("run_id={run_id}&scope=SCOPE_FLEET&samples_per_sim_second=4&percentiles=50,99&lease_ns={}", 30 * S),
    );
    let sid = open.str("subscription_id").expect("subscription_id").to_string();
    assert!(open.u64("lease_expires_at_wall_ns").unwrap() > 0);
    assert!(open.str("rejected_reason").is_none());

    let mut last_t = 0u64;
    for n in 1..=10u64 {
        let ev = sse.update();
        assert_eq!(ev.id, Some(n), "ids are the delivered-update sequence from 1");
        assert_proto_keys(&ev.data);
        let u = parse_json(&ev.data).unwrap();
        assert_eq!(u.str("subscription_id"), Some(sid.as_str()));
        assert_eq!(u.f64("realtime_factor"), Some(4.0));
        assert_eq!(u.bool("final"), Some(false));
        // 4 samples per second on a 250 ms sample interval: every frame, 250 ms apart, never an
        // instant the engine did not record.
        let t = u.u64("sim_time_unix_ns").unwrap();
        if n > 1 {
            assert_eq!(t - last_t, 250_000_000, "update {n}");
        }
        last_t = t;
        let row = u.get("row").unwrap();
        assert_eq!(row.get("target").unwrap().str("scope"), Some("SCOPE_FLEET"));
        assert_eq!(metric_value(row, 40), Some(560.0), "offered rps is the scenario's");
        assert_eq!(metric_value(row, 60), Some(256.0), "ready replicas");
        assert!(metric_value(row, 23).is_some(), "queued");
        if let Some(d) = row.get("distributions").unwrap().get("1") {
            assert_eq!(d.get("percentile"), Some(&Json::Arr(vec![Json::Num(50.0), Json::Num(99.0)])));
            assert!(matches!(d.get("value"), Some(Json::Arr(v)) if v.len() == 2));
            assert!(d.u64("count").unwrap() > 0);
            assert_eq!(d.bool("from_merged_histogram"), Some(true), "frame histograms are bucketed");
        }
    }

    let renewed = renew(addr, &sid, 30 * S);
    assert_eq!(renewed.bool("expired"), Some(false));
    assert!(renewed.u64("lease_expires_at_wall_ns").unwrap() >= open.u64("lease_expires_at_wall_ns").unwrap());

    close(addr, &sid);
    // The stream ends: whatever was in flight, then EOF, within the read timeout.
    let ended = Instant::now();
    while sse.next().is_some() {}
    assert!(ended.elapsed() < Duration::from_secs(5));
    assert_eq!(renew(addr, &sid, 30 * S).bool("expired"), Some(true), "closed is gone");
    close(addr, &sid); // idempotent

    // Replica scope carries only what was asked for, for that replica.
    let (mut sse, open) = subscribe(
        addr,
        &format!("run_id={run_id}&scope=SCOPE_REPLICA&replica_id=3&metrics=METRIC_QUEUED_SEQS,METRIC_KV_UTILIZATION&samples_per_sim_second=1&lease_ns={}", 30 * S),
    );
    let sid = open.str("subscription_id").unwrap().to_string();
    let u = parse_json(&sse.update().data).unwrap();
    let row = u.get("row").unwrap();
    assert_eq!(row.get("target").unwrap().str("scope"), Some("SCOPE_REPLICA"));
    assert_eq!(row.get("target").unwrap().str("replica_id"), Some("3"));
    let values = match row.get("values") {
        Some(Json::Obj(fields)) => fields.iter().map(|(k, _)| k.clone()).collect::<Vec<_>>(),
        other => panic!("{other:?}"),
    };
    assert_eq!(values, vec!["20", "23"]);
    close(addr, &sid);

    // Rejections are a 200 with `rejected_reason`, as the proto says; an unknown run is a 404.
    for bad in [
        format!("run_id={run_id}&scope=SCOPE_POOL&pool_id=1&samples_per_sim_second=1"),
        format!("run_id={run_id}&scope=SCOPE_REPLICA&replica_id=999&samples_per_sim_second=1"),
        format!("run_id={run_id}&scope=SCOPE_FLEET&metrics=METRIC_NOPE&samples_per_sim_second=1"),
        format!("run_id={run_id}&scope=SCOPE_FLEET&metrics=METRIC_STEP_TIME&samples_per_sim_second=1"),
        format!("run_id={run_id}&scope=SCOPE_FLEET&samples_per_sim_second=0"),
        format!("run_id={run_id}&scope=SCOPE_FLEET&samples_per_sim_second=1&percentiles=50,0"),
    ] {
        let (mut sse, open) = subscribe(addr, &bad);
        assert!(open.str("rejected_reason").is_some(), "{bad}");
        assert!(sse.next().is_none(), "a rejected stream ends after the open event");
    }
    assert_eq!(open_subscription(addr, "run_id=r-9&scope=SCOPE_FLEET&samples_per_sim_second=1", &[]).unwrap_err().0, 404);
    assert_eq!(open_subscription(addr, &format!("run_id={run_id}&scope=SCOPE_FLEET&samples_per_sim_second=x"), &[]).unwrap_err().0, 400);

    // On a finished run the stream replays every sample and ends with `final`.
    wait_for_state(addr, &run_id, "STATE_COMPLETE", Duration::from_secs(60));
    let (mut sse, _) = subscribe(addr, &format!("run_id={run_id}&scope=SCOPE_FLEET&samples_per_sim_second=1&lease_ns={}", 30 * S));
    let mut updates = Vec::new();
    while let Some(ev) = sse.next() {
        updates.push(parse_json(&ev.data).unwrap());
    }
    assert_eq!(updates.len(), 40, "one per simulated second of a 40 s run");
    assert!(updates[..39].iter().all(|u| u.bool("final") == Some(false)));
    assert_eq!(updates[39].bool("final"), Some(true));
}

#[test]
fn an_expired_lease_ends_the_stream() {
    let (addr, _server, _dir) = start_server("expiry", 3600 * S);
    let run_id = start_run(addr, "route_p2c.txt", &[("duration_s", "40")], 1.0);
    let (mut sse, open) = subscribe(addr, &format!("run_id={run_id}&scope=SCOPE_FLEET&samples_per_sim_second=4&lease_ns=1"));
    let sid = open.str("subscription_id").unwrap().to_string();
    let started = Instant::now();
    while sse.next().is_some() {}
    assert!(started.elapsed() < Duration::from_secs(5), "a dead lease ends the stream promptly");
    let r = renew(addr, &sid, 30 * S);
    assert_eq!(r.bool("expired"), Some(true), "and it is not resurrected by a renew");
    assert_eq!(renew(addr, "s-999", 30 * S).bool("expired"), Some(true));
    post(addr, "StopRun", &format!("{{\"run_id\":\"{run_id}\"}}"));
}

#[test]
fn a_reconnect_with_last_event_id_resumes_at_the_right_id() {
    let (addr, _server, _dir) = start_server("reconnect", 3600 * S);
    // A paused run that has been stepped a minute: 240 frames, 480 samples at 8 per second, more
    // than the ring holds and no more arriving, so what the ring covers is exactly known.
    let run_id = start_run(addr, "route_p2c.txt", &[("duration_s", "100")], 1.0);
    post(addr, "SetSpeed", &format!("{{\"run_id\":\"{run_id}\",\"paused\":true}}"));
    let stepped = post(addr, "StepForward", &format!("{{\"run_id\":\"{run_id}\",\"sim_duration_ns\":\"{}\"}}", 60 * S));
    assert_eq!(stepped.status, 200, "{}", stepped.body);

    let query = format!("run_id={run_id}&scope=SCOPE_FLEET&samples_per_sim_second=8&lease_ns={}", 60 * S);
    let (mut sse, open) = subscribe(addr, &query);
    let sid = open.str("subscription_id").unwrap().to_string();
    let mut seen = Vec::new();
    for n in 1..=300u64 {
        let ev = sse.update();
        assert_eq!(ev.id, Some(n));
        seen.push(ev.data);
    }
    drop(sse);

    // Too far back for the ring, and an id that was never issued: 410, resubscribe from scratch.
    let resume = |last: &str| open_subscription(addr, &format!("{query}&subscription_id={sid}"), &[("Last-Event-ID", last)]);
    assert_eq!(resume("10").unwrap_err().0, 410);
    assert_eq!(resume("100000").unwrap_err().0, 410);
    assert_eq!(
        open_subscription(addr, &format!("{query}&subscription_id=s-999"), &[("Last-Event-ID", "1")]).unwrap_err().0,
        410
    );

    // Within the ring: the same subscription, the next id, the same bytes.
    let mut sse = resume("299").unwrap_or_else(|(s, b)| panic!("{s}: {b}"));
    let first = sse.next().unwrap();
    assert_eq!(first.event, "open");
    assert_eq!(parse_json(&first.data).unwrap().str("subscription_id"), Some(sid.as_str()));
    let ev = sse.update();
    assert_eq!(ev.id, Some(300));
    assert_eq!(ev.data, seen[299], "the replayed update is byte-identical");
    let ev = sse.update();
    assert_eq!(ev.id, Some(301));
    // Frames repeat at 8 samples per second on a 250 ms interval: the 301st sample is frame 151.
    let t = parse_json(&ev.data).unwrap().u64("sim_time_unix_ns").unwrap();
    assert_eq!(t, lbsim::EPOCH_BASE + 151 * 250_000_000);
    close(addr, &sid);
    post(addr, "StopRun", &format!("{{\"run_id\":\"{run_id}\"}}"));
}


// ---------------------------------------------------------------------------
// Idle shutdown and health
// ---------------------------------------------------------------------------

fn frames_of(server: &Server, run_id: &str) -> usize {
    server.runs.get(run_id).expect("run exists").lock().frames.len()
}

#[test]
fn the_idle_guard_checkpoints_a_finished_run_and_stops_an_unwatched_paced_one() {
    // One second of idleness, injected through the server rather than the environment.
    let (addr, server, dir) = start_server("idle", S);

    // A finished run with no lease: checkpointed within two wall seconds of finishing.
    let run_id = start_run(addr, "route_p2c.txt", &[("duration_s", "20")], 0.0);
    wait_for_state(addr, &run_id, "STATE_COMPLETE", Duration::from_secs(120));
    let run_dir = dir.join("runs").join(&run_id);
    let since = Instant::now();
    while !run_dir.join("result.json").is_file() {
        assert!(since.elapsed() < Duration::from_secs(2), "no checkpoint under {}", run_dir.display());
        std::thread::sleep(Duration::from_millis(20));
    }
    let status = std::fs::read_to_string(run_dir.join("status.json")).unwrap();
    assert!(status.contains("\"state\":\"STATE_COMPLETE\""), "{status}");
    assert_proto_keys(&status);
    let fleet = std::fs::read_to_string(run_dir.join("fleet.jsonl")).unwrap();
    let lines: Vec<&str> = fleet.lines().collect();
    assert_eq!(lines.len(), frames_of(&server, &run_id), "one row per frame");
    assert_eq!(lines.len(), 80, "20 s at 250 ms");
    assert!(lines[79].contains("\"final\":true") && !lines[78].contains("\"final\":true"));
    assert_proto_keys(lines[40]);
    assert_proto_keys(&std::fs::read_to_string(run_dir.join("result.json")).unwrap());
    assert!(std::fs::read_to_string(run_dir.join("scenario.txt")).unwrap().contains("duration_s = 20"));

    // A paced run nobody is watching: stopped, reported STATE_PAUSED with an empty error, and
    // resumed by a subscription opening.
    let run_id = start_run(addr, "route_p2c.txt", &[("duration_s", "100")], 1.0);
    let paused = wait_for_state(addr, &run_id, "STATE_PAUSED", Duration::from_secs(5));
    assert_eq!(paused.str("error"), Some(""));
    assert!(dir.join("runs").join(&run_id).join("fleet.jsonl").is_file());
    let frames = frames_of(&server, &run_id);
    std::thread::sleep(Duration::from_millis(600));
    assert_eq!(frames_of(&server, &run_id), frames, "an idle-stopped run does not advance");

    let (mut sse, open) = subscribe(addr, &format!("run_id={run_id}&scope=SCOPE_FLEET&samples_per_sim_second=4&lease_ns={}", 30 * S));
    let sid = open.str("subscription_id").unwrap().to_string();
    wait_for_state(addr, &run_id, "STATE_RUNNING", Duration::from_secs(5));
    let since = Instant::now();
    while frames_of(&server, &run_id) == frames {
        assert!(since.elapsed() < Duration::from_secs(5), "the resumed run does not advance");
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(sse.update().id.is_some());
    close(addr, &sid);
    post(addr, "StopRun", &format!("{{\"run_id\":\"{run_id}\"}}"));
}

#[test]
fn health_answers_without_touching_a_run() {
    let (addr, server, _dir) = start_server("health", 3600 * S);
    let run_id = start_run(addr, "route_p2c.txt", &[("duration_s", "100")], 1.0);
    let stepped = post(addr, "StepForward", &format!("{{\"run_id\":\"{run_id}\",\"barrier_windows\":8}}"));
    assert_eq!(stepped.status, 200, "{}", stepped.body);
    let frames = frames_of(&server, &run_id);
    assert!(frames >= 8);
    let before = get_run(addr, &run_id);
    for _ in 0..10 {
        for path in ["/health", "/healthz"] {
            let r = request(addr, "GET", path, &[], "");
            assert_eq!((r.status, r.body.as_str()), (200, "ok"), "{path}");
        }
    }
    assert_eq!(frames_of(&server, &run_id), frames, "twenty health checks moved the run");
    let after = get_run(addr, &run_id);
    assert_eq!(after.str("sim_time_unix_ns"), before.str("sim_time_unix_ns"));
    assert_eq!(after.str("state"), Some("STATE_PAUSED"));
    post(addr, "StopRun", &format!("{{\"run_id\":\"{run_id}\"}}"));
}

// ---------------------------------------------------------------------------
// Hardening: a hostile or careless client must not take the server down with it
// ---------------------------------------------------------------------------

#[test]
fn deep_json_nesting_is_refused_not_a_crash() {
    let (addr, _server, _dir) = start_server("deep", 3600 * S);
    // Twenty thousand open brackets: a recursive parser without a depth limit runs off the
    // thread stack here, which aborts the process and every run in it.
    let r = post(addr, "GetRun", &"[".repeat(20 * 1024));
    assert_eq!(r.status, 400, "{}", r.body);
    assert!(r.body.contains("nest"), "{}", r.body);
    let r = request(addr, "GET", "/health", &[], "");
    assert_eq!((r.status, r.body.as_str()), (200, "ok"));
}

/// Poll `probe` until it holds or the deadline passes.
fn wait_until(what: &str, within: Duration, mut probe: impl FnMut() -> bool) {
    let deadline = Instant::now() + within;
    while !probe() {
        assert!(Instant::now() < deadline, "{what}: not within {within:?}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn a_stalled_sse_reader_does_not_pin_the_connection_slot() {
    let (addr, server, _dir) = start_server_with("stall", 3600 * S, Duration::from_millis(500));
    let run_id = start_run(addr, "route_p2c.txt", &[("duration_s", "60")], 0.0);
    // A cadence far above the frame rate gives the writer tens of thousands of updates for a
    // reader that takes none of them, so the socket buffers fill and the next write blocks; with
    // no write timeout it would block, holding its thread and slot, for as long as the reader
    // stayed connected.
    let (sse, open) = subscribe(addr, &format!("run_id={run_id}&scope=SCOPE_FLEET&samples_per_sim_second=1000&lease_ns={}", 3600 * S));
    let sid = open.str("subscription_id").unwrap().to_string();
    assert_eq!(server.connections(), 1);
    wait_until("the stalled writer gave its slot back", Duration::from_secs(20), || {
        let r = request(addr, "GET", "/health", &[], "");
        assert_eq!((r.status, r.body.as_str()), (200, "ok"), "a new connection is accepted meanwhile");
        server.connections() == 0
    });
    // The reader is still connected and its lease still runs: a reconnect within it would resume.
    assert_eq!(server.open_subscriptions(), 1);
    assert_eq!(renew(addr, &sid, 30 * S).bool("expired"), Some(false));
    drop(sse);
    post(addr, "StopRun", &format!("{{\"run_id\":\"{run_id}\"}}"));
}

#[test]
fn a_closed_sse_client_frees_its_slot_at_once_and_is_reaped_when_its_lease_runs_out() {
    let (addr, server, _dir) = start_server("closed", 3600 * S);
    // Paced, so the writer touches the socket every 250 ms of wall time and notices the peer is gone.
    let run_id = start_run(addr, "route_p2c.txt", &[("duration_s", "100")], 1.0);
    let query = format!("run_id={run_id}&scope=SCOPE_FLEET&samples_per_sim_second=4&lease_ns={}", 30 * S);
    let (mut sse, open) = subscribe(addr, &query);
    let sid = open.str("subscription_id").unwrap().to_string();
    let last = sse.update().id.unwrap();
    drop(sse);
    wait_until("the writer of a vanished peer exited", Duration::from_secs(5), || server.connections() == 0);

    // Within the lease the subscription is still there to resume from, per WIRE.md.
    let mut sse = open_subscription(addr, &format!("{query}&subscription_id={sid}"), &[("Last-Event-ID", &last.to_string())])
        .unwrap_or_else(|(s, b)| panic!("{s}: {b}"));
    assert_eq!(sse.next().unwrap().event, "open");
    assert_eq!(sse.update().id, Some(last + 1));
    drop(sse);
    wait_until("the second writer exited", Duration::from_secs(5), || server.connections() == 0);

    // Once the lease runs out with no writer left to notice, the next request reaps the ring.
    assert_eq!(renew(addr, &sid, 1).bool("expired"), Some(false));
    wait_until("the expired subscription was reaped", Duration::from_secs(5), || {
        renew(addr, &sid, 30 * S).bool("expired") == Some(true) && server.open_subscriptions() == 0
    });
    assert_eq!(
        open_subscription(addr, &format!("{query}&subscription_id={sid}"), &[("Last-Event-ID", &last.to_string())]).unwrap_err().0,
        410,
        "a reaped subscription cannot be resumed"
    );
    post(addr, "StopRun", &format!("{{\"run_id\":\"{run_id}\"}}"));
}

#[test]
fn is_final_is_sent_exactly_once() {
    let (addr, _server, _dir) = start_server("final", 3600 * S);
    // Paced at the recorded cadence, so the subscription reads each frame as it closes and reaches
    // the last one before the run has aggregated its result: the window in which a final update
    // could go out twice.
    let run_id = start_run(addr, "route_p2c.txt", &[("duration_s", "20")], 8.0);
    let (mut sse, _open) = subscribe(addr, &format!("run_id={run_id}&scope=SCOPE_FLEET&samples_per_sim_second=4&lease_ns={}", 60 * S));
    let mut updates = Vec::new();
    while let Some(ev) = sse.next() {
        assert_eq!(ev.event, "update", "{}", ev.data);
        assert_eq!(ev.id, Some(updates.len() as u64 + 1));
        let u = parse_json(&ev.data).unwrap();
        updates.push((u.u64("sim_time_unix_ns").unwrap(), u.bool("final") == Some(true)));
    }
    assert!(updates.len() >= 8, "{updates:?}");
    let finals: Vec<usize> = updates.iter().enumerate().filter(|(_, (_, f))| *f).map(|(i, _)| i).collect();
    assert_eq!(finals, vec![updates.len() - 1], "one final update, and it is the last: {updates:?}");
    assert!(updates.windows(2).all(|w| w[0].0 < w[1].0), "no frame is sent twice: {updates:?}");
    let status = wait_for_state(addr, &run_id, "STATE_COMPLETE", Duration::from_secs(5));
    assert_eq!(status.str("error"), Some(""));
}

// ---------------------------------------------------------------------------
// UpdateWorkload and UpdatePolicies: forward-only, applied on the run thread
// ---------------------------------------------------------------------------

fn update(addr: SocketAddr, rpc: &str, run_id: &str, overrides: &[(&str, &str)]) -> Json {
    let ov: Vec<String> = overrides.iter().map(|(k, v)| format!("\"{k}\":\"{v}\"")).collect();
    let r = post(addr, rpc, &format!("{{\"run_id\":\"{run_id}\",\"overrides\":{{{}}}}}", ov.join(",")));
    assert_eq!(r.status, 200, "{}", r.body);
    assert_proto_keys(&r.body);
    r.json()
}

fn step(addr: SocketAddr, run_id: &str, by_ns: u64) -> Json {
    let r = post(addr, "StepForward", &format!("{{\"run_id\":\"{run_id}\",\"sim_duration_ns\":\"{by_ns}\"}}"));
    assert_eq!(r.status, 200, "{}", r.body);
    r.json()
}

/// Mean `offered_rps` over the run's closed frames with `t` in `[from, to)`.
fn mean_offered(server: &Server, run_id: &str, from: u64, to: u64) -> f64 {
    let run = server.runs.get(run_id).expect("run exists");
    let st = run.lock();
    let window: Vec<f64> = st.frames.iter().filter(|f| f.t >= from && f.t < to).map(|f| f.offered_rps).collect();
    assert!(!window.is_empty(), "no frames in [{from}, {to})");
    window.iter().sum::<f64>() / window.len() as f64
}

#[test]
fn update_workload_mid_run_raises_offered_rps() {
    let (addr, server, _dir) = start_server("update-workload", 3600 * S);
    let run_id = start_run(addr, "route_p2c.txt", &[("duration_s", "100")], 1.0);
    post(addr, "SetSpeed", &format!("{{\"run_id\":\"{run_id}\",\"paused\":true,\"realtime_factor\":1}}"));
    let t0: u64 = get_run(addr, &run_id).str("sim_time_unix_ns").unwrap().parse().unwrap();
    step(addr, &run_id, 20 * S);

    // The run is paused: the update must land anyway, without waiting for the next advance.
    let r = update(addr, "UpdateWorkload", &run_id, &[("arrival_rps", "1680")]);
    assert_eq!(r.bool("accepted"), Some(true), "{r:?}");
    assert_eq!(r.bool("required_resimulation"), Some(false));
    assert_eq!(r.str("rewound_to_unix_ns"), Some("0"));
    assert_eq!(r.str("rejected_reason"), Some(""));

    step(addr, &run_id, 20 * S);
    let before = mean_offered(&server, &run_id, t0, t0 + 20 * S);
    let after = mean_offered(&server, &run_id, t0 + 20 * S, t0 + 40 * S);
    let ratio = after / before;
    assert!((2.5..3.5).contains(&ratio), "offered_rps {before} -> {after}: ratio {ratio}, expected about 3");
}

#[test]
fn update_with_a_structural_key_is_rejected_with_a_reason() {
    let (addr, _server, _dir) = start_server("update-structural", 3600 * S);
    let run_id = start_run(addr, "route_p2c.txt", &[("duration_s", "100")], 1.0);
    step(addr, &run_id, 5 * S);
    let before = get_run(addr, &run_id);

    // The whole batch goes: the good key beside the structural one is not applied either.
    let r = update(addr, "UpdateWorkload", &run_id, &[("arrival_rps", "1"), ("replicas", "12")]);
    assert_eq!(r.bool("accepted"), Some(false), "{r:?}");
    let reason = r.str("rejected_reason").unwrap();
    assert!(reason.contains("replicas") && reason.contains("restart"), "{reason}");
    assert_eq!(get_run(addr, &run_id), before);

    // A key nothing recognises is structural too.
    let r = update(addr, "UpdatePolicies", &run_id, &[("no_such_key", "1")]);
    assert_eq!(r.bool("accepted"), Some(false), "{r:?}");
    assert!(r.str("rejected_reason").unwrap().contains("no_such_key"));
}

#[test]
fn update_workload_refuses_a_policy_key() {
    let (addr, _server, _dir) = start_server("update-wrong-group", 3600 * S);
    let run_id = start_run(addr, "route_p2c.txt", &[("duration_s", "100")], 1.0);
    let r = update(addr, "UpdateWorkload", &run_id, &[("routing", "round_robin")]);
    assert_eq!(r.bool("accepted"), Some(false), "{r:?}");
    let reason = r.str("rejected_reason").unwrap();
    assert!(reason.contains("routing") && reason.contains("UpdatePolicies"), "{reason}");

    let r = update(addr, "UpdatePolicies", &run_id, &[("arrival_rps", "5")]);
    assert_eq!(r.bool("accepted"), Some(false), "{r:?}");
    let reason = r.str("rejected_reason").unwrap();
    assert!(reason.contains("arrival_rps") && reason.contains("UpdateWorkload"), "{reason}");
}

#[test]
fn update_policies_switches_routing() {
    let (addr, _server, _dir) = start_server("update-policies", 3600 * S);
    let run_id = start_run(addr, "route_round_robin.txt", &[("duration_s", "100")], 1.0);
    let before: u64 = wait_for_state(addr, &run_id, "STATE_RUNNING", Duration::from_secs(5))
        .str("sim_time_unix_ns").unwrap().parse().unwrap();

    let r = update(addr, "UpdatePolicies", &run_id, &[("routing", "p2c")]);
    assert_eq!(r.bool("accepted"), Some(true), "{r:?}");
    assert_eq!(r.str("rejected_reason"), Some(""));

    // Still running, still advancing: the update did not pause or restart the run.
    let s = get_run(addr, &run_id);
    assert_eq!(s.str("state"), Some("STATE_RUNNING"), "{s:?}");
    wait_until("the run advances past the update", Duration::from_secs(5), || {
        get_run(addr, &run_id).str("sim_time_unix_ns").unwrap().parse::<u64>().unwrap() > before + S
    });

    // Setting the value it already has is accepted and changes nothing.
    let r = update(addr, "UpdatePolicies", &run_id, &[("routing", "p2c")]);
    assert_eq!(r.bool("accepted"), Some(true), "{r:?}");
    assert_eq!(post(addr, "Rewind", &format!("{{\"run_id\":\"{run_id}\"}}")).status, 501);
}
