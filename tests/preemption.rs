//! Preemption and key-value eviction: what a replica does when resident context outgrows its memory.
//!
//! Below pressure a preemption policy must be invisible, because every existing golden run has none
//! and must stay byte-identical. Under pressure the three policies differ only in what they charge:
//! recompute pays prefill again over the whole context, swap pays a transfer out and back, and the
//! replica's resident token count drops by exactly the victim's context either way. The last test is
//! the dynamic the unit exists for: multi-turn sessions park their context on a replica, the parked
//! context fills the cache at a load the device could easily serve, and without eviction the replica
//! is reduced to serving one sequence at a time.

use lbsim::model::{PrefixTree, Replica, StepOutcome, Tiers};
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
        class: 0,
        prefix_node: 0,
        prefix_tokens: 0,
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

/// Session turns carry ids above this; the loop keeps them apart from first attempts and retries.
const SESSION_ID_BASE: u64 = 1 << 40;

#[test]
fn a_session_turn_is_offered_to_admission() {
    // Zero headroom: any expected queue wait at all sheds the request, and the policy estimates that
    // wait from batch slots, so a small batch keeps the fleet queueing when turns come back. Both
    // first turns and follow-ups are then shed.
    let mut sc = scenario("scenarios/kv_spiral_swap.txt");
    sc.admission = "deadline_aware".into();
    sc.admission_headroom = 1.0;
    sc.max_batch = 4;
    let r = sim::run(&sc).unwrap();
    let turns: Vec<_> = r.records.iter().filter(|x| x.attempts == 1 && x.id >= SESSION_ID_BASE).collect();
    assert!(!turns.is_empty(), "the spiral scenario must spawn session turns");
    let shed = turns.iter().filter(|x| x.outcome == lbsim::metrics::Outcome::Rejected).count();
    assert!(shed > 0, "admission never saw a follow-up turn: {} turns, none rejected", turns.len());
    // A follow-up is a request like any other for the retry budget too: every recorded first attempt,
    // session turn or not, was counted before it was placed.
    let first = r.records.iter().filter(|x| x.attempts == 1).count() as u64;
    assert!(
        r.first_attempts >= first,
        "first_attempts {} is below the {} recorded first attempts, so session turns were not counted",
        r.first_attempts, first
    );
}

#[test]
fn eviction_never_swaps_the_queue_heads_own_context() {
    // Two sessions park their context on the replica; one's next turn is already at the head of the
    // queue, the other is still thinking. A lone sequence runs ahead of both, and its decode growth
    // reaches the cap on its last step. Evicting the head's context there is pure waste: it goes out
    // this step and straight back in the next, two transfers charged, while the other session's
    // context is the obvious victim. Cap: 100 running + 100 + 100 parked, plus ten decode steps.
    let mut sc = tiny("swap_to_dram");
    sc.kv_capacity_tokens = 310.0;
    sc.max_batch = 1;
    let cost = sc.cost_model();
    let deadline = EPOCH_BASE + 60 * SECOND;
    let mut r = Replica::default();
    r.park(12, 100, deadline, EPOCH_BASE);
    r.park(11, 100, deadline, EPOCH_BASE + 1);
    r.enqueue(req(1, 100, 11), sc.max_queue).ok().unwrap();
    r.enqueue(req(11, 110, 5), sc.max_queue).ok().unwrap();
    r.wake(EPOCH_BASE);
    let mut now = EPOCH_BASE;
    let last = step_until(&mut r, &sc, &cost, &mut now, |o| !o.finished.is_empty());
    assert_eq!(last.finished[0].req.id, 1);
    assert!(last.preempted <= 1, "one victim at most on the step that reaches the cap");
    // The head's context is resident whichever way the cap was met, so its turn comes in for the
    // price of its ten new tokens: no transfer in, and so no transfer out before it.
    let back = r.step(&sc, &cost, now).unwrap();
    assert_eq!(r.running(), 1);
    assert_eq!(
        back.step_ns,
        cost.step_ns(1, r.kv_tokens(), 10),
        "the head's turn should be admitted on resident context, without a swap-in charge"
    );
    assert_eq!(r.parked(), 1, "the thinking session's context is the one that may be evicted");
}

// ---------------------------------------------------------------------------------------------
// Memory tiering (architecture section 7.2): cluster DRAM and SSD pools, one shared fabric.
// ---------------------------------------------------------------------------------------------

/// The golden row of `kv_spiral_swap` as of the merge that introduced the tier keys. The keys at
/// their defaults, one DRAM tier per replica over an unlimited fabric, must reduce to the arithmetic
/// that produced it, so the fingerprint is asserted by value rather than against another run.
#[test]
fn tier_defaults_leave_the_spiral_swap_run_byte_identical() {
    let sc = scenario("scenarios/kv_spiral_swap.txt");
    assert_eq!(sc.dram_pool_tokens, 0.0);
    assert_eq!(sc.ssd_pool_tokens, 0.0);
    assert_eq!(sc.fabric_gbps, 0.0);
    let r = sim::run(&sc).unwrap();
    assert_eq!(r.fingerprint, 4949261867223813137, "kv_spiral_swap's golden fingerprint moved");
    assert_eq!(r.events, 45412, "kv_spiral_swap's golden event count moved");
}

/// `step_until`, against a shared cluster `Tiers`, which is how the loop steps a replica.
fn step_until_tiered(
    r: &mut Replica,
    sc: &Scenario,
    cost: &CostModel,
    now: &mut Nanos,
    tiers: &mut Tiers,
    stop: impl Fn(&StepOutcome) -> bool,
) -> StepOutcome {
    let mut sched = lbsim::policy::make_scheduling(sc).unwrap();
    for _ in 0..1000 {
        let out = r
            .step_tiered(sc, cost, *now, &PrefixTree::empty(), &mut *sched, tiers)
            .expect("replica went idle before the condition held");
        *now = out.token_at;
        if stop(&out) {
            return out;
        }
    }
    panic!("condition never held");
}

#[test]
fn a_shared_dram_pool_refuses_the_second_replica_and_ssd_takes_the_overflow() {
    // Room in the pool for one 105-token context, not two. Replica A fills it; B's victim is refused.
    let mut sc = tiny("swap_to_dram");
    sc.dram_pool_tokens = 150.0;
    let cost = sc.cost_model();
    let base = cost.step_ns(1, 0, 0);
    let mut tiers = Tiers::new(&sc);
    let (mut a, _) = two_sequences(&sc);
    let (mut b, _) = two_sequences(&sc);
    let mut now_a = EPOCH_BASE;
    let mut now_b = EPOCH_BASE;
    let ev_a = step_until_tiered(&mut a, &sc, &cost, &mut now_a, &mut tiers, |o| o.preempted > 0);
    assert_eq!(ev_a.step_ns, base + cost.swap_ns(105), "A's victim goes to DRAM and pays the copy");
    assert_eq!((a.dram_tokens(), tiers.dram_used()), (105, 105));
    let ev_b = step_until_tiered(&mut b, &sc, &cost, &mut now_b, &mut tiers, |o| o.preempted > 0);
    assert_eq!(ev_b.step_ns, base, "with the pool full and no SSD tier, B's victim is dropped for free");
    assert_eq!((b.dram_tokens(), b.ssd_tokens(), tiers.dram_used(), tiers.ssd_used()), (0, 0, 105, 0));

    // The same, with an SSD pool below the DRAM one: B's victim lands there at SSD's transfer time.
    sc.ssd_pool_tokens = 1000.0;
    sc.ssd_gbps = 10.0;
    let cost = sc.cost_model();
    let mut tiers = Tiers::new(&sc);
    let (mut a, _) = two_sequences(&sc);
    let (mut b, _) = two_sequences(&sc);
    let mut now_a = EPOCH_BASE;
    let mut now_b = EPOCH_BASE;
    step_until_tiered(&mut a, &sc, &cost, &mut now_a, &mut tiers, |o| o.preempted > 0);
    let ev_b = step_until_tiered(&mut b, &sc, &cost, &mut now_b, &mut tiers, |o| o.preempted > 0);
    assert_eq!(cost.ssd_ns(105), 5 * cost.swap_ns(105), "a single drive is five times the DRAM path");
    assert_eq!(ev_b.step_ns, base + cost.ssd_ns(105), "B's victim goes to SSD and pays the slower copy");
    assert_eq!((b.dram_tokens(), b.ssd_tokens(), tiers.dram_used(), tiers.ssd_used()), (0, 105, 105, 105));
    // Re-admission pays the copy back from the tier it went to and gives the pool its tokens back.
    step_until_tiered(&mut b, &sc, &cost, &mut now_b, &mut tiers, |o| !o.finished.is_empty());
    let mut sched = lbsim::policy::make_scheduling(&sc).unwrap();
    let back = b.step_tiered(&sc, &cost, now_b, &PrefixTree::empty(), &mut *sched, &mut tiers).unwrap();
    assert_eq!(back.step_ns, base + cost.ssd_ns(105));
    assert_eq!((b.ssd_tokens(), tiers.ssd_used(), tiers.dram_used()), (0, 0, 105));
}

#[test]
fn the_shared_fabric_serialises_migrations_scheduled_at_the_same_instant() {
    // Two replicas in lockstep evict at the same instant. On a 1 GB/s fabric the first transfer ends
    // at t + B/g and the second, queued behind it, at t + 2B/g: each step is charged the wall time.
    let mut sc = tiny("swap_to_dram");
    sc.fabric_gbps = 1.0;
    let cost = sc.cost_model();
    let base = cost.step_ns(1, 0, 0);
    let bytes = 105.0 * lbsim::physics::KV_BYTES_PER_TOKEN as f64;
    let over_fabric = (bytes / 1e9 * 1e9) as Nanos;
    assert_eq!(cost.swap_ns(105), over_fabric, "the fabric, not the 50 GB/s host link, is the bottleneck");
    let mut tiers = Tiers::new(&sc);
    let (mut a, _) = two_sequences(&sc);
    let (mut b, _) = two_sequences(&sc);
    let mut sched = lbsim::policy::make_scheduling(&sc).unwrap();
    let mut now = EPOCH_BASE;
    for _ in 0..1000 {
        let out_a = a.step_tiered(&sc, &cost, now, &PrefixTree::empty(), &mut *sched, &mut tiers).unwrap();
        let out_b = b.step_tiered(&sc, &cost, now, &PrefixTree::empty(), &mut *sched, &mut tiers).unwrap();
        if out_a.preempted > 0 {
            assert_eq!(out_b.preempted, 1, "identical replicas evict on the same step");
            assert_eq!(out_a.step_ns, base + over_fabric, "the first migration runs at once");
            assert_eq!(out_b.step_ns, base + 2 * over_fabric, "the second waits for the first");
            assert_eq!(tiers.fabric_busy_until(), now + 2 * over_fabric);
            return;
        }
        assert_eq!(out_a.token_at, out_b.token_at);
        now = out_a.token_at;
    }
    panic!("the replicas never reached the cap");
}
