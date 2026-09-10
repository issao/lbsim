//! `buffered_batch`: the GPU scheduler's batch buffer, and the engine's hold timer under it.
//!
//! Issao: *"We need a buffer for collecting multiple prefill/decode requests in a single batch
//! within available hbm bandwidth ... sized at up to the batch size (and max prefill and decode
//! capacity per buffer tunable) as well as have a time parameter that keeps a buffer around for at
//! most n milliseconds."* Four claims are held here: the buffer flushes when it is full and when the
//! hold expires, and not before; a stale hold timer never runs a second step; the bandwidth line is
//! respected by the batch the engine actually builds; and the telemetry's "beyond the open buffer"
//! counts are what the buffer leaves out, and zero under `fifo_chunked`. `fifo_chunked` itself is
//! unchanged, which `check-fingerprints.sh` proves rather than a test here.

mod common;

use common::*;
use lbsim::model::{PrefixTree, Replica};
use lbsim::scenario::Scenario;
use lbsim::sim;
use lbsim::workload::Request;
use lbsim::{Nanos, EPOCH_BASE, MILLI, SECOND};

fn req(id: u64, prompt: u32) -> Request {
    Request {
        id,
        arrived_at: EPOCH_BASE,
        attempt_at: EPOCH_BASE,
        prompt,
        output: 4,
        attempts: 1,
        deadline: EPOCH_BASE + 60 * SECOND,
        is_long: false,
        tenant: 0,
        class: 0,
        prefix_node: 0,
        prefix_tokens: 0,
    }
}

/// One replica under the buffered scheduler: four seats, 1024 prefill tokens, a 5 ms hold.
fn buffered() -> Scenario {
    let mut sc = Scenario::default();
    sc.max_batch = 4;
    sc.max_queue = 1000;
    sc.scheduling = "buffered_batch".into();
    sc.buffer_max_prefill_tokens = 100_000;
    sc
}

#[test]
fn the_name_and_keys_resolve_through_the_scenario() {
    let mut s = small();
    s.scheduling = "buffered_batch".into();
    let r = sim::run(&s).expect("buffered_batch resolves");
    let again = sim::run(&s).unwrap();
    assert_eq!(r.fingerprint, again.fingerprint, "buffered_batch is not deterministic");
    let text = "scheduling = buffered_batch\nbuffer_max_batch = 8\nbuffer_max_prefill_tokens = 512\n\
                buffer_max_decode_seqs = 6\nbuffer_max_hold_ms = 2.5\n";
    let sc = Scenario::parse(text).unwrap();
    assert_eq!((sc.buffer_max_batch, sc.buffer_max_prefill_tokens, sc.buffer_max_decode_seqs), (8, 512, 6));
    assert_eq!(sc.buffer_max_hold_ms, 2.5);
    assert_eq!(Scenario::override_kind("buffer_max_hold_ms"), lbsim::scenario::OverrideKind::Policy);
}

/// One arrival at an idle replica is held: the step event comes back at the hold's deadline having
/// run nothing and cost nothing. At the deadline the step runs with the one entry.
#[test]
fn an_idle_replica_holds_a_lone_arrival_until_the_hold_expires() {
    let sc = buffered();
    let cost = sc.cost_model();
    let mut sched = lbsim::policy::make_scheduling(&sc).unwrap();
    let mut r = Replica::default();
    let t0 = EPOCH_BASE;
    r.enqueue(req(1, 300), sc.max_queue, t0).unwrap();
    assert!(r.wake(t0));
    let held = r.step_scheduled(&sc, &cost, t0, &PrefixTree::empty(), &mut *sched).expect("a held step is an outcome");
    assert_eq!(held.step_ns, 0, "a hold costs no device time");
    assert_eq!(held.token_at, t0 + 5 * MILLI, "the deadline is the oldest entry's enqueue plus the hold");
    assert!(!held.idle && held.finished.is_empty());
    assert_eq!(r.running(), 0, "nothing was admitted while holding");
    assert_eq!(r.busy_ns_through(t0 + 5 * MILLI), 0, "holding is idle time");
    let out = r.step_scheduled(&sc, &cost, held.token_at, &PrefixTree::empty(), &mut *sched).expect("the step runs at the deadline");
    assert!(out.step_ns > 0);
    assert_eq!(r.running(), 1);
}

/// Arrivals that fill the buffer end the hold early: the wake re-asks the scheduler, the full
/// buffer steps at once, and the stale timer for the old deadline is refused when it fires.
#[test]
fn a_full_buffer_flushes_before_the_hold_and_the_stale_timer_is_ignored() {
    let sc = buffered();
    let cost = sc.cost_model();
    let mut sched = lbsim::policy::make_scheduling(&sc).unwrap();
    let mut r = Replica::default();
    let t0 = EPOCH_BASE;
    r.enqueue(req(1, 300), sc.max_queue, t0).unwrap();
    assert!(r.wake(t0));
    let held = r.step_scheduled(&sc, &cost, t0, &PrefixTree::empty(), &mut *sched).unwrap();
    assert_eq!(held.token_at, t0 + 5 * MILLI);
    // Three more a millisecond later: four seats, so the buffer is full.
    let t1 = t0 + MILLI;
    for id in 2..=4 {
        r.enqueue(req(id, 300), sc.max_queue, t1).unwrap();
    }
    assert!(r.wake(t1), "an arrival during a hold schedules a fresh decision");
    let out = r.step_scheduled(&sc, &cost, t1, &PrefixTree::empty(), &mut *sched).expect("the full buffer steps");
    assert!(out.step_ns > 0, "the buffer flushed at capacity, 4 ms before the hold");
    assert_eq!(r.running(), 4);
    // The event for the old deadline still fires, mid-step, and must do nothing.
    let stale: Option<_> = r.step_scheduled(&sc, &cost, t0 + 5 * MILLI, &PrefixTree::empty(), &mut *sched);
    assert!(stale.is_none(), "a superseded hold timer must not run a second step");
    assert_eq!(r.running(), 4);
    assert_eq!(r.next_step_at(), out.token_at, "the real step's follow-up is untouched");
}

/// Two arrivals that do not fill the buffer: the second wake re-holds to the same deadline, since
/// the oldest entry is the same, and both step at it.
#[test]
fn a_second_arrival_that_does_not_fill_the_buffer_keeps_the_first_deadline() {
    let sc = buffered();
    let cost = sc.cost_model();
    let mut sched = lbsim::policy::make_scheduling(&sc).unwrap();
    let mut r = Replica::default();
    let t0 = EPOCH_BASE;
    r.enqueue(req(1, 300), sc.max_queue, t0).unwrap();
    r.wake(t0);
    let first = r.step_scheduled(&sc, &cost, t0, &PrefixTree::empty(), &mut *sched).unwrap();
    r.enqueue(req(2, 300), sc.max_queue, t0 + 2 * MILLI).unwrap();
    assert!(r.wake(t0 + 2 * MILLI));
    let again = r.step_scheduled(&sc, &cost, t0 + 2 * MILLI, &PrefixTree::empty(), &mut *sched).unwrap();
    assert_eq!(again.step_ns, 0);
    assert_eq!(again.token_at, first.token_at, "the hold is measured from the oldest entry, not the latest");
    let out = r.step_scheduled(&sc, &cost, first.token_at, &PrefixTree::empty(), &mut *sched).unwrap();
    assert!(out.step_ns > 0);
    assert_eq!(r.running(), 2, "one step serves both arrivals");
}

/// The bandwidth line, through the engine: at an 11 ms inter-token target the fixed 10.2 ms leaves
/// 0.8 ms of key-value re-read, about 45,700 tokens, so 4,000-token prompts stop at eleven and the
/// step the engine prices without its chunk fits the target. `fifo_chunked` takes all forty.
#[test]
fn the_batch_the_engine_builds_respects_the_bandwidth_line() {
    let mut sc = Scenario::default();
    sc.max_batch = 64;
    sc.max_queue = 1000;
    sc.itl_slo_ms = 11.0;
    sc.buffer_max_prefill_tokens = 1_000_000;
    let cost = sc.cost_model();
    let budget: Nanos = 11 * MILLI;
    for (policy, want) in [("buffered_batch", 11usize), ("fifo_chunked", 40)] {
        sc.scheduling = policy.into();
        let mut sched = lbsim::policy::make_scheduling(&sc).unwrap();
        let mut r = Replica::default();
        for id in 0..40 {
            r.enqueue(req(id, 4000), sc.max_queue, EPOCH_BASE).unwrap();
        }
        r.step_scheduled(&sc, &cost, EPOCH_BASE, &PrefixTree::empty(), &mut *sched).unwrap();
        assert_eq!(r.running(), want, "{policy}");
        let re_read = cost.step_ns(r.running(), r.kv_tokens(), 0);
        if policy == "buffered_batch" {
            assert!(re_read <= budget, "{policy}: the re-read alone takes {re_read} ns against {budget}");
        } else {
            assert!(re_read > budget, "{policy}: expected to blow the line, took {re_read} ns");
        }
    }
}

/// What the router is told. Ten 1,200-token prompts against a 1,024-token chunk: one fits, nine are
/// beyond the buffer, and the same replica under `fifo_chunked` reports nothing beyond.
#[test]
fn queued_work_counts_what_the_buffer_leaves_out_and_nothing_under_fifo() {
    let mut sc = Scenario::default();
    sc.max_batch = 8;
    sc.max_queue = 1000;
    sc.scheduling = "buffered_batch".into();
    let cost = sc.cost_model();
    let sched = lbsim::policy::make_scheduling(&sc).unwrap();
    let mut r = Replica::default();
    for id in 0..10 {
        r.enqueue(req(id, 1200), sc.max_queue, EPOCH_BASE).unwrap();
    }
    let w = r.queued_work(&sc, &cost, &*sched);
    assert_eq!((w.decode, w.prefill, w.decode_beyond, w.prefill_beyond), (0, 10, 0, 9), "{w:?}");
    sc.scheduling = "fifo_chunked".into();
    let fifo = lbsim::policy::make_scheduling(&sc).unwrap();
    let w = r.queued_work(&sc, &cost, &*fifo);
    assert_eq!((w.decode, w.prefill, w.decode_beyond, w.prefill_beyond), (0, 10, 0, 0), "{w:?}");
    // After a step the admitted one is prefilling in the batch (1,024 of its 1,200 done) and nine
    // are still queued: the running one is a prefill item that takes 176 of the next chunk, the
    // 848 left admit the queue's head, and the eight behind it are beyond the buffer.
    sc.scheduling = "buffered_batch".into();
    let mut sched = sched;
    r.step_scheduled(&sc, &cost, EPOCH_BASE, &PrefixTree::empty(), &mut *sched).unwrap();
    let w = r.queued_work(&sc, &cost, &*sched);
    assert_eq!((w.decode, w.prefill, w.prefill_beyond), (0, 10, 8), "{w:?}");
}

/// Through the loop: the hold adds at most its own length to time-to-first-token at a load where
/// every replica is idle between arrivals, and the run is byte-repeatable.
#[test]
fn through_the_loop_the_hold_is_bounded_and_the_run_repeats() {
    let mut fifo = at_load(0.05);
    fifo.max_batch = 4;
    fifo.buffer_max_prefill_tokens = 100_000;
    let mut held = fifo.clone();
    held.scheduling = "buffered_batch".into();
    held.buffer_max_hold_ms = 20.0;
    let a = sim::run(&fifo).unwrap();
    let b = sim::run(&held).unwrap();
    assert_eq!(b.fingerprint, sim::run(&held).unwrap().fingerprint);
    assert_ne!(a.fingerprint, b.fingerprint, "a 20 ms hold on an idle fleet must move something");
    let p50 = |r: &sim::RunResult| r.ttft.percentile(50.0);
    assert!(p50(&b) >= p50(&a), "holding cannot make first tokens earlier: {} vs {}", p50(&b), p50(&a));
    assert!(p50(&b) <= p50(&a) + 20 * MILLI, "the hold adds at most itself: {} vs {}", p50(&b), p50(&a));
}
