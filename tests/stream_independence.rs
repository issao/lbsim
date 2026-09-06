//! Named RNG streams: changing a knob must not change the things that knob does not control.
//!
//! This is the property that makes an A/B comparison an experiment rather than an anecdote. If the
//! router consumed randomness from the same stream as the arrival process, then switching from
//! round-robin to p2c would shift every subsequent arrival and every request size, and the
//! "improvement" measured would be partly the improvement and partly a different workload. The
//! failure is silent, plausible-looking, and invalidates every policy comparison in the project — so
//! it gets tested directly rather than trusted.

mod common;

use common::*;
use lbsim::rng::Streams;
use lbsim::scenario::Scenario;
use lbsim::sim;
use lbsim::workload::Workload;

const ROUTINGS: [&str; 5] = ["round_robin", "random", "p2c", "least_requests", "least_queue_tokens"];

/// A fixture whose settled window contains no failed requests.
///
/// This matters for one specific reason: `RequestRecord::output_tokens` is *zeroed* when a request
/// does not succeed, so on a run with timeouts the recorded shapes are a function of the outcome and
/// not only of the workload. Comparing full (prompt, output) pairs across routings is therefore only
/// well-posed where nothing fails, which a generous client timeout at low load guarantees.
fn no_failure_fixture(routing: &str) -> Scenario {
    let mut s = small();
    s.client_timeout_s = 25.0;
    s.duration_s = 40.0;
    s.routing = routing.into();
    s.arrival_rps = 0.15 * s.rated_rps();
    s
}

/// Two runs differing only in `routing` must see the identical workload: same number of requests,
/// same arrival instants, same request shapes.
#[test]
fn changing_routing_does_not_change_the_workload() {
    let reference = no_failure_fixture(ROUTINGS[0]);
    let r0 = sim::run(&reference).unwrap();
    let recs0 = settled(&r0, &reference);
    assert!(recs0.len() > 40, "fixture too small: {} settled records", recs0.len());
    assert!(
        recs0.iter().all(|r| r.outcome.is_success()),
        "fixture is supposed to have no failures; output_tokens comparison would be meaningless"
    );

    for routing in &ROUTINGS[1..] {
        let s = no_failure_fixture(routing);
        let r = sim::run(&s).unwrap();
        let recs = settled(&r, &s);

        assert!(
            recs.iter().all(|x| x.outcome.is_success()),
            "{routing}: unexpected failures in the settled window"
        );
        assert_eq!(
            recs0.len(),
            recs.len(),
            "{routing}: request count changed with the router (round_robin={}, {routing}={})",
            recs0.len(),
            recs.len()
        );
        assert_eq!(
            arrival_times(&recs0),
            arrival_times(&recs),
            "{routing}: arrival times changed with the router"
        );
        assert_eq!(
            shape_multiset(&recs0),
            shape_multiset(&recs),
            "{routing}: request-shape multiset changed with the router"
        );
    }
}

/// The same invariant where it is hardest to hold: at and beyond rated capacity, where queues are
/// deep, outcomes diverge wildly between policies, and any coupling between routing and the workload
/// streams would have every opportunity to show.
///
/// Only workload-pure quantities are compared here. `output_tokens` is not one of them — it is zeroed
/// on failure — so the assertion is over arrival instants, prompt sizes and counts, which the
/// workload alone determines.
#[test]
fn workload_is_invariant_to_routing_even_under_overload() {
    for &load in &[0.9_f64, 1.3] {
        let mut reference = at_load(load);
        reference.routing = ROUTINGS[0].into();
        let r0 = sim::run(&reference).unwrap();
        let recs0 = settled(&r0, &reference);
        assert!(recs0.len() > 100, "fixture too small at load {load}: {}", recs0.len());

        for routing in &ROUTINGS[1..] {
            let mut s = at_load(load);
            s.routing = (*routing).into();
            let r = sim::run(&s).unwrap();
            let recs = settled(&r, &s);
            let ctx = format!("load={load} routing={routing}");

            assert_eq!(recs0.len(), recs.len(), "request count changed ({ctx})");
            assert_eq!(arrival_times(&recs0), arrival_times(&recs), "arrival times changed ({ctx})");
            assert_eq!(prompt_multiset(&recs0), prompt_multiset(&recs), "prompt sizes changed ({ctx})");
        }
    }
}

/// Same property one level down, with the simulator removed from the picture entirely: the workload
/// generator itself must not read anything from the routing configuration.
///
/// The run-level tests above can only compare requests that produced a record. This one compares the
/// generated stream directly, so the (prompt, output) equality it asserts is exact and unconditional.
#[test]
fn workload_generator_ignores_routing_configuration() {
    let mut reference: Vec<(u64, u32, u32)> = Vec::new();

    for (n, routing) in ROUTINGS.iter().enumerate() {
        let mut s = small();
        s.routing = (*routing).into();
        s.p2c_choices = 2 + n; // another knob that must not touch the workload
        s.probe_live = n % 2 == 0;

        let streams = Streams::new(s.seed);
        let mut w = Workload::new(&streams);
        let mut now = lbsim::EPOCH_BASE;
        let mut seen = Vec::new();
        for _ in 0..2000 {
            let req = w.make(&s, now);
            seen.push((now, req.prompt, req.output));
            now += w.next_gap_ns(&s, (now - lbsim::EPOCH_BASE) as f64 / 1e9).max(1);
        }

        if reference.is_empty() {
            reference = seen;
            assert!(
                reference.iter().any(|&(_, p, _)| p > 10_000),
                "the mixture never produced a long request; the fixture is not exercising the tail"
            );
        } else {
            assert_eq!(reference, seen, "workload stream differs for routing={routing}");
        }
    }
}

/// Independence in the other direction: the arrival stream and the shape stream must be separate, so
/// changing the arrival rate cannot resize requests. Otherwise a load sweep is also a workload sweep.
#[test]
fn changing_arrival_rate_does_not_change_request_shapes() {
    let mut shapes: Option<Vec<(u32, u32)>> = None;
    for &rps in &[1.0_f64, 5.0, 25.0] {
        let mut s = small();
        s.arrival_rps = rps;
        let streams = Streams::new(s.seed);
        let mut w = Workload::new(&streams);
        let got: Vec<(u32, u32)> = (0..1000)
            .map(|_| {
                let r = w.make(&s, lbsim::EPOCH_BASE);
                let _ = w.next_gap_ns(&s, 0.0);
                (r.prompt, r.output)
            })
            .collect();
        match &shapes {
            None => shapes = Some(got),
            Some(want) => assert_eq!(*want, got, "request shapes moved with arrival_rps={rps}"),
        }
    }
}
