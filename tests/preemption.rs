//! Preemption and key-value eviction: what a replica does when resident context outgrows its memory.
//!
//! Below pressure a preemption policy must be invisible, because every existing golden run has none
//! and must stay byte-identical. Under pressure the three policies differ only in what they charge:
//! recompute pays prefill again over the whole context, swap pays a transfer out and back, and the
//! replica's resident token count drops by exactly the victim's context either way. The last test is
//! the dynamic the unit exists for: multi-turn sessions park their context on a replica, the parked
//! context fills the cache at a load the device could easily serve, and without eviction the replica
//! is reduced to serving one sequence at a time.

use lbsim::model::{Replica, StepOutcome};
use lbsim::physics::CostModel;
use lbsim::scenario::Scenario;
use lbsim::sim;
use lbsim::workload::Request;
use lbsim::{Nanos, EPOCH_BASE, SECOND};

fn scenario(path: &str) -> Scenario {
    Scenario::parse(&std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{path}: {e}"))).unwrap()
}

#[test]
fn preemption_never_changes_a_run_with_no_kv_pressure() {
    let mut never = scenario("scenarios/route_p2c.txt");
    never.preemption = "never".into();
    let mut recompute = never.clone();
    recompute.preemption = "recompute".into();
    let a = sim::run(&never).unwrap();
    let b = sim::run(&recompute).unwrap();
    assert_eq!(a.fingerprint, b.fingerprint, "a policy that never fires must not move a fingerprint");
    assert_eq!(a.records.len(), b.records.len());
    assert_eq!(a.frames.iter().map(|f| f.preemptions).sum::<u64>(), 0);
}

fn req(id: u64, prompt: u32, output: u32) -> Request {
    Request {
        id,
        arrived_at: EPOCH_BASE,
        attempt_at: EPOCH_BASE,
        prompt,
        output,
        attempts: 1,
        deadline: EPOCH_BASE + 60 * SECOND,
        is_long: false,
        tenant: 0,
    }
}

/// One replica with room for 210 resident tokens, no bandwidth or per-sequence term, so a step costs
/// the fixed part plus whatever prefill or transfer this unit adds to it.
fn tiny(preemption: &str) -> Scenario {
    let mut s = Scenario::default();
    s.kv_capacity_tokens = 210.0;
    s.max_batch = 8;
    s.step_per_seq_ms = 0.0;
    s.step_per_kv_ktoken_ms = 0.0;
    s.preemption = preemption.into();
    s.preemption_victim = "newest".into();
    s
}

/// Two 100-token prompts of 50 output tokens each. Both fit at admission (200 of 210), and decode
/// grows the pair by two tokens a step, so the sixth step is the one that would exceed the cap.
fn two_sequences(sc: &Scenario) -> (Replica, CostModel) {
    let mut r = Replica::default();
    r.enqueue(req(1, 100, 50), sc.max_queue).ok().unwrap();
    r.enqueue(req(2, 100, 50), sc.max_queue).ok().unwrap();
    r.wake(EPOCH_BASE);
    (r, sc.cost_model())
}

/// Step until `stop` says so; returns the outcome of that step.
fn step_until(
    r: &mut Replica,
    sc: &Scenario,
    cost: &CostModel,
    now: &mut Nanos,
    stop: impl Fn(&StepOutcome) -> bool,
) -> StepOutcome {
    for _ in 0..1000 {
        let out = r.step(sc, cost, *now).expect("replica went idle before the condition held");
        *now = out.token_at;
        if stop(&out) {
            return out;
        }
    }
    panic!("condition never held");
}

#[test]
fn eviction_frees_exactly_the_victim_kv() {
    let sc = tiny("recompute");
    let (mut r, cost) = two_sequences(&sc);
    let mut now = EPOCH_BASE;
    let out = step_until(&mut r, &sc, &cost, &mut now, |o| o.preempted > 0);
    assert_eq!(out.preempted, 1);
    assert_eq!(r.preemptions(), 1);
    // The newest sequence had 100 prompt tokens plus five generated when it was evicted; the survivor
    // had the same and then generated one more this step. Exactly the victim's context left.
    assert_eq!(r.kv_tokens(), 106);
    assert_eq!(r.running(), 1);
}

#[test]
fn recompute_charges_prefill_again() {
    let sc = tiny("recompute");
    let (mut r, cost) = two_sequences(&sc);
    let mut now = EPOCH_BASE;
    let evict = step_until(&mut r, &sc, &cost, &mut now, |o| o.preempted > 0);
    let base = cost.step_ns(1, 0, 0);
    assert_eq!(evict.step_ns, base, "recompute charges nothing at eviction");
    // The victim cannot come back while the survivor is resident, so it re-enters the step after the
    // survivor retires, and that step prefills its whole 105-token context again.
    step_until(&mut r, &sc, &cost, &mut now, |o| !o.finished.is_empty());
    let back = r.step(&sc, &cost, now).unwrap();
    assert_eq!(r.running(), 1);
    let prefill = (105.0 / sc.prefill_tokens_per_s * 1e9) as Nanos;
    assert!(
        back.step_ns >= base + prefill - 1,
        "re-admission step {} ns should carry {} ns of prefill over the {} ns base",
        back.step_ns, prefill, base
    );
}

#[test]
fn swap_charges_the_transfer() {
    let sc = tiny("swap_to_dram");
    let (mut r, cost) = two_sequences(&sc);
    let mut now = EPOCH_BASE;
    let evict = step_until(&mut r, &sc, &cost, &mut now, |o| o.preempted > 0);
    let base = cost.step_ns(1, 0, 0);
    let transfer = cost.swap_ns(105);
    assert!(transfer > 0);
    assert_eq!(evict.step_ns, base + transfer, "the eviction step pays the copy out");
    step_until(&mut r, &sc, &cost, &mut now, |o| !o.finished.is_empty());
    let back = r.step(&sc, &cost, now).unwrap();
    assert_eq!(back.step_ns, base + transfer, "the re-admission step pays the copy in, and no prefill");
    let delta = (evict.step_ns - base) + (back.step_ns - base);
    assert_eq!(delta, 2 * transfer);
    // Swapped context comes back whole: the sequence decodes on its first step back.
    assert_eq!(r.kv_tokens(), 106);
}

/// Share of every request that finished in the last third of the run and met every SLO.
fn late_attainment(r: &sim::RunResult) -> f64 {
    let from = r.measured_to - (r.measured_to - EPOCH_BASE) / 3;
    let late: Vec<_> = r.records.iter().filter(|x| x.finished_at >= from).collect();
    assert!(!late.is_empty());
    let ok = late.iter().filter(|x| x.outcome == lbsim::metrics::Outcome::Ok).count();
    ok as f64 / late.len() as f64
}

#[test]
fn the_spiral_collapses_without_preemption_and_recovers_with_it() {
    let never = sim::run(&scenario("scenarios/kv_spiral_never.txt")).unwrap();
    let swap = sim::run(&scenario("scenarios/kv_spiral_swap.txt")).unwrap();
    assert_eq!(never.scenario.arrival_rps, swap.scenario.arrival_rps, "same offered load");
    let (a, b) = (late_attainment(&never), late_attainment(&swap));
    let per_s = |r: &sim::RunResult| {
        r.frames.iter().map(|f| f.preemptions).sum::<u64>() as f64 / r.scenario.duration_s
    };
    eprintln!(
        "never: late attainment {a:.3}, p99 ttft {:.0} ms, {:.2} preemptions/s; swap: {b:.3}, {:.0} ms, {:.2}/s",
        never.ttft.percentile(99.0) as f64 / 1e6, per_s(&never),
        swap.ttft.percentile(99.0) as f64 / 1e6, per_s(&swap)
    );
    assert!(a < 0.5, "without eviction the parked context should have collapsed service: {a:.3}");
    assert!(b > 0.9, "with swapping the same load should be served: {b:.3}");
    assert_eq!(never.frames.iter().map(|f| f.preemptions).sum::<u64>(), 0);
    assert!(swap.frames.iter().map(|f| f.preemptions).sum::<u64>() > 0);
}
