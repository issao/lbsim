//! A discrete-event simulator for a cloud LLM inference service.
//!
//! This crate is a facade. The simulator is a workspace of crates under `crates/`, one per layer of
//! `docs/ARCHITECTURE.md` section 10.8, with a strictly downward dependency direction that
//! `tests/layering.rs` enforces. This crate re-exports each of them under the module name the original
//! single crate used, so `lbsim::sim::run`, `lbsim::scenario::Scenario` and the rest keep resolving
//! for the integration tests in `tests/` and for any external caller.
//!
//! Nothing lives here. A type or function that needs a home goes in the crate that owns its layer.

pub use sim_core::{queue, rng, Nanos, EPOCH_BASE, MILLI, SECOND};

pub use sim_arena as arena;
pub use sim_leaf as sim;
pub use sim_leaf_api as leaf_api;
pub use sim_metrics as metrics;
pub use sim_model as model;
pub use sim_physics as physics;
pub use sim_policy as policy;
pub use sim_report as report;
pub use sim_scenario as scenario;
pub use sim_workload as workload;

/// The HTTP surface. Named `serve` for compatibility with the original module.
pub mod serve {
    pub use sim_ingress::serve;
}
