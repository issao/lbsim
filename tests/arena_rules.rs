//! The arena's objective under rule set v2, per Issao (`docs/arena.md` section 5b, 16:12): *"You can
//! remove this, I agreed with this."* The score is the minimum over in-scope loads of goodput as a
//! share of offered output tokens, gated by the SLA cap, and every score says which rule set earned it.

mod common;

use lbsim::arena::{score_run, ScoreConfig, RunScore, RULE_SET};
use lbsim::sim;

/// A short run of `common::small()` at `fraction` of rated capacity, scored with `cap`.
fn scored(fraction: f64, cap: f64) -> RunScore {
    let mut sc = common::at_load(fraction);
    sc.name = format!("small at {fraction}x rated");
    sc.routing = "p2c".into();
    let r = sim::run(&sc).unwrap();
    score_run(&r, &ScoreConfig::with_cap(cap))
}

/// Two loads that differ in offered volume: the heavier one, past rated capacity, serves more absolute
/// goodput, the lighter one serves a larger share of what it was offered. Under v1 the heavier run
/// would rank first; under v2 the share decides, so the lighter one does. A cap of zero keeps the gate
/// out of the comparison. The fractions sit where both gaps are wide on the small fixture (0.48 vs 0.27
/// in share, 2.5k vs 1.1k tokens/s absolute), not at the extremes, where a 30 s window is noisy.
#[test]
fn score_is_share_not_absolute() {
    let light = scored(0.3, 0.0);
    let heavy = scored(1.2, 0.0);
    assert!(
        heavy.goodput_tokens_s > light.goodput_tokens_s,
        "the heavier load must serve more absolute goodput: {} vs {}",
        heavy.goodput_tokens_s,
        light.goodput_tokens_s
    );
    assert!(
        light.goodput_share > heavy.goodput_share,
        "the lighter load must serve a larger share: {} vs {}",
        light.goodput_share,
        heavy.goodput_share
    );
    assert_eq!(light.score, light.goodput_share);
    assert_eq!(heavy.score, heavy.goodput_share);
    assert!(light.score > heavy.score, "the share order wins under v2");
    assert!(light.score <= 1.0 && heavy.score <= 1.0, "a share is at most one");
}

#[test]
fn every_score_carries_the_rule_set() {
    for fraction in [0.05, 0.5] {
        let s = scored(fraction, 0.95);
        assert_eq!(s.rule_set, RULE_SET);
    }
    assert!(RULE_SET.starts_with("v2:"), "{RULE_SET}");
    assert!(RULE_SET.contains("share"), "{RULE_SET}");
}

/// The gate is unchanged by the change of unit: a breach is a hard zero, not a discount.
#[test]
fn gate_still_zeroes_a_breach() {
    let passing = scored(0.05, 0.0);
    assert!(!passing.gated());
    assert!(passing.score > 0.0);

    // No policy attains 100% on every request, so a cap above one is a certain breach.
    let breached = scored(0.05, 1.01);
    assert!(breached.gated());
    assert_eq!(breached.score, 0.0);
    assert_eq!(
        breached.goodput_share, passing.goodput_share,
        "the gate zeroes the score, not the diagnostic"
    );
    assert_eq!(breached.rule_set, RULE_SET);
}
