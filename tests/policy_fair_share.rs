//! Weighted fair share: the over-share tenant is the one shed, and nobody else pays for it.
//!
//! Three tenants with equal entitlements, one of them offering four times what the others do, on a
//! fleet at 1.2x rated capacity. Without admission control the overload is shared by everyone in
//! proportion to what they send, so the two tenants staying inside their share are punished for the
//! third's excess. `fair_share` is supposed to move that cost onto the tenant that caused it, and
//! nothing else: same workload, same arrivals, and a strict no-op when there is only one tenant.

mod common;

use common::*;
use lbsim::metrics::{Outcome, RequestRecord};
use lbsim::policy::{make_admission, ADMISSION};
use lbsim::rng::Streams;
use lbsim::scenario::Scenario;
use lbsim::sim::{self, RunResult, NO_REPLICA};
use lbsim::workload::{Request, Workload};
use lbsim::EPOCH_BASE;

fn tenanted(admission: &str) -> Scenario {
    let mut s = at_load(1.2);
    s.tenants = 3;
    s.tenant_weights = vec![1.0, 1.0, 1.0];
    s.tenant_demand = vec![4.0, 1.0, 1.0];
    s.admission = admission.into();
    s
}

/// The workload's own view of every request the run offered, by replaying its draws.
///
/// `RequestRecord` carries no tenant, and ids are assigned in arrival order by a generator whose
/// tenant stream is independent of everything else, so replaying `Workload::make` in order recovers
/// the mapping exactly. This is the simpler of the two options: it needs no join, only a lookup.
/// Sized from `first_attempts` rather than from the records, because a request still in flight when
/// the run stops has no record, and which ones those are depends on the policy.
fn replay(s: &Scenario, r: &RunResult) -> Vec<Request> {
    let mut wl = Workload::new(&Streams::new(s.seed));
    (1..=r.first_attempts)
        .map(|id| {
            let req = wl.make(s, EPOCH_BASE);
            assert_eq!(req.id, id, "the workload does not number requests sequentially");
            req
        })
        .collect()
}

/// Indexed by request id; ids start at one, so slot zero is a sentinel.
fn tenant_of(s: &Scenario, r: &RunResult) -> Vec<u32> {
    std::iter::once(u32::MAX).chain(replay(s, r).iter().map(|q| q.tenant)).collect()
}

/// Shed by admission, before any replica was involved. Distinct from a queue-full rejection at a
/// replica, which is the fleet saying no rather than the policy.
fn shed_by_admission(r: &RequestRecord) -> bool {
    r.outcome == Outcome::Rejected && r.replica == NO_REPLICA
}

fn p99(mut v: Vec<u64>) -> u64 {
    assert!(!v.is_empty(), "no samples");
    v.sort_unstable();
    v[((v.len() - 1) as f64 * 0.99).round() as usize]
}

#[test]
fn fair_share_is_registered_and_labelled() {
    let entry = ADMISSION.iter().find(|e| e.names.contains(&"fair_share"));
    assert!(entry.is_some(), "fair_share is not in the admission registry");
    assert_eq!(entry.unwrap().file, "fair_share.rs");

    let mut s = Scenario::default();
    s.admission = "fair_share".into();
    assert_eq!(make_admission(&s).unwrap().label(), "fair_share(burst=2)");
    s.fair_share_burst = 1.5;
    assert_eq!(make_admission(&s).unwrap().label(), "fair_share(burst=1.5)");
}

/// One tenant means no tenancy, and no tenancy means the policy must not exist as far as the run is
/// concerned: not merely zero rejections, but the same run to the byte.
#[test]
fn single_tenant_is_never_shed() {
    let mut base = at_load(1.2);
    base.admission = "accept_all".into();
    let mut fs = base.clone();
    fs.admission = "fair_share".into();

    let a = sim::run(&base).unwrap();
    let b = sim::run(&fs).unwrap();
    let shed = b.records.iter().filter(|r| shed_by_admission(r)).count();
    assert_eq!(shed, 0, "fair_share shed {shed} requests with a single tenant");
    assert_eq!(a.fingerprint, b.fingerprint, "fair_share is not a no-op with one tenant");
    assert_eq!(a.records.len(), b.records.len());
}

#[test]
fn the_over_share_tenant_is_the_one_shed() {
    let s = tenanted("fair_share");
    let r = sim::run(&s).unwrap();
    let tenant = tenant_of(&s, &r);
    let mut shed = [0u64; 3];
    for rec in r.records.iter().filter(|x| shed_by_admission(x)) {
        shed[tenant[rec.id as usize] as usize] += 1;
    }
    let total: u64 = shed.iter().sum();
    assert!(total > 0, "fair_share shed nothing at 1.2x load; the fixture is not contended");
    let share0 = shed[0] as f64 / total as f64;
    assert!(
        share0 > 0.95,
        "tenant 0 offers two thirds of the load against a one-third entitlement, yet only {:.1}% of \
         the {total} shed requests were its own: shed per tenant = {shed:?}",
        share0 * 100.0
    );
}

/// The point of the policy: the tenants inside their share must do better than they did when the
/// overload was shared out by arrival. Measured per tenant on both axes the report uses, attainment
/// (fraction completed) and p99 end-to-end latency of what completed.
///
/// Attainment is the strict assertion, because it is where the effect is large (measured: tenant 1
/// 0.767 to 0.854, tenant 2 0.792 to 0.865). The p99 of the requests that completed sits just under
/// the 8 s client timeout under either policy, because that is what overload does to the survivors,
/// so it moves by tens of milliseconds (7813 to 7561 ms and 7870 to 7843 ms) and is only guarded
/// against getting worse: the gain must not have been bought with latency.
#[test]
fn under_share_tenants_get_better_service_than_without_the_policy() {
    let plain = tenanted("accept_all");
    let fair = tenanted("fair_share");
    let a = sim::run(&plain).unwrap();
    let b = sim::run(&fair).unwrap();
    let tenant = tenant_of(&fair, &b);
    assert_eq!(tenant, tenant_of(&plain, &a), "the tenant assignment differs between the runs");

    let per_tenant = |r: &RunResult, t: u32| -> (f64, u64) {
        let recs: Vec<&RequestRecord> =
            settled(r, &fair).into_iter().filter(|x| tenant[x.id as usize] == t).collect();
        let done = recs.iter().filter(|x| x.outcome.is_success()).count();
        let e2e: Vec<u64> = recs.iter().filter_map(|x| x.e2e()).collect();
        (done as f64 / recs.len() as f64, p99(e2e))
    };
    for t in [1_u32, 2] {
        let (att_plain, p99_plain) = per_tenant(&a, t);
        let (att_fair, p99_fair) = per_tenant(&b, t);
        let report = format!(
            "tenant {t}: accept_all attainment {att_plain:.3} p99 e2e {} ms, \
             fair_share attainment {att_fair:.3} p99 e2e {} ms",
            p99_plain / 1_000_000,
            p99_fair / 1_000_000
        );
        assert!(att_fair > att_plain, "{report}");
        assert!(p99_fair as f64 <= p99_plain as f64 * 1.05, "{report}");
    }
}

/// Admission decides what is served, never what is offered.
///
/// A shed request is recorded with zero output tokens, since it produced none, so the full shape
/// multiset cannot match between a policy that sheds and one that does not. What can, and must, is
/// everything the workload decided: arrival times, prompt lengths, and the output length the
/// workload drew for every request, which the replay knows even when the record does not.
#[test]
fn workload_is_identical_across_the_two_policies() {
    let plain = tenanted("accept_all");
    let fair = tenanted("fair_share");
    let a = sim::run(&plain).unwrap();
    let b = sim::run(&fair).unwrap();
    assert_eq!(a.first_attempts, b.first_attempts, "the number of offered requests differs");
    let sa = settled(&a, &plain);
    let sb = settled(&b, &fair);
    assert_eq!(prompt_multiset(&sa), prompt_multiset(&sb), "prompt lengths differ between policies");
    assert_eq!(arrival_times(&sa), arrival_times(&sb), "arrival times differ between policies");

    let offered = replay(&fair, &b);
    for rec in sa.iter().chain(sb.iter()) {
        let q = &offered[rec.id as usize - 1];
        assert_eq!((rec.prompt_tokens, rec.arrived_at), (q.prompt, rec.arrived_at));
        if rec.outcome.is_success() {
            assert_eq!(rec.output_tokens, q.output, "request {} was served a different output", rec.id);
        }
    }
}
