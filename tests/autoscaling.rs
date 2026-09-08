//! Autoscaling with a cold start: dynamic 12's single-cluster half.
//!
//! The fleet is not a constant. A controller reads the delayed views and turns replicas up and down;
//! a turned-up replica serves nothing for `warmup_delay_s`, a turned-down one takes nothing new and
//! finishes what it has. These tests pin the lifecycle rather than any policy's judgement: that a
//! scenario without an autoscaler is the run it always was, that a saturated fleet grows through
//! WARMING into READY and never past `max_replicas`, that an idle one drains down to `min_replicas`
//! and never below, that nothing is ever routed to a slot that is not ready, and that the fleet row on
//! the wire counts what the frame shows.

mod common;

use common::*;
use lbsim::metrics::Frame;
use lbsim::scenario::Scenario;
use lbsim::sim;
use lbsim::EPOCH_BASE;
use sim_ingress::{export, wire};

/// `small()`'s four replicas with room to grow to eight, deciding every 2 s, warming in 3 s.
fn elastic() -> Scenario {
    let mut s = small();
    s.replicas = 4;
    s.min_replicas = 2;
    s.max_replicas = 8;
    s.autoscaling = "target_utilization".into();
    s.autoscale_target = 0.7;
    s.autoscale_interval_s = 2.0;
    s.autoscale_step = 8;
    s.autoscale_cooldown_s = 4.0;
    s.warmup_delay_s = 3.0;
    s.drain_timeout_s = 10.0;
    s.duration_s = 60.0;
    s.warmup_s = 3.0;
    s
}

fn secs(f: &Frame) -> f64 {
    (f.t - EPOCH_BASE) as f64 / 1e9
}

fn count(f: &Frame, state: u8) -> usize {
    f.replicas.iter().filter(|r| r.state == state).count()
}

/// (a) With `autoscaling = none` the engine schedules no tick and builds exactly `replicas` slots, so
/// the run is byte-identical to the one before the seam existed. Pinned against `origin/master`
/// before any engine change.
#[test]
fn no_autoscaling_is_byte_identical() {
    let sc = small();
    assert_eq!(sc.autoscaling, "none");
    let r = sim::run(&sc).unwrap();
    assert_eq!(r.fingerprint, 13602603559065206507, "the replica loop changed with no autoscaler configured");
    assert_eq!(r.events, 17777, "the event stream changed with no autoscaler configured");
    assert!(r.frames.iter().all(|f| f.replicas.len() == 8 && count(f, 1) == 8));
}

/// (b) Saturated: after the first decision the frame shows warming slots, after the cold start the
/// ready count has grown, and the fleet never exceeds `max_replicas`.
#[test]
fn a_saturated_fleet_warms_then_grows_and_never_past_max() {
    let mut sc = elastic();
    sc.arrival_rps = 3.0 * sc.rated_rps(); // rated is for 4 replicas; this saturates 8
    let r = sim::run(&sc).unwrap();
    assert!(r.frames.iter().all(|f| f.replicas.len() == 8), "every slot exists from the start");
    let first_warm = r.frames.iter().find(|f| count(f, 4) > 0).expect("a warming slot after the first decisions");
    // Two intervals, not one: the first decision reads views sampled before the batches had filled,
    // which is the controller's lag and part of what the seam models.
    assert!(secs(first_warm) <= 2.0 * sc.autoscale_interval_s + 0.5, "first warming at {} s", secs(first_warm));
    assert_eq!(count(first_warm, 1) + count(first_warm, 2), 4, "the cold start adds no capacity yet");
    let grown = r
        .frames
        .iter()
        .find(|f| count(f, 1) > 4)
        .expect("the ready count grows once the cold start ends");
    assert!(
        secs(grown) >= secs(first_warm) + sc.warmup_delay_s - 0.5,
        "ready grew at {} s, warming began at {} s: no cold start was paid",
        secs(grown),
        secs(first_warm)
    );
    for f in &r.frames {
        let up = count(f, 1) + count(f, 2) + count(f, 3) + count(f, 4);
        assert!(up <= 8, "{} replicas ready or warming at {} s", up, secs(f));
    }
    assert_eq!(count(r.frames.last().unwrap(), 1), 8, "a saturated fleet ends at max_replicas");
}

/// (c) Load removed: draining begins within a cooldown plus an interval, then the ready count falls to
/// `min_replicas` and never below it.
#[test]
fn an_idle_fleet_drains_down_to_min_and_never_below() {
    let mut sc = elastic();
    sc.arrival_rps = 0.02 * sc.rated_rps();
    // Nothing to speak of arrives after 20 s.
    sc.load_step_at_s = 20.0;
    sc.load_step_factor = 0.001;
    sc.load_step_until_s = sc.duration_s;
    let r = sim::run(&sc).unwrap();
    let first_drain = r
        .frames
        .iter()
        .find(|f| count(f, 5) > 0 || count(f, 1) < 4)
        .expect("a draining slot, or one already gone");
    assert!(
        secs(first_drain) <= sc.autoscale_cooldown_s + sc.autoscale_interval_s + 0.5,
        "first drain at {} s",
        secs(first_drain)
    );
    for f in &r.frames {
        assert!(count(f, 1) + count(f, 2) + count(f, 4) >= 2, "{} ready at {} s, below min_replicas", count(f, 1), secs(f));
    }
    assert_eq!(count(r.frames.last().unwrap(), 1), 2, "an idle fleet ends at min_replicas");
    assert_eq!(count(r.frames.last().unwrap(), 0), 6);
}

/// (d) A slot that is absent or warming holds nothing, and a draining one only loses work: its load
/// never rises while it drains. Routing to any of them would break one of those two.
#[test]
fn a_warming_or_draining_replica_is_never_routed_to() {
    let mut sc = elastic();
    sc.arrival_rps = 3.0 * sc.rated_rps();
    sc.load_step_at_s = 25.0;
    sc.load_step_factor = 0.001;
    sc.load_step_until_s = sc.duration_s;
    let r = sim::run(&sc).unwrap();
    let mut saw_warming = false;
    let mut saw_draining = false;
    for (k, f) in r.frames.iter().enumerate() {
        for (i, rep) in f.replicas.iter().enumerate() {
            match rep.state {
                0 | 4 => {
                    saw_warming |= rep.state == 4;
                    assert_eq!(rep.queued + rep.running, 0, "slot {i} holds work in state {} at {} s", rep.state, secs(f));
                }
                5 => {
                    saw_draining = true;
                    if let Some(prev) = k.checked_sub(1).map(|p| &r.frames[p].replicas[i]) {
                        if prev.state == 5 {
                            assert!(
                                rep.queued + rep.running <= prev.queued + prev.running,
                                "slot {i} took work while draining at {} s",
                                secs(f)
                            );
                        }
                    }
                }
                _ => {}
            }
        }
    }
    assert!(saw_warming && saw_draining, "the fixture must exercise both states");
}

/// (e) The fleet row's 60, 61 and 62 are the frame's own counts.
#[test]
fn the_fleet_row_counts_ready_warming_and_draining_from_the_sample() {
    let mut sc = elastic();
    sc.arrival_rps = 3.0 * sc.rated_rps();
    sc.load_step_at_s = 25.0;
    sc.load_step_factor = 0.001;
    sc.load_step_until_s = sc.duration_s;
    let r = sim::run(&sc).unwrap();
    let rows = export::fleet_rows(&r);
    assert_eq!(rows.len(), r.frames.len());
    let value = |row: &wire::MetricRow, m: i32| row.values.iter().find(|(k, _)| *k == m).map(|(_, v)| *v);
    let mut seen = [false; 3];
    for (u, f) in rows.iter().zip(&r.frames) {
        let ready = (count(f, 1) + count(f, 2)) as f64;
        assert_eq!(value(&u.row, wire::METRIC_READY_REPLICAS), Some(ready));
        assert_eq!(value(&u.row, wire::METRIC_WARMING_REPLICAS), Some(count(f, 4) as f64));
        assert_eq!(value(&u.row, wire::METRIC_DRAINING_REPLICAS), Some(count(f, 5) as f64));
        seen[0] |= ready < 8.0;
        seen[1] |= count(f, 4) > 0;
        seen[2] |= count(f, 5) > 0;
    }
    assert_eq!(seen, [true; 3], "the fixture must show all three counts moving");
}

/// A fleet that is not asked to move builds no extra slots and rejects bounds that cannot hold it.
#[test]
fn bounds_are_validated_before_anything_runs() {
    let mut sc = small();
    sc.max_replicas = 4;
    let err = sim::run(&sc).err().expect("max_replicas below replicas must be an error");
    assert!(err.contains("max_replicas"), "{err}");
    let mut sc = small();
    sc.min_replicas = 12;
    sc.max_replicas = 10;
    let err = sim::run(&sc).err().expect("min above max must be an error");
    assert!(err.contains("min_replicas"), "{err}");
    let mut sc = small();
    sc.autoscaling = "predictive".into();
    let err = sim::run(&sc).err().expect("an unknown autoscaling name must be an error");
    assert!(err.contains("predictive"), "{err}");
}
