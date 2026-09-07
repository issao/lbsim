//! The request-trace wire, held to `metrics.proto` and WIRE.md.
//!
//! The engine does not record spans yet; `sim_metrics::trace::fixtures` stands in. Everything here
//! is therefore a property of the encoder, the filters, the sampler and the export, and none of it
//! changes when the engine starts filling the struct.

use lbsim::metrics::trace::{fixtures, RequestTrace, TraceBucket, TraceSampler};
use lbsim::metrics::{Outcome, RequestRecord};
use lbsim::rng::Rng;
use lbsim::EPOCH_BASE;
use sim_ingress::export;
use sim_ingress::trace_wire::{self, GetTracesRequest, OutcomeFilter};
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

fn workspace() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

fn fresh_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("lbsim-trace-wire-{}-{name}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// Every `"key":` in a JSON document; the same scan `tests/wire_export.rs` uses.
fn json_keys(text: &str) -> BTreeSet<String> {
    let mut keys = BTreeSet::new();
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'"' {
            let start = i + 1;
            let mut j = start;
            while j < bytes.len() && bytes[j] != b'"' {
                if bytes[j] == b'\\' {
                    j += 1;
                }
                j += 1;
            }
            if j + 1 < bytes.len() && bytes[j + 1] == b':' {
                keys.insert(text[start..j].to_string());
            }
            i = j + 1;
        } else {
            i += 1;
        }
    }
    keys
}

/// Field names declared in a proto file: the identifier before `=` on every `... name = N;` line.
fn proto_field_names(file: &str) -> BTreeSet<String> {
    let text = fs::read_to_string(workspace().join("proto/lbsim/v1").join(file)).unwrap();
    text.lines()
        .filter_map(|l| {
            let l = l.split("//").next()?.trim();
            let (decl, _) = l.split_once('=')?;
            Some(decl.split_whitespace().last()?.to_string())
        })
        .collect()
}

/// The proto's enum value names, so an enum on the wire can be checked to be one of them.
fn proto_enum_values(file: &str, prefix: &str) -> BTreeSet<String> {
    proto_field_names(file).into_iter().filter(|n| n.starts_with(prefix)).collect()
}

#[test]
fn trace_field_names_exist_in_metrics_proto() {
    // `RequestTrace` and `TraceSpan` live in metrics.proto; the record inside is request.proto's.
    let mut allowed = proto_field_names("metrics.proto");
    allowed.extend(proto_field_names("request.proto"));
    allowed.extend(proto_field_names("ingress.proto"));

    let traces = fixtures::sample_traces(30);
    let mut offenders = BTreeSet::new();
    for t in &traces {
        for key in json_keys(&trace_wire::request_trace_json(t)) {
            if !allowed.contains(&key) {
                offenders.insert(key);
            }
        }
    }
    let response = trace_wire::get_traces_json(&traces, &GetTracesRequest::default());
    for key in json_keys(&response) {
        if !allowed.contains(&key) {
            offenders.insert(key);
        }
    }
    assert!(offenders.is_empty(), "field names not in any proto: {offenders:?}");

    // And the fields that matter are all present, not merely all valid.
    let keys = json_keys(&trace_wire::request_trace_json(&traces[0]));
    for want in [
        "record", "spans", "id", "tenant_id", "outcome", "arrived_at_unix_ns", "finished_at_unix_ns",
        "prompt_tokens", "output_tokens", "replica_id", "start_unix_ns", "end_unix_ns", "component",
        "operation", "concurrent_seqs", "kv_utilization", "tokens_processed", "kv_tier", "e2e_ns",
    ] {
        assert!(keys.contains(want), "{want} missing from the trace document");
    }
}

#[test]
fn uint64_and_enum_encoding_follows_wire_rules() {
    let traces = fixtures::sample_traces(30);
    let outcomes = proto_enum_values("common.proto", "OUTCOME_");
    let tiers = proto_enum_values("common.proto", "MEMORY_TIER_");

    for t in &traces {
        let text = trace_wire::request_trace_json(t);
        // Rule 2: every uint64 is a decimal string. Epoch nanoseconds cannot be JSON numbers.
        for (key, v) in [
            ("id", t.record.id),
            ("tenant_id", t.tenant_id),
            ("arrived_at_unix_ns", t.record.arrived_at),
            ("finished_at_unix_ns", t.record.finished_at),
            ("queue_wait_ns", t.record.queue_wait()),
            ("p99_itl_ns", t.record.max_itl),
        ] {
            assert!(text.contains(&format!(r#""{key}":"{v}""#)), "{key} not a decimal string in {text}");
        }
        for s in &t.spans {
            assert!(text.contains(&format!(r#""start_unix_ns":"{}""#, s.start_unix_ns)));
            assert!(text.contains(&format!(r#""end_unix_ns":"{}""#, s.end_unix_ns)));
        }
        // uint32 and double are bare numbers.
        assert!(text.contains(&format!(r#""prompt_tokens":{},"#, t.record.prompt_tokens)));
        assert!(text.contains(r#""concurrent_seqs":"#) && !text.contains(r#""concurrent_seqs":""#));
        assert!(text.contains(r#""kv_utilization":0"#), "kv_utilization is a bare number: {text}");
        // Rule 3: enums by proto name.
        assert!(text.contains(&format!(r#""outcome":"{}""#, trace_wire::outcome_name(t.record.outcome))));
        for key in json_keys(&text) {
            let _ = key;
        }
        for value in text.split(r#""outcome":""#).skip(1).map(|r| r.split('"').next().unwrap()) {
            assert!(outcomes.contains(value), "{value} is not a common.proto Outcome");
        }
        for value in text.split(r#""kv_tier":""#).skip(1).map(|r| r.split('"').next().unwrap()) {
            assert!(tiers.contains(value), "{value} is not a common.proto MemoryTier");
        }
        // Rule 5: absent, never null. A shed request has no first token and no e2e.
        assert!(!text.contains("null"));
        if t.record.outcome == Outcome::Rejected {
            assert!(!text.contains(r#""e2e_ns""#) && !text.contains(r#""ttft_ns""#), "{text}");
        }
        assert!(!text.contains("NaN") && !text.contains("inf"));
    }
    // The gateway and router spans carry no replica, so the proto's zero is omitted rather than
    // written as "0".
    let first = trace_wire::request_trace_json(&traces[0]);
    let spans = first.split(r#""spans":["#).nth(1).unwrap();
    let gateway = spans.split('}').next().unwrap();
    assert!(gateway.contains(r#""component":"gateway""#) && !gateway.contains("replica_id"), "{gateway}");
}

#[test]
fn get_traces_filters_by_outcome_min_e2e_tenant_and_limit() {
    let traces = fixtures::sample_traces(60);
    let n_with = |f: &dyn Fn(&RequestTrace) -> bool| traces.iter().filter(|t| f(t)).count();

    let all = trace_wire::select(&traces, &GetTracesRequest::default());
    assert_eq!(all.len(), 60);

    let timeouts = GetTracesRequest { outcome: OutcomeFilter::Only(Outcome::TimeoutRunning), ..Default::default() };
    let got = trace_wire::select(&traces, &timeouts);
    assert!(!got.is_empty());
    assert_eq!(got.len(), n_with(&|t| t.record.outcome == Outcome::TimeoutRunning));
    assert!(got.iter().all(|t| t.record.outcome == Outcome::TimeoutRunning));

    let floor = traces.iter().filter_map(|t| t.record.e2e()).max().unwrap() / 2;
    let slow = GetTracesRequest { min_e2e_ns: floor, ..Default::default() };
    let got = trace_wire::select(&traces, &slow);
    assert!(!got.is_empty() && got.len() < 60);
    assert_eq!(got.len(), n_with(&|t| t.record.e2e().is_some_and(|e| e >= floor)));

    let tenant = GetTracesRequest { tenant_id: 2, ..Default::default() };
    let got = trace_wire::select(&traces, &tenant);
    assert_eq!(got.len(), 20);
    assert!(got.iter().all(|t| t.tenant_id == 2));

    let limited = GetTracesRequest { tenant_id: 2, limit: 3, ..Default::default() };
    let got = trace_wire::select(&traces, &limited);
    assert_eq!(got.len(), 3);
    // In stored order, so the same request with a larger limit is a superset that starts the same way.
    assert!(got.iter().zip(trace_wire::select(&traces, &tenant)).all(|(a, b)| a.record.id == b.record.id));

    // The filters compose, and the response document is `GetTracesResponse`.
    let combined = GetTracesRequest {
        run_id: "r-1".into(),
        outcome: OutcomeFilter::Only(Outcome::Ok),
        min_e2e_ns: floor,
        tenant_id: 1,
        limit: 2,
    };
    let text = trace_wire::get_traces_json(&traces, &combined);
    assert!(text.starts_with(r#"{"traces":["#) && text.ends_with("]}"));
    let want = n_with(&|t| t.record.outcome == Outcome::Ok && t.tenant_id == 1 && t.record.e2e().is_some_and(|e| e >= floor)).min(2);
    assert_eq!(text.matches(r#""record":"#).count(), want);

    // The request as a client sends it parses to the same filters.
    let parsed = trace_wire::parse_get_traces_request(&format!(
        r#"{{"run_id":"r-1","outcome":"OUTCOME_OK","min_e2e_ns":"{floor}","tenant_id":"1","limit":2}}"#
    ))
    .unwrap();
    assert_eq!(parsed, combined);
    assert_eq!(trace_wire::get_traces_json(&traces, &parsed), text);
}

/// Records whose latency is lognormal with a heavy tail, in a deterministic order.
fn synthetic_records(n: u64, seed: u64) -> Vec<RequestRecord> {
    let mut rng = Rng::from_seed(seed);
    (0..n)
        .map(|i| {
            let latency = (rng.lognormal(800e6, 1.5)) as u64 + 50_000_000;
            let outcome = match rng.below(400) {
                0 => Outcome::TimeoutRunning,
                1 => Outcome::Rejected,
                2..=9 => Outcome::OkSloViolated,
                _ => Outcome::Ok,
            };
            let arrived = EPOCH_BASE + i * 10_000_000;
            RequestRecord {
                id: i,
                arrived_at: arrived,
                admitted_at: arrived + 1_000_000,
                first_token_at: arrived + latency / 4,
                finished_at: arrived + latency,
                prompt_tokens: 512,
                output_tokens: 40,
                replica: 1,
                attempts: 1,
                outcome,
                max_itl: 30_000_000,
                mean_itl: 25_000_000,
            }
        })
        .collect()
}

fn kept_ids(records: &[RequestRecord], seed: u64, rate: f64, per_bucket: u32, per_outcome: u32) -> Vec<(u64, TraceBucket)> {
    let mut sampler = TraceSampler::new(Rng::from_seed(seed), rate, per_bucket, per_outcome);
    records.iter().filter_map(|r| sampler.keep(r).map(|b| (r.id, b))).collect()
}

#[test]
fn sampler_keeps_every_tail_bucket_at_low_rates() {
    let records = synthetic_records(20_000, 7);
    let kept = kept_ids(&records, 99, 0.001, 4, 2);

    // Against the population's own percentiles, not the sampler's running ones: the question is
    // whether the traces a reader gets include the true tail.
    let mut latencies: Vec<u64> = records.iter().map(|r| r.finished_at - r.arrived_at).collect();
    latencies.sort_unstable();
    let q = |p: f64| latencies[((p / 100.0 * latencies.len() as f64).ceil() as usize).max(1) - 1];
    let thresholds = [q(50.0), q(90.0), q(99.0)];
    let p999 = q(99.9);
    let mut per_bucket = [0usize; 4];
    let mut above_p999 = 0;
    for (id, _) in &kept {
        let lat = records[*id as usize].finished_at - records[*id as usize].arrived_at;
        per_bucket[TraceBucket::of(lat, thresholds).index()] += 1;
        if lat >= p999 {
            above_p999 += 1;
        }
    }
    assert!(per_bucket.iter().all(|n| *n > 0), "every bucket is represented: {per_bucket:?}");
    assert!(above_p999 >= 4, "the p99.9 tail is kept, not merely the p99 bucket: {above_p999} of {}", kept.len());
    for o in [Outcome::TimeoutRunning, Outcome::Rejected, Outcome::OkSloViolated] {
        assert!(kept.iter().any(|(id, _)| records[*id as usize].outcome == o), "{o:?} is represented");
    }
    // And it is still a sample: the quotas are proportional to the rate, so the total is a few
    // hundred out of twenty thousand, not most of them.
    assert!(kept.len() < 600, "{} kept of 20000", kept.len());
    assert!(kept.len() > 20);
}

#[test]
fn sampler_is_deterministic_for_a_seed() {
    let records = synthetic_records(5_000, 3);
    let a = kept_ids(&records, 11, 0.01, 2, 1);
    let b = kept_ids(&records, 11, 0.01, 2, 1);
    assert_eq!(a, b);
    let c = kept_ids(&records, 12, 0.01, 2, 1);
    assert_ne!(a, c, "a different seed keeps a different set");
    // Rate zero keeps exactly the quotas and nothing else, so the draw is never the deciding factor.
    let quota_only = kept_ids(&records, 11, 0.0, 1, 0);
    assert_eq!(quota_only.len(), 4, "one per bucket over the whole run: {quota_only:?}");
    assert!(kept_ids(&records, 11, 0.0, 0, 0).is_empty());
}

#[test]
fn export_traces_stays_within_budget_and_keeps_all_failures() {
    let traces = fixtures::sample_traces(120);
    let failures = traces.iter().filter(|t| !t.record.outcome.is_success()).count();
    assert!(failures >= 5, "the fixture has failures to keep: {failures}");
    let full: u64 = traces.iter().map(|t| trace_wire::request_trace_json(t).len() as u64 + 1).sum();

    let dir = fresh_dir("budget");
    let budget = full / 5;
    let m = export::export_traces(&traces, &dir, budget).unwrap();
    let body = fs::read_to_string(dir.join("traces.jsonl")).unwrap();
    assert_eq!(body.len() as u64, m.bytes);
    assert!(m.bytes <= budget, "{} bytes against a budget of {budget}", m.bytes);
    assert_eq!(m.available, 120);
    assert_eq!(m.kept, body.lines().count());
    assert!(m.kept < 120 && m.kept > failures, "{m:?}");
    assert!(m.sampling.ends_with("by latency stride, all failures kept"), "{}", m.sampling);
    let kept_failures = body.lines().filter(|l| !l.contains(r#""outcome":"OUTCOME_OK"#)).count();
    assert_eq!(kept_failures, failures, "every failure survives the budget");
    // The stride is by latency, so the slowest success is in the sample, and the file is in id
    // order, which is the fixture's completion order.
    let slowest = traces.iter().filter(|t| t.record.e2e().is_some()).max_by_key(|t| t.record.e2e()).unwrap();
    assert!(body.contains(&format!(r#""id":"{}""#, slowest.record.id)), "slowest success kept");
    let ids: Vec<&str> = body.lines().map(|l| l.split(r#""id":""#).nth(1).unwrap().split('"').next().unwrap()).collect();
    let mut sorted = ids.clone();
    sorted.sort_by_key(|s| s.parse::<u64>().unwrap());
    assert_eq!(ids, sorted);

    let manifest = fs::read_to_string(dir.join("manifest.json")).unwrap();
    assert!(manifest.contains(&format!(r#""kept":{},"available":120,"bytes":"{}","budget_bytes":"{budget}""#, m.kept, m.bytes)), "{manifest}");

    // A budget that fits everything is complete; one that fits nothing but failures says so.
    let m = export::export_traces(&traces, &fresh_dir("complete"), full).unwrap();
    assert_eq!((m.kept, m.sampling.as_str()), (120, "complete"));
    let d = fresh_dir("failures");
    let m = export::export_traces(&traces, &d, m.bytes / 40).unwrap();
    assert!(m.bytes <= full / 40);
    assert!(m.sampling == "failures only" || m.sampling.starts_with("1 in"), "{}", m.sampling);
    assert!(fs::read_to_string(d.join("traces.jsonl")).unwrap().lines().all(|l| !l.contains(r#""outcome":"OUTCOME_OK""#) || m.sampling.starts_with("1 in")));

    // Through the run export, the file lands beside the run's other documents.
    let dir = fresh_dir("run");
    let mut fixture_scenario = lbsim::scenario::Scenario::default();
    fixture_scenario.duration_s = 10.0;
    fixture_scenario.warmup_s = 2.0;
    let r = lbsim::sim::run(&fixture_scenario).unwrap();
    let run_dir = export::export_run_with_traces(&r, &traces, "t/fixture", None, &dir, export::DEFAULT_TRACE_BUDGET_BYTES).unwrap();
    for doc in ["status.json", "result.json", "fleet.jsonl", "traces.jsonl", "manifest.json"] {
        assert!(run_dir.join(doc).is_file(), "{doc}");
    }
    assert_eq!(fs::read_to_string(run_dir.join("traces.jsonl")).unwrap().lines().count(), 120);
}

#[test]
fn fixture_trace_round_trips_through_the_encoder() {
    let traces = fixtures::sample_traces(8);
    let a: Vec<String> = traces.iter().map(trace_wire::request_trace_json).collect();
    let b: Vec<String> = fixtures::sample_traces(8).iter().map(trace_wire::request_trace_json).collect();
    assert_eq!(a, b, "the fixture and its encoding are deterministic");

    let t = &traces[0];
    let text = &a[0];
    // Every span reaches the document, in order, with the derived fields the proto asks for.
    assert_eq!(text.matches(r#""start_unix_ns":"#).count(), t.spans.len());
    let ops: Vec<&str> = text.split(r#""operation":""#).skip(1).map(|r| r.split('"').next().unwrap()).collect();
    assert_eq!(ops, t.spans.iter().map(|s| s.kind.operation()).collect::<Vec<_>>());
    let prefill = t.spans.iter().find(|s| s.kind.operation() == "prefill").unwrap();
    assert!(text.contains(&format!(
        r#""component":"replica:{}","operation":"prefill","replica_id":"{}","concurrent_seqs":{},"kv_utilization":{},"tokens_processed":{},"kv_tier":"MEMORY_TIER_HBM""#,
        prefill.replica_id, prefill.replica_id, prefill.resource.running, prefill.resource.kv_utilization(), prefill.kind.tokens_processed()
    )), "{text}");
    assert!(text.contains(&format!(r#""e2e_ns":"{}""#, t.record.e2e().unwrap())));
    assert_eq!(text.matches("MEMORY_TIER_HBM").count(), 43, "3 prefill chunks and 40 decode steps on HBM");

    // The parse side accepts what the encoder writes for the request's own fields.
    let req = trace_wire::parse_get_traces_request(&format!(r#"{{"run_id":"x","tenant_id":"{}"}}"#, t.tenant_id)).unwrap();
    assert!(trace_wire::select(&traces, &req).iter().any(|k| k.record.id == t.record.id));
}
