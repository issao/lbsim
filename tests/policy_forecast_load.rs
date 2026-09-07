//! `forecast_load`: power-of-two-choices scored on a predicted queue depth instead of the stale
//! scrape. See `crates/sim-policy/src/forecast_load.rs` for the estimator; this file checks the two
//! observable claims the module doc makes: the candidate draw is byte-identical to `p2c`'s at a
//! seed, and once telemetry is stale enough to matter, the forecast actually beats a plain scrape.
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

/// The module doc's first claim: forecast_load draws candidates from the shared rng in exactly
/// p2c's sequence (same `d`, same skip-on-unusable, same tie-break), and never spends a draw of its
/// own. At `d = 1` there is nothing to score -- whichever single candidate is drawn wins outright if
/// usable -- so the two policies can only diverge here if forecast_load's draw sequence itself
/// diverges from p2c's. A stray draw smuggled onto the shared rng (instead of a named stream of its
/// own) would shift every later decision and show up as a fingerprint mismatch.
#[test]
fn forecast_load_draws_the_same_candidates_as_p2c() {
    let mut p2c = at_load(0.3);
    p2c.p2c_choices = 1;
    p2c.routing = "p2c".into();
    let rp2c = sim::run(&p2c).unwrap();

    let mut fl = at_load(0.3);
    fl.p2c_choices = 1;
    fl.routing = "forecast_load".into();
    let rfl = sim::run(&fl).unwrap();

    assert_eq!(
        rp2c.fingerprint, rfl.fingerprint,
        "forecast_load's candidate draw diverged from p2c's at d=1, where scoring cannot differ"
    );
    assert_eq!(rp2c.events, rfl.events);
    assert_eq!(rp2c.records.len(), rfl.records.len());
}

/// The case forecast_load exists for: telemetry stale enough (4 s) that a plain snapshot read is
/// fighting old information. The forecast should actually beat the stale read, not merely tie it.
#[test]
fn forecast_load_beats_p2c_under_stale_telemetry() {
    let mut p2c = at_load(0.3);
    p2c.telemetry_interval_ms = 4000.0;
    p2c.routing = "p2c".into();
    let rp2c = sim::run(&p2c).unwrap();
    let cv_p2c = imbalance_cv(&rp2c);
    let att_p2c = rp2c.slo_attainment();

    let mut fl = at_load(0.3);
    fl.telemetry_interval_ms = 4000.0;
    fl.routing = "forecast_load".into();
    let rfl = sim::run(&fl).unwrap();
    let cv_fl = imbalance_cv(&rfl);
    let att_fl = rfl.slo_attainment();

    eprintln!(
        "stale (4000ms): p2c CV = {cv_p2c:.6}, attainment = {att_p2c:.6}; forecast_load CV = {cv_fl:.6}, attainment = {att_fl:.6}"
    );
    assert!(
        cv_fl < cv_p2c || att_fl > att_p2c,
        "forecast_load did not beat p2c under stale telemetry: CV {cv_fl:.6} vs {cv_p2c:.6}, attainment {att_fl:.6} vs {att_p2c:.6}"
    );
}
