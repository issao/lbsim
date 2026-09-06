//! `least_kv_probe`: power-of-d-choices on live KV occupancy, with the probe paid for.
//!
//! The policy exists to make a specific comparison honest. `p2c` reads the delayed snapshot for free;
//! `least_kv_probe` samples the same way but asks each sampled replica for its state *now*, and the
//! engine charges one modelled round trip per probe. So two things must be true at once: the fresher
//! signal must show up as better balance when the snapshot is stale, and the cost of that freshness
//! must show up in queue wait rather than vanish. A test on only one of them would let a policy claim
//! the benefit without the bill.

mod common;

use common::*;
use lbsim::sim;
use lbsim::Nanos;

/// What the engine charges per probe. Mirrored here rather than imported because the test is about
/// the observable effect on records, not the constant's name.
const PROBE_MS: f64 = 1.0;

fn median_queue_wait_ms(r: &sim::RunResult) -> f64 {
    let mut w: Vec<Nanos> = r.records.iter().map(|x| x.queue_wait()).collect();
    assert!(!w.is_empty(), "no records to take a median over");
    w.sort_unstable();
    w[w.len() / 2] as f64 / 1e6
}

#[test]
fn least_kv_probe_is_registered_and_labelled() {
    let mut s = at_load(0.2);
    s.routing = "least_kv_probe".into();
    let r = sim::run(&s).expect("least_kv_probe must resolve through the routing registry");
    assert_eq!(r.routing_label, "least_kv_probe(d=2)");
    assert_eq!(r.replicas_inspected_per_decision, 2);
}

/// Every decision probes `d` replicas, and every probe lands between arrival and admission, so no
/// request under this policy can be admitted sooner than `d * 1 ms` after it arrived.
///
/// The bill is asserted per request rather than as a median shift. `admitted_at` is stamped when a
/// request joins a batch at a step boundary, so a request delayed 2 ms often joins the same step it
/// would have joined anyway and the *median* moves by less than the full cost (measured 1.65 ms at
/// this load). The floor is the exact claim; the median direction is the honest weaker one.
#[test]
fn probes_are_paid_in_queue_wait() {
    let mut p2c = at_load(0.3);
    p2c.routing = "p2c".into();
    let mut probe = p2c.clone();
    probe.routing = "least_kv_probe".into();

    let a = sim::run(&p2c).unwrap();
    let b = sim::run(&probe).unwrap();
    let bill = (2.0 * PROBE_MS * 1e6) as Nanos;

    let cheapest_p2c = a.records.iter().map(|r| r.queue_wait()).min().unwrap();
    assert!(
        cheapest_p2c < bill,
        "p2c's cheapest admission waited {} ns; the fixture never lets a request in for free, so a \
         2 ms floor would not be evidence of anything",
        cheapest_p2c
    );
    let unpaid: Vec<u64> =
        b.records.iter().filter(|r| r.queue_wait() < bill).map(|r| r.id).collect();
    assert!(
        unpaid.is_empty(),
        "{} of {} requests under least_kv_probe were admitted in under 2 ms; the probe round trips \
         were not charged: ids {:?}",
        unpaid.len(),
        b.records.len(),
        unpaid
    );

    let (med_a, med_b) = (median_queue_wait_ms(&a), median_queue_wait_ms(&b));
    assert!(
        med_b > med_a,
        "median queue wait: p2c {med_a:.3} ms, least_kv_probe {med_b:.3} ms; paying for probes \
         did not raise the typical wait at all"
    );
}

/// With the snapshot refreshed every 4 s a free reader is steering on history. A paid probe is not,
/// so the balance it achieves must be no worse, and paying 2 ms per request must not cost service.
#[test]
fn live_probes_do_not_herd_under_stale_telemetry() {
    let mut p2c = at_load(0.5);
    p2c.telemetry_interval_ms = 4000.0;
    p2c.routing = "p2c".into();
    let mut probe = p2c.clone();
    probe.routing = "least_kv_probe".into();

    let a = sim::run(&p2c).unwrap();
    let b = sim::run(&probe).unwrap();

    let (cv_p2c, cv_probe) = (a.load_imbalance_cv(), b.load_imbalance_cv());
    assert!(cv_p2c.is_finite() && cv_probe.is_finite(), "load imbalance was not measurable");
    assert!(
        cv_probe <= cv_p2c,
        "under 4 s telemetry, least_kv_probe CV {cv_probe:.4} is worse than p2c CV {cv_p2c:.4}; \
         a live probe should not balance worse than a stale snapshot"
    );

    let (att_p2c, att_probe) = (a.slo_attainment(), b.slo_attainment());
    assert!(
        att_probe >= att_p2c - 0.01,
        "under 4 s telemetry, least_kv_probe attainment {att_probe:.4} trails p2c {att_p2c:.4} by \
         more than 0.01; the probe cost is eating the freshness benefit"
    );
}

/// The workload is drawn from its own named stream, so a policy that consumes the routing stream
/// differently (or probes) cannot change what arrives or when. If this ever fails, a comparison
/// between the two policies is measuring the dice rather than the decision.
///
/// The engine records `output_tokens = 0` for a request that failed, so the shape multiset is only
/// a workload comparison when nothing fails; that is arranged with a generous client timeout at low
/// load, and checked. Prompt sizes and arrival instants are workload-only fields, so those are also
/// compared at a load where failures do occur.
#[test]
fn workload_is_identical_across_the_two_policies() {
    let mut p2c = small();
    p2c.client_timeout_s = 25.0;
    p2c.duration_s = 40.0;
    p2c.arrival_rps = 0.15 * p2c.rated_rps();
    p2c.routing = "p2c".into();
    let mut probe = p2c.clone();
    probe.routing = "least_kv_probe".into();

    let a = sim::run(&p2c).unwrap();
    let b = sim::run(&probe).unwrap();
    let (ra, rb) = (settled(&a, &p2c), settled(&b, &probe));
    assert!(ra.len() > 40, "fixture too small: {} settled records", ra.len());
    for (name, recs) in [("p2c", &ra), ("least_kv_probe", &rb)] {
        assert!(
            recs.iter().all(|x| x.outcome.is_success()),
            "{name}: failures in the settled window make output_tokens meaningless here"
        );
    }
    assert_eq!(arrival_times(&ra), arrival_times(&rb), "arrival times differ between policies");
    assert_eq!(shape_multiset(&ra), shape_multiset(&rb), "request shapes differ between policies");

    let mut p2c = at_load(0.5);
    p2c.routing = "p2c".into();
    let mut probe = p2c.clone();
    probe.routing = "least_kv_probe".into();
    let a = sim::run(&p2c).unwrap();
    let b = sim::run(&probe).unwrap();
    let (ra, rb) = (settled(&a, &p2c), settled(&b, &probe));
    assert_eq!(arrival_times(&ra), arrival_times(&rb), "arrival times differ at 0.5 load");
    assert_eq!(prompt_multiset(&ra), prompt_multiset(&rb), "prompt sizes differ at 0.5 load");
}
