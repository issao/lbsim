//! `weighted_random`: the routing half of Issao's buffer instruction, *"a weighed random policy where
//! the weight for any given node is a linear function of C_1 + C_2*has_queue_decode +
//! C_3*has_queued_prefill + C_4*queued_decode_beyond_current_open_buffer +
//! C_5*queued_prefill_beyond_current_open_buffer."* The unit tests in
//! `crates/sim-policy/src/weighted_random.rs` hold the weight, the proportional draw and the
//! per-decision cost; this file holds the two claims that need a whole run: with `c1` alone the
//! policy is `random` to the byte, and with the buffer's counts weighed it is deterministic and
//! reads a fixed number of views per decision.

mod common;

use common::*;
use lbsim::sim;

#[test]
fn with_c1_alone_it_is_random_to_the_byte() {
    for &load in &[0.3_f64, 1.2] {
        let mut random = at_load(load);
        random.routing = "random".into();
        let mut weighted = at_load(load);
        weighted.routing = "weighted_random".into();
        weighted.wr_c1 = 2.5;
        let a = sim::run(&random).unwrap();
        let b = sim::run(&weighted).unwrap();
        assert_eq!(a.fingerprint, b.fingerprint, "load {load}: c1 alone must draw exactly as random does");
        assert_eq!(a.events, b.events);
        assert_eq!(format!("{:?}", a.records), format!("{:?}", b.records));
        assert_eq!(b.routing_label, "weighted_random(c1=2.5,c2=0,c3=0,c4=0,c5=0)");
        assert_eq!(b.replicas_inspected_per_decision, 1);
    }
}

#[test]
fn weighed_by_the_buffer_it_is_deterministic_and_bounded_per_decision() {
    let mut s = at_load(1.2);
    s.routing = "weighted_random".into();
    s.scheduling = "buffered_batch".into();
    s.wr_c1 = 1.0;
    s.wr_c2 = -0.3;
    s.wr_c3 = -0.5;
    s.wr_c4 = -0.05;
    s.wr_c5 = -0.1;
    let a = sim::run(&s).unwrap();
    let b = sim::run(&s).unwrap();
    assert_eq!(a.fingerprint, b.fingerprint);
    assert_eq!(format!("{:?}", a.records), format!("{:?}", b.records));
    // `REFRESH_PER_DECISION` views re-read round the fleet plus the drawn replica's, whatever N.
    assert_eq!(a.replicas_inspected_per_decision, 5);
    assert!(a.completed() > 0);
    // Steering away from queues over the stale view is not the same run as ignoring them.
    let mut plain = s.clone();
    plain.wr_c2 = 0.0;
    plain.wr_c3 = 0.0;
    plain.wr_c4 = 0.0;
    plain.wr_c5 = 0.0;
    assert_ne!(sim::run(&plain).unwrap().fingerprint, a.fingerprint);
}
