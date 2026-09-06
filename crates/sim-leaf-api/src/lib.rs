//! The Ingress-to-Leaf seam, in-process: `proto/lbsim/v1/leaf.proto` as a Rust trait.
//!
//! Issao's decision of 2026-09-06: "Leaf shards should become separate processes in a sharded
//! server." This crate is what makes that possible without deciding it yet. The trait below has the
//! shape of the `Leaf` service, message for message and field for field, so `sim-ingress` can be
//! written against it today while `sim-leaf` implements it in-process; a process transport later is
//! one more implementor, serialising these structs, and nothing above the seam changes. The crate
//! depends on nothing that computes physics, which is what lets `sim-ingress` depend on it under the
//! rule of `docs/ARCHITECTURE.md` 10.8.
//!
//! # The plan for splitting `Sim` along this trait
//!
//! `sim_leaf::Sim` is today one event loop with one queue. Its events fall on two sides of the seam:
//!
//! **Ingress side.** `Arrival` (the workload draws a request and the router places it; needs the
//! delayed `views`, the tenant shares, the route RNG and the admission policy, none of which a leaf
//! may hold), the retry dispatch inside `Timeout` (the retry budget and the re-route are global
//! policy), `TelemetryDeliver` (the delayed snapshot is the router's state, and the delay is the
//! Ingress-to-Leaf hop), and the `Timeout` timer itself. Admission-shed records (`Rejected` at
//! `NO_REPLICA`) are produced here; everything else that ends a request ends it at a replica.
//!
//! **Leaf side.** `Admit` (arrives as [`DispatchedWork`] at `deliver_at_unix_ns`), `Step` (the
//! replica engine in `sim-model`, never leaving the shard), `TelemetryPublish` (the leaf samples its
//! replicas and ships them batched in [`AdvanceResponse::telemetry`], and Ingress schedules the
//! delayed `TelemetryDeliver` for them) and `Sample` (the leaf closes a [`Frame`] per replica set it
//! owns; Ingress merges frames across shards by adding counts and merging histograms bucket-wise,
//! which is why they are mergeable). The determinism fingerprint is a fold over step times, so it is
//! leaf-side too, folded per shard and combined in shard order.
//!
//! **What crosses.** A timeout is an Ingress timer whose *effect* is at the replica, so it crosses as
//! [`ControlKind::Cancel`] applied at the deadline; the leaf removes the request from its queue or its
//! batch and reports a `TimeoutQueued` or `TimeoutRunning` record in `completed`, and Ingress runs the
//! retry logic when that record comes back. A probe (`RouteContext::probe`) reads live replica state
//! at the router's `now`, so across the seam it is a query the leaf can only answer once it stands at
//! exactly that instant: the leaf is advanced to `now`, answers, and the next window starts there. In
//! process that is a direct read; as a process it costs a barrier per probe, which is the price the
//! modelled round trip already charges in simulated time.
//!
//! **The barrier window.** The loop runs windows of `(advanced_to, advance_until]`. Ingress dispatches
//! its own events up to a horizon `H`, turning each placement into work with `deliver_at = now +
//! delay`, then calls `advance` on every shard with `advance_until = H` and the work due inside the
//! window. `H` may be at most `ingress_now + lookahead`, where lookahead is the minimum
//! Ingress-to-Leaf latency (10.5), because that is what proves no work can land before `H` that
//! Ingress has not yet sent. Today the modelled dispatch delay is `probes * PROBE_COST`, which is zero
//! for an unprobed placement, so the lookahead is zero and every window would hold one event: the
//! split needs a scenario-level minimum dispatch latency before it is worth having, and that knob is
//! an M3 decision rather than something this crate assumes. Each shard replies with
//! `next_event_unix_ns`; Ingress takes the minimum with its own next event to size the next window,
//! so idle stretches cost one barrier rather than one per lookahead.
//!
//! **Ordering, and what will move.** Today ties at one instant break by insertion sequence in a single
//! queue. After the split the total order is `(time, shard_id, sequence_within_shard)` per 10.5, and
//! work delivered into a leaf window is scheduled into the leaf's queue in the order Ingress sent it.
//! An `Admit` at the same instant as a `Step` of the same replica can therefore change places
//! relative to today, which moves the fingerprint. That is a one-time, explained golden update in the
//! commit that performs the split, not a silent drift: the fingerprint must be identical across
//! *shard counts* from then on, which is the test 10.5 asks for, and it need not be identical to the
//! pre-split loop.

use sim_core::Nanos;
use sim_metrics::{Frame, RequestRecord};
use sim_policy::ReplicaView;
use sim_scenario::Scenario;

/// The Leaf service of `leaf.proto`. One implementor per transport; the shard is the unit.
pub trait Leaf {
    /// `Configure`. A rejected scenario comes back as `accepted: false` with the reason, as on the
    /// wire; `Err` is for the transport itself failing.
    fn configure(
        &mut self,
        shard_id: u32,
        sc: &Scenario,
        replica_ids: &[u64],
        shard_seed: u64,
    ) -> Result<ConfigureShardResponse, String>;

    /// `Advance`: the synchronisation primitive. Apply the work and control actions at their own
    /// instants, advance no further than `advance_until_unix_ns`, and report what happened.
    fn advance(&mut self, req: AdvanceRequest) -> Result<AdvanceResponse, String>;

    /// `Snapshot`: the shard's state at `at`, opaque to Ingress. May be `Err("not implemented")`
    /// until rewind lands.
    fn snapshot(&self, at: Nanos) -> Result<Vec<u8>, String>;

    /// `Restore`: the inverse of `snapshot`.
    fn restore(&mut self, at: Nanos, state: &[u8]) -> Result<(), String>;
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct ConfigureShardResponse {
    pub accepted: bool,
    pub rejected_reason: String,
    pub next_event_unix_ns: Nanos,
}

#[derive(Clone, Debug, Default)]
pub struct AdvanceRequest {
    pub shard_id: u32,
    /// Advance no further than this. Ingress guarantees no message will arrive before it.
    pub advance_until_unix_ns: Nanos,
    /// Work whose delivery time falls inside the window. Each carries its own delivery instant and
    /// the shard applies it there, not at the window boundary.
    pub work: Vec<DispatchedWork>,
    /// Control-plane actions decided by global policy, each applied at its own instant.
    pub control: Vec<ControlAction>,
}

/// One routed request arriving at a replica. The observable fields a router may see are in
/// `request`; what only the physics may know is in `truth`, on this hop alone.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DispatchedWork {
    pub deliver_at_unix_ns: Nanos,
    pub replica_id: u64,
    pub request: ExecuteRequest,
    pub truth: Truth,
}

/// The observable part of a request, as `serving.proto`'s `ExecuteRequest` carries it.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct ExecuteRequest {
    pub id: u64,
    pub tenant_id: u64,
    pub prompt_tokens: u32,
    pub arrived_at_unix_ns: Nanos,
    pub deadline_unix_ns: Nanos,
    pub attempts: u32,
}

/// Facts the simulator knows and no policy may see. The leaf needs the true output length to
/// advance a sequence; nothing above the seam is allowed to.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Truth {
    pub id: u64,
    pub true_output_tokens: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ControlAction {
    pub apply_at_unix_ns: Nanos,
    pub kind: ControlKind,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ControlKind {
    /// A client gave up: remove the request wherever it is. The leaf reports how far it had got.
    Cancel { request_id: u64 },
    /// Take a replica out of routing, or put it back. The replica keeps running what it holds.
    Eject { replica_id: u64, eject: bool },
    /// Provisioning and draining land at the leaf because the leaf owns the replica's existence.
    Lifecycle { replica_id: u64, op: LifecycleOp },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LifecycleOp {
    BeginWarmup,
    MarkReady,
    BeginDrain,
    Terminate,
    InjectFailure,
}

#[derive(Clone, Debug, Default)]
pub struct AdvanceResponse {
    pub shard_id: u32,
    /// How far the shard actually got. Never beyond `advance_until_unix_ns`.
    pub advanced_to_unix_ns: Nanos,
    /// The earliest future event in this shard, or zero when there is none. Ingress takes the
    /// minimum across shards to size the next window.
    pub next_event_unix_ns: Nanos,
    /// Requests that finished, failed or were shed inside the window, with their timing. Ingress
    /// needs these to release per-request routing state and to feed client retry behaviour.
    pub completed: Vec<RequestRecord>,
    /// First-token instants, separately, because a request can live for minutes after its first
    /// token and the client is already measuring.
    pub first_tokens: Vec<FirstToken>,
    /// Control telemetry: every replica this shard owns, as it stood at `advanced_to_unix_ns`.
    /// Batched, so message count is O(shards) while bytes stay O(machines), which is the scaling
    /// under study and must not be optimised away.
    pub telemetry: Vec<ReplicaView>,
    /// Whether a publish instant fell inside the window, so Ingress knows to schedule delivery.
    pub telemetry_due: bool,
    /// Observability metrics: one [`Frame`] per sample instant inside the window, fixed size per
    /// shard whatever it holds. The in-process form of `SubscriptionUpdate`; Ingress merges.
    pub metrics: Vec<Frame>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FirstToken {
    pub request_id: u64,
    pub at_unix_ns: Nanos,
    pub cached_prefix_tokens: u32,
}
