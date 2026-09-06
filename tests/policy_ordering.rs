//! Herding: why global least-loaded is worse than a two-sample choice when telemetry is stale.
//!
//! `least_requests` picks the apparently-emptiest replica in the fleet. Every router looking at the
//! same stale snapshot picks the *same* replica, and they all pile onto it before the next telemetry
//! tick reveals what they did. `p2c` samples two replicas at random and takes the better of the two,
//! so no single apparently-idle replica is visible to more than a fraction of decisions at once.
//!
//! The whole point of modelling telemetry delay is to reproduce that effect. If this ordering
//! reversed, the delay mechanism would not be doing anything and the simulator's central claim about
//! load-balancer design would be unsupported. Direction is asserted, not magnitude — the size of the
//! gap depends on the telemetry period, and that is a result to be measured rather than pinned.

mod common;

use common::*;
use lbsim::sim;

#[test]
fn least_requests_herds_more_than_p2c_under_stale_telemetry() {
    for &load in &[0.15_f64, 0.3, 0.6, 0.9] {
        for seed in [1_u64, 7, 42] {
            let mut p2c = at_load(load);
            p2c.seed = seed;
            p2c.routing = "p2c".into();
            let mut least = p2c.clone();
            least.routing = "least_requests".into();

            let a = sim::run(&p2c).unwrap();
            let b = sim::run(&least).unwrap();

            let cv_p2c = a.load_imbalance_cv();
            let cv_least = b.load_imbalance_cv();
            assert!(cv_p2c.is_finite() && cv_least.is_finite(), "load imbalance was not measurable");

            assert!(
                cv_least > cv_p2c * 1.1,
                "load={load} seed={seed}: least_requests CV {cv_least:.4} is not clearly worse than \
                 p2c CV {cv_p2c:.4}; the herding effect has disappeared"
            );
        }
    }
}

/// Telemetry staleness is the mechanism, so removing it must shrink the gap.
///
/// With the snapshot delivered almost instantly and refreshed almost every step, a router looking at
/// the whole fleet sees close to the truth, and global least-loaded stops being a trap. This is what
/// separates "least_requests is a bad policy" from "least_requests is a bad policy *because it reads
/// stale state*", which is the claim the simulator is actually making.
#[test]
fn least_requests_herding_is_driven_by_telemetry_staleness() {
    let fresh_gap = |stale: bool| -> f64 {
        let mut p2c = at_load(0.6);
        p2c.seed = 7;
        p2c.routing = "p2c".into();
        if !stale {
            p2c.telemetry_interval_ms = 5.0;
            p2c.telemetry_delay_ms = 0.0;
        }
        let mut least = p2c.clone();
        least.routing = "least_requests".into();
        sim::run(&least).unwrap().load_imbalance_cv() / sim::run(&p2c).unwrap().load_imbalance_cv()
    };

    let stale_ratio = fresh_gap(true);
    let fresh_ratio = fresh_gap(false);
    assert!(
        fresh_ratio < stale_ratio,
        "least_requests/p2c imbalance ratio was {fresh_ratio:.3} with fresh telemetry and \
         {stale_ratio:.3} with stale telemetry; staleness is not what causes the herding"
    );
}

/// A policy that inspects the whole fleet must say so, because the cost of a decision is a first-class
/// result here: `least_requests` is the baseline whose O(N) decision is the thing being ruled out.
#[test]
fn decision_cost_is_reported_per_policy() {
    let expect = [
        ("round_robin", 1),
        ("random", 1),
        ("p2c", 2),
        ("least_requests", 8),
        ("least_queue_tokens", 8),
    ];
    for (routing, inspected) in expect {
        let mut s = at_load(0.2);
        s.routing = routing.into();
        assert_eq!(s.replicas, 8);
        let r = sim::run(&s).unwrap();
        assert_eq!(
            r.replicas_inspected_per_decision, inspected,
            "{routing} reported the wrong decision cost"
        );
    }
}

/// An unknown policy name must be an error, not a silent fallback to whatever is first in the match.
#[test]
fn unknown_routing_policy_is_rejected() {
    let mut s = at_load(0.2);
    s.routing = "least_loaded".into(); // plausible, and not a policy that exists
    let err = match sim::run(&s) {
        Ok(_) => panic!("unknown routing policy was accepted"),
        Err(e) => e,
    };
    assert!(err.contains("least_loaded"), "error does not name the offending policy: {err}");
}
