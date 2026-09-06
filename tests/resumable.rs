//! `Sim`: the run as a struct that can stop and continue.
//!
//! `StepForward`, `SetSpeed` and the Leaf's `Advance` all need to drive the loop a window at a time.
//! What must hold is that a run driven in pieces is the run: same fingerprint, same events, same
//! records, same series and frames, whatever the piece boundaries. These tests check exactly that,
//! with boundaries chosen to fall between events, on events, and past the end.

mod common;

use lbsim::scenario::Scenario;
use lbsim::sim::{self, RunResult, Sim};
use lbsim::{Nanos, SECOND};

fn fixture() -> Scenario {
    let mut s = common::at_load(1.2);
    s.duration_s = 20.0;
    s.warmup_s = 2.0;
    s
}

/// Everything a run reports, compared field by field, so a mismatch names what diverged.
fn assert_same(a: &RunResult, b: &RunResult, how: &str) {
    assert_eq!(a.fingerprint, b.fingerprint, "{how}: fingerprint");
    assert_eq!(a.events, b.events, "{how}: events");
    assert_eq!(a.records.len(), b.records.len(), "{how}: record count");
    assert_eq!(format!("{:?}", a.records), format!("{:?}", b.records), "{how}: records");
    assert_eq!(a.outcomes, b.outcomes, "{how}: outcomes");
    assert_eq!(a.retries, b.retries, "{how}: retries");
    assert_eq!(a.first_attempts, b.first_attempts, "{how}: first attempts");
    let series = |r: &RunResult| -> Vec<(String, Vec<Nanos>, Vec<f64>)> {
        let mut all: Vec<&lbsim::metrics::Series> =
            vec![&r.fleet_queue, &r.fleet_running, &r.fleet_kv_utilization, &r.offered_rps];
        all.extend(r.replica_load.iter());
        all.into_iter().map(|s| (s.name.clone(), s.t.clone(), s.v.clone())).collect()
    };
    assert_eq!(series(a), series(b), "{how}: series");
    assert_eq!(a.frames, b.frames, "{how}: frames");
    for q in [50.0, 90.0, 99.0] {
        assert_eq!(a.ttft.percentile(q), b.ttft.percentile(q), "{how}: ttft p{q}");
        assert_eq!(a.e2e.percentile(q), b.e2e.percentile(q), "{how}: e2e p{q}");
        assert_eq!(a.itl_max.percentile(q), b.itl_max.percentile(q), "{how}: itl p{q}");
        assert_eq!(a.queue_wait.percentile(q), b.queue_wait.percentile(q), "{how}: wait p{q}");
    }
}

fn drive(sc: &Scenario, targets: &[Nanos]) -> RunResult {
    let mut s = Sim::new(sc).unwrap();
    for &t in targets {
        s.advance_to(t).unwrap();
        assert!(s.now() <= t.max(s.end()) && s.now() <= s.end(), "now passed its target");
    }
    assert!(s.finished(), "the last target was the end, so the run must be over");
    s.into_result().unwrap()
}

#[test]
fn advancing_in_many_steps_equals_one_run() {
    let sc = fixture();
    let whole = sim::run(&sc).unwrap();
    let start = lbsim::EPOCH_BASE;
    let end = start + (sc.duration_s * 1e9) as Nanos;

    // One-second chunks, boundaries that coincide with sample instants and telemetry publishes.
    let seconds: Vec<Nanos> = (1..=20).map(|k| start + k * SECOND).collect();
    assert_same(&whole, &drive(&sc, &seconds), "1 s chunks");

    // Seventeen uneven chunks, so boundaries fall between events and on none of the periodic ones,
    // plus a final target past the end.
    let mut uneven = Vec::new();
    let mut t = start;
    for k in 1..=16u64 {
        t += 700 * lbsim::MILLI + k * 137 * lbsim::MILLI + k * k * 1_234_567;
        uneven.push(t.min(end - 1));
    }
    uneven.push(end + 5 * SECOND);
    assert!(uneven.windows(2).all(|w| w[0] <= w[1]));
    assert_same(&whole, &drive(&sc, &uneven), "17 uneven chunks");

    // Repeating a target is a no-op, and so is asking for the past.
    let mut s = Sim::new(&sc).unwrap();
    s.advance_to(start + 3 * SECOND).unwrap();
    s.advance_to(start + 3 * SECOND).unwrap();
    s.advance_to(start + SECOND).unwrap();
    assert_eq!(s.now(), start + 3 * SECOND);
    s.advance_to(end).unwrap();
    assert_same(&whole, &s.into_result().unwrap(), "repeated and backwards targets");
}

#[test]
fn advance_to_never_passes_its_target() {
    let sc = fixture();
    let mut s = Sim::new(&sc).unwrap();
    let start = lbsim::EPOCH_BASE;
    assert_eq!(s.now(), start);
    let mut t = start;
    // Steps of a few milliseconds, so most targets land between events of the same replica.
    while t < s.end() {
        t += 37 * lbsim::MILLI;
        s.advance_to(t).unwrap();
        assert!(s.now() <= t, "now {} passed target {}", s.now(), t);
        // Nothing left behind: every frame closed so far is at or before now.
        assert!(s.frames().iter().all(|f| f.t <= s.now()));
        assert_eq!(s.latest_views().len(), sc.replicas);
    }
    assert!(s.finished());
    assert_eq!(s.now(), s.end());
    // Past the end, nothing changes.
    let before = s.frames().len();
    s.advance_to(s.end() + SECOND).unwrap();
    assert_eq!(s.frames().len(), before);
    let r = s.into_result().unwrap();
    assert_same(&sim::run(&sc).unwrap(), &r, "37 ms steps");
}

#[test]
fn frames_are_available_before_the_run_ends() {
    let sc = fixture();
    let mut s = Sim::new(&sc).unwrap();
    let start = lbsim::EPOCH_BASE;
    let sample = (sc.sample_interval_ms * 1e6) as Nanos;

    s.advance_to(start + 5 * SECOND).unwrap();
    assert!(!s.finished());
    let frames_at_5s = s.frames().len();
    assert_eq!(frames_at_5s as u64, 5 * SECOND / sample, "one frame per sample instant so far");
    assert_eq!(s.frames().last().unwrap().t, start + 5 * SECOND);

    // The live views reflect what the replicas hold now, not the delayed telemetry.
    let views = s.latest_views();
    let live: u64 = views.iter().map(|v| (v.queued + v.running) as u64).sum();
    let last = s.frames().last().unwrap();
    let sampled: u64 = last.replicas.iter().map(|r| (r.queued + r.running) as u64).sum();
    assert_eq!(live, sampled, "the sample at now and the live view at now agree");
    assert!(views.iter().all(|v| v.sampled_at == s.now()));

    s.advance_to(start + 10 * SECOND).unwrap();
    assert_eq!(s.frames().len() as u64, 10 * SECOND / sample);
    // Frames already handed out do not change as the run continues.
    let whole = sim::run(&sc).unwrap();
    assert_eq!(&whole.frames[..frames_at_5s], &s.frames()[..frames_at_5s]);
}
