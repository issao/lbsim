//! A discrete-event simulator for a cloud LLM inference service.
//!
//! Scope today is `docs/scope-today.md` package B: the queueing and control dynamics plus two-phase
//! request timing, so time-to-first-token and inter-token latency are separate observables. KV
//! capacity, preemption, prefix caching, tiering and autoscaling are deliberately absent; see that
//! document for why each was cut.
//!
//! Deviation from `docs/ARCHITECTURE.md` section 10.8 worth naming: that specifies a workspace of
//! several crates with an enforced dependency direction. This is one crate with modules, because
//! today's constraint is a deadline and the split is mechanical to do later. The module boundaries
//! match the eventual crate boundaries so that split stays cheap.

pub mod metrics;
pub mod policy;
pub mod queue;
pub mod report;
pub mod rng;
pub mod scenario;
pub mod sim;
pub mod workload;

/// Simulated time, absolute Unix epoch nanoseconds, per `proto/lbsim/v1/common.proto`.
///
/// Never a float. 2026 is about 1.79e18 nanoseconds since the epoch and an `f64` resolves only
/// ~200 ns at that magnitude, so absolute epoch time makes `u64` mandatory rather than preferable.
pub type Nanos = u64;

/// The origin of simulated time, derived from the seed so runs are reproducible while timestamps
/// still format like real ones. 2026-01-01T00:00:00Z.
pub const EPOCH_BASE: Nanos = 1_767_225_600_000_000_000;

pub const SECOND: Nanos = 1_000_000_000;
pub const MILLI: Nanos = 1_000_000;
