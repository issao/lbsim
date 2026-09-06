//! `metrics::Histogram`, tested directly.
//!
//! The histogram is where every latency claim in the project comes from, and a percentile that is
//! quietly wrong is indistinguishable from a fleet that is quietly slow. It is also the one component
//! whose correctness is checkable exactly, against a distribution whose true quantiles are known, so
//! there is no excuse for not doing so.

use lbsim::metrics::Histogram;

/// Two significant digits, per the module docs: 6 sub-bucket bits give 64 sub-buckets per power of
/// two, so a bucket is at most 1/64 = 1.5625% wide. `percentile` reports a bucket's lower bound, so a
/// reported value is never above the truth and never more than one bucket width below it.
const BUCKET_RESOLUTION: f64 = 1.0 / 64.0;

fn uniform(n: u64) -> Histogram {
    let mut h = Histogram::new();
    for v in 1..=n {
        h.record(v);
    }
    h
}

#[test]
fn percentiles_of_a_known_distribution_are_within_one_bucket_width() {
    let n = 10_000;
    let h = uniform(n);
    assert_eq!(h.count(), n);

    for q in [0.0_f64, 1.0, 10.0, 50.0, 75.0, 90.0, 99.0, 99.9] {
        // The q-th percentile of the integers 1..=n, under the same rank convention the
        // implementation uses (the smallest value whose cumulative count reaches ceil(q/100 * n)).
        let truth = ((q / 100.0) * n as f64).ceil().max(1.0);
        let got = h.percentile(q) as f64;
        assert!(
            got <= truth,
            "p{q} reported {got}, above the true {truth}; percentile must report a bucket's lower bound"
        );
        assert!(
            got >= truth * (1.0 - BUCKET_RESOLUTION),
            "p{q} reported {got}, more than one bucket width ({:.2}%) below the true {truth}",
            BUCKET_RESOLUTION * 100.0
        );
    }
}

/// Values below the first power-of-two boundary land in unit-wide buckets, so there they are exact.
/// Worth pinning separately: it is the only regime where the histogram has no error at all, and an
/// off-by-one in the index/value round trip would show up here and nowhere else.
#[test]
fn small_values_are_recorded_exactly() {
    for v in 0..64_u64 {
        let mut h = Histogram::new();
        h.record(v);
        assert_eq!(h.percentile(50.0), v, "value {v} did not round-trip exactly");
        assert_eq!(h.min(), v);
        assert_eq!(h.max(), v);
        assert_eq!(h.mean(), v as f64);
    }
}

/// Percentiles must be non-decreasing in q. Not a nicety: a report that showed p99 below p50 would be
/// read as a data problem in the simulated fleet rather than a bug in the histogram.
#[test]
fn percentiles_are_monotone_in_q() {
    let mut h = Histogram::new();
    // A skewed distribution, so buckets of very different widths are exercised.
    let mut x = 7_u64;
    for _ in 0..5000 {
        x = x.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        h.record(x % 5_000_000_000);
    }
    let mut prev = 0;
    let mut q = 0.0_f64;
    while q <= 100.0 {
        let v = h.percentile(q);
        assert!(v >= prev, "p{q} = {v} is below the previous percentile {prev}");
        prev = v;
        q += 0.25;
    }
    assert!(h.percentile(100.0) <= h.max());
}

/// Merging is the reason this is a histogram and not a list of samples: percentiles do not average, so
/// a distributed run must combine bucket counts. Merging two histograms has to be indistinguishable
/// from having recorded everything into one.
#[test]
fn merge_is_equivalent_to_recording_both_sets() {
    let mut left = Histogram::new();
    let mut right = Histogram::new();
    let mut both = Histogram::new();

    let mut x = 12345_u64;
    for i in 0..4000 {
        x = x.wrapping_mul(6364136223846793005).wrapping_add(1);
        let v = x % 2_000_000;
        if i % 3 == 0 {
            left.record(v);
        } else {
            right.record(v);
        }
        both.record(v);
    }

    let mut merged = left.clone();
    merged.merge(&right);

    assert_eq!(merged.count(), both.count(), "merged count differs");
    assert_eq!(merged.min(), both.min(), "merged min differs");
    assert_eq!(merged.max(), both.max(), "merged max differs");
    assert_eq!(merged.mean().to_bits(), both.mean().to_bits(), "merged mean differs");
    let mut q = 0.0_f64;
    while q <= 100.0 {
        assert_eq!(merged.percentile(q), both.percentile(q), "merged p{q} differs");
        q += 0.5;
    }
}

/// Merging an empty histogram must be a no-op, in particular it must not drag `min` down to the
/// sentinel. A fleet with an idle shard is the normal case, so this path runs constantly.
#[test]
fn merging_an_empty_histogram_changes_nothing() {
    let h = uniform(500);
    let mut merged = h.clone();
    merged.merge(&Histogram::new());
    assert_eq!(merged.count(), h.count());
    assert_eq!(merged.min(), h.min());
    assert_eq!(merged.max(), h.max());
    assert_eq!(merged.percentile(99.0), h.percentile(99.0));

    // And the empty side gaining everything from a full one.
    let mut other = Histogram::new();
    other.merge(&h);
    assert_eq!(other.count(), h.count());
    assert_eq!(other.min(), h.min(), "min was left at its sentinel after merging into an empty histogram");
    assert_eq!(other.max(), h.max());
    assert_eq!(other.percentile(99.0), h.percentile(99.0));
}

/// `fraction_below` is what SLO attainment is computed from, so its error has to be bounded and its
/// direction known: it counts the whole bucket containing the threshold, so it can only over-report.
#[test]
fn fraction_below_over_reports_by_at_most_one_bucket() {
    let n = 20_000_u64;
    let h = uniform(n);
    for v in [1_u64, 64, 100, 1_000, 5_000, 10_000, 19_999] {
        let truth = v as f64 / n as f64;
        let got = h.fraction_below(v);
        assert!(got >= truth - 1e-12, "fraction_below({v}) = {got} under-reports the true {truth}");
        // A bucket at v is at most v/64 wide, so at most that many extra samples can be counted.
        let slack = (v as f64 * BUCKET_RESOLUTION + 1.0) / n as f64;
        assert!(
            got <= truth + slack,
            "fraction_below({v}) = {got} exceeds the true {truth} by more than one bucket ({slack})"
        );
    }
    assert_eq!(h.fraction_below(n), 1.0, "everything should be below the maximum");
    assert_eq!(h.fraction_below(u64::MAX >> 2), 1.0, "a huge threshold should include everything");
}

/// An empty histogram is what a run with no successful requests produces, which is exactly the
/// situation a report most needs to survive. Percentiles must not panic, and the statistics that have
/// no meaning must be NaN rather than a plausible zero that would be plotted as a good result.
#[test]
fn empty_histogram_is_safe_and_reports_nothing_plausible() {
    let h = Histogram::new();
    assert_eq!(h.count(), 0);
    assert_eq!(h.min(), 0, "min of an empty histogram must not leak the u64::MAX sentinel");
    assert_eq!(h.max(), 0);
    assert!(h.mean().is_nan(), "mean of nothing must be NaN, not 0.0");
    assert!(h.fraction_below(1_000).is_nan(), "fraction of nothing must be NaN, not 0.0 or 1.0");
    for q in [0.0_f64, 50.0, 99.0, 100.0] {
        assert_eq!(h.percentile(q), 0, "p{q} of an empty histogram should be 0");
    }
}

/// Values beyond the top bucket must clamp, not index out of bounds. A latency in a collapsed fleet
/// can be arbitrarily large, and a panic there would take out the very run that needed reporting.
#[test]
fn extreme_values_clamp_instead_of_panicking() {
    let mut h = Histogram::new();
    h.record(0);
    h.record(u64::MAX);
    h.record(1 << 62);
    assert_eq!(h.count(), 3);
    assert_eq!(h.max(), u64::MAX);
    assert_eq!(h.min(), 0);
    assert!(h.percentile(100.0) > 0);
    assert!(h.percentile(50.0) <= h.percentile(100.0));
}
