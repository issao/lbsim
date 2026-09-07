//! Trace replay: a recorded workload must be reproduced exactly, and enabling the feature must not
//! move a single synthetic bit.
//!
//! The trace path is the one place the simulator takes real-world input, and its value is that a
//! recorded burst can be replayed under a different policy with nothing else changed. That only holds
//! if replay is exact: an arrival a nanosecond off, or a shape drawn from a distribution instead of
//! the row, and the comparison is against a different load.

mod common;

use common::*;
use lbsim::rng::Streams;
use lbsim::scenario::Scenario;
use lbsim::sim;
use lbsim::workload::Workload;
use lbsim::{Nanos, EPOCH_BASE};

const SAMPLE: &str = "scenarios/traces/sample.csv";

struct Row {
    t_ns: Nanos,
    prompt: u32,
    output: u32,
    tenant: u32,
}

/// Parsed independently of the crate under test, so the test does not share its bugs.
fn sample_rows() -> Vec<Row> {
    let text = std::fs::read_to_string(SAMPLE).expect("sample trace is missing");
    text.lines()
        .skip(1)
        .filter(|l| !l.trim().is_empty())
        .map(|l| {
            let f: Vec<&str> = l.split(',').collect();
            Row {
                t_ns: (f[0].parse::<f64>().unwrap() * 1e9).round() as Nanos,
                prompt: f[1].parse().unwrap(),
                output: f[2].parse().unwrap(),
                tenant: f[3].parse().unwrap(),
            }
        })
        .collect()
}

fn trace_scenario() -> Scenario {
    let mut s = small();
    s.tenants = 3;
    s.workload = "trace".into();
    s.trace_file = SAMPLE.into();
    s
}

#[test]
fn trace_mode_replays_rows_exactly() {
    let rows = sample_rows();
    assert!(rows.len() > 100, "sample trace is too small to be a useful fixture");
    assert!(rows.last().unwrap().t_ns < 30 * lbsim::SECOND, "sample must fit the 30 s fixture");

    // Drive the workload the way the leaf's arrival loop does: an unconditional first arrival at the
    // start, then each arrival schedules the next at `now + gap`.
    let sc = trace_scenario();
    let mut w = Workload::new(&Streams::new(sc.seed));
    let start = EPOCH_BASE;
    let end = start + (sc.duration_s * 1e9) as Nanos;
    let mut now = start;
    let mut got = Vec::new();
    while now <= end {
        let elapsed = (now - start) as f64 / 1e9;
        let req = w.make(&sc, now);
        got.push((now - start, req.prompt, req.output, req.tenant));
        let gap = w.next_gap_ns(&sc, elapsed);
        now += gap.max(1);
    }

    assert_eq!(got.len(), rows.len(), "arrival count differs from the row count");
    for (i, (row, g)) in rows.iter().zip(&got).enumerate() {
        assert_eq!(g.0, row.t_ns, "row {i}: arrival time is not the CSV time to the nanosecond");
        assert_eq!((g.1, g.2, g.3), (row.prompt, row.output, row.tenant), "row {i}: shape differs");
    }

    // Through the whole simulator too, so the leaf's loop and the workload agree. A record is written
    // at completion and carries no tenant, so what can be checked is that every completed request is
    // one of the rows, by time and prompt, and that most of the trace completed within the run.
    let r = sim::run(&sc).unwrap();
    assert_eq!(r.first_attempts as usize, rows.len(), "the run offered a different number of arrivals");
    let by_time: std::collections::BTreeMap<Nanos, u32> =
        rows.iter().map(|row| (row.t_ns, row.prompt)).collect();
    assert!(r.records.len() * 2 > rows.len(), "only {} of {} rows completed", r.records.len(), rows.len());
    for rec in &r.records {
        let prompt = by_time.get(&(rec.arrived_at - start));
        assert_eq!(prompt, Some(&rec.prompt_tokens), "record {} is not a trace row", rec.id);
    }

    // Reproducible: the same run twice is the same run.
    assert_eq!(sim::run(&sc).unwrap().fingerprint, r.fingerprint, "a trace run is not reproducible");

    // And with a seedless router the seed is irrelevant: nothing on the workload side draws from it.
    let mut a = trace_scenario();
    a.routing = "round_robin".into();
    let mut b = a.clone();
    b.seed = a.seed + 1;
    assert_eq!(
        sim::run(&a).unwrap().fingerprint,
        sim::run(&b).unwrap().fingerprint,
        "a trace run under a seedless router still depends on the seed"
    );
}

/// The synthetic fingerprint measured before the `workload` and `trace_file` keys existed. Adding a
/// mode must not touch the random streams of the mode that was already there.
const SMALL_FINGERPRINT_BEFORE: u64 = 13602603559065206507;

#[test]
fn synthetic_mode_is_byte_identical_with_the_new_keys() {
    let r = sim::run(&small()).unwrap();
    assert_eq!(r.fingerprint, SMALL_FINGERPRINT_BEFORE, "the default workload moved");

    // Naming the mode, and even pointing at a trace file without selecting trace mode, changes nothing.
    let mut explicit = small();
    explicit.workload = "synthetic".into();
    explicit.trace_file = SAMPLE.into();
    assert_eq!(sim::run(&explicit).unwrap().fingerprint, SMALL_FINGERPRINT_BEFORE);
}

/// R3 finding on U35: rows are not required to arrive sorted, and anchoring on row 0's time
/// underflows (Nanos is u64) if an earlier row follows it. Input here is exactly that: `5.0` before
/// `4.0`. The loader must sort before anchoring, so the earlier row replays first, at offset 0.
#[test]
fn unsorted_rows_are_replayed_in_time_order() {
    let path = std::env::temp_dir().join(format!("lbsim_unsorted_trace_{}.csv", std::process::id()));
    std::fs::write(&path, "t_s,prompt_tokens,output_tokens,tenant\n5.0,100,50,0\n4.0,200,60,0\n")
        .expect("failed to write the scratch trace file");

    let mut sc = small();
    sc.workload = "trace".into();
    sc.trace_file = path.to_str().unwrap().into();

    let mut w = Workload::new(&Streams::new(sc.seed));
    let start = EPOCH_BASE;
    let mut now = start;
    let mut got = Vec::new();
    for _ in 0..2 {
        let elapsed = (now - start) as f64 / 1e9;
        let req = w.make(&sc, now);
        got.push((now - start, req.prompt));
        let gap = w.next_gap_ns(&sc, elapsed);
        now += gap.max(1);
    }
    let _ = std::fs::remove_file(&path);

    assert_eq!(got.len(), 2, "both rows must be replayed");
    assert_eq!(got[0], (0, 200), "the 4.0s row is earlier and must be replayed first, at offset 0");
    assert_eq!(got[1], (lbsim::SECOND, 100), "the 5.0s row follows one second behind the first");
}

/// R3 finding on U35: `Frame.offered_rps` was `Workload::rate_at`, the synthetic arrival rate, even
/// in trace mode, where there is no such rate — the dashboard's offered-load curve was fiction. It
/// must instead reflect what the CSV recorded: high across sample.csv's 12-14s burst, and exactly
/// zero across the gap from 19.70s to 20.77s.
#[test]
fn offered_rps_in_trace_mode_follows_the_rows() {
    let sc = trace_scenario();
    let r = sim::run(&sc).unwrap();
    assert!(!r.frames.is_empty(), "no frames were sampled");
    let start = EPOCH_BASE;

    // sample.csv packs 64 of its ~195 rows into 12-14s: any window closing in that range must show a
    // rate far above the trace's overall average of about 6.5 rows/s, never the synthetic scenario's
    // arrival_rps.
    let burst_max = r
        .frames
        .iter()
        .filter(|f| {
            let elapsed = f.t - start;
            elapsed > 12 * lbsim::SECOND && elapsed <= 14 * lbsim::SECOND
        })
        .map(|f| f.offered_rps)
        .fold(0.0_f64, f64::max);
    assert!(burst_max > 20.0, "the 12-14s burst does not show up in offered_rps (max was {burst_max})");

    // The window closing exactly at 20.0s falls entirely inside that gap, so it has no rows and must
    // show zero, not the synthetic default rate.
    let quiet = r
        .frames
        .iter()
        .find(|f| f.t - start == 20 * lbsim::SECOND)
        .unwrap_or_else(|| panic!("no frame closes at exactly 20s (sample interval assumption changed)"));
    assert_eq!(quiet.offered_rps, 0.0, "a window with no rows must show zero offered load");
}
