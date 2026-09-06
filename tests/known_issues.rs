//! Defects found by this suite, written as the test that *should* pass.
//!
//! Every test in this file is `#[ignore]`d and every one of them fails when run. That is the point:
//! the property each asserts is one the project should hold, and encoding it here means the day
//! someone fixes the underlying issue they get told, instead of having to rediscover what the
//! intended behaviour was. Run them with `cargo test -- --ignored`.
//!
//! None of these are fixed here, by design — this suite does not touch `src/`.

mod common;

use common::*;
use lbsim::metrics::Series;
use lbsim::scenario::Scenario;
use lbsim::sim;

/// **Malformed numbers panic instead of returning an error.** `src/scenario.rs`, in `Scenario::parse`:
///
/// ```text
/// let f = |d: &str| -> f64 { v.parse::<f64>().unwrap_or_else(|_| panic!("{} is not a number in {}", v, d)) };
/// ```
///
/// `parse` returns `Result<Scenario, String>` and takes care to report an unknown key as an `Err`, but
/// a *valid* key with an unparseable value aborts the process. The two failures are the same class of
/// mistake in a hand-written scenario file, and the CLI is built to print `error: {e}` and exit 1 — so
/// one typo gets a clean diagnostic and the other gets a panic and a backtrace. Anything embedding the
/// simulator as a library (a sweep runner loading many scenario files, a web handler) cannot contain
/// the failure at all.
///
/// Smallest reproduction: `Scenario::parse("replicas = eight")`.
#[test]
#[ignore = "known defect: Scenario::parse panics on a malformed numeric value instead of returning Err"]
fn malformed_numeric_values_are_reported_as_errors_not_panics() {
    for bad in [
        "replicas = eight",
        "arrival_rps = 12,5",
        "duration_s = 30s",
        "seed = -1",
        "max_batch =",
    ] {
        let got = Scenario::parse(bad);
        assert!(
            got.is_err(),
            "{bad:?} was accepted; it should be an Err naming the offending key"
        );
    }
}

/// **A fleet at a tenth of its rated capacity cannot meet its own ITL SLO.**
///
/// At 0.1x rated capacity nothing queues, nothing is shed, nothing times out, and no request misses
/// its end-to-end deadline — and yet SLO attainment is only about 0.82 to 0.93 across seeds. Every one
/// of those misses is the inter-token-latency SLO (see
/// `load_response.rs::low_load_slo_misses_are_caused_by_inter_token_latency_not_congestion`), and the
/// cause is arithmetic in the default scenario rather than anything dynamic:
///
/// ```text
/// full chunked-prefill step = step_base_ms + step_token_budget / prefill_tokens_per_s
///                           = 10.2 ms     + 2048 / 28286 s
///                           = 82.6 ms   >   itl_slo_ms = 80 ms
/// ```
///
/// Prefill and decode share the device, so any sequence decoding on a replica that is prefilling a
/// full 2048-token chunk sees a token gap of at least 82.6 ms. With `long_probability = 0.08` and
/// `long_prompt_mean = 24000`, a long request occupies 12+ consecutive full chunks, and every request
/// co-resident with it is guaranteed to violate. No amount of spare capacity or better routing can fix
/// it: the floor on achievable ITL is above the SLO.
///
/// So the three defaults are mutually inconsistent, and at least one has to move — a smaller
/// `step_token_budget` (1024 gives a 46.4 ms step), a higher `itl_slo_ms`, or a prefill/decode split
/// that stops them contending. Which one is a modelling decision, not a test's call. Recording it here
/// because a headline "SLO attainment" that cannot reach 1.0 even on an idle fleet will be read as a
/// property of load balancing, which it is not.
#[test]
#[ignore = "known finding: a full prefill chunk costs 82.6 ms against an 80 ms ITL SLO, so low-load attainment cannot approach 1.0"]
fn low_load_slo_attainment_should_be_near_perfect() {
    let mut s = small();
    s.client_timeout_s = Scenario::default().client_timeout_s;
    s.arrival_rps = 0.1 * s.rated_rps();

    // The inconsistency, stated directly. This half is the actual finding and holds independently of
    // any run.
    let full_chunk_ms = s.step_base_ms + (s.step_token_budget as f64 / s.prefill_tokens_per_s) * 1000.0;
    assert!(
        full_chunk_ms <= s.itl_slo_ms,
        "a full chunked-prefill step costs {full_chunk_ms:.2} ms against an ITL SLO of {} ms, \
         so the ITL SLO is unreachable whenever prefill and decode share a replica",
        s.itl_slo_ms
    );

    for seed in 1..=8_u64 {
        s.seed = seed;
        let r = sim::run(&s).unwrap();
        let a = r.slo_attainment();
        assert!(
            a >= 0.98,
            "seed {seed}: SLO attainment {a:.4} at a tenth of rated capacity, with no shedding, \
             no timeouts and no end-to-end misses"
        );
    }
}

/// **`Series::max()` on an empty series returns `f64::MIN`.** `src/metrics.rs`:
///
/// ```text
/// pub fn max(&self) -> f64 { self.v.iter().cloned().fold(f64::MIN, f64::max) }
/// ```
///
/// Its sibling `mean()` guards the empty case and returns `NaN`, which a plot or a table renders as
/// missing data. `max()` returns -1.8e308, which renders as a number. An empty series is reachable
/// whenever a run is shorter than one sample interval, so a report can print a peak queue depth of
/// -1.8e308 for a run that simply had nothing to sample.
#[test]
#[ignore = "known defect: Series::max() returns f64::MIN for an empty series where mean() returns NaN"]
fn empty_series_max_is_not_a_plausible_number() {
    let s = Series::new("empty");
    assert!(s.mean().is_nan(), "mean of an empty series is already NaN, as it should be");
    assert!(
        s.max().is_nan(),
        "max of an empty series is {}, which will be plotted as a real value",
        s.max()
    );
}
