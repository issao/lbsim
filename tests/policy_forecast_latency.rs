//! `forecast_latency`: routes on predicted time to first token rather than on queued tokens alone.
//!
//! The comparison against `p2c` only means something when the two policies can disagree, which needs
//! prompt sizes varied enough that a replica's queued *tokens* and its predicted *TTFT* rank
//! candidates differently. `long_probability = 0.16` is exactly that: a long prompt both adds a lot of
//! queued work and takes a long time itself to prefill, so which replica minimizes this request's own
//! TTFT is not always the one with the fewest queued tokens once `last_step_ns` and the request's own
//! prefill are folded in.

mod common;

use common::*;
use lbsim::policy::{make_routing, ReplicaView, RequestView, RouteContext};
use lbsim::rng::Rng;
use lbsim::scenario::Scenario;
use lbsim::sim;
use lbsim::Nanos;

fn ttft_p99_ms(r: &sim::RunResult, s: &Scenario) -> f64 {
    let mut w: Vec<Nanos> = settled(r, s).iter().filter_map(|x| x.ttft()).collect();
    assert!(!w.is_empty(), "no settled records with a TTFT to take a percentile over");
    w.sort_unstable();
    let idx = (((w.len() - 1) as f64) * 0.99).round() as usize;
    w[idx] as f64 / 1e6
}

/// `forecast_latency` is documented to keep `p2c`'s candidate draw exactly: same number of draws, same
/// order, off the same shared routing stream, so the two policies see byte-identical candidate sets at
/// a seed. Proved here by handing each policy its own clone of an identically seeded `Rng`, letting
/// each make one routing decision, then drawing one more value from each `Rng`: if the two policies
/// consumed the stream identically, the two `Rng`s are left in identical internal state and the next
/// draw from each must agree. A single extra or missing draw would make the two diverge.
#[test]
fn forecast_latency_draws_the_same_candidates_as_p2c() {
    let mut sc = at_load(0.3);
    sc.routing = "p2c".into();
    let mut p2c = make_routing(&sc).expect("p2c must be registered");
    sc.routing = "forecast_latency".into();
    let mut forecast = make_routing(&sc).expect("forecast_latency must be registered");

    // A fleet with varied queued tokens and step times, and a long request, so the two policies'
    // ranking keys would actually disagree if they were choosing from different candidates or if a
    // draw went missing or extra.
    let views: Vec<ReplicaView> = (0..16u64)
        .map(|i| ReplicaView {
            sampled_at: 0,
            queued: i as u32,
            running: 0,
            queued_tokens: i * 4_096,
            kv_tokens: 0,
            last_step_ns: 5_000_000 + i * 250_000,
            ejected: false,
        })
        .collect();
    let request =
        RequestView { id: 1, prompt_tokens: 20_000, arrived_at: 0, deadline: 60_000_000_000, tenant: 0, attempts: 1 };

    // Neither policy is documented to probe: their candidates come from the shared snapshot. A probe
    // here would itself be a divergence from p2c's draw pattern.
    let no_probe = |_: usize| -> ReplicaView {
        panic!("forecast_latency and p2c both read the snapshot; neither should probe for a candidate")
    };

    const SEED: u64 = 20260906;
    let mut rng_p2c = Rng::from_seed(SEED);
    let mut rng_forecast = rng_p2c.clone();

    {
        let mut ctx = RouteContext::new(0, &views, &request, &mut rng_p2c, &no_probe);
        p2c.choose(&mut ctx);
    }
    {
        let mut ctx = RouteContext::new(0, &views, &request, &mut rng_forecast, &no_probe);
        forecast.choose(&mut ctx);
    }

    assert_eq!(
        rng_p2c.next_u64(),
        rng_forecast.next_u64(),
        "p2c and forecast_latency left the shared rng stream in different states, so they did not draw \
         the same candidates in the same order"
    );
}

/// Long prompts are where a predicted-TTFT ranking should beat a queued-tokens ranking: a replica can
/// have fewer queued tokens than another yet still be the worse place to send a long prompt, once its
/// own prefill and the step it must wait to join are counted. The win can show up as a lower TTFT p99
/// or as higher SLO attainment (fewer requests pushed past their deadline while queued behind the
/// wrong replica's prefill) — either is the predicted direction, so either satisfies the test, and
/// both numbers are printed regardless of which one carries it.
#[test]
fn forecast_latency_beats_p2c_with_long_prompts() {
    // 0.8 of rated capacity: light enough that most decisions still have a real choice among usable
    // replicas, heavy enough that a long prompt's queued prefill work actually varies across the
    // fleet, which is what lets a TTFT forecast disagree with a queued-tokens count. At lower load in
    // this tiny 8-replica fixture there is rarely enough queued work for the two rankings to diverge.
    let mut base = at_load(0.8);
    base.long_probability = 0.16;

    let mut p2c = base.clone();
    p2c.routing = "p2c".into();
    let mut forecast = base.clone();
    forecast.routing = "forecast_latency".into();

    let a = sim::run(&p2c).unwrap();
    let b = sim::run(&forecast).unwrap();
    let (p99_p2c, p99_forecast) = (ttft_p99_ms(&a, &p2c), ttft_p99_ms(&b, &forecast));
    let (attain_p2c, attain_forecast) = (a.slo_attainment(), b.slo_attainment());

    println!(
        "long_probability=0.16, load=0.8: p2c ttft p99 {p99_p2c:.3} ms / attainment {attain_p2c:.4}; \
         forecast_latency ttft p99 {p99_forecast:.3} ms / attainment {attain_forecast:.4}"
    );

    let lower_ttft = p99_forecast < p99_p2c;
    let higher_attainment = attain_forecast > attain_p2c;
    assert!(
        lower_ttft || higher_attainment,
        "forecast_latency neither lowered TTFT p99 ({p99_forecast:.3} ms vs p2c's {p99_p2c:.3} ms) nor \
         raised SLO attainment ({attain_forecast:.4} vs p2c's {attain_p2c:.4}) under long prompts"
    );
}
