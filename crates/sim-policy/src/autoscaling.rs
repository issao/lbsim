//! The autoscaling seam: how many replicas the fleet should have, decided from the same delayed
//! view every other policy reads.
//!
//! Dynamic 12's single-cluster half: the fleet is not a constant. A controller turns replicas up and
//! down, a turned-up replica serves nothing for a cold start, and a turned-down one has to finish what
//! it holds. The policy here decides only the number; the engine owns the lifecycle, the warm-up delay,
//! the drain and the choice of which replica goes, so a policy cannot cheat the cold start by
//! pretending a replica is ready. It sees the fleet as a controller sees it, through the delayed
//! views, which is why its decisions lag the load: a replica that became ready is still `ejected` in
//! its view until its first telemetry lands, and a queue that formed a second ago is not in the view
//! yet. That lag is part of what the seam measures.

use crate::routing::ReplicaView;
use sim_core::Nanos;

/// The fleet as the controller sees it. `ready` counts replicas that are up (crashed or degraded
/// ones included, since a controller only learns of those through the views), `warming` those
/// turned up and not yet serving, `draining` those turned down and still finishing. `views` has one
/// entry per slot, `max` of them; a slot that is absent, warming or draining is `ejected` there.
pub struct FleetView<'a> {
    pub now: Nanos,
    pub ready: usize,
    pub warming: usize,
    pub draining: usize,
    pub max: usize,
    pub min: usize,
    pub views: &'a [ReplicaView],
}

pub trait AutoscalingPolicy {
    fn label(&self) -> String;

    /// The number of replicas that should be ready or warming after this decision. The engine clamps
    /// it to `[min, max]`; returning `ready + warming` changes nothing.
    fn desired(&mut self, v: &FleetView) -> usize;
}
