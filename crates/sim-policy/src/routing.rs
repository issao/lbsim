//! The routing seam: what a router may see, and what it may do.

use sim_core::rng::Rng;
use sim_core::Nanos;

/// What a replica reports about itself. Not everything the simulator knows: a replica cannot report
/// the true output length of its running requests, because it does not know it.
#[derive(Clone, Copy, Default, Debug)]
pub struct ReplicaView {
    pub sampled_at: Nanos,
    pub queued: u32,
    pub running: u32,
    /// Queued work in tokens rather than requests. One long-context request costs what many chat
    /// turns cost, so a policy counting requests is measuring the wrong quantity.
    pub queued_tokens: u64,
    /// Resident key-value tokens. The load signal that is actually in the right unit.
    pub kv_tokens: u64,
    pub last_step_ns: Nanos,
    pub ejected: bool,
}

/// What a router may know about the request it is placing. Deliberately no output length: that lives
/// in the workload's truth and is not observable, which is the defining difficulty of this domain.
#[derive(Clone, Copy, Debug)]
pub struct RequestView {
    pub id: u64,
    pub prompt_tokens: u32,
    pub arrived_at: Nanos,
    pub deadline: Nanos,
    pub tenant: u32,
    pub attempts: u32,
}

/// Everything one routing decision is made from.
///
/// `views` is the delayed snapshot. `probe` is the only way to see fresher state, and every probe is
/// counted so the engine can charge the modelled round trip for it: freshness has a price, and a
/// policy that pays it must pay it visibly.
pub struct RouteContext<'a> {
    pub now: Nanos,
    pub views: &'a [ReplicaView],
    pub request: &'a RequestView,
    pub rng: &'a mut Rng,
    live: &'a dyn Fn(usize) -> ReplicaView,
    probes: u32,
}

impl<'a> RouteContext<'a> {
    pub fn new(
        now: Nanos,
        views: &'a [ReplicaView],
        request: &'a RequestView,
        rng: &'a mut Rng,
        live: &'a dyn Fn(usize) -> ReplicaView,
    ) -> Self {
        RouteContext { now, views, request, rng, live, probes: 0 }
    }

    /// The replica's state *now*, at the cost of a modelled round trip per call.
    pub fn probe(&mut self, replica: usize) -> ReplicaView {
        self.probes += 1;
        (self.live)(replica)
    }

    /// How many probes this decision paid for.
    pub fn probes(&self) -> u32 {
        self.probes
    }

    pub fn fleet(&self) -> usize {
        self.views.len()
    }

    #[inline]
    pub fn usable(&self, i: usize) -> bool {
        !self.views[i].ejected
    }

    /// The first usable replica in index order, the fallback every sampling policy shares when its
    /// samples all land on ejected replicas.
    pub fn first_usable(&self) -> Option<usize> {
        (0..self.fleet()).find(|&i| self.usable(i))
    }
}

/// A routing policy. Holds its own private state (cursors, estimators) and never touches simulation
/// state; it cannot, because nothing mutable reaches it.
pub trait RoutingPolicy {
    /// The name a report prints, including any parameter that matters, e.g. `p2c(d=2)`.
    fn label(&self) -> String;

    /// Cost of one decision, in replicas inspected, for a fleet of `fleet`. Recorded so a policy that
    /// cannot hold at target scale is visible rather than merely slow.
    fn inspected(&self, fleet: usize) -> usize;

    /// Pick a replica index, or `None` if nothing is usable.
    fn choose(&mut self, ctx: &mut RouteContext<'_>) -> Option<usize>;
}
