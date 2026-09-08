//! Replica failures: silent slowdowns, hangs and announced crashes.
//!
//! VISION section 4: a machine may fail silently and to the client look like it is running very
//! slowly. The failure schedule is the one input that makes a replica misbehave, so these tests pin
//! three things: that an empty schedule changes nothing at all, that a gray failure is invisible to a
//! load-based router and still drags the tail, and that an announced crash reaches the router only
//! after the telemetry delay.

mod common;

use common::*;
use lbsim::metrics::{Outcome, RequestRecord};
use lbsim::scenario::Scenario;
use lbsim::sim::{self, RunResult, NO_REPLICA};
use lbsim::{Nanos, EPOCH_BASE};

/// Four replicas at 30% of rated load, long enough for a failure at t=60 s to have a before and an
/// after of equal length.
fn four(routing: &str) -> Scenario {
    let mut s = small();
    s.replicas = 4;
    s.routing = routing.into();
    s.duration_s = 120.0;
    s.warmup_s = 5.0;
    s.arrival_rps = 0.3 * s.rated_rps();
    s
}

fn at(seconds: f64) -> Nanos {
    EPOCH_BASE + (seconds * 1e9) as Nanos
}

fn p99_ttft_s(recs: &[&RequestRecord]) -> f64 {
    let mut v: Vec<Nanos> = recs.iter().filter_map(|r| r.ttft()).collect();
    assert!(v.len() >= 50, "only {} first tokens; the fixture is too small for a p99", v.len());
    v.sort_unstable();
    v[(v.len() - 1) * 99 / 100] as f64 / 1e9
}

fn routed(r: &RunResult) -> Vec<&RequestRecord> {
    r.records.iter().filter(|x| x.replica != NO_REPLICA).collect()
}

/// The failure path schedules nothing when the schedule is empty, so a run without failures must
/// be the run it was before failures existed. Re-pinned when the baseline fleet moved to 256 replicas
/// at 560 rps (Issao, 2026-09-07).
#[test]
fn no_failures_is_byte_identical() {
    let text = std::fs::read_to_string("scenarios/route_p2c.txt").unwrap();
    let sc = Scenario::parse(&text).unwrap();
    assert!(sc.failures.is_empty());
    let r = sim::run(&sc).unwrap();
    assert_eq!(r.fingerprint, 15913688846466930736, "the replica loop changed with no failures configured");
    assert_eq!(r.events, 2391338, "the event stream changed with no failures configured");
}

/// A replica at 0.3x speed reports itself healthy, so a router that reads queue depth keeps sending
/// it a real share of the traffic, and every request it gets is slow. That is a gray failure: nothing
/// is ejected, and the tail is what tells you.
#[test]
fn a_slow_replica_is_still_sent_traffic_by_least_requests_and_drags_p99() {
    let mut s = four("least_requests");
    s.failures = "t=60,replica=3,kind=slow=0.3".into();
    let r = sim::run(&s).unwrap();
    let all = routed(&r);
    let before: Vec<&RequestRecord> =
        all.iter().copied().filter(|x| x.arrived_at >= at(s.warmup_s) && x.arrived_at < at(60.0)).collect();
    let after: Vec<&RequestRecord> = all.iter().copied().filter(|x| x.arrived_at >= at(60.0)).collect();

    let (p99_before, p99_after) = (p99_ttft_s(&before), p99_ttft_s(&after));
    let share = after.iter().filter(|x| x.replica == 3).count() as f64 / after.len() as f64;
    let failed = |recs: &[&RequestRecord]| recs.iter().filter(|x| !x.outcome.is_success()).count();
    let (failed_before, failed_after) = (failed(&before), failed(&after));
    eprintln!(
        "gray failure: p99 TTFT {p99_before:.3} s before, {p99_after:.3} s after; slow replica share \
         {:.1}% of {} routed; {failed_before} failed before, {failed_after} after",
        100.0 * share, after.len()
    );
    // The p99 is over requests that got a first token at all; most of what the slow replica takes
    // never does, because at 0.3x a mean decode outlasts the 8 s client timeout. Both tells are asserted.
    assert!(
        p99_after >= 1.5 * p99_before,
        "p99 TTFT went {p99_before:.3} s -> {p99_after:.3} s; a 0.3x replica should drag it"
    );
    assert!(failed_after > failed_before, "timeouts did not climb: {failed_before} -> {failed_after}");
    // In-flight counts equalise, so a router that reads them backs off a slow replica only as far as
    // its latency ratio; timeouts cap the in-flight count and keep it in the rotation.
    assert!(share > 0.10, "least_requests starved the slow replica to {:.1}%; it should not notice", 100.0 * share);
    // And nothing announced it.
    assert!(r.records.iter().all(|x| x.replica != NO_REPLICA || x.outcome == Outcome::Rejected));
}

/// A crash is announced through the same delayed telemetry as everything else, so for one delay the
/// router keeps sending to a replica that is gone. Round robin, so the share it keeps
/// sending is exact rather than a matter of which stale view looked least loaded.
#[test]
fn a_crash_is_announced_after_the_telemetry_delay() {
    let mut s = four("round_robin");
    s.duration_s = 40.0;
    s.telemetry_delay_ms = 2000.0;
    s.telemetry_interval_ms = 100.0;
    s.max_attempts = 3;
    s.failures = "t=20,replica=1,kind=crash".into();
    let r = sim::run(&s).unwrap();
    let crash = at(20.0);
    // Refused dispatches, as opposed to the requests it held at the crash, which fail at that instant.
    let refused: Vec<&RequestRecord> = r
        .records
        .iter()
        .filter(|x| x.replica == 1 && x.outcome == Outcome::TimeoutQueued && x.finished_at > crash)
        .collect();
    let stale = refused.iter().filter(|x| x.finished_at <= crash + 2_000_000_000).count();
    // One publish interval past the delay, the ejected view has certainly landed.
    let announced = refused.iter().filter(|x| x.finished_at > crash + 2_200_000_000).count();
    let held = r.records.iter().filter(|x| x.replica == 1 && x.finished_at == crash).count();
    eprintln!("crash: {held} held requests failed at once, {stale} sent on stale views, {announced} after");
    assert!(held > 0, "the crashed replica held nothing");
    assert!(stale > 0, "the router never sent to the crashed replica on a stale view");
    assert_eq!(announced, 0, "requests still reached the crashed replica after the announcement");
    assert!(r.retries > 0, "nothing retried");
    assert!(
        r.records.iter().all(|x| x.replica != 1 || x.finished_at <= crash + 2_200_000_000 || x.outcome.is_success() || x.arrived_at < crash),
        "a request arriving after the announcement still landed on the crashed replica"
    );
}

/// A hung replica finishes nothing: every request it holds, and every one dispatched to it, ends in a
/// client timeout, and each first attempt retries.
#[test]
fn a_hang_times_out_its_requests() {
    let mut s = four("p2c");
    s.duration_s = 60.0;
    s.max_attempts = 2;
    s.failures = "t=10,replica=2,kind=hang".into();
    let r = sim::run(&s).unwrap();
    let hang = at(10.0);
    let on_hung: Vec<&RequestRecord> =
        r.records.iter().filter(|x| x.replica == 2 && x.finished_at > hang).collect();
    assert!(on_hung.len() > 10, "only {} requests met the hung replica", on_hung.len());
    for x in &on_hung {
        assert!(
            matches!(x.outcome, Outcome::TimeoutQueued | Outcome::TimeoutRunning),
            "request {} on the hung replica ended {:?}",
            x.id, x.outcome
        );
        // A retry that starts too late to time out before the run ends leaves no record.
        let settles = x.finished_at + ((s.retry_backoff_s + s.client_timeout_s + 0.5) * 1e9) as Nanos <= at(s.duration_s);
        if x.attempts == 1 && settles {
            let retry = 1_000_000_000 + x.id * 8 + 2;
            assert!(r.records.iter().any(|y| y.id == retry), "request {} timed out and did not retry", x.id);
        }
    }
    eprintln!("hang: {} requests timed out on the hung replica, {} retries in the run", on_hung.len(), r.retries);
}
