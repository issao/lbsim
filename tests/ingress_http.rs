//! The live Ingress endpoint, driven over TCP exactly as a browser or curl would drive it.
//!
//! The server is the real one from `sim_ingress::serve_on`, bound to an ephemeral port and given
//! the idle threshold directly rather than through the environment, so the tests in this binary
//! can run in parallel without sharing a process-wide setting. The client below is a few dozen
//! lines of `std::net`: no HTTP library, because the point is that the wire is plain HTTP/1.1 and
//! plain server-sent events, readable with nothing else.

use sim_ingress::server::{parse_json, Json};
use sim_ingress::Server;
use std::collections::BTreeSet;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const S: u64 = 1_000_000_000;

fn workspace() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

/// A server on an ephemeral port serving a fresh directory. Returns the address and the directory,
/// which is where an idle checkpoint lands.
fn start_server(name: &str, idle_threshold_ns: u64) -> (SocketAddr, PathBuf) {
    let dir = std::env::temp_dir().join(format!("lbsim-ingress-http-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let server = Server::new(dir.clone(), idle_threshold_ns);
    std::thread::spawn(move || sim_ingress::serve_on(server, listener));
    (addr, dir)
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
// Lifecycle
// ---------------------------------------------------------------------------

#[test]
fn a_run_starts_completes_and_has_a_positive_goodput() {
    let (addr, _dir) = start_server("lifecycle", 3600 * S);
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
    let (addr, _dir) = start_server("determinism", 3600 * S);
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
    let (addr, _dir) = start_server("control", 3600 * S);
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
