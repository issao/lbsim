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

#[test]
fn forecast_latency_is_registered_and_labelled() {
    let mut s = at_load(0.2);
    s.routing = "forecast_latency".into();
    let r = sim::run(&s).expect("forecast_latency must resolve through the routing registry");
    assert_eq!(r.routing_label, "forecast_latency(d=2)");
    assert_eq!(r.replicas_inspected_per_decision, 2);
}

#[test]
fn forecast_latency_is_deterministic() {
    let mut s = at_load(0.3);
    s.routing = "forecast_latency".into();

    let a = sim::run(&s).unwrap();
    let b = sim::run(&s).unwrap();
    assert_eq!(a.fingerprint, b.fingerprint, "same config and seed must give a byte-identical run");
}

/// Long prompts are where a predicted-TTFT ranking should beat a queued-tokens ranking: a replica can
/// have fewer queued tokens than another yet still be the worse place to send a long prompt, once its
/// own prefill and the step it must wait to join are counted.
#[test]
fn with_long_prompts_forecast_latency_cuts_ttft_p99_against_p2c() {
    let mut base = at_load(0.3);
    base.long_probability = 0.16;

    let mut p2c = base.clone();
    p2c.routing = "p2c".into();
    let mut forecast = base.clone();
    forecast.routing = "forecast_latency".into();

    let a = sim::run(&p2c).unwrap();
    let b = sim::run(&forecast).unwrap();
    let (p99_p2c, p99_forecast) = (ttft_p99_ms(&a, &p2c), ttft_p99_ms(&b, &forecast));

    println!("TTFT p99 (long_probability=0.16, load=0.3): p2c {p99_p2c:.3} ms, forecast_latency {p99_forecast:.3} ms");
    assert!(
        p99_forecast <= p99_p2c,
        "forecast_latency TTFT p99 {p99_forecast:.3} ms exceeds p2c's {p99_p2c:.3} ms; expected \
         predicted-TTFT routing to be no worse under long prompts"
    );
}

/// With prompts nearly uniform in size, queued tokens and predicted TTFT rank replicas almost the
/// same way, so the two policies should land within measurement noise of each other rather than one
/// dominating.
#[test]
fn with_uniform_prompts_the_two_agree_within_noise() {
    let mut base = at_load(0.3);
    base.long_probability = 0.0;
    base.prompt_cv = 0.1;

    let mut p2c = base.clone();
    p2c.routing = "p2c".into();
    let mut forecast = base.clone();
    forecast.routing = "forecast_latency".into();

    let a = sim::run(&p2c).unwrap();
    let b = sim::run(&forecast).unwrap();
    let (p99_p2c, p99_forecast) = (ttft_p99_ms(&a, &p2c), ttft_p99_ms(&b, &forecast));

    println!("TTFT p99 (long_probability=0.0, load=0.3): p2c {p99_p2c:.3} ms, forecast_latency {p99_forecast:.3} ms");
    let tolerance = 0.05 * p99_p2c;
    assert!(
        (p99_forecast - p99_p2c).abs() <= tolerance,
        "TTFT p99 differs by more than 5% with uniform prompts: p2c {p99_p2c:.3} ms, forecast_latency \
         {p99_forecast:.3} ms (tolerance {tolerance:.3} ms)"
    );
}
