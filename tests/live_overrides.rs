//! Live overrides: a run that is under way takes a new workload or policy setting, forward only.
//!
//! The engine half of `UpdateWorkload` / `UpdatePolicies`. The properties that matter are the ones a
//! viewer would notice if they broke: the past does not change, the change takes hold from the next
//! arrival, a setting the engine cannot take mid-run is refused before anything moves, and the same
//! change at the same instant gives the same run.

mod common;

use common::*;
use lbsim::scenario::Scenario;
use lbsim::sim::{RunResult, Sim};
use lbsim::{Nanos, EPOCH_BASE};

/// Lightly loaded so tripling the arrival rate stays well inside capacity, and long enough that the
/// window after the change holds enough arrivals for a ratio to be meaningful.
fn fixture() -> Scenario {
    let mut s = at_load(0.25);
    s.duration_s = 60.0;
    s.routing = "round_robin".into();
    s
}

fn at(s: f64) -> Nanos {
    EPOCH_BASE + (s * 1e9) as Nanos
}

/// Advance to `when`, apply `overrides`, run to the end. An empty batch is the control.
fn drive(sc: &Scenario, when: f64, overrides: &[(&str, &str)]) -> RunResult {
    let mut sim = Sim::new(sc).unwrap();
    sim.advance_to(at(when)).unwrap();
    let applied = sim.apply_overrides(overrides).unwrap();
    assert_eq!(applied.at, sim.now());
    assert_eq!(applied.changed.len(), overrides.len());
    let end = sim.end();
    sim.advance_to(end).unwrap();
    sim.into_result().unwrap()
}

fn arrivals_in(r: &RunResult, lo: f64, hi: f64) -> usize {
    r.records.iter().filter(|x| x.arrived_at >= at(lo) && x.arrived_at < at(hi)).count()
}

/// Mean across-replica coefficient of variation of load, over the samples at or after `from`.
fn load_cv_from(r: &RunResult, from: f64) -> f64 {
    let t = &r.replica_load[0].t;
    let mut acc = 0.0;
    let mut n = 0usize;
    for s in 0..t.len() {
        if t[s] < at(from) {
            continue;
        }
        let vals: Vec<f64> = r.replica_load.iter().map(|x| x.v[s]).collect();
        let mean = vals.iter().sum::<f64>() / vals.len() as f64;
        if mean <= 0.0 {
            continue;
        }
        let var = vals.iter().map(|x| (x - mean) * (x - mean)).sum::<f64>() / vals.len() as f64;
        acc += var.sqrt() / mean;
        n += 1;
    }
    assert!(n > 10, "too few samples after {from} s: {n}");
    acc / n as f64
}

#[test]
fn raising_arrival_rps_mid_run_raises_arrivals_after_but_not_before() {
    let sc = fixture();
    let tripled = format!("{}", 3.0 * sc.arrival_rps);
    let control = drive(&sc, 20.0, &[]);
    let raised = drive(&sc, 20.0, &[("arrival_rps", &tripled)]);

    assert_eq!(arrivals_in(&raised, 0.0, 20.0), arrivals_in(&control, 0.0, 20.0));

    // Only arrivals that are guaranteed to have terminated by the end of the run leave a record.
    let hi = sc.duration_s - sc.client_timeout_s;
    let before = arrivals_in(&control, 20.0, hi) as f64;
    let after = arrivals_in(&raised, 20.0, hi) as f64;
    assert!(before > 50.0, "fixture too small: {before} control arrivals");
    let ratio = after / before;
    assert!((2.3..=3.7).contains(&ratio), "expected about 3x the arrivals, got {ratio:.2}x");
}

#[test]
fn a_structural_key_is_refused_and_nothing_changes() {
    let sc = fixture();
    let control = drive(&sc, 20.0, &[]);

    let mut sim = Sim::new(&sc).unwrap();
    sim.advance_to(at(20.0)).unwrap();
    let tripled = format!("{}", 3.0 * sc.arrival_rps);
    // A workload key first, so the refusal is shown to cover the whole batch and not only the offender.
    let err = sim.apply_overrides(&[("arrival_rps", &tripled), ("replicas", "4")]).unwrap_err();
    assert!(err.contains("replicas"), "the error must name the key: {err}");
    let err = sim.apply_overrides(&[("no_such_key", "1")]).unwrap_err();
    assert!(err.contains("no_such_key"), "the error must name the key: {err}");
    let end = sim.end();
    sim.advance_to(end).unwrap();
    let r = sim.into_result().unwrap();

    assert_eq!(r.fingerprint, control.fingerprint);
}

#[test]
fn switching_routing_mid_run_takes_effect() {
    let sc = fixture();
    let control = drive(&sc, 20.0, &[]);
    let switched = drive(&sc, 20.0, &[("routing", "p2c")]);

    let before = load_cv_from(&control, 20.0);
    let after = load_cv_from(&switched, 20.0);
    assert!(
        (before - after).abs() > 1e-6,
        "routing switch had no effect on load balance: control {before:.4}, switched {after:.4}"
    );
    assert_ne!(switched.fingerprint, control.fingerprint);
}

#[test]
fn the_same_overrides_at_the_same_time_are_deterministic() {
    let sc = fixture();
    let tripled = format!("{}", 3.0 * sc.arrival_rps);
    let overrides = [("arrival_rps", tripled.as_str()), ("routing", "p2c")];
    let a = drive(&sc, 20.0, &overrides);
    let b = drive(&sc, 20.0, &overrides);
    assert_eq!(a.fingerprint, b.fingerprint);
    assert_ne!(a.fingerprint, drive(&sc, 20.0, &[]).fingerprint, "the overrides must have done something");
}
