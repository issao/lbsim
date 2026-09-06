//! Frames: the per-sample view of a run.
//!
//! The run-wide series and histograms answer "how did the run go". A frame answers "what was
//! happening in this interval", which is what a live dashboard and the Leaf-to-Ingress metrics flow
//! need. These tests pin the bookkeeping: a frame per sample, window counts that add up to the run's
//! totals, per-replica samples that add up to the fleet gauges, and a sparse histogram that answers
//! exactly as the dense one it was frozen from.

mod common;

use lbsim::metrics::Histogram;
use lbsim::scenario::Scenario;
use lbsim::sim;

/// Loaded enough to produce every outcome, and with no warmup and a sample interval that divides the
/// duration, so every record falls inside some frame's window and the run's totals are the frames'.
fn fixture() -> Scenario {
    let mut s = common::at_load(1.4);
    s.warmup_s = 0.0;
    s.sample_interval_ms = 250.0;
    s.duration_s = 30.0;
    s
}

#[test]
fn frames_cover_every_sample_instant() {
    let r = sim::run(&fixture()).unwrap();
    assert!(!r.frames.is_empty(), "a 30 s run at 4 samples/s produced no frames");
    assert_eq!(r.frames.len(), r.fleet_queue.t.len(), "one frame per sample");
    for (f, t) in r.frames.iter().zip(r.fleet_queue.t.iter()) {
        assert_eq!(f.t, *t, "frame instants must be the sample instants");
    }
    for (f, v) in r.frames.iter().zip(r.offered_rps.v.iter()) {
        assert_eq!(f.offered_rps, *v);
    }
}

/// Windows partition the run by finish time; records are selected by arrival time. With no warmup
/// the two selections coincide, and the counts must then agree exactly.
#[test]
fn window_counts_sum_to_the_run_totals() {
    let sc = fixture();
    let r = sim::run(&sc).unwrap();
    let sum = |f: fn(&lbsim::metrics::Frame) -> u64| r.frames.iter().map(f).sum::<u64>();

    let successes = r.records.iter().filter(|x| x.outcome.is_success()).count() as u64;
    assert_eq!(sum(|f| f.completed), successes);
    assert_eq!(sum(|f| f.within_slo), r.outcome("ok"));
    assert_eq!(sum(|f| f.rejected), r.outcome("rejected"));
    assert_eq!(sum(|f| f.timed_out), r.outcome("timeout_queued") + r.outcome("timeout_running"));
    assert_eq!(sum(|f| f.output_tokens), r.records.iter().map(|x| x.output_tokens as u64).sum::<u64>());
    assert_eq!(
        sum(|f| f.goodput_tokens),
        r.records
            .iter()
            .filter(|x| x.outcome == lbsim::metrics::Outcome::Ok)
            .map(|x| x.output_tokens as u64)
            .sum::<u64>()
    );
    // Every request that was neither shed by admission nor shed at a full queue entered a queue, and
    // either finished, so it has a record, or is still queued or running at the final sample, which
    // is the last event of the run and so sees exactly the requests that never got a record.
    let entered = r.records.iter().filter(|x| x.outcome != lbsim::metrics::Outcome::Rejected).count() as u64;
    let last = r.frames.last().unwrap();
    let in_flight: u64 = last.replicas.iter().map(|x| (x.queued + x.running) as u64).sum();
    assert_eq!(sum(|f| f.admitted), entered + in_flight);

    // The histograms are the run-wide ones split by window: same sample counts in total.
    let ttft: u64 = r.frames.iter().map(|f| f.ttft.count()).sum();
    assert_eq!(ttft, r.ttft.count());
    let e2e: u64 = r.frames.iter().map(|f| f.e2e.count()).sum();
    assert_eq!(e2e, r.e2e.count());
    let itl: u64 = r.frames.iter().map(|f| f.itl_max.count()).sum();
    assert_eq!(itl, r.itl_max.count());
    assert!(successes > 0, "fixture produced no completions; the identities above are vacuous");
}

#[test]
fn per_replica_samples_sum_to_fleet_series() {
    let sc = fixture();
    let r = sim::run(&sc).unwrap();
    for (i, f) in r.frames.iter().enumerate() {
        assert_eq!(f.replicas.len(), sc.replicas);
        let queued: u32 = f.replicas.iter().map(|x| x.queued).sum();
        let running: u32 = f.replicas.iter().map(|x| x.running).sum();
        assert_eq!(queued as f64, r.fleet_queue.v[i], "frame {i}: queued");
        assert_eq!(running as f64, r.fleet_running.v[i], "frame {i}: running");
        assert_eq!((queued + running) as f64, r.fleet_queue.v[i] + r.fleet_running.v[i]);
        for (k, x) in f.replicas.iter().enumerate() {
            assert_eq!((x.queued + x.running) as f64, r.replica_load[k].v[i], "frame {i} replica {k}");
        }
    }
}

/// A sparse histogram is the dense one with the zeros removed, so every answer must be bit-identical,
/// including the edge cases: an empty histogram, a single value, values below the linear range and
/// values in the saturated top bucket.
#[test]
fn sparse_histogram_answers_like_the_dense_one() {
    let mut dense = Histogram::new();
    // A deterministic spread across every regime of the bucket layout, without a dependency on the
    // simulator's RNG: an LCG over 64 bits, shifted so values span from 0 to well past 2^48.
    let mut x: u64 = 0x9E37_79B9_7F4A_7C15;
    let mut values = Vec::new();
    for i in 0..20_000u64 {
        x = x.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        let shift = (i % 60) as u32;
        let v = x >> shift;
        values.push(v);
        dense.record(v);
    }
    let sparse = dense.to_sparse();

    assert_eq!(sparse.count(), dense.count());
    assert_eq!(sparse.min(), dense.min());
    assert_eq!(sparse.max(), dense.max());
    assert_eq!(sparse.mean(), dense.mean());
    assert!(sparse.pairs.windows(2).all(|w| w[0].0 < w[1].0), "pairs must be in bucket order");
    assert!(sparse.pairs.iter().all(|(_, c)| *c > 0), "a sparse histogram carries no empty buckets");

    let mut q = 0.0;
    while q <= 100.0 {
        assert_eq!(sparse.percentile(q), dense.percentile(q), "p{q}");
        q += 0.05;
    }
    for v in values.iter().step_by(7).chain([0, 1, 63, 64, 65, u64::MAX].iter()) {
        assert_eq!(sparse.fraction_below(*v), dense.fraction_below(*v), "fraction_below({v})");
    }

    let empty = Histogram::new().to_sparse();
    assert_eq!(empty.count(), 0);
    assert_eq!(empty.percentile(50.0), 0);
    assert!(empty.fraction_below(1).is_nan());
    assert_eq!(empty.min(), Histogram::new().min());
    assert!(empty.pairs.is_empty());

    let mut one = Histogram::new();
    one.record(4_200);
    let one_s = one.to_sparse();
    assert_eq!(one_s.pairs.len(), 1);
    assert_eq!(one_s.percentile(0.0), one.percentile(0.0));
    assert_eq!(one_s.percentile(100.0), one.percentile(100.0));
    assert_eq!(one_s.fraction_below(4_199), one.fraction_below(4_199));
    assert_eq!(one_s.fraction_below(4_200), one.fraction_below(4_200));
}
