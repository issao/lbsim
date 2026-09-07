//! `RequestTrace` on the wire, and the `GetTraces` RPC's filters, per `WIRE.md`.
//!
//! The encoder emits exactly the proto's fields, no more: the test in `tests/trace_wire.rs` holds
//! every emitted key to `metrics.proto`, `request.proto` and `common.proto`, and a field that exists
//! only in a JSON document is the drift the rule exists to prevent. `ResourceState`'s queue depth,
//! step time, roofline, KV capacity and routing candidates now cross this hop (`metrics.proto` at
//! f5eddf1); a zero resource field is the proto's default and is omitted, same as `replica_id`.
//! What is still derived rather than carried structurally: `kv_utilization` from the resident and
//! capacity counts, `concurrent_seqs` from the in-flight count.

use crate::server::{parse_json, Json as Value};
use crate::wire::Json;
use sim_metrics::trace::{BandwidthOrCompute, RequestTrace, SpanKind, TraceBucket, TraceSpan};
use sim_metrics::Outcome;

/// `common.proto` `Outcome` names by the engine's outcome, for the `outcome` field and the filter.
pub fn outcome_name(o: Outcome) -> &'static str {
    match o {
        Outcome::Ok => "OUTCOME_OK",
        Outcome::OkSloViolated => "OUTCOME_OK_SLO_VIOLATED",
        Outcome::Rejected => "OUTCOME_REJECTED",
        Outcome::TimeoutQueued => "OUTCOME_TIMEOUT_QUEUED",
        Outcome::TimeoutRunning => "OUTCOME_TIMEOUT_RUNNING",
    }
}

/// `metrics.proto` `StepBound` names by `BandwidthOrCompute`. The proto's zero, `STEP_BOUND_UNSPECIFIED`,
/// has no counterpart on the engine side: every step the engine models is bandwidth- or compute-bound.
pub fn step_bound_name(b: BandwidthOrCompute) -> &'static str {
    match b {
        BandwidthOrCompute::Bandwidth => "STEP_BOUND_BANDWIDTH",
        BandwidthOrCompute::Compute => "STEP_BOUND_COMPUTE",
    }
}

/// `metrics.proto` `TraceBucket` names by `sim_metrics::trace::TraceBucket`. Mirrors `TraceBucket::label()`,
/// but the proto's enumerator names rather than the dashboard's short labels.
pub fn trace_bucket_name(b: TraceBucket) -> &'static str {
    match b {
        TraceBucket::P50 => "TRACE_BUCKET_P50",
        TraceBucket::P90 => "TRACE_BUCKET_P90",
        TraceBucket::P99 => "TRACE_BUCKET_P99",
        TraceBucket::P999 => "TRACE_BUCKET_P999",
    }
}

/// `GetTracesRequest.outcome` as a filter. Proto3's zero, `OUTCOME_UNSPECIFIED`, is "any".
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum OutcomeFilter {
    #[default]
    Any,
    Only(Outcome),
    /// An outcome the proto names but the engine never produces (cancelled, failed). A client
    /// written against the proto may ask; it gets an empty list rather than an error.
    Nothing,
}

impl OutcomeFilter {
    fn matches(self, o: Outcome) -> bool {
        match self {
            OutcomeFilter::Any => true,
            OutcomeFilter::Only(want) => want == o,
            OutcomeFilter::Nothing => false,
        }
    }
}

/// The filter for a proto outcome name, or its number as a string, since a client may send either.
pub fn outcome_from_name(name: &str) -> Result<OutcomeFilter, String> {
    Ok(OutcomeFilter::Only(match name {
        "" | "0" | "OUTCOME_UNSPECIFIED" => return Ok(OutcomeFilter::Any),
        "1" | "OUTCOME_OK" => Outcome::Ok,
        "2" | "OUTCOME_OK_SLO_VIOLATED" => Outcome::OkSloViolated,
        "3" | "OUTCOME_REJECTED" => Outcome::Rejected,
        "4" | "OUTCOME_TIMEOUT_QUEUED" => Outcome::TimeoutQueued,
        "5" | "OUTCOME_TIMEOUT_RUNNING" => Outcome::TimeoutRunning,
        "6" | "OUTCOME_CANCELLED" | "7" | "OUTCOME_FAILED" => return Ok(OutcomeFilter::Nothing),
        other => return Err(format!("unknown outcome {other:?}")),
    }))
}

// ---------------------------------------------------------------------------
// Encoders
// ---------------------------------------------------------------------------

/// `request.proto` `RequestRecord`, from the engine's record plus the tenant the trace carries.
/// Fields the engine does not record yet (`slo_class`, `cluster_id`, `replica_path_id`,
/// `preemptions`, `preempted_ns`, `cached_prefix_tokens`) are omitted per WIRE.md rule 5, and the
/// derived latencies are omitted when the record has none, exactly as `requests.csv` leaves them blank.
pub fn request_record(j: &mut Json, t: &RequestTrace) {
    let r = &t.record;
    j.begin_object()
        .field_u64("id", r.id)
        .field_u64("tenant_id", t.tenant_id)
        .field_str("outcome", outcome_name(r.outcome))
        .field_u64("arrived_at_unix_ns", r.arrived_at)
        .field_u64("admitted_at_unix_ns", r.admitted_at)
        .field_u64("first_token_at_unix_ns", r.first_token_at)
        .field_u64("finished_at_unix_ns", r.finished_at)
        .field_int("prompt_tokens", r.prompt_tokens as i64)
        .field_int("output_tokens", r.output_tokens as i64)
        .field_u64("replica_id", r.replica as u64)
        .field_int("attempts", r.attempts as i64)
        .field_u64("queue_wait_ns", r.queue_wait());
    if let Some(ttft) = r.ttft() {
        j.field_u64("ttft_ns", ttft);
    }
    if let Some(e2e) = r.e2e() {
        j.field_u64("e2e_ns", e2e);
    }
    j.field_u64("mean_itl_ns", r.mean_itl)
        // The proto's per-request p99 gap; the engine keeps the worst gap, which for one request's
        // forty-odd tokens is the same stall the field exists to show.
        .field_u64("p99_itl_ns", r.max_itl)
        .end_object();
}

/// `metrics.proto` `TraceSpan`.
pub fn trace_span(j: &mut Json, s: &TraceSpan) {
    j.begin_object()
        .field_u64("start_unix_ns", s.start_unix_ns)
        .field_u64("end_unix_ns", s.end_unix_ns)
        .field_str("component", &s.component())
        .field_str("operation", s.kind.operation());
    if s.replica_id != 0 {
        j.field_u64("replica_id", s.replica_id);
    }
    j.field_int("concurrent_seqs", s.resource.running as i64)
        .field_f64("kv_utilization", s.resource.kv_utilization())
        .field_int("tokens_processed", s.kind.tokens_processed() as i64)
        .field_str("kv_tier", s.kv_tier.name());
    // The resource state added at f5eddf1. Zero is the proto's default and is omitted, per WIRE.md
    // rule 5, the same as `replica_id` above.
    if s.resource.batch_size != 0 {
        j.field_int("batch_size", s.resource.batch_size as i64);
    }
    if s.resource.queued != 0 {
        j.field_int("queued", s.resource.queued as i64);
    }
    if s.resource.kv_tokens_resident != 0 {
        j.field_u64("kv_tokens_resident", s.resource.kv_tokens_resident);
    }
    if s.resource.kv_capacity != 0 {
        j.field_u64("kv_capacity", s.resource.kv_capacity);
    }
    if s.resource.step_ns != 0 {
        j.field_u64("step_ns", s.resource.step_ns);
    }
    j.field_str("bound", step_bound_name(s.resource.bound));
    // Routing spans only: which replicas the router looked at and how stale its view was.
    if let SpanKind::RoutingDecision { candidates, stale_view_age } = &s.kind {
        j.key("candidates").begin_array();
        for c in candidates {
            j.u64_str(*c);
        }
        j.end_array();
        if *stale_view_age != 0 {
            j.field_u64("stale_view_age_ns", *stale_view_age);
        }
    }
    j.end_object();
}

/// `metrics.proto` `RequestTrace`.
pub fn request_trace(j: &mut Json, t: &RequestTrace) {
    j.begin_object().key("record");
    request_record(j, t);
    j.field_str("bucket", trace_bucket_name(t.bucket));
    j.key("spans").begin_array();
    for s in &t.spans {
        trace_span(j, s);
    }
    j.end_array().end_object();
}

pub fn request_trace_json(t: &RequestTrace) -> String {
    let mut j = Json::new();
    request_trace(&mut j, t);
    j.finish()
}

// ---------------------------------------------------------------------------
// GetTraces
// ---------------------------------------------------------------------------

/// `ingress.proto` `GetTracesRequest`. Zero is "unset" for every filter, as in proto3: no outcome,
/// no latency floor, any tenant, no limit.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GetTracesRequest {
    pub run_id: String,
    pub outcome: OutcomeFilter,
    pub min_e2e_ns: u64,
    pub tenant_id: u64,
    pub limit: u32,
}

impl GetTracesRequest {
    /// Whether a trace passes the filters. The latency floor compares the proto's `e2e_ns`, which a
    /// failed request does not have, so `min_e2e_ns` selects among successes; ask by outcome for
    /// the failures.
    pub fn matches(&self, t: &RequestTrace) -> bool {
        if !self.outcome.matches(t.record.outcome) {
            return false;
        }
        if self.min_e2e_ns > 0 && !matches!(t.record.e2e(), Some(e) if e >= self.min_e2e_ns) {
            return false;
        }
        if self.tenant_id > 0 && t.tenant_id != self.tenant_id {
            return false;
        }
        true
    }
}

/// The traces a request selects, in stored order, cut at `limit`.
pub fn select<'a>(traces: &'a [RequestTrace], req: &GetTracesRequest) -> Vec<&'a RequestTrace> {
    let limit = if req.limit == 0 { usize::MAX } else { req.limit as usize };
    traces.iter().filter(|t| req.matches(t)).take(limit).collect()
}

/// `ingress.proto` `GetTracesResponse` for `req` over a run's traces.
pub fn get_traces(j: &mut Json, traces: &[RequestTrace], req: &GetTracesRequest) {
    j.begin_object().key("traces").begin_array();
    for t in select(traces, req) {
        request_trace(j, t);
    }
    j.end_array().end_object();
}

pub fn get_traces_json(traces: &[RequestTrace], req: &GetTracesRequest) -> String {
    let mut j = Json::new();
    get_traces(&mut j, traces, req);
    j.finish()
}

/// Parse a `GetTracesRequest` body with `server.rs`'s reader. `uint64` is accepted as either a
/// decimal string (what WIRE.md asks a client to send) or a bare number (what a hand-typed curl
/// sends). Unknown keys are ignored, as proto3 JSON parsers do; a malformed document is a 400.
/// Nested containers are refused even under an unknown key: nothing in `GetTracesRequest` has one.
pub fn parse_get_traces_request(text: &str) -> Result<GetTracesRequest, String> {
    let Value::Obj(fields) = parse_json(text)? else { return Err("expected an object".into()) };
    let mut req = GetTracesRequest::default();
    for (key, value) in &fields {
        if matches!(value, Value::Obj(_) | Value::Arr(_)) {
            return Err(format!("{key}: nested container"));
        }
        match key.as_str() {
            "run_id" => req.run_id = string(value, key)?,
            "outcome" => req.outcome = outcome_from_name(&string(value, key)?)?,
            "min_e2e_ns" => req.min_e2e_ns = u64(value, key)?,
            "tenant_id" => req.tenant_id = u64(value, key)?,
            "limit" => req.limit = u32::try_from(u64(value, key)?).map_err(|_| "limit exceeds uint32".to_string())?,
            _ => {}
        }
    }
    Ok(req)
}

/// A string field. A number is accepted for the enum, since a client may send the number; `null`
/// is proto3's absent.
fn string(v: &Value, key: &str) -> Result<String, String> {
    match v {
        Value::Str(s) => Ok(s.clone()),
        Value::Num(n) => Ok(format!("{n}")),
        Value::Null => Ok(String::new()),
        _ => Err(format!("{key}: expected a string")),
    }
}

fn u64(v: &Value, key: &str) -> Result<u64, String> {
    match v {
        Value::Str(s) if s.is_empty() => Ok(0),
        Value::Str(s) => s.parse().map_err(|_| format!("{key}: {s:?} is not an unsigned integer")),
        Value::Num(n) if *n >= 0.0 && n.fract() == 0.0 => Ok(*n as u64),
        Value::Num(n) => Err(format!("{key}: {n} is not an unsigned integer")),
        Value::Null => Ok(0),
        _ => Err(format!("{key}: expected an unsigned integer")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_parses_strings_numbers_and_ignores_unknown_keys() {
        let req = parse_get_traces_request(
            r#" { "run_id": "r-1", "outcome": "OUTCOME_TIMEOUT_RUNNING", "min_e2e_ns": "2500000000",
                 "tenant_id": 2, "limit": 5, "extra": null, "flag": true } "#,
        )
        .unwrap();
        assert_eq!(
            req,
            GetTracesRequest {
                run_id: "r-1".into(),
                outcome: OutcomeFilter::Only(Outcome::TimeoutRunning),
                min_e2e_ns: 2_500_000_000,
                tenant_id: 2,
                limit: 5,
            }
        );
        assert_eq!(parse_get_traces_request("{}").unwrap(), GetTracesRequest::default());
        assert_eq!(parse_get_traces_request(r#"{"outcome":"OUTCOME_UNSPECIFIED","outcome":0}"#).unwrap().outcome, OutcomeFilter::Any);
        assert_eq!(parse_get_traces_request(r#"{"run_id":"a\"bé"}"#).unwrap().run_id, "a\"bé");
    }

    #[test]
    fn request_rejects_malformed_bodies() {
        for bad in [
            "", "[]", r#"{"run_id":}"#, r#"{"run_id":"x"} x"#, r#"{"limit":"many"}"#,
            r#"{"outcome":"OUTCOME_BOGUS"}"#, r#"{"scenario":{"text":""}}"#, r#"{"limit":-1}"#,
            r#"{"tenant_id":true}"#,
        ] {
            assert!(parse_get_traces_request(bad).is_err(), "{bad:?} should be refused");
        }
    }

    #[test]
    fn proto_only_outcomes_filter_to_nothing_rather_than_erroring() {
        assert_eq!(outcome_from_name("OUTCOME_CANCELLED").unwrap(), OutcomeFilter::Nothing);
        assert_eq!(outcome_from_name("7").unwrap(), OutcomeFilter::Nothing);
        let req = GetTracesRequest { outcome: OutcomeFilter::Nothing, ..Default::default() };
        assert!(select(&sim_metrics::trace::fixtures::sample_traces(5), &req).is_empty());
    }
}
