//! Shared scenario fixtures for the integration tests.
//!
//! Everything here is deliberately tiny: 8 replicas and tens of simulated seconds. The properties
//! under test are structural, not statistical, so a big scenario would buy nothing but wall time.

#![allow(dead_code)]

use lbsim::metrics::RequestRecord;
use lbsim::scenario::Scenario;
use lbsim::sim::RunResult;
use lbsim::{Nanos, EPOCH_BASE};
use std::collections::BTreeMap;
use std::fs;
use std::ops::Deref;
use std::path::{Path, PathBuf};

/// A scratch directory under `/tmp` that removes itself (`remove_dir_all`, errors ignored) when
/// dropped, so a passing test leaves nothing behind. A failing test skips the removal —
/// `std::thread::panicking()` is true while this guard's `Drop` runs during a panic's unwind — so
/// its directory survives for inspection. `Deref<Target = Path>` lets call sites read `&dir`
/// exactly as they did when this was a bare `PathBuf`.
pub struct ScratchDir {
    pub path: PathBuf,
}

impl Deref for ScratchDir {
    type Target = Path;
    fn deref(&self) -> &Path {
        &self.path
    }
}

impl AsRef<Path> for ScratchDir {
    fn as_ref(&self) -> &Path {
        &self.path
    }
}

impl Drop for ScratchDir {
    fn drop(&mut self) {
        if !std::thread::panicking() {
            let _ = fs::remove_dir_all(&self.path);
        }
    }
}

/// A fresh, created directory `/tmp/lbsim-<name>-<pid>`. Sweeps sibling `/tmp/lbsim-*` directories
/// older than an hour first, best effort, so leftovers from a killed run do not accumulate forever.
pub fn scratch(name: &str) -> ScratchDir {
    let tmp = std::env::temp_dir();
    sweep_stale_scratch_dirs(&tmp);
    let path = tmp.join(format!("lbsim-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&path);
    fs::create_dir_all(&path).unwrap();
    ScratchDir { path }
}

/// Best-effort sweep of leftovers from killed runs: any `lbsim-*` sibling directory (this test
/// binary's or another one's) whose mtime is more than an hour old. Errors — permissions, a race
/// with another process removing the same directory — are ignored; this is opportunistic
/// housekeeping, not a correctness requirement.
fn sweep_stale_scratch_dirs(tmp: &Path) {
    let cutoff = std::time::SystemTime::now()
        .checked_sub(std::time::Duration::from_secs(3600))
        .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
    let Ok(entries) = fs::read_dir(tmp) else { return };
    for entry in entries.flatten() {
        if !entry.file_name().to_string_lossy().starts_with("lbsim-") {
            continue;
        }
        let Ok(meta) = entry.metadata() else { continue };
        if !meta.is_dir() {
            continue;
        }
        if meta.modified().is_ok_and(|m| m < cutoff) {
            let _ = fs::remove_dir_all(entry.path());
        }
    }
}

#[cfg(test)]
mod scratch_dir_tests {
    use super::*;

    #[test]
    fn scratch_dir_is_created_and_removed_on_drop() {
        let path;
        {
            let dir = scratch("guard-basic");
            path = dir.path.clone();
            assert!(path.is_dir(), "scratch() must create the directory");
        }
        assert!(!path.exists(), "{path:?} must be gone after the guard drops");
    }

    #[test]
    fn a_panicking_scope_keeps_its_directory() {
        let dir = scratch("guard-panic");
        let path = dir.path.clone();
        let result = std::panic::catch_unwind(move || {
            let _dir = dir; // moved in so it drops mid-unwind, with panicking() true
            panic!("intentional, to exercise the panicking-drop path");
        });
        assert!(result.is_err());
        assert!(path.is_dir(), "a panicking scope should keep its scratch dir: {path:?}");
        let _ = fs::remove_dir_all(&path);
    }
}

/// The standard small fleet. 8 replicas, 30 simulated seconds, 3 s of warmup.
pub fn small() -> Scenario {
    let mut s = Scenario::default();
    s.name = "test_small".into();
    s.seed = 42;
    s.replicas = 8;
    s.max_batch = 16;
    s.duration_s = 30.0;
    s.warmup_s = 3.0;
    s.client_timeout_s = 8.0;
    s.routing = "p2c".into();
    // Pinned rather than derived from Scenario::default(): the fixture is 20 rps on 8 replicas, and
    // the fingerprints tests pin against it must not move when the demo default fleet does.
    s.arrival_rps = 20.0; // overwritten by most callers
    s
}

/// `small()` with the arrival rate set to a fraction of rated capacity, so the tests are expressed
/// in load units rather than in a magic rps that silently drifts when the cost model is recalibrated.
pub fn at_load(fraction: f64) -> Scenario {
    let mut s = small();
    s.arrival_rps = fraction * s.rated_rps();
    s
}

/// The last arrival time that is *guaranteed* to have produced a record.
///
/// A record only exists once a request terminates, and the run stops hard at `duration_s`. Any
/// request arriving later than `duration_s - client_timeout_s` may still be in flight when the run
/// ends and would then be missing from `records` — through no fault of the workload. Comparing
/// workloads across runs is only meaningful inside this window.
pub fn settled_window(s: &Scenario) -> (Nanos, Nanos) {
    let lo = EPOCH_BASE + (s.warmup_s * 1e9) as Nanos;
    let hi = EPOCH_BASE + ((s.duration_s - s.client_timeout_s) * 1e9) as Nanos;
    assert!(hi > lo, "settled window is empty; raise duration_s or lower client_timeout_s");
    (lo, hi)
}

pub fn settled<'a>(r: &'a RunResult, s: &Scenario) -> Vec<&'a RequestRecord> {
    let (lo, hi) = settled_window(s);
    r.records
        .iter()
        .filter(|x| x.arrived_at >= lo && x.arrived_at <= hi)
        .collect()
}

/// Multiset of request shapes, as generated by the workload.
pub fn shape_multiset(recs: &[&RequestRecord]) -> BTreeMap<(u32, u32), u32> {
    let mut m = BTreeMap::new();
    for r in recs {
        *m.entry((r.prompt_tokens, r.output_tokens)).or_insert(0) += 1;
    }
    m
}

pub fn prompt_multiset(recs: &[&RequestRecord]) -> BTreeMap<u32, u32> {
    let mut m = BTreeMap::new();
    for r in recs {
        *m.entry(r.prompt_tokens).or_insert(0) += 1;
    }
    m
}

pub fn arrival_times(recs: &[&RequestRecord]) -> Vec<Nanos> {
    let mut v: Vec<Nanos> = recs.iter().map(|r| r.arrived_at).collect();
    v.sort_unstable();
    v
}
