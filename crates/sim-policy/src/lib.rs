//! Policies, and the two seams they plug into.
//!
//! `docs/ARCHITECTURE.md` section 5: a policy is a pure function from a **stale observation** to a
//! decision. Every policy here sees only [`ReplicaView`], a *delayed* snapshot, and [`RequestView`],
//! which carries what a router can know about a request and never its true output length. That is
//! structural rather than a convention: a policy has no access to live replica state, so the staleness
//! dynamics in section 6 are a property of the architecture. The one exception is an explicit probe,
//! [`RouteContext::probe`], which pays a modelled round trip so the cost of freshness is visible.
//!
//! **Adding a policy is one file.** Write `src/<name>.rs` implementing [`RoutingPolicy`] or
//! [`AdmissionPolicy`], with a first line declaring it:
//!
//! ```text
//! //! lbsim-policy: routing names=<canonical>,<alias>...
//! ```
//!
//! `build.rs` reads that header from every file in `src/` and generates the `mod` lines and the
//! [`ROUTING`] and [`ADMISSION`] tables, sorted by file name. Nothing in the engine changes; the engine
//! resolves the scenario's `routing` and `admission` names through [`make_routing`] and
//! [`make_admission`]. The registry used to be a hand-written table here, and three policy branches
//! written in parallel conflicted on it in one afternoon; a generated table cannot conflict. This is
//! also how the arena's generated policies work: a generated policy is a file with that header like any
//! other, and `PolicyEntry::file` is what a run records the source hash of.
//!
//! The measured constraint from section 10.4 applies to all of them: a decision must be O(1) or
//! O(log N), never a scan of the fleet. `least_requests` and `least_queue_tokens` violate that
//! deliberately, because they are the baselines whose cost and behaviour are the point, and
//! [`RoutingPolicy::inspected`] is how the violation is reported rather than hidden.


pub mod admission;
pub mod routing;

pub use admission::{Admission, AdmissionContext, AdmissionPolicy};
pub use routing::{ReplicaView, RequestView, RouteContext, RoutingPolicy};

use sim_scenario::Scenario;

/// One registered policy. `names` are what a scenario's `routing =` or `admission =` line may say;
/// the first is canonical. `file` is the source file, recorded so a run can hash what decided it.
pub struct PolicyEntry<T: ?Sized> {
    pub names: &'static [&'static str],
    pub file: &'static str,
    pub make: fn(&Scenario) -> Box<T>,
}

include!(concat!(env!("OUT_DIR"), "/registry.rs"));

fn lookup<'a, T: ?Sized>(
    table: &'a [PolicyEntry<T>],
    name: &str,
) -> Option<&'a PolicyEntry<T>> {
    table.iter().find(|e| e.names.contains(&name))
}

/// Resolve the scenario's `routing` name. An unknown name is an error that names it, never a silent
/// fallback to whatever is first in the table.
pub fn make_routing(sc: &Scenario) -> Result<Box<dyn RoutingPolicy>, String> {
    match lookup(ROUTING, sc.routing.as_str()) {
        Some(e) => Ok((e.make)(sc)),
        None => Err(format!("unknown routing policy {:?}", sc.routing)),
    }
}

/// Resolve the scenario's `admission` name.
pub fn make_admission(sc: &Scenario) -> Result<Box<dyn AdmissionPolicy>, String> {
    match lookup(ADMISSION, sc.admission.as_str()) {
        Some(e) => Ok((e.make)(sc)),
        None => Err(format!("unknown admission policy {:?}", sc.admission)),
    }
}

/// Canonical names of every routing policy, for the arena and the CLI.
pub fn routing_names() -> Vec<&'static str> {
    ROUTING.iter().map(|e| e.names[0]).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_registered_file_exists_and_every_name_is_unique() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut seen = std::collections::BTreeSet::new();
        for e in ROUTING {
            assert!(dir.join(e.file).is_file(), "{} is registered but missing", e.file);
            for n in e.names {
                assert!(seen.insert(*n), "routing name {n} registered twice");
            }
        }
        let mut seen = std::collections::BTreeSet::new();
        for e in ADMISSION {
            assert!(dir.join(e.file).is_file(), "{} is registered but missing", e.file);
            for n in e.names {
                assert!(seen.insert(*n), "admission name {n} registered twice");
            }
        }
    }

    #[test]
    fn unknown_names_are_errors_that_name_the_offender() {
        let mut sc = Scenario::default();
        sc.routing = "least_loaded".into();
        let err = make_routing(&sc).err().expect("unknown routing name must be an error");
        assert!(err.contains("least_loaded"), "{err}");
        sc.routing = "p2c".into();
        sc.admission = "bouncer".into();
        let err = make_admission(&sc).err().expect("unknown admission name must be an error");
        assert!(err.contains("bouncer"), "{err}");
    }
}
