//! The foundation property: a scenario plus a seed is a complete description of a run.
//!
//! Nothing else in this suite means anything without this. Every comparison the simulator exists to
//! make — this policy against that one, this load against that one — is a difference between two
//! runs, and a difference is only attributable to the changed input if everything else about a run
//! is pinned by the scenario. A single leak of ambient state (hash iteration order, address-derived
//! ordering, wall-clock time) turns every conclusion in the project into noise that happens to look
//! like a result.

mod common;

use common::*;
use lbsim::sim;

/// Repeating a run in the same process must reproduce it exactly, not approximately.
///
/// Asserted on five independent observables at different depths of the pipeline: the step-time
/// fingerprint (the inside of the replica loop), the dispatched event count (the shape of the event
/// graph), the record count and the full record stream (per-request outcomes), and the p99 of
/// end-to-end latency (the aggregate anyone would actually publish). A determinism break that hides
/// from all five is not a break worth having.
#[test]
fn repeated_runs_of_the_same_scenario_and_seed_are_bit_identical() {
    for routing in ["p2c", "round_robin", "least_requests", "random", "least_queue_tokens"] {
        for &load in &[0.1_f64, 0.6, 1.2] {
            let mut s = at_load(load);
            s.routing = routing.into();

            let a = sim::run(&s).unwrap();
            let b = sim::run(&s).unwrap();

            let ctx = format!("routing={routing} load={load}");
            assert_eq!(a.fingerprint, b.fingerprint, "fingerprint differs ({ctx})");
            assert_eq!(a.events, b.events, "event count differs ({ctx})");
            assert_eq!(a.records.len(), b.records.len(), "record count differs ({ctx})");
            assert_eq!(
                a.e2e.percentile(99.0),
                b.e2e.percentile(99.0),
                "p99 e2e differs ({ctx})"
            );

            // The whole per-request stream, in order: the strongest statement available, and it
            // catches reordering that every aggregate above would average away.
            assert_eq!(
                format!("{:?}", a.records),
                format!("{:?}", b.records),
                "per-request records differ ({ctx})"
            );

            // Sampled gauges too, since a report reads these directly.
            assert_eq!(a.fleet_queue.v, b.fleet_queue.v, "fleet_queue series differs ({ctx})");
            for (i, (x, y)) in a.replica_load.iter().zip(b.replica_load.iter()).enumerate() {
                assert_eq!(x.v, y.v, "replica_load[{i}] series differs ({ctx})");
            }
        }
    }
}

/// A run must be non-trivial for the identity above to mean anything. A simulator that produced
/// zero records would pass every determinism check ever written.
#[test]
fn determinism_fixture_actually_simulates_something() {
    let s = at_load(0.6);
    let r = sim::run(&s).unwrap();
    assert!(r.records.len() > 50, "only {} records; fixture is too small to be a real test", r.records.len());
    assert!(r.events > 1000, "only {} events dispatched", r.events);
    assert!(r.fingerprint != 0, "fingerprint never accumulated anything");
    assert!(r.e2e.count() > 0, "no end-to-end latencies recorded");
}

/// Changing the seed must change the run. The converse of determinism: if the seed were ignored,
/// every run would be "reproducible" and every ensemble would be one sample repeated.
#[test]
fn different_seeds_produce_different_runs() {
    let mut s = at_load(0.6);
    s.seed = 1;
    let a = sim::run(&s).unwrap();
    s.seed = 2;
    let b = sim::run(&s).unwrap();
    assert_ne!(a.fingerprint, b.fingerprint, "seed had no effect on the fingerprint");
    assert_ne!(
        arrival_times(&settled(&a, &s)),
        arrival_times(&settled(&b, &s)),
        "seed had no effect on the arrival process"
    );
}
