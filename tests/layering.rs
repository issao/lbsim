//! The crate graph is the layer diagram, and this test is what keeps it one.
//!
//! `docs/ARCHITECTURE.md` section 10.8 fixes a strictly downward dependency direction and names two
//! invariants worth enforcing in CI rather than in review: `sim-policy` cannot reach mutable simulation
//! state, and `sim-ingress` cannot depend on `sim-model` or `sim-physics`, because "if it can compute
//! replica physics, someone eventually will, and the layer boundary Issao specified will erode."
//!
//! Rust already makes a crate unable to name types from crates it does not depend on, so the direct
//! dependency list in each `Cargo.toml` is the whole boundary. This test reads those lists and compares
//! them against the table below. A new crate, or a new edge, is a deliberate edit to the table, which
//! is the point: the graph changes in a reviewable diff, never by accident.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

/// Every crate and the direct dependencies it may have. Order is top-down within each entry only for
/// readability; the check is set-based.
const ALLOWED: &[(&str, &[&str])] = &[
    ("sim-core", &[]),
    ("sim-physics", &["sim-core"]),
    ("sim-scenario", &["sim-physics"]),
    ("sim-metrics", &["sim-core"]),
    ("sim-workload", &["sim-core", "sim-scenario"]),
    ("sim-policy", &["sim-core", "sim-scenario"]),
    ("sim-model", &["sim-core", "sim-scenario", "sim-physics", "sim-workload"]),
    (
        "sim-leaf",
        &["sim-core", "sim-scenario", "sim-physics", "sim-metrics", "sim-workload", "sim-policy", "sim-model"],
    ),
    ("sim-report", &["sim-core", "sim-scenario", "sim-metrics", "sim-leaf"]),
    ("sim-arena", &["sim-core", "sim-scenario", "sim-metrics", "sim-workload", "sim-leaf"]),
    // Ingress drives runs through the leaf and speaks the Frontend API. It never sees replica physics.
    ("sim-ingress", &["sim-core", "sim-scenario", "sim-metrics", "sim-policy", "sim-leaf"]),
    ("sim-run", &["sim-core", "sim-scenario", "sim-leaf", "sim-report", "sim-arena", "sim-ingress"]),
];

/// Crates that must never appear, directly, below `sim-ingress`. Section 10.8's invariant, verbatim.
const INGRESS_MAY_NOT_REACH: &[&str] = &["sim-model", "sim-physics"];

fn workspace_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

/// Direct dependencies from a manifest, by the crate name in `[dependencies]`. A dependency of any
/// kind counts: a dev-dependency or a build-dependency is still a way to reach the crate's types.
fn direct_deps(manifest: &str) -> Vec<String> {
    let mut deps = Vec::new();
    let mut in_deps = false;
    for raw in manifest.lines() {
        let line = raw.trim();
        if line.starts_with('[') {
            in_deps = line.contains("dependencies]");
            continue;
        }
        if !in_deps || line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some((name, _)) = line.split_once('=') {
            deps.push(name.trim().trim_matches('"').to_string());
        }
    }
    deps
}

fn crates_on_disk() -> BTreeMap<String, Vec<String>> {
    let dir = workspace_root().join("crates");
    let mut out = BTreeMap::new();
    for entry in fs::read_dir(&dir).expect("crates/ exists") {
        let entry = entry.unwrap();
        let manifest = entry.path().join("Cargo.toml");
        if !manifest.is_file() {
            continue;
        }
        let text = fs::read_to_string(&manifest).unwrap();
        let name = text
            .lines()
            .find_map(|l| l.trim().strip_prefix("name = ").map(|v| v.trim_matches('"').to_string()))
            .expect("manifest has a name");
        out.insert(name, direct_deps(&text));
    }
    out
}

#[test]
fn every_crate_depends_only_on_the_layers_below_it() {
    let on_disk = crates_on_disk();
    let mut failures = Vec::new();
    for (name, deps) in &on_disk {
        let Some((_, allowed)) = ALLOWED.iter().find(|(n, _)| n == name) else {
            failures.push(format!("{name}: not in the layering table; add it deliberately"));
            continue;
        };
        for d in deps {
            if !allowed.contains(&d.as_str()) {
                failures.push(format!("{name} -> {d}: edge is not in the layering table"));
            }
        }
        // Nothing depends on the facade. The facade is the top, and a crate that reached it could reach
        // everything.
        if deps.iter().any(|d| d == "lbsim") {
            failures.push(format!("{name} depends on the facade crate"));
        }
    }
    for (name, _) in ALLOWED {
        if !on_disk.contains_key(*name) && *name != "sim-model" {
            failures.push(format!("{name}: in the layering table but not on disk"));
        }
    }
    assert!(failures.is_empty(), "layering violations:\n  {}", failures.join("\n  "));
}

#[test]
fn ingress_cannot_reach_replica_physics() {
    let on_disk = crates_on_disk();
    let deps = on_disk.get("sim-ingress").expect("sim-ingress exists");
    for forbidden in INGRESS_MAY_NOT_REACH {
        assert!(
            !deps.iter().any(|d| d == forbidden),
            "sim-ingress depends on {forbidden}; docs/ARCHITECTURE.md 10.8 forbids it"
        );
    }
    // Belt and braces: the identifiers must not appear in its source either, so a re-export through
    // some intermediate crate cannot smuggle them in.
    let src = workspace_root().join("crates/sim-ingress/src");
    for entry in fs::read_dir(&src).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().and_then(|e| e.to_str()) != Some("rs") {
            continue;
        }
        let text = fs::read_to_string(&path).unwrap();
        for ident in ["sim_model", "sim_physics", "CostModel"] {
            assert!(
                !text.contains(ident),
                "{} mentions {ident}; ingress must not compute replica physics",
                path.display()
            );
        }
    }
}

#[test]
fn the_config_crate_does_not_re_export_physics() {
    // sim-scenario depends on sim-physics for one derived number, rated capacity. It must not become a
    // path by which anything above it reaches the cost model.
    let text = fs::read_to_string(workspace_root().join("crates/sim-scenario/src/lib.rs")).unwrap();
    assert!(!text.contains("pub use sim_physics"), "sim-scenario re-exports sim-physics");
}
