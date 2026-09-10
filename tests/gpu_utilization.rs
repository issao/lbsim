//! GPU utilization is time in step; GPU useful is work done over the maximum possible.
//!
//! Issao: *"when I run a simulation with 10rps and 50 replicas, utilization is quite high (mean at
//! 53%) with pretty much any policy. That seems way off."* It is what a GPU counter reports: a
//! replica decoding one sequence reads all its weights every step, so it is busy nearly all the
//! time while doing 1/max_batch of the work it could, and Issao asked for the second number rather
//! than a redefinition: *"Keep the old gpu utilization, but add a new metric with useful GPU work /
//! max possible, so we can get a sense of how small the batches are."* These three windows pin the
//! two apart: batch 1 is busy 1.0 and useful 1/effective_batch_limit, a full batch is both 1.0, and
//! a full prefill chunk is both 1.0 because prefill is compute-bound.

mod common;

use lbsim::model::{PrefixTree, Replica};
use lbsim::scenario::Scenario;
use lbsim::workload::Request;
use lbsim::{EPOCH_BASE, SECOND};

fn req(id: u64, prompt: u32, output: u32) -> Request {
    Request {
        id,
        arrived_at: EPOCH_BASE,
        attempt_at: EPOCH_BASE,
        prompt,
        output,
        attempts: 1,
        deadline: EPOCH_BASE + 3600 * SECOND,
        is_long: false,
        tenant: 0,
        class: 0,
        prefix_node: 0,
        prefix_tokens: 0,
    }
}

/// Step `r` back to back from `from` until the step clock passes `until`, and return the busy and
/// useful shares of the window `(from, until]`, read the way `sim-leaf` reads them: differences of
/// the clipped clocks, so a step straddling `until` counts only up to it.
fn window(r: &mut Replica, sc: &Scenario, from: u64, until: u64) -> (f64, f64) {
    let cost = sc.cost_model();
    let mut sched = lbsim::policy::make_scheduling(sc).unwrap();
    let busy0 = r.busy_ns_through(from);
    let useful0 = r.useful_ns_through(from);
    let mut now = from;
    while now < until {
        let out = r.step_scheduled(sc, &cost, now, &PrefixTree::empty(), &mut *sched).expect("work left");
        now = out.token_at;
    }
    let w = (until - from) as f64;
    ((r.busy_ns_through(until) - busy0) as f64 / w, (r.useful_ns_through(until) - useful0) as f64 / w)
}

/// One sequence past its prefill, decoding alone for a second: busy the whole second, useful
/// 1/effective_batch_limit of it.
#[test]
fn batch_one_is_busy_but_one_batch_slot_useful() {
    let sc = Scenario::default();
    let cost = sc.cost_model();
    let mut sched = lbsim::policy::make_scheduling(&sc).unwrap();
    let mut r = Replica::default();
    r.enqueue(req(1, 16, 100_000), sc.max_queue).unwrap();
    // The first step carries the 16-token prefill; the window starts after it.
    let first = r.step_scheduled(&sc, &cost, EPOCH_BASE, &PrefixTree::empty(), &mut *sched).unwrap();
    let (busy, useful) = window(&mut r, &sc, first.token_at, first.token_at + SECOND);
    let want = 1.0 / sc.effective_batch();
    assert!((busy - 1.0).abs() < 1e-6, "busy {busy}");
    assert!((useful - want).abs() < 1e-3, "useful {useful}, want 1/{} = {want}", sc.effective_batch());
    assert!(useful < 0.1, "a lone sequence must not read as a utilized GPU: {useful}");
}

/// The batch limit's worth of sequences decoding together: every step is fully useful.
#[test]
fn a_full_batch_is_fully_useful() {
    let sc = Scenario::default();
    let cost = sc.cost_model();
    let mut sched = lbsim::policy::make_scheduling(&sc).unwrap();
    let n = sc.effective_batch() as usize;
    assert_eq!(n, sc.max_batch, "the default scenario's limit is the sequence cap");
    let mut r = Replica::default();
    for id in 0..n as u64 {
        r.enqueue(req(id, 16, 100_000), sc.max_queue).unwrap();
    }
    // 32 prefills of 16 tokens fit one 1,024-token chunk, so after one step everything decodes.
    let first = r.step_scheduled(&sc, &cost, EPOCH_BASE, &PrefixTree::empty(), &mut *sched).unwrap();
    assert_eq!(r.running(), n);
    let (busy, useful) = window(&mut r, &sc, first.token_at, first.token_at + SECOND);
    assert!((busy - 1.0).abs() < 1e-6, "busy {busy}");
    assert!((useful - 1.0).abs() < 1e-6, "useful {useful}");
}

/// One long prompt chunked at the step budget: prefill is compute-bound, so a window of full
/// chunks is as useful as it is busy.
#[test]
fn a_prefill_window_is_fully_useful() {
    let sc = Scenario::default();
    let mut r = Replica::default();
    // 200,000 tokens at 1,024 a step is ~195 steps of ~46 ms; the window ends well inside them.
    r.enqueue(req(1, 200_000, 4), sc.max_queue).unwrap();
    let (busy, useful) = window(&mut r, &sc, EPOCH_BASE, EPOCH_BASE + SECOND);
    assert!(r.running() == 1 && r.completed() == 0, "the window must end mid-prefill");
    assert!((busy - 1.0).abs() < 1e-6, "busy {busy}");
    assert!((useful - 1.0).abs() < 1e-6, "useful {useful}");
}
