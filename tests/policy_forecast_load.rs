//! `forecast_load`: power-of-two-choices scored on a predicted queue depth instead of the stale
//! scrape. See `crates/sim-policy/src/forecast_load.rs` for the estimator; this file only checks the
//! observable difference it is meant to make: no worse balance than `p2c` once telemetry goes stale,
//! and no meaningful difference from `p2c` when telemetry is fresh enough that the prediction
//! collapses back to the stale value.
//!
//! The candidate draw is identical to `p2c`'s (same `d`, same rng sequence, same tie-break), so any
//! difference measured here is the scoring, not the sampling.

mod common;

use common::*;
use lbsim::sim;

/// Coefficient of variation of each replica's mean load over the run: `stddev / mean` across the 32
/// per-replica sample means. Lower is more balanced.
fn imbalance_cv(r: &sim::RunResult) -> f64 {
    let means: Vec<f64> = r
        .replica_load
        .iter()
        .map(|s| {
            assert!(!s.v.is_empty(), "a replica_load series has no samples");
            s.v.iter().sum::<f64>() / s.v.len() as f64
        })
        .collect();
    let n = means.len() as f64;
    let mean = means.iter().sum::<f64>() / n;
    let var = means.iter().map(|m| (m - mean).powi(2)).sum::<f64>() / n;
    var.sqrt() / mean
}

#[test]
fn forecast_load_is_registered_and_labelled() {
    let mut s = at_load(0.2);
    s.routing = "forecast_load".into();
    let r = sim::run(&s).expect("forecast_load must resolve through the routing registry");
    assert_eq!(r.routing_label, "forecast_load(d=2)");
    assert_eq!(r.replicas_inspected_per_decision, 2);
}

#[test]
fn forecast_load_is_deterministic() {
    let mut s = at_load(0.3);
    s.routing = "forecast_load".into();
    let a = sim::run(&s).unwrap();
    let b = sim::run(&s).unwrap();
    assert_eq!(a.fingerprint, b.fingerprint, "forecast_load fingerprint is not repeatable");
    assert_eq!(a.events, b.events);
    assert_eq!(a.records.len(), b.records.len());
}

/// The case forecast_load exists for: telemetry stale enough (4 s) that a plain snapshot read is
/// fighting old information, and the forecast should not do worse than reading it straight.
#[test]
fn under_stale_telemetry_forecast_load_balances_no_worse_than_p2c() {
    let mut p2c = at_load(0.3);
    p2c.telemetry_interval_ms = 4000;
    p2c.routing = "p2c".into();
    let rp2c = sim::run(&p2c).unwrap();
    let cv_p2c = imbalance_cv(&rp2c);

    let mut fl = at_load(0.3);
    fl.telemetry_interval_ms = 4000;
    fl.routing = "forecast_load".into();
    let rfl = sim::run(&fl).unwrap();
    let cv_fl = imbalance_cv(&rfl);

    eprintln!("stale (4000ms): p2c CV = {cv_p2c:.6}, forecast_load CV = {cv_fl:.6}");
    assert!(
        cv_fl <= cv_p2c * 1.05,
        "forecast_load CV {cv_fl:.6} exceeds p2c CV {cv_p2c:.6} by more than 5%"
    );
}

/// With fresh telemetry (100 ms), two distinct views are rarely both available at decision time, so
/// the estimator falls back to the stale value almost every time and forecast_load should track p2c
/// closely. If the occasional slope term breaks a tie differently, the fingerprints can diverge even
/// though the balance does not; in that case we fall back to comparing CVs within 2%.
#[test]
fn with_fresh_telemetry_forecast_load_equals_p2c_within_noise() {
    let mut p2c = at_load(0.3);
    p2c.telemetry_interval_ms = 100;
    p2c.routing = "p2c".into();
    let rp2c = sim::run(&p2c).unwrap();

    let mut fl = at_load(0.3);
    fl.telemetry_interval_ms = 100;
    fl.routing = "forecast_load".into();
    let rfl = sim::run(&fl).unwrap();

    if rp2c.fingerprint == rfl.fingerprint {
        eprintln!("fresh (100ms): fingerprints identical, as expected when predictions collapse to the stale value");
        assert_eq!(rp2c.records.len(), rfl.records.len());
    } else {
        let cv_p2c = imbalance_cv(&rp2c);
        let cv_fl = imbalance_cv(&rfl);
        eprintln!(
            "fresh (100ms): fingerprints differ (a slope term perturbed a tie); p2c CV = {cv_p2c:.6}, forecast_load CV = {cv_fl:.6}"
        );
        let diff = (cv_fl - cv_p2c).abs();
        assert!(
            diff <= cv_p2c.max(cv_fl) * 0.02,
            "forecast_load CV {cv_fl:.6} differs from p2c CV {cv_p2c:.6} by more than 2%"
        );
    }
}
