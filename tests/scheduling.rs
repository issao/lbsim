//! The scheduling seam: a replica's scheduler is a policy, and its cost is bounded by the batch.
//!
//! Issao: *"I assumed policies would cover the routing decision, the host and gpu local scheduling
//! decision and the global admission/rejection decisions."* The first test is the property that makes
//! the seam safe to open: a policy never receives the whole queue, so a scheduling decision cannot
//! become an O(queue) scan per step under exactly the overload it exists for. The rest check that the
//! three policies plug in through the scenario and that the classes they order by actually move.

mod common;

use common::*;
use lbsim::model::Replica;
use lbsim::scenario::Scenario;
use lbsim::sim;
use lbsim::workload::Request;
use lbsim::{EPOCH_BASE, SECOND};

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

/// Ten thousand queued requests, a batch of eight: the scheduler sees eight, whatever the policy.
#[test]
fn a_policy_never_receives_more_than_max_batch_queued_entries() {
    for policy in ["fifo_chunked", "class_priority", "deadline_first"] {
        let mut sc = Scenario::default();
        sc.max_batch = 8;
        sc.max_queue = 20_000;
        sc.scheduling = policy.into();
        let cost = sc.cost_model();
        let mut sched = lbsim::policy::make_scheduling(&sc).unwrap();
        let mut r = Replica::default();
        for id in 0..10_000 {
            r.enqueue(req(id, 16), sc.max_queue).expect("queue has room");
        }
        assert_eq!(r.queued(), 10_000);
        let mut now = EPOCH_BASE;
        for _ in 0..50 {
            let out = r
                .step_scheduled(&sc, &cost, now, &lbsim::model::PrefixTree::empty(), &mut *sched)
                .expect("work left");
            assert!(
                r.last_view_len() <= sc.max_batch,
                "{policy}: the scheduler saw {} queued entries, more than max_batch {}",
                r.last_view_len(),
                sc.max_batch
            );
            now = out.token_at;
        }
        assert!(r.queued() > 9_000, "{policy}: fifty steps of eight cannot have drained the queue");
    }
}

/// The three names resolve through the scenario key, and an unknown one is an error that names it.
#[test]
fn scheduling_key_selects_the_policy() {
    for policy in ["fifo_chunked", "fcfs", "class_priority", "deadline_first", "edf"] {
        let mut s = small();
        s.scheduling = policy.into();
        sim::run(&s).unwrap_or_else(|e| panic!("{policy}: {e}"));
    }
    let mut s = small();
    s.scheduling = "shortest_job_first".into();
    let err = sim::run(&s).err().expect("unknown scheduling name must be an error");
    assert!(err.contains("shortest_job_first"), "{err}");
}

/// Without classes, class_priority has nothing to order by and is FIFO with `newest` eviction, so
/// it must not move the run. deadline_first is not FIFO even then: a request's slack is its deadline
/// less the prefill it still owes, so a long prompt legitimately moves ahead of a short one whose
/// deadline is sooner.
#[test]
fn without_classes_class_priority_is_fifo() {
    let base = small();
    let fifo = sim::run(&base).unwrap();
    let mut s = base.clone();
    s.scheduling = "class_priority".into();
    let r = sim::run(&s).unwrap();
    assert_eq!(r.fingerprint, fifo.fingerprint, "class_priority moved a run it had nothing to order");
    s.scheduling = "deadline_first".into();
    let r = sim::run(&s).unwrap();
    assert_ne!(r.fingerprint, fifo.fingerprint, "deadline_first ordered nothing on a mixed workload");
}

/// Under a queue, class_priority moves interactive ahead of batch on the same arrivals: more
/// interactive requests finish inside the client timeout and at a shorter time to first token, and
/// fewer batch requests finish at all. Batch is what pays; the policy created no capacity. Success
/// counts rather than batch's mean TTFT, because the mean over survivors hides who was shed.
#[test]
fn class_priority_moves_interactive_ahead_and_batch_pays() {
    let mut s = small();
    s.arrival_rps = 60.0;
    s.max_batch = 4;
    s.duration_s = 40.0;
    s.slo_classes = "interactive:0.5,batch:0.5".into();
    let fifo = sim::run(&s).unwrap();
    s.scheduling = "class_priority".into();
    let cp = sim::run(&s).unwrap();
    let mean_ttft = |r: &sim::RunResult, class: u8| {
        let v: Vec<f64> = r
            .records
            .iter()
            .filter(|x| x.class == class && x.outcome.is_success())
            .filter_map(|x| x.ttft().map(|t| t as f64))
            .collect();
        assert!(!v.is_empty(), "class {class} has no successes");
        v.iter().sum::<f64>() / v.len() as f64
    };
    let successes = |r: &sim::RunResult, class: u8| {
        r.records.iter().filter(|x| x.class == class && x.outcome.is_success()).count()
    };
    for (name, r) in [("fifo", &fifo), ("class_priority", &cp)] {
        println!(
            "{name}: interactive ttft {:.0} ms, {} ok, attainment {:.3}; batch ttft {:.0} ms, {} ok, attainment {:.3}; records {}",
            mean_ttft(r, 1) / 1e6, successes(r, 1), r.class_attainment(1),
            mean_ttft(r, 3) / 1e6, successes(r, 3), r.class_attainment(3), r.records.len()
        );
    }
    assert!(
        mean_ttft(&cp, 1) < mean_ttft(&fifo, 1),
        "interactive TTFT {:.0} ms under class_priority, {:.0} ms under fifo",
        mean_ttft(&cp, 1) / 1e6,
        mean_ttft(&fifo, 1) / 1e6
    );
    assert!(
        successes(&cp, 1) > successes(&fifo, 1),
        "interactive successes {} under class_priority, {} under fifo",
        successes(&cp, 1),
        successes(&fifo, 1)
    );
    assert!(
        successes(&cp, 3) < successes(&fifo, 3),
        "batch successes {} under class_priority, {} under fifo",
        successes(&cp, 3),
        successes(&fifo, 3)
    );
}

/// Demo 16's per-class table, which the HTML report does not render. Ignored because it is three
/// 120 s runs; `SCHED_DEMO_RPS` overrides the load:
/// `tools/build.sh test --release --test scheduling -- --ignored --nocapture demo_16_class_table`.
#[test]
#[ignore]
fn demo_16_class_table() {
    let rps: Option<f64> = std::env::var("SCHED_DEMO_RPS").ok().and_then(|v| v.parse().ok());
    println!("scenario          class         goodput tok/s   attainment   successes   records");
    for file in ["sched_fifo", "sched_class", "sched_deadline"] {
        let text = std::fs::read_to_string(format!("scenarios/{file}.txt")).unwrap();
        let mut s = Scenario::parse(&text).unwrap();
        if let Some(rps) = rps {
            s.arrival_rps = rps;
        }
        let r = sim::run(&s).unwrap();
        for class in r.classes() {
            let name = lbsim::scenario::SLO_CLASSES[class as usize - 1].name;
            let n = r.records.iter().filter(|x| x.class == class).count();
            let ok = r.records.iter().filter(|x| x.class == class && x.outcome.is_success()).count();
            println!(
                "{file:<17} {name:<12} {:>14.0} {:>12.3} {ok:>11} {n:>9}",
                r.class_goodput_tokens_s(class),
                r.class_attainment(class)
            );
        }
        println!("{file:<17} {:<12} {:>14.0} {:>12.3}", "all", r.goodput_tokens_s(), r.slo_attainment());
    }
}
