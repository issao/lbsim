//! The admission seam: whether to accept a request at all, before it costs anything.
//!
//! `scenario.proto` `AdmissionPolicy`: rate limits are denominated in tokens, never requests, and
//! shedding happens *before* a request consumes device time. Systems that check afterwards spend most
//! of their capacity under overload producing tokens nobody will read.

use crate::routing::{ReplicaView, RequestView};
use sim_core::Nanos;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Admission {
    Admit,
    Reject,
}

/// What an admission decision is made from: the same stale fleet view a router sees, and the request.
pub struct AdmissionContext<'a> {
    pub now: Nanos,
    pub views: &'a [ReplicaView],
    pub request: &'a RequestView,
    /// Per-tenant weights from the scenario, normalised to sum to one; empty means one tenant.
    pub tenant_weights: &'a [f64],
}

pub trait AdmissionPolicy {
    fn label(&self) -> String;

    fn admit(&mut self, ctx: &AdmissionContext<'_>) -> Admission;

    /// Completion feedback, so a policy that charges at admission can refund, and a fair-share policy
    /// can account for what each tenant actually consumed. The default ignores it.
    fn on_complete(&mut self, _tenant: u32, _output_tokens: u32, _now: Nanos) {}
}
