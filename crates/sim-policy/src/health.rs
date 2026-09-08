//! The health seam: which replicas the router should treat as ejected, decided from the same delayed
//! view every other policy reads.
//!
//! VISION section 4: a machine may fail silently and to the client look like it is running very
//! slowly. Telemetry never announces that. `ReplicaView::ejected` carries only a crash, so a policy
//! that wants a gray replica out of the rotation has to infer it from what the view does carry, and
//! the growing `last_step_ns` is the one tell it has. A health policy runs once per telemetry
//! delivery, never per request, and its verdict is written back into the delayed views so routing
//! and admission see an ejection through the seam they already read. Nothing else in the engine
//! changes, which is why the policy is pluggable at all.

use crate::routing::ReplicaView;
use sim_core::Nanos;

pub trait HealthPolicy {
    fn label(&self) -> String;

    /// One flag per replica: whether the router should treat it as ejected right now. A crash that
    /// arrives through `views[i].ejected` must stay ejected whatever the policy thinks.
    fn assess(&mut self, now: Nanos, views: &[ReplicaView]) -> Vec<bool>;
}
