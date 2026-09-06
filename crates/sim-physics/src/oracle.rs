//! The naive per-step replica run Issao asked for, against the epoch run, on identical sequences.
//!
//! `bench/validate_epochs.py` properties P3 and P4: a replica full of sequences with different
//! remaining lengths finishes each of them at the same instant whether time is advanced one step at
//! a time with no algebra, or one epoch at a time with the closed form. The Python compared floats
//! to 1e-9; here both runs produce rationals and the completion times must be equal. This is the
//! differential test `docs/execution-plan.md` M2 names as its exit condition, and it is deliberately
//! dumb: `run_stepwise` recomputes the lines from the batch at every step and adds one step time,
//! exactly as a per-token engine would.
//!
//! Token counts are integers here, so the oracle takes speculation profiles with a whole `M`. The
//! fractional `M = 9/2` profile is covered by the per-epoch differential test in `epoch.rs`, whose
//! closed form is what carries the fraction.

use crate::epoch::{EpochModel, EpochState, Spec};
use crate::rational::Rational;
use sim_core::rng::Rng;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Seq {
    pub id: usize,
    /// Tokens already resident in KV: the prompt plus output so far.
    pub held: u64,
    /// Output tokens still to decode.
    pub remaining: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunResult {
    /// `(id, completion time in seconds)`, in id order.
    pub completions: Vec<(usize, Rational)>,
    /// Steps taken, or epochs opened: the cost of the run.
    pub iterations: u64,
}

fn whole_accepted(spec: Spec) -> u64 {
    let acc = spec.accepted.reduced();
    assert_eq!(acc.den(), 1, "the replica oracle needs a whole number of accepted tokens");
    acc.num() as u64
}

fn state_of(live: &[Seq], spec: Spec) -> EpochState {
    let resident: u64 = live.iter().map(|s| s.held).sum();
    EpochState::uniform(live.len() as u64, resident, spec)
}

/// Grow every live sequence by up to `tokens`, record the ones that finish at `now`, drop them.
fn advance(live: &mut Vec<Seq>, tokens: u64, now: Rational, done: &mut Vec<(usize, Rational)>) {
    for s in live.iter_mut() {
        let grown = s.remaining.min(tokens);
        s.held += grown;
        s.remaining -= grown;
    }
    live.retain(|s| {
        let finished = s.remaining == 0;
        if finished {
            done.push((s.id, now));
        }
        !finished
    });
}

/// One iteration per step, no algebra.
pub fn run_stepwise(model: &EpochModel, seqs: &[Seq], spec: Spec) -> RunResult {
    let m = whole_accepted(spec);
    let mut live = seqs.to_vec();
    let mut done = Vec::with_capacity(seqs.len());
    let mut t = Rational::ZERO;
    let mut iterations = 0;
    while !live.is_empty() {
        let epoch = model.epoch(&state_of(&live, spec));
        t = t.add(epoch.step_time(0));
        iterations += 1;
        advance(&mut live, m, t, &mut done);
    }
    done.sort_by_key(|(id, _)| *id);
    RunResult { completions: done, iterations }
}

/// One iteration per composition change: run to the first completion in closed form, repeat.
pub fn run_epochs(model: &EpochModel, seqs: &[Seq], spec: Spec) -> RunResult {
    let m = whole_accepted(spec);
    let mut live = seqs.to_vec();
    let mut done = Vec::with_capacity(seqs.len());
    let mut t = Rational::ZERO;
    let mut iterations = 0;
    while !live.is_empty() {
        let n = live.iter().map(|s| s.remaining.div_ceil(m)).min().expect("live is non-empty");
        let epoch = model.epoch(&state_of(&live, spec));
        t = t.add(epoch.duration(n));
        iterations += 1;
        advance(&mut live, m * n, t, &mut done);
    }
    done.sort_by_key(|(id, _)| *id);
    RunResult { completions: done, iterations }
}

/// The Python's P3/P4 population: a few to a few dozen sequences, chat-sized prompts, outputs from
/// one token to three thousand.
pub fn random_batch(rng: &mut Rng, max_seqs: u64) -> Vec<Seq> {
    let n = 2 + rng.below(max_seqs - 1);
    (0..n as usize)
        .map(|id| Seq { id, held: 200 + rng.below(7_801), remaining: 1 + rng.below(3_000) })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::epoch::{Spec, NO_SPEC, REF, SPEC_PROFILES};

    const WHOLE_SPECS: [Spec; 3] = [NO_SPEC, SPEC_PROFILES[1], SPEC_PROFILES[2]];

    /// Both runs over the same cases; returns (naive steps, epochs) summed.
    fn compare(seed: u64, cases: usize, max_seqs: u64, specs: &[Spec]) -> (u64, u64) {
        let model = REF.compile();
        let mut rng = Rng::from_seed(seed);
        let (mut steps, mut epochs) = (0, 0);
        for case in 0..cases {
            let seqs = random_batch(&mut rng, max_seqs);
            let spec = specs[rng.below(specs.len() as u64) as usize];
            let naive = run_stepwise(&model, &seqs, spec);
            let fast = run_epochs(&model, &seqs, spec);
            assert_eq!(naive.completions.len(), seqs.len());
            assert_eq!(
                naive.completions, fast.completions,
                "case {case}: completion times differ between the naive and epoch runs"
            );
            steps += naive.iterations;
            epochs += fast.iterations;
        }
        (steps, epochs)
    }

    #[test]
    fn replica_run_completion_times_agree_exactly() {
        compare(999, 8, 64, &[NO_SPEC]);
        compare(4_242, 8, 48, &WHOLE_SPECS[1..]);
    }

    #[test]
    fn iteration_count_ratio_is_reported() {
        let (plain_steps, plain_epochs) = compare(999, 8, 64, &[NO_SPEC]);
        let (spec_steps, spec_epochs) = compare(4_242, 8, 48, &WHOLE_SPECS[1..]);
        let (steps, epochs) = (plain_steps + spec_steps, plain_epochs + spec_epochs);
        println!(
            "naive per-step iterations {steps} against closed-form epoch evaluations {epochs}: \
             {:.1}x fewer (no speculation {plain_steps}/{plain_epochs} = {:.1}x, speculation \
             {spec_steps}/{spec_epochs} = {:.1}x); the Python reference reports 82x",
            steps as f64 / epochs as f64,
            plain_steps as f64 / plain_epochs as f64,
            spec_steps as f64 / spec_epochs as f64,
        );
        assert!(steps > 10 * epochs, "{steps} steps vs {epochs} epochs");
    }
}
