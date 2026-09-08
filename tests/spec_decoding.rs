//! Speculative decoding: a small draft model proposes N tokens, the big model verifies them in one
//! step, and each decoding sequence advances an expected (1 - a^(N+1)) / (1 - a) tokens a step instead
//! of one. The value is tokens per second while the step is dominated by the fixed weight read; the
//! cost is that verifying N extra tokens per sequence is prefill-class compute, so at a batch where
//! that compute matches the fixed part the gain is gone. Off, the feature must be invisible: every
//! golden run has it off and must stay byte-identical.

use lbsim::model::Replica;
use lbsim::scenario::Scenario;
use lbsim::sim;
use lbsim::workload::Request;
use lbsim::{EPOCH_BASE, SECOND};

/// `1-routing/01-p2c` in bench/golden-fingerprints.txt, re-pinned when the baseline fleet moved to 256
/// replicas at 560 rps (Issao, 2026-09-07).
const ROUTE_P2C_GOLDEN: u64 = 15913688846466930736;

const DRAFTS: u32 = 4;
const ACCEPT: f64 = 0.7;

fn scenario(path: &str) -> Scenario {
    Scenario::parse(&std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{path}: {e}"))).unwrap()
}

#[test]
fn spec_off_is_byte_identical() {
    let off = sim::run(&scenario("scenarios/spec_off.txt")).unwrap();
    let p2c = sim::run(&scenario("scenarios/route_p2c.txt")).unwrap();
    assert_eq!(off.fingerprint, ROUTE_P2C_GOLDEN, "spec_off moved the route_p2c golden fingerprint");
    assert_eq!(p2c.fingerprint, ROUTE_P2C_GOLDEN, "route_p2c itself moved");
}

fn req(id: u64, prompt: u32, output: u32) -> Request {
    Request {
        id,
        arrived_at: EPOCH_BASE,
        attempt_at: EPOCH_BASE,
        prompt,
        output,
        attempts: 1,
        deadline: EPOCH_BASE + 600 * SECOND,
        is_long: false,
        tenant: 0,
        class: 0,
    }
}

/// One replica with only the fixed step cost and the verify compute, so the crossover below is exactly
/// the one the cost-model numbers predict; the bandwidth term would only push it further out.
fn bench(spec: bool) -> Scenario {
    let mut s = Scenario::default();
    s.max_batch = 1024;
    s.kv_capacity_tokens = 10_000_000.0;
    s.max_queue = 2048;
    s.step_per_seq_ms = 0.0;
    s.step_per_kv_ktoken_ms = 0.0;
    if spec {
        s.spec_draft_tokens = DRAFTS;
        s.spec_accept_rate = ACCEPT;
    }
    s
}

/// Output tokens per second one replica sustains with `batch` sequences decoding `output` tokens each,
/// from admission to the last retirement.
fn tokens_per_s(sc: &Scenario, batch: usize, output: u32) -> f64 {
    let cost = sc.cost_model();
    let mut r = Replica::default();
    for id in 0..batch as u64 {
        r.enqueue(req(id, 8, output), sc.max_queue).ok().unwrap();
    }
    r.wake(EPOCH_BASE);
    let mut now = EPOCH_BASE;
    for _ in 0..100_000 {
        let out = r.step(sc, &cost, now).expect("replica went idle with work left");
        now = out.token_at;
        if out.idle {
            let elapsed = (now - EPOCH_BASE) as f64 / SECOND as f64;
            return (batch as u32 * output) as f64 / elapsed;
        }
    }
    panic!("the batch never drained");
}

#[test]
fn n4_at_small_batch_raises_tokens_per_s() {
    let off = tokens_per_s(&bench(false), 4, 400);
    let on = tokens_per_s(&bench(true), 4, 400);
    assert!(
        on >= 1.5 * off,
        "four sequences: {on:.0} tok/s with speculation vs {off:.0} without, gain {:.2}x",
        on / off
    );
}

/// The verify compute is `decoding * N / prefill_tokens_per_s` and grows with the batch; the fixed
/// step cost does not. Where they are equal the step has doubled, so the gain is the formula's expected
/// tokens a step over two; well past it the gain is gone.
#[test]
fn the_gain_erodes_at_large_batch() {
    let sc = bench(true);
    let crossover = (sc.step_base_ms / 1000.0) / (DRAFTS as f64 / sc.prefill_tokens_per_s);
    assert!(crossover > 50.0 && crossover < 100.0, "crossover batch {crossover:.1}");
    let below = (crossover / 2.0) as usize;
    let above = (crossover * 2.0) as usize;

    let gain_below = tokens_per_s(&sc, below, 400) / tokens_per_s(&bench(false), below, 400);
    let gain_above = tokens_per_s(&sc, above, 400) / tokens_per_s(&bench(false), above, 400);
    assert!(gain_below >= 1.5, "batch {below}: gain {gain_below:.2}x should still be worth it");
    assert!(gain_above < 1.2, "batch {above}: gain {gain_above:.2}x should have eroded");
    assert!(gain_above < gain_below);
}

/// The accumulator is deterministic and its long-run rate is the formula: over 1,000 steps a lone
/// sequence emits floor(1000 * expected) or one more, never a random count.
#[test]
fn expected_tokens_per_step_matches_the_formula() {
    let sc = bench(true);
    let cost = sc.cost_model();
    let expected = (1.0 - ACCEPT.powi(DRAFTS as i32 + 1)) / (1.0 - ACCEPT);
    assert!((cost.spec_tokens_per_step() - expected).abs() < 1e-12);
    assert!((bench(false).cost_model().spec_tokens_per_step() - 1.0).abs() == 0.0);

    let mut r = Replica::default();
    r.enqueue(req(1, 8, 1_000_000), sc.max_queue).ok().unwrap();
    r.wake(EPOCH_BASE);
    let mut now = EPOCH_BASE;
    // The first step is the prefill; tokens start on the second.
    now = r.step(&sc, &cost, now).unwrap().token_at;
    let before = r.kv_tokens();
    for _ in 0..1000 {
        now = r.step(&sc, &cost, now).unwrap().token_at;
    }
    let emitted = (r.kv_tokens() - before) as f64;
    let want = (1000.0 * expected).floor();
    assert!(
        emitted == want || emitted == want + 1.0,
        "1,000 steps emitted {emitted} tokens; the formula says {want}"
    );
}
