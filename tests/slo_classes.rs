//! SLO classes: the same fleet judged against per-class targets, without touching the load.
//!
//! Turning classes on must relabel requests and nothing else. A class is drawn from a stream of its
//! own, so arrivals and shapes stay byte-identical, and the class only decides which targets the
//! finished request is measured against. Batch has no inter-token target at all, which is the whole
//! point of having classes: an even token cadence is a chat requirement, not a batch one.

mod common;

use common::*;
use lbsim::sim;

fn base() -> lbsim::scenario::Scenario {
    let mut s = small();
    s.arrival_rps = 20.0;
    s
}

/// Pinned from a run before classes existed: the classes-off path must not have moved.
#[test]
fn classes_off_is_byte_identical() {
    let r = sim::run(&base()).unwrap();
    assert_eq!(r.fingerprint, 13602603559065206507, "classes-off fingerprint moved");
    assert_eq!(r.records.len(), 522);
    assert!(r.records.iter().all(|rec| rec.class == 0));
    assert_eq!(r.classes(), vec![0]);
}

/// A step slow enough that every inter-token gap breaks the interactive target, on a fleet otherwise
/// idle enough that nothing else fails: interactive attainment collapses, batch stays perfect,
/// because batch is never judged on ITL.
#[test]
fn batch_class_never_fails_on_itl() {
    let mut s = small();
    s.arrival_rps = 2.0;
    s.step_base_ms = 100.0;
    s.prompt_mean = 200.0;
    s.output_mean = 20.0;
    s.output_cv = 0.1;
    s.client_timeout_s = 60.0;
    s.slo_classes = "interactive:0.5,batch:0.5".into();
    let r = sim::run(&s).unwrap();
    assert_eq!(r.classes(), vec![1, 3]);

    let hostile = |class: u8| {
        r.records
            .iter()
            .filter(|rec| rec.class == class && rec.outcome.is_success())
            .all(|rec| rec.max_itl > 80 * lbsim::MILLI)
    };
    assert!(hostile(1) && hostile(3), "the fixture's ITL is not hostile to the 80 ms target");
    assert!(r.class_attainment(1) < 1.0, "interactive attainment {}", r.class_attainment(1));
    assert_eq!(r.class_attainment(3), 1.0, "batch attainment {}", r.class_attainment(3));
    assert!(r.class_goodput_tokens_s(3) > 0.0);
    assert_eq!(r.class_goodput_tokens_s(1), 0.0);
}

#[test]
fn class_shares_are_honoured() {
    let mut s = base();
    s.duration_s = 110.0;
    s.slo_classes = "interactive:0.7,agent:0.2,batch:0.1".into();
    let r = sim::run(&s).unwrap();
    let n = r.records.len();
    assert!(n >= 2000, "only {n} records");
    assert_eq!(r.classes(), vec![1, 2, 3]);
    for (class, share) in [(1u8, 0.7), (2, 0.2), (3, 0.1)] {
        let got = r.records.iter().filter(|rec| rec.class == class).count() as f64 / n as f64;
        assert!((got - share).abs() < 0.05, "class {class}: share {got:.3}, wanted {share}");
    }
}

/// Classes on and off, otherwise the same scenario: every request has the same shape at the same
/// time, and the replica loop ran identically. Only the labels may differ.
#[test]
fn class_draw_does_not_perturb_shapes() {
    let off = sim::run(&base()).unwrap();
    let mut s = base();
    s.slo_classes = "interactive:0.7,agent:0.2,batch:0.1".into();
    let on = sim::run(&s).unwrap();

    assert_eq!(on.fingerprint, off.fingerprint);
    assert_eq!(on.records.len(), off.records.len());
    let shapes = |r: &sim::RunResult| {
        let mut v: Vec<(u64, u64, u32, u32)> =
            r.records.iter().map(|x| (x.id, x.arrived_at, x.prompt_tokens, x.output_tokens)).collect();
        v.sort_unstable();
        v
    };
    assert_eq!(shapes(&on), shapes(&off));
    // And the labels do move: a class's targets are not the scenario's.
    assert!(on.records.iter().any(|x| x.class != 0));
}
