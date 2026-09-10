//! The trace engine: spans recorded for a seeded sample of requests.
//!
//! Four things are worth proving. Tracing changes nothing but the traces, because its sample is
//! drawn from its own stream. A traced request's spans reconstruct its journey exactly: the queue at
//! the gateway, the routing decision, the replica queue, then prefill chunks that add up to the
//! prompt and one decode step per output token, all in order and none overlapping. The sample is
//! deterministic and reaches every latency bucket, since the tail is what a trace is opened to see.
//! And the export writes the engine's spans rather than the fixture that stood in for them.

mod common;

use lbsim::metrics::trace::{fixtures, latency_of, SpanKind, TraceBucket};
use lbsim::scenario::Scenario;
use lbsim::sim::{self, RunResult};
use sim_ingress::{export, trace_wire};
use std::fs;
use std::path::Path;

fn p2c(rate: f64) -> Scenario {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("scenarios/route_p2c.txt");
    let mut s = Scenario::parse(&fs::read_to_string(&path).unwrap()).unwrap();
    s.trace_sample_rate = rate;
    s
}

/// The shared baseline, cut to a length the inner loop tolerates.
fn short(rate: f64) -> Scenario {
    let mut s = p2c(rate);
    s.duration_s = 40.0;
    s.warmup_s = 5.0;
    s
}

fn fresh_dir(name: &str) -> common::ScratchDir {
    common::scratch(&format!("trace-engine-{name}"))
}

#[test]
fn tracing_does_not_change_the_fingerprint() {
    let off = sim::run(&p2c(0.0)).unwrap();
    let on = sim::run(&p2c(0.2)).unwrap();
    assert_eq!(off.fingerprint, on.fingerprint, "tracing moved the fingerprint");
    assert_eq!(off.records.len(), on.records.len());
    assert!(off.traces.is_empty(), "rate 0 recorded {} traces", off.traces.len());
    assert!(!on.traces.is_empty(), "rate 0.2 recorded no traces");
    assert!(
        on.traces.len() < on.records.len(),
        "{} traces for {} records is not a sample",
        on.traces.len(),
        on.records.len()
    );
}

#[test]
fn a_traced_request_has_queue_route_prefill_and_decode_spans_in_order() {
    let r = sim::run(&short(0.2)).unwrap();
    let ok: Vec<_> = r.traces.iter().filter(|t| t.record.outcome.is_success() && t.record.attempts == 1).collect();
    assert!(ok.len() > 20, "only {} successful traces", ok.len());

    for t in ok {
        let rec = &t.record;
        let kinds: Vec<&'static str> = t.spans.iter().map(|s| s.kind.operation()).collect();
        assert_eq!(&kinds[..3], ["queue", "route", "queue"], "request {}: {kinds:?}", rec.id);
        // Spans follow one another, with one physical exception: the step that finishes a prompt's
        // prefill also emits its first token, so the last prefill chunk and the first decode step are
        // the same step, and say so by having the same start and end.
        for w in t.spans.windows(2) {
            let (a, b) = (&w[0], &w[1]);
            assert!(a.start_unix_ns <= a.end_unix_ns, "request {}: span ends before it starts", rec.id);
            let same_step = matches!(a.kind, SpanKind::PrefillChunk { .. })
                && b.kind == SpanKind::DecodeStep
                && (a.start_unix_ns, a.end_unix_ns) == (b.start_unix_ns, b.end_unix_ns);
            assert!(b.start_unix_ns >= a.end_unix_ns || same_step, "request {}: spans overlap: {kinds:?}", rec.id);
        }
        assert_eq!(t.spans[0].start_unix_ns, rec.arrived_at);
        assert_eq!(t.spans[2].end_unix_ns, rec.admitted_at, "request {}: replica queue ends at admission", rec.id);
        assert_eq!(t.spans.last().unwrap().end_unix_ns, rec.finished_at);

        let mut prefill = 0u32;
        let mut decode = 0u32;
        let mut first_token = 0;
        let mut saw_decode = false;
        for s in &t.spans {
            match s.kind {
                SpanKind::PrefillChunk { tokens } => {
                    assert!(!saw_decode, "request {}: prefill after decode", rec.id);
                    prefill += tokens;
                }
                SpanKind::DecodeStep => {
                    if !saw_decode {
                        first_token = s.end_unix_ns;
                    }
                    saw_decode = true;
                    decode += 1;
                }
                SpanKind::RoutingDecision { ref candidates, .. } => {
                    assert!(candidates.contains(&(rec.replica as u64)), "request {}: chosen replica not a candidate", rec.id);
                }
                _ => {}
            }
            if s.kind.is_replica_span() {
                assert_eq!(s.replica_id, rec.replica as u64);
                assert!(s.resource.step_ns > 0, "request {}: {} span has no step time", rec.id, s.kind.operation());
                assert!(s.resource.batch_size >= 1, "request {}: empty batch", rec.id);
                assert!(s.resource.kv_capacity > 0);
            }
        }
        assert_eq!(prefill, rec.prompt_tokens, "request {}: prefill chunks do not add up to the prompt", rec.id);
        assert_eq!(decode, rec.output_tokens, "request {}: one decode step per output token", rec.id);
        assert_eq!(first_token, rec.first_token_at, "request {}: first decode step ends at the first token", rec.id);
    }
}

/// Drives the `Tracer` directly (not through a full simulation) with continuous load: the replica
/// never has zero tracked requests, which is exactly the condition U65 flagged — before the fix,
/// `steps` grows one entry per step for as long as that holds. `batch_size` is set to the driving
/// step's own index, so a returned `ResourceSnapshot`'s `batch_size` doubles as a check that `take`
/// still hands back the snapshot belonging to the right step, not just that memory is bounded.
#[test]
fn snapshots_are_bounded_by_the_oldest_tracked_request() {
    use sim_model::trace::{ResourceSnapshot, StepEvent, Tracer};

    const STEPS: u32 = 10_000;
    const CONCURRENCY: u64 = 5;
    const LIFETIME: u32 = 7;

    let mut tracer = Tracer::default();
    let mut next_id = 0u64;
    // id -> (admitted_step, decode steps seen).
    let mut active: Vec<(u64, u32, u32)> = Vec::new();
    for _ in 0..CONCURRENCY {
        let id = next_id;
        next_id += 1;
        tracer.track(id);
        active.push((id, 0, 0));
    }

    for step in 0..STEPS {
        for &(id, admitted_at, _) in &active {
            if admitted_at == step {
                tracer.admitted(id);
            }
        }
        tracer.snapshot(ResourceSnapshot { batch_size: step, ..Default::default() });
        for slot in &mut active {
            tracer.decode_step(slot.0);
            slot.2 += 1;
        }

        // Retire whichever ids have lived out their lifetime, verify what `take` hands back, then
        // immediately backfill so the replica stays continuously busy.
        let due: Vec<usize> = active
            .iter()
            .enumerate()
            .filter(|&(_, &(_, admitted_at, decodes))| decodes >= LIFETIME && admitted_at <= step)
            .map(|(i, _)| i)
            .collect();
        for i in due {
            let (id, admitted_at, decodes) = active[i];
            tracer.retired(id);
            let events = tracer.take(id);
            // Admitted, one decode per step lived, then retired: `decodes` counts this step's decode
            // too, so that many `DecodeStep`s plus the bookends.
            assert_eq!(events.len() as u32, decodes + 2, "request {id}: wrong event count");
            assert!(matches!(events[0].0, StepEvent::Admitted { id: eid, .. } if eid == id));
            assert_eq!(events[0].1.batch_size, admitted_at, "request {id}: admitted snapshot is stale");
            for (j, ev) in events[1..events.len() - 1].iter().enumerate() {
                assert!(matches!(ev.0, StepEvent::DecodeStep { id: eid, .. } if eid == id));
                assert_eq!(ev.1.batch_size, admitted_at + j as u32, "request {id}: decode snapshot is stale");
            }
            let last = events.last().unwrap();
            assert!(matches!(last.0, StepEvent::Retired { id: eid, .. } if eid == id));
            assert_eq!(last.1.batch_size, step, "request {id}: retired snapshot is stale");

            let new_id = next_id;
            next_id += 1;
            tracer.track(new_id);
            active[i] = (new_id, step + 1, 0);
        }

        // The recorder must never carry more than the span still owed to the oldest tracked request,
        // which on this continuously-busy replica is a handful of steps, never the whole run.
        let oldest = active.iter().map(|&(_, admitted_at, _)| admitted_at).min().unwrap();
        let bound = (step - oldest.min(step) + 1) as usize;
        assert!(
            tracer.snapshot_len() <= bound,
            "step {step}: {} snapshots held, only {bound} steps are still owed",
            tracer.snapshot_len()
        );
    }

    assert!(
        tracer.snapshot_len() < 100,
        "10,000 steps of continuous load left {} snapshots; growth was not bounded",
        tracer.snapshot_len()
    );
}

fn ids(r: &RunResult) -> Vec<(u64, TraceBucket)> {
    r.traces.iter().map(|t| (t.record.id, t.bucket)).collect()
}

#[test]
fn sampling_is_stratified_and_deterministic() {
    let a = sim::run(&short(0.2)).unwrap();
    let b = sim::run(&short(0.2)).unwrap();
    assert_eq!(ids(&a), ids(&b), "same seed, different sample");

    // Every bucket the run populated has a trace in it, by the run's own thresholds.
    let mut hist = lbsim::metrics::Histogram::new();
    for rec in &a.records {
        hist.record(latency_of(rec));
    }
    let thresholds = [hist.percentile(50.0), hist.percentile(90.0), hist.percentile(99.0)];
    for bucket in TraceBucket::ALL {
        let populated = a.records.iter().any(|rec| TraceBucket::of(latency_of(rec), thresholds) == bucket);
        let traced = a.traces.iter().any(|t| TraceBucket::of(t.latency_ns(), thresholds) == bucket);
        assert!(!populated || traced, "bucket {} has completions but no trace", bucket.label());
    }
    // The stamp is the sampler's, against what it had seen; every bucket it hands out is a real one.
    assert!(a.traces.iter().any(|t| t.bucket != TraceBucket::P50), "every trace stamped p50");
}

#[test]
fn export_writes_engine_traces() {
    let r = sim::run(&short(0.2)).unwrap();
    let dir = fresh_dir("export");
    let run_dir = export::export_run_with_traces(&r, &r.traces, "engine", None, &dir, export::DEFAULT_TRACE_BUDGET_BYTES).unwrap();
    let text = fs::read_to_string(run_dir.join("traces.jsonl")).unwrap();
    let lines: Vec<&str> = text.lines().collect();
    // Decode spans are one per token, so the default budget does not fit every trace of a run this
    // size; the manifest says how many there were and how many were kept.
    let manifest = fs::read_to_string(run_dir.join("manifest.json")).unwrap();
    assert!(manifest.contains(&format!("\"available\":{}", r.traces.len())), "{manifest}");
    assert!(manifest.contains(&format!("\"kept\":{}", lines.len())), "{manifest}");
    assert!(!lines.is_empty() && lines.len() <= r.traces.len());

    let fixture: Vec<String> = fixtures::sample_traces(r.traces.len()).iter().map(trace_wire::request_trace_json).collect();
    assert!(lines.iter().all(|l| !fixture.iter().any(|f| f == l)), "the export is the fixture");
    assert!(lines.iter().any(|l| l.contains(r#""operation":"prefill""#)));
    assert!(lines.iter().any(|l| l.contains(r#""operation":"decode""#)));
    let ids: std::collections::BTreeSet<u64> = r.records.iter().map(|rec| rec.id).collect();
    for t in &r.traces {
        assert!(ids.contains(&t.record.id), "trace {} is not one of the run's records", t.record.id);
    }
}

/// Issao, 2026-09-10: "looking at traces, I see out of SLO request where there is a very short queue
/// (31ms) then a big gap (1.6s) until the first prefill. There shouldn't be a big gap in the trace
/// view with unaccounted time." The replica-queue span ends at admission, but a sequence admitted
/// into a batch is served only once the step's prefill budget reaches it, and that wait was engine
/// time the trace did not record. Every step a sequence spends in the batch is now a span, so the
/// replica's part of a journey is contiguous: each span starts exactly where the one before it ended
/// (or in the same step, for the last prefill chunk and the first decode step).
#[test]
fn a_traced_request_has_no_unaccounted_time_on_the_replica() {
    let s = short(0.2);
    let r = sim::run(&s).unwrap();
    let preempted: u64 = r.frames.iter().map(|f| f.preemptions).sum();
    assert_eq!(preempted, 0, "this scenario must not preempt: an evicted sequence leaves the batch, which is a gap of its own kind");
    let ok: Vec<_> = r.traces.iter().filter(|t| t.record.outcome.is_success() && t.record.attempts == 1).collect();
    assert!(ok.len() > 20, "only {} successful traces", ok.len());

    let (mut gaps, mut worst) = (0usize, 0u64);
    let mut worst_id = 0;
    for t in &ok {
        for w in t.spans.windows(2) {
            let (a, b) = (&w[0], &w[1]);
            if !(a.kind.is_replica_span() && b.kind.is_replica_span()) {
                continue;
            }
            let gap = b.start_unix_ns.saturating_sub(a.end_unix_ns);
            if gap > 0 {
                gaps += 1;
                if gap > worst {
                    worst = gap;
                    worst_id = t.record.id;
                }
            }
        }
    }
    eprintln!("gap statistics: {} traced requests, {gaps} gaps between replica spans, worst {:.1} ms on request {worst_id}", ok.len(), worst as f64 / 1e6);
    assert_eq!(gaps, 0, "{gaps} gaps between consecutive replica spans, the worst {:.1} ms on request {worst_id}", worst as f64 / 1e6);
}

/// The reproduction: 50 replicas at 20 rps under p2c, the fleet main ran when it reproduced Issao's
/// report. A request that waits in the batch has a `prefill_wait` span for every step it waited, and
/// each one is explained by the step's whole prefill budget going to the sequences ahead of it: the
/// tokens the span carries are exactly `step_token_budget`, none of them this request's.
#[test]
fn a_prefill_wait_is_explained_by_the_budget_going_to_others() {
    let mut s = short(0.2);
    s.replicas = 50;
    s.arrival_rps = 20.0;
    let r = sim::run(&s).unwrap();
    let ok: Vec<_> = r.traces.iter().filter(|t| t.record.outcome.is_success() && t.record.attempts == 1).collect();
    assert!(ok.len() > 20, "only {} successful traces", ok.len());

    let (mut waited, mut wait_spans, mut longest_wait) = (0usize, 0usize, 0u64);
    for t in &ok {
        let mut waiting_ns = 0u64;
        let mut seen_chunk = false;
        let mut seen_decode = false;
        for s in &t.spans {
            match s.kind {
                SpanKind::PrefillWait { others_prefill } => {
                    assert!(!seen_decode, "request {}: waiting for prefill after decoding began", t.record.id);
                    assert_eq!(
                        others_prefill, r.scenario.step_token_budget,
                        "request {}: waited while the step spent {others_prefill} of {} prefill tokens on others",
                        t.record.id, r.scenario.step_token_budget
                    );
                    assert_eq!(s.kind.tokens_processed(), others_prefill);
                    assert!(s.resource.batch_size >= 2, "request {}: waited alone in the batch", t.record.id);
                    assert!(s.end_unix_ns > s.start_unix_ns, "request {}: a wait of no time", t.record.id);
                    wait_spans += 1;
                    waiting_ns += s.end_unix_ns - s.start_unix_ns;
                    let _ = seen_chunk;
                }
                SpanKind::PrefillChunk { .. } => seen_chunk = true,
                SpanKind::DecodeStep => seen_decode = true,
                _ => {}
            }
        }
        if waiting_ns > 0 {
            waited += 1;
            longest_wait = longest_wait.max(waiting_ns);
        }
    }
    eprintln!("{waited} of {} traced requests waited in the batch, {wait_spans} wait spans, longest total wait {:.1} ms", ok.len(), longest_wait as f64 / 1e6);
    assert!(waited > 0, "the reproduction shows no request waiting in the batch");
    assert!(longest_wait >= 100_000_000, "the longest wait is {:.1} ms; the report saw hundreds", longest_wait as f64 / 1e6);
}
