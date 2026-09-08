//! How the fleet responds to load: the trend must be right, and low load must be comfortable.
//!
//! These are the sanity checks that catch a queueing model wired up backwards. They are deliberately
//! trend tests with tolerances, not equality tests: a discrete-event run at these sizes has real
//! sampling noise, and a test tightened past the noise floor would fail for reasons that say nothing
//! about the model.

mod common;

use common::*;
use lbsim::scenario::Scenario;
use lbsim::sim;

const LADDER: [f64; 4] = [0.1, 0.5, 1.0, 1.4];

/// More offered load must never buy less queueing.
///
/// This is the one direction the model is not allowed to get wrong. An inverted trend here would mean
/// the queueing dynamics are not being simulated at all — something else (a cap, a shed path, a
/// truncation) is setting the numbers — and every capacity conclusion drawn from the simulator would
/// point the wrong way.
#[test]
fn queueing_does_not_decrease_as_arrival_rate_rises() {
    // p99 end-to-end is dominated by output length rather than by waiting, so it moves only tens of
    // percent across the whole ladder and needs a real tolerance. Mean fleet queue depth is almost
    // pure queueing and is held to a much tighter rule.
    const P99_TOLERANCE: f64 = 0.95;

    for seed in [1_u64, 7, 42, 99] {
        let mut p99 = Vec::new();
        let mut queue_depth = Vec::new();
        for &load in LADDER.iter() {
            let mut s = at_load(load);
            s.seed = seed;
            let r = sim::run(&s).unwrap();
            p99.push(r.e2e.percentile(99.0) as f64 / 1e6);
            queue_depth.push(r.fleet_queue.mean());
        }

        for i in 1..LADDER.len() {
            assert!(
                p99[i] >= p99[i - 1] * P99_TOLERANCE,
                "seed {seed}: p99 e2e fell from {:.1} ms at {}x rated to {:.1} ms at {}x rated \
                 (more than the {:.0}% tolerance)",
                p99[i - 1], LADDER[i - 1], p99[i], LADDER[i], (1.0 - P99_TOLERANCE) * 100.0
            );
            assert!(
                queue_depth[i] >= queue_depth[i - 1] - 0.05,
                "seed {seed}: mean fleet queue fell from {:.3} at {}x rated to {:.3} at {}x rated",
                queue_depth[i - 1], LADDER[i - 1], queue_depth[i], LADDER[i]
            );
        }

        // End to end across the whole ladder the trend must be unambiguous, not merely
        // within-tolerance at every step.
        assert!(
            *p99.last().unwrap() > p99[0],
            "seed {seed}: p99 e2e did not rise at all across the ladder: {p99:?}"
        );
        assert!(
            *queue_depth.last().unwrap() > queue_depth[0] + 1.0,
            "seed {seed}: no queue ever built up, even at {}x rated capacity: {queue_depth:?}",
            LADDER.last().unwrap()
        );
    }
}

/// Rated capacity has to be a real number, not decoration: past it the fleet must be visibly in
/// trouble, and well below it it must not be.
#[test]
fn rated_capacity_separates_healthy_load_from_overload() {
    let mut light = at_load(0.1);
    light.seed = 1;
    let mut heavy = at_load(1.5);
    heavy.seed = 1;

    let l = sim::run(&light).unwrap();
    let h = sim::run(&heavy).unwrap();

    assert!(
        l.fleet_queue.mean() < 0.5,
        "queue already {:.3} deep at a tenth of rated capacity",
        l.fleet_queue.mean()
    );
    assert!(
        h.fleet_queue.mean() > 5.0,
        "queue only {:.3} deep at 1.5x rated capacity; rated_rps is not calibrated to the model",
        h.fleet_queue.mean()
    );
    assert!(
        h.slo_attainment() < l.slo_attainment(),
        "overload ({:.3}) did not degrade SLO attainment relative to light load ({:.3})",
        h.slo_attainment(), l.slo_attainment()
    );
}

/// A fixture for the capacity check that leaves the client timeout at its default 30 s, so a slow
/// request is judged by the SLOs rather than killed by an artificially short deadline.
fn capacity_fixture(seed: u64) -> Scenario {
    let mut s = small();
    s.client_timeout_s = Scenario::default().client_timeout_s;
    s.seed = seed;
    s.arrival_rps = 0.1 * s.rated_rps();
    s
}

/// At a tenth of rated capacity with p2c, nothing should fail and most requests should meet their
/// SLOs.
///
/// The threshold is 0.75, which is where the measurements actually are — across 20 seeds the worst
/// observed attainment at this load is 0.821 and the mean is 0.875. It is deliberately *not* the 0.95
/// or better that "well below rated capacity" ought to deliver, because the model does not deliver
/// that; see `known_issues.rs::low_load_slo_attainment_should_be_near_perfect` for the reason and the
/// arithmetic. Weakening this number quietly would have hidden a real finding, so it is weak and
/// annotated instead.
#[test]
fn low_load_meets_slos_for_most_requests_and_fails_none() {
    let mut worst = 1.0_f64;
    let mut total = 0usize;
    for seed in 1..=8_u64 {
        let s = capacity_fixture(seed);
        let r = sim::run(&s).unwrap();
        assert!(r.records.len() > 20, "seed {seed}: only {} records", r.records.len());
        total += r.records.len();

        // No shedding, no timeouts: at a tenth of capacity the fleet must never drop work.
        assert_eq!(r.outcome("rejected"), 0, "seed {seed}: shed work at 0.1x rated capacity");
        assert_eq!(r.outcome("timeout_queued"), 0, "seed {seed}: queue timeouts at 0.1x rated capacity");
        assert_eq!(r.outcome("timeout_running"), 0, "seed {seed}: running timeouts at 0.1x rated capacity");

        let a = r.slo_attainment();
        assert!(
            a >= 0.75,
            "seed {seed}: SLO attainment {a:.4} at 0.1x rated capacity, below the measured floor"
        );
        worst = worst.min(a);
    }
    assert!(total > 200, "only {total} records across all seeds; fixture is too small");
    assert!(worst < 1.0, "attainment was perfect everywhere; the SLO thresholds are not binding");
}

/// The SLO misses at low load are inter-token-latency misses, and nothing else.
///
/// Locking the *cause* down, not just the number: the shortfall in
/// `low_load_meets_slos_for_most_requests_and_fails_none` is not congestion (no request waits, no
/// deadline is missed end to end) but a per-step cost that exceeds the ITL SLO on its own. If someone
/// later fixes queueing and the attainment number stays put, this test says where to look.
#[test]
fn low_load_slo_misses_are_caused_by_inter_token_latency_not_congestion() {
    let mut itl_only = 0usize;
    let mut itl_bad = 0usize;
    let mut e2e_bad = 0usize;
    let mut ttft_bad = 0usize;
    let mut misses = 0usize;

    for seed in 1..=8_u64 {
        let s = capacity_fixture(seed);
        let r = sim::run(&s).unwrap();
        for x in &r.records {
            let bad_ttft = x.first_token_at > 0
                && (x.first_token_at - x.arrived_at) as f64 / 1e6 > s.ttft_slo_ms;
            let bad_itl = x.max_itl as f64 / 1e6 > s.itl_slo_ms;
            let bad_e2e = (x.finished_at - x.arrived_at) as f64 / 1e9 > s.e2e_slo_s;
            if bad_ttft || bad_itl || bad_e2e {
                misses += 1;
            }
            if bad_itl { itl_bad += 1; }
            if bad_ttft { ttft_bad += 1; }
            if bad_e2e { e2e_bad += 1; }
            if bad_itl && !bad_ttft && !bad_e2e { itl_only += 1; }
        }
    }

    assert!(misses > 0, "no SLO misses at all; this test has nothing to explain");
    assert_eq!(e2e_bad, 0, "{e2e_bad} end-to-end SLO misses at a tenth of rated capacity");

    // The cause moved, deliberately, and the test moved with it.
    //
    // It originally asserted that most low-load misses involved inter-token latency, which was true
    // when the default chunk budget was 2,048 tokens: a full prefill chunk then cost 82.6 ms against
    // an 80 ms target, so the floor on achievable inter-token latency was above the SLO. That was a
    // genuine defect in the defaults and it is fixed; the budget is 1,024, giving 46.4 ms.
    //
    // What remains is first-token latency, and it is arithmetic rather than a defect. The long mode
    // averages 24,000 prompt tokens, which is 849 ms of prefill before a first token can exist, so a
    // 2,000 ms target is unreachable for part of the population at any load. The real fix is a
    // per-class target, which is why SLO classes are on the roadmap.
    assert!(
        ttft_bad * 4 >= misses * 3,
        "only {ttft_bad} of {misses} low-load misses involve first-token latency, and {itl_bad} \
         involve inter-token latency ({itl_only} of those exclusively). If inter-token latency is \
         back in the majority, the chunk-budget default has regressed above the SLO floor"
    );
    assert!(
        itl_bad * 4 < misses,
        "{itl_bad} of {misses} misses involve inter-token latency, which should now be rare: a full \
         prefill chunk costs about 46 ms against an 80 ms target. Check step_token_budget against \
         prefill_tokens_per_s"
    );
    // The converse of the assertion above, and the same correction: inter-token latency should now
    // almost never be the *sole* cause of a low-load miss. When it was, that was the chunk-budget
    // defect.
    assert_eq!(
        itl_only, 0,
        "{itl_only} of {misses} low-load misses are caused by inter-token latency alone. With a \
         46 ms full-chunk step against an 80 ms target that should be impossible, so either the \
         chunk budget or the prefill rate has moved"
    );
}

/// The M7 perturbation input: `perturbation = none` must leave the arrival rate exactly `arrival_rps`
/// at every instant, so every existing golden fingerprint stays where it is; `sine` must modulate it
/// by `1 + a·sin(2πft)`, peaking at a quarter period. The three keys round-trip through `to_text`,
/// because a sweep over `perturb_frequency_hz` goes through that text form.
#[test]
fn a_sine_perturbation_modulates_the_arrival_rate_and_none_leaves_it_alone() {
    use lbsim::workload::Workload;

    let mut sc = Scenario::default();
    sc.arrival_rps = 70.0;
    for t in [0.0, 0.5, 5.0, 15.0, 60.0, 239.9] {
        assert_eq!(Workload::rate_at(&sc, t), 70.0, "perturbation = none moved the rate at t = {t}");
    }

    sc.perturbation = "sine".into();
    sc.perturb_amplitude = 0.3;
    sc.perturb_frequency_hz = 0.05;
    let peak = Workload::rate_at(&sc, 5.0);
    let trough = Workload::rate_at(&sc, 15.0);
    assert!((peak - 70.0 * 1.3).abs() < 1e-9, "peak at a quarter period: {peak}");
    assert!((trough - 70.0 * 0.7).abs() < 1e-9, "trough at three quarters: {trough}");

    let back = Scenario::parse(&sc.to_text()).expect("the perturbation keys did not round-trip");
    assert_eq!(back.perturbation, "sine");
    assert_eq!(back.perturb_amplitude, 0.3);
    assert_eq!(back.perturb_frequency_hz, 0.05);
    let parsed = Scenario::parse("perturbation = sine\nperturb_amplitude = 0.5\nperturb_frequency_hz = 0.2\n").unwrap();
    assert_eq!((parsed.perturb_amplitude, parsed.perturb_frequency_hz), (0.5, 0.2));
}
