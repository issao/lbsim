//! Retries must add retries, and nothing else.
//!
//! A retry is the one place in the model where a *failure* creates new work, so it is also the one
//! place where the arrival process can be corrupted from the inside. This file is a regression guard
//! on exactly that: the number of genuinely new requests offered to the fleet is a property of the
//! workload alone, and enabling retries must not change it.

mod common;

use common::*;
use lbsim::scenario::Scenario;
use lbsim::sim;

/// Overloaded, with a short client deadline, so a large number of requests actually time out and the
/// retry path is exercised hard.
fn retry_fixture(max_attempts: u32, budget: f64) -> Scenario {
    let mut s = small();
    s.arrival_rps = 1.3 * s.rated_rps();
    s.client_timeout_s = 3.0;
    s.duration_s = 12.0;
    s.warmup_s = 1.0;
    s.max_attempts = max_attempts;
    s.retry_budget_fraction = budget;
    s
}

/// Enabling retries must not change how many *first* attempts the workload offers.
///
/// This is a regression test for a real bug (fixed in `17b3843`): the retry path used to schedule a
/// fresh `Arrival` event alongside re-dispatching the retried request. Because every arrival schedules
/// its own successor, each retry permanently forked the arrival chain, so offered load grew
/// geometrically in the number of retries — one measured run went from 504 first attempts to 239,397.
/// The symptom looked exactly like a retry storm, which is the phenomenon the scenario was built to
/// study, so it would have been read as a result rather than as a bug.
#[test]
fn enabling_retries_does_not_change_the_number_of_first_attempts() {
    let baseline = sim::run(&retry_fixture(1, 1.0)).unwrap();
    assert_eq!(baseline.retries, 0, "max_attempts = 1 must never retry");
    assert!(baseline.first_attempts > 100, "fixture too small: {} first attempts", baseline.first_attempts);

    for max_attempts in [2_u32, 3, 4] {
        let r = sim::run(&retry_fixture(max_attempts, 1.0)).unwrap();
        assert!(
            r.retries > 0,
            "max_attempts = {max_attempts} produced no retries; the fixture stopped timing out"
        );
        assert_eq!(
            r.first_attempts, baseline.first_attempts,
            "max_attempts = {max_attempts} changed the count of first attempts from {} to {}: \
             the arrival process is being perturbed by the retry path",
            baseline.first_attempts, r.first_attempts
        );
    }
}

/// The retry budget is what stops a slowdown becoming a collapse, so it has to actually bind.
#[test]
fn retries_stay_within_the_configured_budget() {
    for budget in [0.05_f64, 0.2, 0.5] {
        let r = sim::run(&retry_fixture(4, budget)).unwrap();
        let ratio = r.retries as f64 / r.first_attempts as f64;
        // The budget is checked before a retry is counted, so it can be exceeded by exactly one.
        let allowance = budget + 1.0 / r.first_attempts as f64;
        assert!(
            ratio <= allowance,
            "retry fraction {ratio:.4} exceeds the budget of {budget} \
             ({} retries against {} first attempts)",
            r.retries, r.first_attempts
        );
        assert!(r.retries > 0, "budget {budget} allowed no retries at all");
    }
}

/// A retry must not reset the user's clock. Latency is measured from the first arrival, because from
/// the user's point of view the wait started when they first asked — a retried request that "meets" its
/// SLO by forgetting the first attempt would make a collapsing fleet look healthy.
#[test]
fn retried_requests_keep_their_original_arrival_time() {
    let s = retry_fixture(3, 1.0);
    let r = sim::run(&s).unwrap();
    let retried: Vec<_> = r.records.iter().filter(|x| x.attempts > 1).collect();
    assert!(!retried.is_empty(), "no retried request produced a record");

    let backoff_ns = (s.retry_backoff_s * 1e9) as u64;
    for x in &retried {
        assert!(
            x.finished_at >= x.arrived_at,
            "record {} finished before it arrived",
            x.id
        );
        // A second attempt cannot have finished sooner than one backoff after the first arrival.
        assert!(
            x.finished_at - x.arrived_at >= backoff_ns,
            "record {} on attempt {} shows only {} ns of latency, less than one backoff: the clock \
             was reset by the retry",
            x.id, x.attempts, x.finished_at - x.arrived_at
        );
    }
}
