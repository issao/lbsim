//! The JSON wire encoding of the Frontend-to-Ingress messages, per `WIRE.md`.
//!
//! Hand-written rather than generated because there is no codegen yet and no serde: the whole
//! project builds with zero dependencies in about a second, and a JSON writer is a hundred lines.
//! The shapes follow proto3's canonical JSON mapping, so when `prost` lands the transport changes and
//! the documents do not. The rules that keep this honest live in `WIRE.md`, and two tests below hold
//! this file to them: the metric table cannot drift from `subscription.proto`, and every field name
//! emitted must exist in a proto.
//!
//! The types here are the messages as the server holds them in memory. The live SSE stream and the
//! static export in `export.rs` both go through the same encoders, so a pre-baked run and a streamed
//! one are byte-for-byte the same documents.

/// `Metric` enum numbers, mirrored from `subscription.proto`. The number is the wire identity: map
/// keys in `MetricRow` are the number as a string, because the client already indexes by number.
///
/// Held as one table rather than an enum because the only operations are "name of" and "number of",
/// and a table is what the proto-drift test compares against.
pub const METRICS: &[(&str, i32)] = &[
    ("METRIC_UNSPECIFIED", 0),
    ("METRIC_TTFT", 1),
    ("METRIC_ITL", 2),
    ("METRIC_E2E", 3),
    ("METRIC_QUEUE_WAIT", 4),
    ("METRIC_PREEMPTED_TIME", 5),
    ("METRIC_PROMPT_TOKENS", 6),
    ("METRIC_OUTPUT_TOKENS", 7),
    ("METRIC_STEP_TIME", 8),
    ("METRIC_KV_UTILIZATION", 20),
    ("METRIC_KV_TOKENS_RESIDENT", 21),
    ("METRIC_RUNNING_SEQS", 22),
    ("METRIC_QUEUED_SEQS", 23),
    ("METRIC_QUEUED_PREFILL_TOKENS", 24),
    ("METRIC_BATCH_SIZE", 25),
    ("METRIC_PREFIX_CACHE_TOKENS", 26),
    ("METRIC_TIER_UTILIZATION", 27),
    ("METRIC_TIER_BANDWIDTH_UTILIZATION", 28),
    ("METRIC_TELEMETRY_STALENESS", 29),
    ("METRIC_OFFERED_RPS", 40),
    ("METRIC_ADMITTED_RPS", 41),
    ("METRIC_COMPLETED_RPS", 42),
    ("METRIC_REJECTED_RPS", 43),
    ("METRIC_OUTPUT_TOKENS_PER_S", 44),
    ("METRIC_GOODPUT_TOKENS_PER_S", 45),
    ("METRIC_PREEMPTIONS_PER_S", 46),
    ("METRIC_RETRIES_PER_S", 47),
    ("METRIC_PREFIX_HIT_RATE", 48),
    ("METRIC_READY_REPLICAS", 60),
    ("METRIC_WARMING_REPLICAS", 61),
    ("METRIC_DRAINING_REPLICAS", 62),
    ("METRIC_WARM_IDLE_REPLICAS", 63),
    ("METRIC_LOAD_IMBALANCE_CV", 64),
    ("METRIC_WASTED_GPU_FRACTION", 65),
    ("METRIC_SLO_ATTAINMENT", 66),
    ("METRIC_GPU_UTILIZATION", 67),
    ("METRIC_GPU_COMPUTE_BOUND_FRACTION", 68),
];

// The metrics the encoders and the exporter name directly. Same numbers as the table; the drift test
// covers the table, and `metric_constants_are_in_the_table` covers these.
pub const METRIC_TTFT: i32 = 1;
pub const METRIC_ITL: i32 = 2;
pub const METRIC_E2E: i32 = 3;
pub const METRIC_QUEUE_WAIT: i32 = 4;
pub const METRIC_STEP_TIME: i32 = 8;
pub const METRIC_KV_UTILIZATION: i32 = 20;
pub const METRIC_KV_TOKENS_RESIDENT: i32 = 21;
pub const METRIC_RUNNING_SEQS: i32 = 22;
pub const METRIC_QUEUED_SEQS: i32 = 23;
pub const METRIC_OFFERED_RPS: i32 = 40;
pub const METRIC_ADMITTED_RPS: i32 = 41;
pub const METRIC_COMPLETED_RPS: i32 = 42;
pub const METRIC_REJECTED_RPS: i32 = 43;
pub const METRIC_OUTPUT_TOKENS_PER_S: i32 = 44;
pub const METRIC_GOODPUT_TOKENS_PER_S: i32 = 45;
pub const METRIC_READY_REPLICAS: i32 = 60;
pub const METRIC_LOAD_IMBALANCE_CV: i32 = 64;
pub const METRIC_SLO_ATTAINMENT: i32 = 66;
pub const METRIC_GPU_UTILIZATION: i32 = 67;
pub const METRIC_GPU_COMPUTE_BOUND_FRACTION: i32 = 68;

/// `common.proto` `Outcome` numbers, keyed by the engine's outcome labels. `Scorecard.outcome_counts`
/// is keyed by this number.
pub const OUTCOMES: &[(&str, i32)] = &[
    ("ok", 1),
    ("ok_slo_violated", 2),
    ("rejected", 3),
    ("timeout_queued", 4),
    ("timeout_running", 5),
];

pub fn metric_name(number: i32) -> Option<&'static str> {
    METRICS.iter().find(|(_, n)| *n == number).map(|(name, _)| *name)
}

// ---------------------------------------------------------------------------
// The writer
// ---------------------------------------------------------------------------

/// A streaming JSON writer. Compact output, no whitespace, keys in the order they are written.
///
/// Object and array nesting is tracked so commas are placed correctly without the caller counting.
/// Nothing is validated beyond that: a caller that writes two values where one is expected gets
/// invalid JSON, and the tests are the guard.
pub struct Json {
    buf: String,
    /// One entry per open container: whether a separator is needed before the next item.
    stack: Vec<bool>,
}

impl Default for Json {
    fn default() -> Self {
        Self::new()
    }
}

impl Json {
    pub fn new() -> Self {
        Json { buf: String::new(), stack: Vec::new() }
    }

    pub fn finish(self) -> String {
        self.buf
    }

    fn separate(&mut self) {
        if let Some(needs_comma) = self.stack.last_mut() {
            if *needs_comma {
                self.buf.push(',');
            }
            *needs_comma = true;
        }
    }

    pub fn begin_object(&mut self) -> &mut Self {
        self.separate();
        self.buf.push('{');
        self.stack.push(false);
        self
    }

    pub fn end_object(&mut self) -> &mut Self {
        self.stack.pop();
        self.buf.push('}');
        self
    }

    pub fn begin_array(&mut self) -> &mut Self {
        self.separate();
        self.buf.push('[');
        self.stack.push(false);
        self
    }

    pub fn end_array(&mut self) -> &mut Self {
        self.stack.pop();
        self.buf.push(']');
        self
    }

    /// A key inside an object. The value that follows must not add its own separator, so the
    /// container's flag is cleared here and set again by the value.
    pub fn key(&mut self, k: &str) -> &mut Self {
        self.separate();
        push_escaped(&mut self.buf, k);
        self.buf.push(':');
        if let Some(flag) = self.stack.last_mut() {
            *flag = false;
        }
        self
    }

    pub fn string(&mut self, s: &str) -> &mut Self {
        self.separate();
        push_escaped(&mut self.buf, s);
        self
    }

    /// A `double`. NaN and infinities are not JSON; the caller omits the field instead (WIRE.md rule
    /// 5: absent, not null), and this panics in debug builds so the omission is never forgotten.
    pub fn number(&mut self, v: f64) -> &mut Self {
        debug_assert!(v.is_finite(), "non-finite double {v} on the wire; omit the field instead");
        self.separate();
        push_f64(&mut self.buf, v);
        self
    }

    /// A `uint64`, as a decimal string. WIRE.md rule 2: above 2^53 a JSON number is lossy in the
    /// browser, and epoch nanoseconds are far above it.
    pub fn u64_str(&mut self, v: u64) -> &mut Self {
        self.separate();
        self.buf.push('"');
        self.buf.push_str(&v.to_string());
        self.buf.push('"');
        self
    }

    /// An `int32` or `uint32`: within 2^53, so a plain number.
    pub fn int(&mut self, v: i64) -> &mut Self {
        self.separate();
        self.buf.push_str(&v.to_string());
        self
    }

    pub fn boolean(&mut self, v: bool) -> &mut Self {
        self.separate();
        self.buf.push_str(if v { "true" } else { "false" });
        self
    }

    // Field helpers: key and value in one call, for the common case of a flat message.
    pub fn field_str(&mut self, k: &str, v: &str) -> &mut Self {
        self.key(k).string(v)
    }
    pub fn field_f64(&mut self, k: &str, v: f64) -> &mut Self {
        self.key(k).number(v)
    }
    pub fn field_u64(&mut self, k: &str, v: u64) -> &mut Self {
        self.key(k).u64_str(v)
    }
    pub fn field_int(&mut self, k: &str, v: i64) -> &mut Self {
        self.key(k).int(v)
    }
    pub fn field_bool(&mut self, k: &str, v: bool) -> &mut Self {
        self.key(k).boolean(v)
    }
}

/// Escape per RFC 8259: the two mandatory characters, the C0 controls, and nothing else. Non-ASCII
/// passes through as UTF-8, which JSON permits and which keeps scenario names readable.
fn push_escaped(buf: &mut String, s: &str) {
    buf.push('"');
    for c in s.chars() {
        match c {
            '"' => buf.push_str("\\\""),
            '\\' => buf.push_str("\\\\"),
            '\n' => buf.push_str("\\n"),
            '\r' => buf.push_str("\\r"),
            '\t' => buf.push_str("\\t"),
            '\u{08}' => buf.push_str("\\b"),
            '\u{0c}' => buf.push_str("\\f"),
            c if (c as u32) < 0x20 => {
                buf.push_str(&format!("\\u{:04x}", c as u32));
            }
            c => buf.push(c),
        }
    }
    buf.push('"');
}

/// Shortest round-trip decimal, never exponent notation, so the output is a JSON number under every
/// parser and byte-stable across runs. `-0.0` is written as `0`: the sign carries no meaning for any
/// metric here and would make two equal readings differ on disk.
fn push_f64(buf: &mut String, v: f64) {
    if v == 0.0 {
        buf.push('0');
    } else {
        buf.push_str(&format!("{v}"));
    }
}

// ---------------------------------------------------------------------------
// The messages
// ---------------------------------------------------------------------------

/// `RunStatus.State`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum State {
    Queued,
    Running,
    Paused,
    Complete,
    Failed,
}

impl State {
    pub fn name(self) -> &'static str {
        match self {
            State::Queued => "STATE_QUEUED",
            State::Running => "STATE_RUNNING",
            State::Paused => "STATE_PAUSED",
            State::Complete => "STATE_COMPLETE",
            State::Failed => "STATE_FAILED",
        }
    }
}

/// `ingress.proto` `RunStatus`.
#[derive(Clone, Debug)]
pub struct RunStatus {
    pub run_id: String,
    pub state: State,
    pub sim_time_unix_ns: u64,
    pub sim_end_unix_ns: u64,
    pub realtime_factor: f64,
    pub error: String,
}

/// `subscription.proto` `Target`: exactly one entity. Only the two scopes the first server speaks.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Target {
    Fleet,
    Replica(u64),
}

/// `subscription.proto` `Distribution`, the percentile form. The histogram form never crosses the
/// Frontend hop, so it is not modelled here.
#[derive(Clone, Debug, Default)]
pub struct Distribution {
    pub count: u64,
    pub mean: f64,
    pub min: f64,
    pub max: f64,
    pub percentile: Vec<f64>,
    pub value: Vec<f64>,
    pub from_merged_histogram: bool,
}

impl Distribution {
    /// Exact percentiles of a sample, nearest-rank, the same rule `sim_metrics::Histogram` applies
    /// to its buckets so a windowed value and a whole-run value are comparable. Values are whatever
    /// unit the caller measured in; the proto says nanoseconds for latencies.
    pub fn exact(samples: &mut Vec<u64>, percentiles: &[f64]) -> Distribution {
        samples.sort_unstable();
        let n = samples.len();
        if n == 0 {
            return Distribution { percentile: percentiles.to_vec(), ..Default::default() };
        }
        let value = percentiles
            .iter()
            .map(|q| {
                let rank = ((q / 100.0) * n as f64).ceil().max(1.0) as usize;
                samples[rank.min(n) - 1] as f64
            })
            .collect();
        let sum: u128 = samples.iter().map(|v| *v as u128).sum();
        Distribution {
            count: n as u64,
            mean: sum as f64 / n as f64,
            min: samples[0] as f64,
            max: samples[n - 1] as f64,
            percentile: percentiles.to_vec(),
            value,
            from_merged_histogram: false,
        }
    }
}

/// `subscription.proto` `MetricRow`. Both maps are kept as vectors keyed by metric number and are
/// sorted on encoding, so the document does not depend on insertion order.
#[derive(Clone, Debug)]
pub struct MetricRow {
    pub target: Target,
    pub values: Vec<(i32, f64)>,
    pub distributions: Vec<(i32, Distribution)>,
}

impl MetricRow {
    pub fn new(target: Target) -> Self {
        MetricRow { target, values: Vec::new(), distributions: Vec::new() }
    }

    /// Record a gauge or rate. A non-finite value is dropped rather than written, since JSON has no
    /// NaN; the absent key is how "no reading in this window" reaches the client.
    pub fn value(&mut self, metric: i32, v: f64) {
        if v.is_finite() {
            self.values.push((metric, v));
        }
    }

    /// Record a distribution. An empty one is dropped for the same reason a NaN gauge is: a p99 of
    /// nothing is not zero, and a chart should show a gap there.
    pub fn distribution(&mut self, metric: i32, d: Distribution) {
        if d.count > 0 {
            self.distributions.push((metric, d));
        }
    }
}

/// `subscription.proto` `SubscriptionUpdate`.
#[derive(Clone, Debug)]
pub struct SubscriptionUpdate {
    pub subscription_id: String,
    pub sim_time_unix_ns: u64,
    pub realtime_factor: f64,
    pub row: MetricRow,
    pub is_final: bool,
}

/// `metrics.proto` `Scorecard`, the part of `RunResult` today's engine can fill.
#[derive(Clone, Debug, Default)]
pub struct Scorecard {
    pub values: Vec<(i32, f64)>,
    pub distributions: Vec<(i32, Distribution)>,
    /// Keyed by `common.proto` `Outcome` number.
    pub outcome_counts: Vec<(i32, u64)>,
    pub declared_rated_capacity_rps: f64,
    pub metastable_collapse: bool,
}

/// `metrics.proto` `RunResult`, as `GetResult` returns it. `scenario_serialized` is omitted: the
/// scenario text travels beside the result as its own file, and a `bytes` field would only base64
/// the same text.
#[derive(Clone, Debug)]
pub struct RunResult {
    pub run_id: String,
    pub seed: u64,
    pub event_count: u64,
    pub state_checksum: u64,
    pub overall: Scorecard,
}

// ---------------------------------------------------------------------------
// Encoders. Each writes one complete message into the writer.
// ---------------------------------------------------------------------------

pub fn run_status(j: &mut Json, s: &RunStatus) {
    j.begin_object()
        .field_str("run_id", &s.run_id)
        .field_str("state", s.state.name())
        .field_u64("sim_time_unix_ns", s.sim_time_unix_ns)
        .field_u64("sim_end_unix_ns", s.sim_end_unix_ns)
        .field_f64("realtime_factor", s.realtime_factor)
        .field_str("error", &s.error)
        .end_object();
}

pub fn target(j: &mut Json, t: Target) {
    j.begin_object();
    match t {
        Target::Fleet => {
            j.field_str("scope", "SCOPE_FLEET");
        }
        Target::Replica(id) => {
            j.field_str("scope", "SCOPE_REPLICA").field_u64("replica_id", id);
        }
    }
    j.end_object();
}

pub fn distribution(j: &mut Json, d: &Distribution) {
    j.begin_object()
        .field_u64("count", d.count)
        .field_f64("mean", d.mean)
        .field_f64("min", d.min)
        .field_f64("max", d.max);
    j.key("percentile").begin_array();
    for p in &d.percentile {
        j.number(*p);
    }
    j.end_array();
    j.key("value").begin_array();
    for v in &d.value {
        j.number(*v);
    }
    j.end_array();
    j.field_bool("from_merged_histogram", d.from_merged_histogram).end_object();
}

/// The two enum-keyed maps, sorted by metric number so equal rows encode identically.
fn metric_maps(j: &mut Json, values: &[(i32, f64)], distributions: &[(i32, Distribution)]) {
    let mut values: Vec<&(i32, f64)> = values.iter().collect();
    values.sort_by_key(|(k, _)| *k);
    j.key("values").begin_object();
    for (k, v) in values {
        j.key(&k.to_string()).number(*v);
    }
    j.end_object();

    let mut dists: Vec<&(i32, Distribution)> = distributions.iter().collect();
    dists.sort_by_key(|(k, _)| *k);
    j.key("distributions").begin_object();
    for (k, d) in dists {
        j.key(&k.to_string());
        distribution(j, d);
    }
    j.end_object();
}

pub fn metric_row(j: &mut Json, row: &MetricRow) {
    j.begin_object().key("target");
    target(j, row.target);
    metric_maps(j, &row.values, &row.distributions);
    j.end_object();
}

pub fn subscription_update(j: &mut Json, u: &SubscriptionUpdate) {
    j.begin_object()
        .field_str("subscription_id", &u.subscription_id)
        .field_u64("sim_time_unix_ns", u.sim_time_unix_ns)
        .field_f64("realtime_factor", u.realtime_factor)
        .key("row");
    metric_row(j, &u.row);
    j.field_bool("final", u.is_final).end_object();
}

pub fn scorecard(j: &mut Json, s: &Scorecard) {
    j.begin_object();
    metric_maps(j, &s.values, &s.distributions);
    let mut outcomes: Vec<&(i32, u64)> = s.outcome_counts.iter().collect();
    outcomes.sort_by_key(|(k, _)| *k);
    j.key("outcome_counts").begin_object();
    for (k, n) in outcomes {
        j.key(&k.to_string()).u64_str(*n);
    }
    j.end_object();
    j.field_f64("declared_rated_capacity_rps", s.declared_rated_capacity_rps)
        .field_bool("metastable_collapse", s.metastable_collapse)
        .end_object();
}

pub fn run_result(j: &mut Json, r: &RunResult) {
    j.begin_object()
        .field_str("run_id", &r.run_id)
        .field_u64("seed", r.seed)
        .field_u64("event_count", r.event_count)
        .field_u64("state_checksum", r.state_checksum)
        .key("overall");
    scorecard(j, &r.overall);
    j.end_object();
}

// Single-document conveniences.
pub fn run_status_json(s: &RunStatus) -> String {
    let mut j = Json::new();
    run_status(&mut j, s);
    j.finish()
}
pub fn subscription_update_json(u: &SubscriptionUpdate) -> String {
    let mut j = Json::new();
    subscription_update(&mut j, u);
    j.finish()
}
pub fn run_result_json(r: &RunResult) -> String {
    let mut j = Json::new();
    run_result(&mut j, r);
    j.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn proto(name: &str) -> String {
        let p = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../proto/lbsim/v1").join(name);
        std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
    }

    /// The `Metric` enum's `NAME = N;` lines, read straight from the proto text.
    fn proto_metric_enum() -> Vec<(String, i32)> {
        let text = proto("subscription.proto");
        let body = text
            .split("enum Metric {")
            .nth(1)
            .and_then(|rest| rest.split('}').next())
            .expect("enum Metric in subscription.proto");
        body.lines()
            .filter_map(|l| {
                let l = l.split("//").next()?.trim().trim_end_matches(';');
                let (name, num) = l.split_once('=')?;
                Some((name.trim().to_string(), num.trim().parse().ok()?))
            })
            .collect()
    }

    #[test]
    fn metric_table_matches_the_proto_enum() {
        let from_proto = proto_metric_enum();
        assert!(from_proto.len() > 30, "parsed only {} metrics from the proto", from_proto.len());
        for (name, num) in &from_proto {
            assert!(
                METRICS.contains(&(name.as_str(), *num)),
                "{name} = {num} is in subscription.proto but not in wire::METRICS"
            );
        }
        for (name, num) in METRICS {
            assert!(
                from_proto.iter().any(|(n, k)| n == name && k == num),
                "{name} = {num} is in wire::METRICS but not in subscription.proto"
            );
        }
    }

    #[test]
    fn metric_constants_are_in_the_table() {
        for (name, num) in [
            ("METRIC_TTFT", METRIC_TTFT),
            ("METRIC_ITL", METRIC_ITL),
            ("METRIC_E2E", METRIC_E2E),
            ("METRIC_QUEUE_WAIT", METRIC_QUEUE_WAIT),
            ("METRIC_STEP_TIME", METRIC_STEP_TIME),
            ("METRIC_KV_UTILIZATION", METRIC_KV_UTILIZATION),
            ("METRIC_KV_TOKENS_RESIDENT", METRIC_KV_TOKENS_RESIDENT),
            ("METRIC_RUNNING_SEQS", METRIC_RUNNING_SEQS),
            ("METRIC_QUEUED_SEQS", METRIC_QUEUED_SEQS),
            ("METRIC_OFFERED_RPS", METRIC_OFFERED_RPS),
            ("METRIC_ADMITTED_RPS", METRIC_ADMITTED_RPS),
            ("METRIC_COMPLETED_RPS", METRIC_COMPLETED_RPS),
            ("METRIC_REJECTED_RPS", METRIC_REJECTED_RPS),
            ("METRIC_OUTPUT_TOKENS_PER_S", METRIC_OUTPUT_TOKENS_PER_S),
            ("METRIC_GOODPUT_TOKENS_PER_S", METRIC_GOODPUT_TOKENS_PER_S),
            ("METRIC_READY_REPLICAS", METRIC_READY_REPLICAS),
            ("METRIC_LOAD_IMBALANCE_CV", METRIC_LOAD_IMBALANCE_CV),
            ("METRIC_SLO_ATTAINMENT", METRIC_SLO_ATTAINMENT),
        ] {
            assert_eq!(metric_name(num), Some(name));
        }
    }

    #[test]
    fn outcome_numbers_match_common_proto() {
        let text = proto("common.proto");
        for (label, num) in OUTCOMES {
            let want = format!("OUTCOME_{} = {num};", label.to_uppercase());
            assert!(text.contains(&want), "{want} not in common.proto");
        }
    }

    #[test]
    fn json_strings_are_escaped() {
        let mut j = Json::new();
        j.begin_object().field_str("k\"ey", "a\"b\\c\nd\te\u{01}f é").end_object();
        assert_eq!(j.finish(), r#"{"k\"ey":"a\"b\\c\nd\te\u0001f é"}"#);
    }

    #[test]
    fn uint64_fields_are_strings_and_doubles_are_numbers() {
        let s = RunStatus {
            run_id: "r-1".into(),
            state: State::Complete,
            sim_time_unix_ns: 1_767_225_720_000_000_000,
            sim_end_unix_ns: 1_767_225_720_000_000_000,
            realtime_factor: 2.5,
            error: String::new(),
        };
        assert_eq!(
            run_status_json(&s),
            r#"{"run_id":"r-1","state":"STATE_COMPLETE","sim_time_unix_ns":"1767225720000000000","sim_end_unix_ns":"1767225720000000000","realtime_factor":2.5,"error":""}"#
        );

        let mut row = MetricRow::new(Target::Fleet);
        row.value(METRIC_QUEUED_SEQS, 12.0);
        row.value(METRIC_OFFERED_RPS, 70.0);
        row.value(METRIC_SLO_ATTAINMENT, f64::NAN);
        row.distribution(
            METRIC_TTFT,
            Distribution::exact(&mut vec![700_000_000, 2_100_000_000, 120_000_000], &[50.0, 99.0]),
        );
        row.distribution(METRIC_E2E, Distribution::exact(&mut Vec::new(), &[50.0]));
        let u = SubscriptionUpdate {
            subscription_id: "s-7".into(),
            sim_time_unix_ns: 1_767_225_615_000_000_000,
            realtime_factor: 0.0,
            row,
            is_final: true,
        };
        let text = subscription_update_json(&u);
        // Map keys are sorted by number, the NaN gauge and the empty distribution are absent, the
        // count is a string, and every double is a bare number.
        assert_eq!(
            text,
            r#"{"subscription_id":"s-7","sim_time_unix_ns":"1767225615000000000","realtime_factor":0,"row":{"target":{"scope":"SCOPE_FLEET"},"values":{"23":12,"40":70},"distributions":{"1":{"count":"3","mean":973333333.3333334,"min":120000000,"max":2100000000,"percentile":[50,99],"value":[700000000,2100000000],"from_merged_histogram":false}}},"final":true}"#
        );

        let mut j = Json::new();
        j.begin_array().number(0.1).number(-0.0).number(1e-7).number(1e21).u64_str(u64::MAX).end_array();
        assert_eq!(
            j.finish(),
            r#"[0.1,0,0.0000001,1000000000000000000000,"18446744073709551615"]"#
        );
    }

    #[test]
    fn exact_percentiles_use_nearest_rank_like_the_histogram() {
        let d = Distribution::exact(&mut (1..=100).collect(), &[50.0, 90.0, 99.0, 99.9, 100.0]);
        assert_eq!(d.value, vec![50.0, 90.0, 99.0, 100.0, 100.0]);
        assert_eq!(d.count, 100);
        assert_eq!(d.min, 1.0);
        assert_eq!(d.max, 100.0);
        assert_eq!(d.mean, 50.5);
    }
}
