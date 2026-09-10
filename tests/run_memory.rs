//! Finished runs leave memory. Issao, 2026-09-10: "I have been hitting LBSIM_MEMORY_BUDGET_MB ...
//! can you check if there is some kind of memory leak or otherwise runaway memory usage issue".
//! There was: a completed run stayed in the registry, frames and all, until the instance died, so
//! every dashboard run on the public site added ninety-odd MB for good. This binary drives three
//! consecutive runs through the real server with a one-second retention and reads the heap off a
//! counting allocator, because resident-set numbers include what glibc keeps after a free and would
//! pass a leak or fail a fix on allocator mood.
//!
//! The allocator is the one `unsafe` in the workspace's tests: `GlobalAlloc` cannot be implemented
//! without it, and it wraps `System` with two atomics and nothing else.

mod common;

use sim_ingress::server::{parse_json, Json};
use sim_ingress::Server;
use std::alloc::{GlobalAlloc, Layout, System};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

struct Counting;

static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

fn count(delta: isize) {
    let live = if delta >= 0 {
        LIVE.fetch_add(delta as usize, Ordering::Relaxed) + delta as usize
    } else {
        LIVE.fetch_sub((-delta) as usize, Ordering::Relaxed) - (-delta) as usize
    };
    PEAK.fetch_max(live, Ordering::Relaxed);
}

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let p = System.alloc(layout);
        if !p.is_null() {
            count(layout.size() as isize);
        }
        p
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        System.dealloc(ptr, layout);
        count(-(layout.size() as isize));
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let p = System.realloc(ptr, layout, new_size);
        if !p.is_null() {
            count(new_size as isize - layout.size() as isize);
        }
        p
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

const MB: usize = 1024 * 1024;
const S: u64 = 1_000_000_000;

fn workspace() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

fn start_server(name: &str, retention_ns: u64) -> (SocketAddr, Arc<Server>, common::ScratchDir) {
    let dir = common::scratch(&format!("run-memory-{name}"));
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let server = Arc::new(Server::new(dir.to_path_buf(), 3600 * S).with_completed_retention(retention_ns));
    let handle = Arc::clone(&server);
    std::thread::spawn(move || sim_ingress::serve_on(handle, listener));
    (addr, server, dir)
}

fn request(addr: SocketAddr, method: &str, target: &str, body: &str) -> (u16, String) {
    let mut stream = TcpStream::connect(addr).unwrap();
    stream.set_read_timeout(Some(Duration::from_secs(60))).unwrap();
    write!(stream, "{method} {target} HTTP/1.1\r\nHost: localhost\r\nContent-Length: {}\r\n\r\n{body}", body.len()).unwrap();
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    reader.read_line(&mut line).unwrap();
    let status: u16 = line.split_whitespace().nth(1).expect("status line").parse().unwrap();
    let mut length = 0usize;
    let mut event_stream = false;
    loop {
        line.clear();
        reader.read_line(&mut line).unwrap();
        if line == "\r\n" || line.is_empty() {
            break;
        }
        let lower = line.to_ascii_lowercase();
        if let Some(v) = lower.strip_prefix("content-length:") {
            length = v.trim().parse().unwrap();
        }
        if lower.starts_with("content-type:") && lower.contains("text/event-stream") {
            event_stream = true;
        }
    }
    if event_stream {
        return (status, String::new());
    }
    let mut body = vec![0u8; length];
    reader.read_exact(&mut body).unwrap();
    (status, String::from_utf8(body).unwrap())
}

fn post(addr: SocketAddr, rpc: &str, body: &str) -> (u16, String) {
    request(addr, "POST", &format!("/v1/ingress/{rpc}"), body)
}

/// An unpaced 300 s run of the shared baseline at a size a debug build finishes in seconds. The
/// numbers under test are per-run retention, not the scenario's statistics.
fn start_run(addr: SocketAddr) -> String {
    let text = std::fs::read_to_string(workspace().join("scenarios/route_p2c.txt")).unwrap();
    let escaped = text.replace('\\', "\\\\").replace('"', "\\\"").replace('\n', "\\n");
    let body = format!(
        "{{\"scenario\":{{\"text\":\"{escaped}\",\"overrides\":{{\"duration_s\":\"300\",\"replicas\":\"32\",\"arrival_rps\":\"70\"}}}},\"max_realtime_factor\":0}}"
    );
    let (status, body) = post(addr, "StartRun", &body);
    assert_eq!(status, 200, "{body}");
    parse_json(&body).unwrap().str("run_id").unwrap().to_string()
}

fn state_of(addr: SocketAddr, run_id: &str) -> (u16, String) {
    let (status, body) = post(addr, "GetRun", &format!("{{\"run_id\":\"{run_id}\"}}"));
    let state = parse_json(&body).ok().and_then(|j| j.str("state").map(str::to_string)).unwrap_or_default();
    (status, state)
}

fn wait_for(what: &str, secs: u64, mut done: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(secs);
    while !done() {
        assert!(Instant::now() < deadline, "{what} did not happen within {secs} s");
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// One run to completion and then to release; returns its id and the `GetResult` body taken while
/// the run was still in memory.
fn run_to_release(addr: SocketAddr, server: &Server) -> (String, String) {
    let id = start_run(addr);
    wait_for("the run's completion", 300, || state_of(addr, &id).1 == "STATE_COMPLETE");
    let (status, in_memory) = post(addr, "GetResult", &format!("{{\"run_id\":\"{id}\"}}"));
    assert_eq!(status, 200, "{in_memory}");
    wait_for("the release", 30, || server.runs.held().iter().all(|h| h.run_id != id));
    (id, in_memory)
}

#[test]
fn three_finished_runs_leave_the_heap_where_one_did() {
    let (addr, server, dir) = start_server("three", S);
    let base = LIVE.load(Ordering::Relaxed);

    let (first, first_result) = run_to_release(addr, &server);
    let peak_one = PEAK.load(Ordering::Relaxed);
    let after_one = LIVE.load(Ordering::Relaxed);
    assert!(peak_one > base + 4 * MB, "a 300 s run should have held frames worth measuring: peak {peak_one} over base {base}");

    // Released, and still answered: the status from the stub, the result from the checkpoint, byte
    // for byte what the in-memory answer was.
    assert_eq!(state_of(addr, &first), (200, "STATE_COMPLETE".to_string()));
    let (status, from_disk) = post(addr, "GetResult", &format!("{{\"run_id\":\"{first}\"}}"));
    assert_eq!(status, 200, "{from_disk}");
    assert_eq!(from_disk, first_result);
    for name in ["status.json", "scenario.txt", "result.json", "fleet.jsonl", "replicas.jsonl", "checkpoint.json"] {
        assert!(dir.join("runs").join(&first).join(name).is_file(), "{name} is the checkpoint");
    }
    // What needs the engine, or the frames, says so rather than pretending.
    let (status, body) = post(addr, "StopRun", &format!("{{\"run_id\":\"{first}\"}}"));
    assert_eq!(status, 410, "{body}");
    assert!(body.contains("released") && body.contains(&format!("runs/{first}/")), "{body}");
    let (status, _) = request(addr, "GET", &format!("/v1/ingress/OpenSubscription?run_id={first}&scope=SCOPE_FLEET&samples_per_sim_second=4"), "");
    assert_eq!(status, 410);
    let (status, body) = post(addr, "GetRun", "{\"run_id\":\"r-99\"}");
    assert_eq!(status, 404, "{body}");

    let (second, _) = run_to_release(addr, &server);
    let (third, _) = run_to_release(addr, &server);
    let peak_three = PEAK.load(Ordering::Relaxed);
    let after_three = LIVE.load(Ordering::Relaxed);

    assert!(server.runs.held().is_empty(), "{:?}", server.runs.held());
    println!(
        "heap: base {} MB, peak after one run {} MB, after three {} MB; live after one {} MB, after three {} MB",
        base / MB,
        peak_one / MB,
        peak_three / MB,
        after_one / MB,
        after_three / MB
    );
    assert!(
        peak_three < peak_one + 20 * MB,
        "three runs peaked at {} MB, one at {} MB: a finished run is still held",
        peak_three / MB,
        peak_one / MB
    );
    assert!(
        after_three < after_one + 2 * MB,
        "the heap after three runs ({} MB) is not where it was after one ({} MB)",
        after_three / MB,
        after_one / MB
    );
    assert!(after_three < peak_one / 2, "released runs still hold most of a run's peak: {} of {}", after_three, peak_one);

    // Every run is still listed, complete, and the log's head says what is held.
    let (status, body) = post(addr, "ListRuns", "{}");
    assert_eq!(status, 200);
    let runs = match parse_json(&body).unwrap().get("runs") {
        Some(Json::Arr(items)) => items.clone(),
        other => panic!("{other:?}"),
    };
    let mut ids: Vec<String> = runs.iter().map(|r| r.str("run_id").unwrap().to_string()).collect();
    ids.sort();
    assert_eq!(ids, [first.clone(), second, third]);
    assert!(runs.iter().all(|r| r.str("state") == Some("STATE_COMPLETE")), "{body}");
    let (status, log) = request(addr, "GET", "/requests.log", "");
    assert_eq!(status, 200);
    let head = log.lines().next().unwrap_or("");
    assert!(head.starts_with("# rss_mb=") && head.contains("held_runs=0") && head.contains("released_runs=3"), "{head}");
    assert_eq!(server.memory_report().lines().count(), 1, "{}", server.memory_report());
}

/// The one entry of `runs/index.json` whose `run_id` is `id`, or `None`. Read over HTTP, the way
/// the dashboard's replay picker (`web/src/lib/replay.ts`) reads it: `GET /runs/index.json` is a
/// plain static file under the served directory, the index is a bare JSON array, not an object.
fn index_entry(addr: SocketAddr, id: &str) -> Option<Json> {
    let (status, body) = request(addr, "GET", "/runs/index.json", "");
    assert_eq!(status, 200, "{body}");
    match parse_json(&body).unwrap() {
        Json::Arr(items) => items.into_iter().find(|e| e.str("run_id") == Some(id)),
        other => panic!("runs/index.json is not an array: {other:?}"),
    }
}

/// A released live run answers the replay picker exactly as an export of the same run would.
/// Issao, 2026-09-10: "checkpoint under runs/<id>/ ... is not in runs/index.json, so the
/// dashboard's replay mode cannot open it."
#[test]
fn a_released_run_is_indexed_for_replay() {
    let (addr, server, _dir) = start_server("indexed", S);
    let (id, _) = run_to_release(addr, &server);

    let entry = index_entry(addr, &id).unwrap_or_else(|| panic!("no runs/index.json entry for {id}"));
    assert_eq!(entry.str("routing"), Some("p2c(d=2)"), "{entry:?}");
    assert_eq!(entry.u64("replicas"), Some(32), "{entry:?}");
    assert!(entry.f64("sample_interval_ms").is_some_and(|ms| ms > 0.0), "{entry:?}");
    let (start, end) = (entry.u64("sim_start_unix_ns").unwrap(), entry.u64("sim_end_unix_ns").unwrap());
    assert!(end > start, "the run's window should have positive length: {entry:?}");
    assert!(entry.u64("replica_sample_stride").is_some_and(|s| s >= 1), "{entry:?}");

    // A second run releases without disturbing the first's entry: `merge_index` keeps every run.
    let (second, _) = run_to_release(addr, &server);
    assert!(index_entry(addr, &id).is_some(), "the first run's entry survives a second release");
    assert!(index_entry(addr, &second).is_some(), "the second run is indexed too");
}

#[test]
fn a_finished_run_stays_for_the_retention_and_while_leased() {
    let (addr, server, _dir) = start_server("retention", 3600 * S);
    let id = start_run(addr);
    wait_for("the run's completion", 300, || state_of(addr, &id).1 == "STATE_COMPLETE");
    wait_for("the checkpoint", 30, || server.runs.get(&id).unwrap().lock().terminal_checkpoint);
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(server.runs.held().len(), 1, "an hour's retention holds the run");
    let (status, body) = post(addr, "GetResult", &format!("{{\"run_id\":\"{id}\"}}"));
    assert_eq!(status, 200, "{body}");
    assert!(server.memory_report().contains(&format!("# run {id} STATE_COMPLETE frames=")), "{}", server.memory_report());

    // A lease holds a run past its retention: a viewer reading the final frames is not cut off.
    server.runs.set_completed_retention_ns(0);
    let lease = server.runs.leases().open(&id, 60 * S, sim_ingress::idle::wall_now_ns());
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(server.runs.held().len(), 1, "leased, so held at zero retention");
    server.runs.leases().close(&lease.id);
    wait_for("the release once the lease closed", 10, || server.runs.held().is_empty());
    assert_eq!(state_of(addr, &id), (200, "STATE_COMPLETE".to_string()));
}
