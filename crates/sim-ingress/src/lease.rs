//! Subscription leases: the wall-clock side of `proto/lbsim/v1/subscription.proto`.
//!
//! A subscription is a real network stream, and a closed browser tab sends no goodbye. The lease is
//! the only thing that stops a vanished client from keeping Ingress and every Leaf shipping data
//! forever, and it is also the signal the idle guard in [`crate::idle`] reads to decide whether the
//! instance may be reaped. Everything here is in *wall* nanoseconds, never simulated time: the proto
//! reserves `_unix_ns` for simulated instants and names the lease field `lease_expires_at_wall_ns`,
//! and this module follows that convention so the two clocks cannot be confused in a signature.
//!
//! Time is an argument, not a system call. The server reads the clock once per request and passes
//! it in, so the registry is deterministic, testable at any instant, and cannot drift from the
//! server's view of "now" within a single request.

use std::collections::BTreeMap;

/// Cloud Run is deployed with `--timeout 900` (docs/execution-plan.md section 3.4). A lease longer
/// than the request timeout would promise a stream the platform will cut anyway, so the registry
/// clamps to this and tells the client the real expiry.
pub const MAX_LEASE_NS: u64 = 900 * 1_000_000_000;

/// Monotonic from 1, never reused, so a stale id from a previous stream can never alias a new one.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SubscriptionId(pub u64);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Lease {
    pub id: SubscriptionId,
    pub run_id: String,
    /// The deadline instant is itself expired: a lease is live only while `now < expires_at_wall_ns`.
    /// Inclusive at the deadline so that `open(.., lease_ns = 0, now)` is dead at `now`, never live
    /// for a zero-length instant that depends on clock granularity.
    pub expires_at_wall_ns: u64,
}

impl Lease {
    pub fn is_live(&self, now_wall_ns: u64) -> bool {
        now_wall_ns < self.expires_at_wall_ns
    }
}

/// What a renewal did. Mirrors `RenewSubscriptionResponse` field for field so the server can copy
/// it straight into the wire message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RenewOutcome {
    pub expires_at_wall_ns: u64,
    /// True when the lease was already dead, or never existed. The client must reopen; the proto
    /// makes this flag the only authority on liveness, so it is never inferred from timestamps.
    pub expired: bool,
}

#[derive(Debug, Default)]
pub struct LeaseRegistry {
    /// Ordered by id so `expire` returns dead leases in opening order, which keeps the server's
    /// stream teardown deterministic under test.
    leases: BTreeMap<SubscriptionId, Lease>,
    next_id: u64,
}

impl LeaseRegistry {
    pub fn new() -> Self {
        Self { leases: BTreeMap::new(), next_id: 1 }
    }

    /// Clamps `lease_ns` to [`MAX_LEASE_NS`] and returns the actual expiry, so a client that asked
    /// for too much is told the truth rather than silently cut off.
    ///
    /// A client must ask for a positive lease: `lease_ns = 0` yields a lease that expires at `now`,
    /// which by the inclusive-deadline rule is already dead. The registry does not reject it, because
    /// the honest expiry in the response is a clearer signal than an error the client has to parse.
    pub fn open(&mut self, run_id: &str, lease_ns: u64, now_wall_ns: u64) -> Lease {
        let id = SubscriptionId(self.next_id);
        self.next_id += 1;
        let lease = Lease {
            id: id.clone(),
            run_id: run_id.to_string(),
            expires_at_wall_ns: expiry(now_wall_ns, lease_ns),
        };
        self.leases.insert(id, lease.clone());
        lease
    }

    /// Extends from `now`, not from the old expiry: a renewal is a heartbeat ("still here"), and
    /// measuring from the old deadline would let an early renewer bank time past the platform
    /// timeout. Clamped like `open`.
    ///
    /// After expiry the outcome is `expired = true` and the lease is not resurrected, even if it has
    /// not yet been swept by [`expire`](Self::expire): the server may have already torn the stream
    /// down, and a client that missed its deadline must reopen so it gets a fresh id and a fresh
    /// stream rather than a half-alive one.
    pub fn renew(&mut self, id: &SubscriptionId, lease_ns: u64, now_wall_ns: u64) -> RenewOutcome {
        match self.leases.get_mut(id) {
            Some(lease) if lease.is_live(now_wall_ns) => {
                lease.expires_at_wall_ns = expiry(now_wall_ns, lease_ns);
                RenewOutcome { expires_at_wall_ns: lease.expires_at_wall_ns, expired: false }
            }
            Some(lease) => RenewOutcome { expires_at_wall_ns: lease.expires_at_wall_ns, expired: true },
            None => RenewOutcome { expires_at_wall_ns: 0, expired: true },
        }
    }

    /// Idempotent. Returns whether the lease was present, live or not; a close racing an expiry
    /// sweep must not error, because the client did the right thing either way.
    pub fn close(&mut self, id: &SubscriptionId) -> bool {
        self.leases.remove(id).is_some()
    }

    /// Drops every lease whose expiry is at or before `now` and returns them, so the server can end
    /// their streams. Returned in id order.
    pub fn expire(&mut self, now_wall_ns: u64) -> Vec<Lease> {
        let dead: Vec<SubscriptionId> = self
            .leases
            .values()
            .filter(|l| !l.is_live(now_wall_ns))
            .map(|l| l.id.clone())
            .collect();
        dead.iter().filter_map(|id| self.leases.remove(id)).collect()
    }

    /// Live leases at `now`. Counted rather than read from `len()` because a lease can be dead
    /// before it is swept, and the idle guard must not see a dead lease as activity.
    pub fn live(&self, now_wall_ns: u64) -> usize {
        self.leases.values().filter(|l| l.is_live(now_wall_ns)).count()
    }

    pub fn live_for_run(&self, run_id: &str, now_wall_ns: u64) -> usize {
        self.leases
            .values()
            .filter(|l| l.run_id == run_id && l.is_live(now_wall_ns))
            .count()
    }

    pub fn get(&self, id: &SubscriptionId) -> Option<&Lease> {
        self.leases.get(id)
    }
}

/// Saturating on purpose: a client sending `u64::MAX` must not wrap into the past and get a lease
/// that is dead on arrival for the wrong reason.
fn expiry(now_wall_ns: u64, lease_ns: u64) -> u64 {
    now_wall_ns.saturating_add(lease_ns.min(MAX_LEASE_NS))
}

#[cfg(test)]
mod tests {
    use super::*;

    const S: u64 = 1_000_000_000;

    #[test]
    fn lease_expires_at_its_deadline_instant_and_not_before() {
        let mut reg = LeaseRegistry::new();
        let lease = reg.open("run-a", 10 * S, 100 * S);
        assert_eq!(lease.expires_at_wall_ns, 110 * S);
        assert_eq!(reg.live(110 * S - 1), 1, "one nanosecond before the deadline is live");
        assert_eq!(reg.live(110 * S), 0, "the deadline instant itself is expired");
        assert!(reg.renew(&lease.id, 10 * S, 110 * S).expired);

        // A zero lease is dead at the instant it is opened.
        let zero = reg.open("run-a", 0, 200 * S);
        assert_eq!(zero.expires_at_wall_ns, 200 * S);
        assert!(!zero.is_live(200 * S));
    }

    #[test]
    fn renew_extends_from_now_not_from_the_old_expiry() {
        let mut reg = LeaseRegistry::new();
        let lease = reg.open("run-a", 10 * S, 100 * S);
        // Renewing early at t=103 for 10s must land at 113, not 120.
        let out = reg.renew(&lease.id, 10 * S, 103 * S);
        assert_eq!(out, RenewOutcome { expires_at_wall_ns: 113 * S, expired: false });
        assert_eq!(reg.get(&lease.id).unwrap().expires_at_wall_ns, 113 * S);
        // Renewing for less than the remaining time shortens it: it is a heartbeat, not a max.
        let out = reg.renew(&lease.id, 2 * S, 104 * S);
        assert_eq!(out.expires_at_wall_ns, 106 * S);
    }

    #[test]
    fn renew_after_expiry_reports_expired_and_does_not_resurrect() {
        let mut reg = LeaseRegistry::new();
        let lease = reg.open("run-a", 10 * S, 100 * S);
        let out = reg.renew(&lease.id, 10 * S, 111 * S);
        assert!(out.expired);
        assert_eq!(out.expires_at_wall_ns, 110 * S, "the old deadline is reported, not a new one");
        assert_eq!(reg.get(&lease.id).unwrap().expires_at_wall_ns, 110 * S);
        assert_eq!(reg.live(111 * S), 0);
        // And it stays dead: a second renew is not a reopen.
        assert!(reg.renew(&lease.id, 10 * S, 112 * S).expired);
    }

    #[test]
    fn renew_of_an_unknown_id_reports_expired() {
        let mut reg = LeaseRegistry::new();
        let out = reg.renew(&SubscriptionId(42), 10 * S, 100 * S);
        assert_eq!(out, RenewOutcome { expires_at_wall_ns: 0, expired: true });
        let lease = reg.open("run-a", 10 * S, 100 * S);
        assert!(reg.close(&lease.id));
        assert!(reg.renew(&lease.id, 10 * S, 101 * S).expired);
    }

    #[test]
    fn close_is_idempotent() {
        let mut reg = LeaseRegistry::new();
        let lease = reg.open("run-a", 10 * S, 100 * S);
        assert!(reg.close(&lease.id));
        assert!(!reg.close(&lease.id));
        assert!(!reg.close(&SubscriptionId(999)));
        assert!(reg.get(&lease.id).is_none());
        // Ids are never reused after a close.
        let next = reg.open("run-a", 10 * S, 100 * S);
        assert!(next.id > lease.id);
    }

    #[test]
    fn lease_is_clamped_to_the_maximum_and_the_clamp_is_reported() {
        let mut reg = LeaseRegistry::new();
        let lease = reg.open("run-a", MAX_LEASE_NS * 3, 100 * S);
        assert_eq!(lease.expires_at_wall_ns, 100 * S + MAX_LEASE_NS);
        assert_eq!(reg.get(&lease.id).unwrap().expires_at_wall_ns, 100 * S + MAX_LEASE_NS);
        let out = reg.renew(&lease.id, u64::MAX, 200 * S);
        assert_eq!(out.expires_at_wall_ns, 200 * S + MAX_LEASE_NS);
        assert!(!out.expired);
        // The clamp is exactly the Cloud Run request timeout, not a rounder number.
        assert_eq!(MAX_LEASE_NS, 15 * 60 * S);
    }

    #[test]
    fn expire_returns_exactly_the_dead_leases_and_keeps_the_live_ones() {
        let mut reg = LeaseRegistry::new();
        let a = reg.open("run-a", 5 * S, 100 * S); // dies at 105
        let b = reg.open("run-a", 20 * S, 100 * S); // dies at 120
        let c = reg.open("run-b", 10 * S, 100 * S); // dies at 110
        assert!(reg.expire(104 * S).is_empty());
        let dead = reg.expire(110 * S);
        assert_eq!(dead.iter().map(|l| l.id.clone()).collect::<Vec<_>>(), vec![a.id.clone(), c.id.clone()]);
        assert_eq!(dead[1], c);
        assert!(reg.get(&a.id).is_none());
        assert!(reg.get(&c.id).is_none());
        assert_eq!(reg.get(&b.id), Some(&b));
        assert_eq!(reg.live(110 * S), 1);
        // Sweeping again at the same instant finds nothing: expire is not a repeat notifier.
        assert!(reg.expire(110 * S).is_empty());
    }

    #[test]
    fn live_for_run_counts_only_that_run() {
        let mut reg = LeaseRegistry::new();
        reg.open("run-a", 10 * S, 100 * S);
        reg.open("run-a", 10 * S, 100 * S);
        reg.open("run-b", 10 * S, 100 * S);
        let dead = reg.open("run-b", 1 * S, 100 * S);
        assert_eq!(reg.live_for_run("run-a", 105 * S), 2);
        assert_eq!(reg.live_for_run("run-b", 105 * S), 1, "an unswept dead lease is not live");
        assert_eq!(reg.live_for_run("run-c", 105 * S), 0);
        assert_eq!(reg.live(105 * S), 3);
        assert!(reg.get(&dead.id).is_some(), "dead but not yet swept");
    }

    #[test]
    fn ids_are_unique_and_monotonic() {
        let mut reg = LeaseRegistry::new();
        let ids: Vec<u64> = (0..5).map(|_| reg.open("r", S, 0).id.0).collect();
        assert_eq!(ids, vec![1, 2, 3, 4, 5]);
    }
}
