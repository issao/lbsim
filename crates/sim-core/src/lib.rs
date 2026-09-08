//! The layer that knows nothing about language models: simulated time, the event queue, and the
//! named random streams. Everything above depends on this; this depends on nothing.

pub mod queue;
pub mod rng;
pub mod scheduling;

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
