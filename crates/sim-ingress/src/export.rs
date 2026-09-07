//! Pre-baked runs: a finished `RunResult` written as the exact documents the live SSE stream carries.
//!
//! The dashboard needs real runs to show before a live server exists, and the shortest path is to
//! serve the stream's own documents as static files under `--dir`. The encoders in `wire.rs` are
//! shared with the server, so nothing here is a second format: `fleet.jsonl` is what a fleet-scope
//! subscription at the scenario's sample interval would have delivered, one `SubscriptionUpdate` per
//! line, and `result.json` is what `GetResult` returns.
//!
//! Layout under `dir`:
//!
//! ```text
//! runs/index.json                 every exported run, merged across invocations
//! runs/<run_id>/status.json       RunStatus, STATE_COMPLETE
//! runs/<run_id>/scenario.txt      the resolved scenario, as `sim-run run` reads it
//! runs/<run_id>/result.json       RunResult: the scorecard
//! runs/<run_id>/fleet.jsonl       SubscriptionUpdate per sample instant, SCOPE_FLEET
//! runs/<run_id>/traces.jsonl      RequestTrace per line, when the run recorded traces
//! runs/<run_id>/manifest.json     how many traces were kept of how many, when traces.jsonl exists
//! ```
//!
//! `index.json` and `manifest.json` are the documents with no proto message behind them; WIRE.md
//! names the index as the deviation. Everything else uses proto field names verbatim, and
//! `tests/wire_export.rs` and `tests/trace_wire.rs` check that.

use crate::trace_wire;
use crate::wire::{self, Distribution, MetricRow, RunStatus, State, SubscriptionUpdate, Target};
use sim_core::Nanos;
use sim_leaf::RunResult;
use sim_metrics::trace::RequestTrace;
use sim_metrics::Outcome;
use sim_scenario::Scenario;
use std::path::{Path, PathBuf};

/// The percentiles every exported distribution carries, whole-run and windowed alike.
pub const PERCENTILES: &[f64] = &[50.0, 90.0, 99.0, 99.9];

/// The subscription id stamped on every exported update. A static file has no lease and no
/// subscription; the field is kept because the document shape is the stream's, and a client that
/// reads both must not need two parsers.
const EXPORT_SUBSCRIPTION_ID: &str = "export";

/// Where the simulated clock started. `RunResult` records the measured window, and the engine placed
/// warmup immediately before it with the same cast, so this recovers the origin exactly.
fn sim_start(r: &RunResult) -> Nanos {
    r.measured_from.saturating_sub((r.scenario.warmup_s * 1e9) as Nanos)
}

fn sample_interval(r: &RunResult) -> Nanos {
    (r.scenario.sample_interval_ms * 1e6) as Nanos
}

pub fn status(r: &RunResult, run_id: &str) -> RunStatus {
    RunStatus {
        run_id: run_id.to_string(),
        state: State::Complete,
        sim_time_unix_ns: r.measured_to,
        sim_end_unix_ns: r.measured_to,
        // Zero is "as fast as possible" in `SetSpeedRequest`, and an export was never paced.
        realtime_factor: 0.0,
        error: String::new(),
    }
}

/// The whole-run scorecard, as `GetResult` would return it.
pub fn result(r: &RunResult, run_id: &str) -> wire::RunResult {
    let mut card = wire::Scorecard {
        declared_rated_capacity_rps: r.rated_rps,
        // A collapse is a run that had a spike and did not come back. No spike, no verdict.
        metastable_collapse: matches!(r.recovery(), Some((_, _, false))),
        ..Default::default()
    };
    let mut value = |m: i32, v: f64| {
        if v.is_finite() {
            card.values.push((m, v));
        }
    };
    value(wire::METRIC_OFFERED_RPS, r.scenario.arrival_rps);
    value(wire::METRIC_COMPLETED_RPS, r.completed_rps());
    value(wire::METRIC_OUTPUT_TOKENS_PER_S, r.throughput_tokens_s());
    value(wire::METRIC_GOODPUT_TOKENS_PER_S, r.goodput_tokens_s());
    value(wire::METRIC_SLO_ATTAINMENT, r.slo_attainment());
    value(wire::METRIC_LOAD_IMBALANCE_CV, r.load_imbalance_cv());
    value(wire::METRIC_READY_REPLICAS, r.scenario.replicas as f64);

    for (metric, hist) in [
        (wire::METRIC_TTFT, &r.ttft),
        (wire::METRIC_ITL, &r.itl_max),
        (wire::METRIC_E2E, &r.e2e),
        (wire::METRIC_QUEUE_WAIT, &r.queue_wait),
    ] {
        if hist.count() == 0 {
            continue;
        }
        card.distributions.push((
            metric,
            Distribution {
                count: hist.count(),
                mean: hist.mean(),
                min: hist.min() as f64,
                max: hist.max() as f64,
                percentile: PERCENTILES.to_vec(),
                value: PERCENTILES.iter().map(|q| hist.percentile(*q) as f64).collect(),
                // The engine's whole-run histograms are bucketed, so these percentiles are accurate
                // at bucket resolution rather than exact. That is what the flag warns a reader
                // about, whether or not a merge happened; the windowed rows below are exact.
                from_merged_histogram: true,
            },
        ));
    }
    for (label, number) in wire::OUTCOMES {
        card.outcome_counts.push((*number, r.outcome(label)));
    }
    wire::RunResult {
        run_id: run_id.to_string(),
        seed: r.scenario.seed,
        event_count: r.events,
        state_checksum: r.fingerprint,
        overall: card,
    }
}

/// What one sample window saw: the requests that finished inside it.
#[derive(Default)]
struct Window {
    all: u64,
    completed: u64,
    ok: u64,
    rejected: u64,
    output_tokens: u64,
    good_tokens: u64,
    ttft: Vec<u64>,
    itl: Vec<u64>,
    e2e: Vec<u64>,
    queue_wait: Vec<u64>,
}

/// Bucket the measured records by `finished_at` into the sample windows: window `k` is
/// `(t_k - interval, t_k]`. A request that finished after the last sample instant, in the tail the
/// engine never sampled, lands in the last window rather than nowhere, so the windows partition the
/// records and their sums equal the scorecard's.
fn windows(r: &RunResult) -> Vec<Window> {
    let n = r.fleet_queue.t.len();
    let mut out: Vec<Window> = (0..n).map(|_| Window::default()).collect();
    if n == 0 {
        return out;
    }
    let start = sim_start(r);
    let iv = sample_interval(r).max(1);
    for rec in &r.records {
        let k = (rec.finished_at.saturating_sub(start) + iv - 1) / iv;
        let w = &mut out[(k as usize).clamp(1, n) - 1];
        w.all += 1;
        if rec.outcome.is_success() {
            w.completed += 1;
            w.output_tokens += rec.output_tokens as u64;
        }
        if rec.outcome == Outcome::Ok {
            w.ok += 1;
            w.good_tokens += rec.output_tokens as u64;
        }
        if rec.outcome == Outcome::Rejected {
            w.rejected += 1;
        }
        // The same membership rules as the engine's whole-run histograms, so a windowed p99 and the
        // scorecard's are the same statistic over different populations.
        if let Some(t) = rec.ttft() {
            w.ttft.push(t);
        }
        if rec.max_itl > 0 {
            w.itl.push(rec.max_itl);
        }
        if let Some(t) = rec.e2e() {
            w.e2e.push(t);
        }
        w.queue_wait.push(rec.queue_wait());
    }
    out
}

/// Coefficient of variation of per-replica load at one sample, the per-instant form of
/// `RunResult::load_imbalance_cv`. NaN when the fleet is idle, which the row then omits.
fn imbalance_at(r: &RunResult, s: usize) -> f64 {
    let n = r.replica_load.len();
    if n == 0 {
        return f64::NAN;
    }
    let vals = r.replica_load.iter().map(|series| series.v[s]);
    let mean = vals.clone().sum::<f64>() / n as f64;
    if mean <= 0.0 {
        return f64::NAN;
    }
    let var = vals.map(|x| (x - mean) * (x - mean)).sum::<f64>() / (n - 1).max(1) as f64;
    var.sqrt() / mean
}

/// The fleet-scope stream: one update per sample instant, the last one marked final.
pub fn fleet_rows(r: &RunResult) -> Vec<SubscriptionUpdate> {
    let wins = windows(r);
    let n = wins.len();
    let iv_s = sample_interval(r) as f64 / 1e9;
    let mut out = Vec::with_capacity(n);
    for (s, w) in wins.iter().enumerate() {
        let mut row = MetricRow::new(Target::Fleet);
        row.value(wire::METRIC_OFFERED_RPS, r.offered_rps.v[s]);
        row.value(wire::METRIC_QUEUED_SEQS, r.fleet_queue.v[s]);
        row.value(wire::METRIC_RUNNING_SEQS, r.fleet_running.v[s]);
        // The engine's series is in percent; the wire carries a fraction, which is what the
        // dashboard's thresholds (0.85 tight, 0.95 preempting) are written against.
        row.value(wire::METRIC_KV_UTILIZATION, r.fleet_kv_utilization.v[s] / 100.0);
        row.value(wire::METRIC_COMPLETED_RPS, w.completed as f64 / iv_s);
        // There is no admission series in today's RunResult, and a record's `admitted_at` is
        // overloaded for rejected requests, so admitted is reported as completed until the engine
        // records admissions per window. Under a shedding policy this understates admissions by the
        // requests that were admitted and then timed out.
        row.value(wire::METRIC_ADMITTED_RPS, w.completed as f64 / iv_s);
        row.value(wire::METRIC_REJECTED_RPS, w.rejected as f64 / iv_s);
        row.value(wire::METRIC_OUTPUT_TOKENS_PER_S, w.output_tokens as f64 / iv_s);
        row.value(wire::METRIC_GOODPUT_TOKENS_PER_S, w.good_tokens as f64 / iv_s);
        // Same denominator as the scorecard: every request that ended in the window, shed ones
        // included, so a policy cannot look good by shedding.
        row.value(
            wire::METRIC_SLO_ATTAINMENT,
            if w.all == 0 { f64::NAN } else { w.ok as f64 / w.all as f64 },
        );
        row.value(wire::METRIC_LOAD_IMBALANCE_CV, imbalance_at(r, s));
        row.value(wire::METRIC_READY_REPLICAS, r.scenario.replicas as f64);
        row.distribution(wire::METRIC_TTFT, Distribution::exact(&mut w.ttft.clone(), PERCENTILES));
        row.distribution(wire::METRIC_ITL, Distribution::exact(&mut w.itl.clone(), PERCENTILES));
        row.distribution(wire::METRIC_E2E, Distribution::exact(&mut w.e2e.clone(), PERCENTILES));
        row.distribution(
            wire::METRIC_QUEUE_WAIT,
            Distribution::exact(&mut w.queue_wait.clone(), PERCENTILES),
        );
        out.push(SubscriptionUpdate {
            subscription_id: EXPORT_SUBSCRIPTION_ID.to_string(),
            sim_time_unix_ns: r.fleet_queue.t[s],
            realtime_factor: 0.0,
            row,
            is_final: s + 1 == n,
        });
    }
    out
}

/// The per-replica rows for sample `s`, one `SCOPE_REPLICA` update per replica, in replica order.
///
/// The instant is the fleet row's, so a client can join the two streams on `sim_time_unix_ns`
/// alone. `frames[s]` is the same sample as `fleet_queue.t[s]`: the engine closes both at once, and
/// `tests/wire_export.rs` holds it to that. Step time crosses as seconds like every other duration
/// gauge, and KV as the same fraction the fleet row carries, so one threshold serves both scopes.
pub fn replica_rows(r: &RunResult, s: usize) -> Vec<SubscriptionUpdate> {
    let frame = &r.frames[s];
    let last_sample = s + 1 == r.frames.len();
    let n = frame.replicas.len();
    let mut out = Vec::with_capacity(n);
    for (id, rep) in frame.replicas.iter().enumerate() {
        let mut row = MetricRow::new(Target::Replica(id as u64));
        row.value(wire::METRIC_QUEUED_SEQS, rep.queued as f64);
        row.value(wire::METRIC_RUNNING_SEQS, rep.running as f64);
        row.value(wire::METRIC_KV_TOKENS_RESIDENT, rep.kv_tokens as f64);
        row.value(wire::METRIC_KV_UTILIZATION, rep.kv_tokens as f64 / r.scenario.kv_capacity_tokens.max(1.0));
        row.value(wire::METRIC_STEP_TIME, rep.last_step_ns as f64 / 1e9);
        out.push(SubscriptionUpdate {
            subscription_id: EXPORT_SUBSCRIPTION_ID.to_string(),
            sim_time_unix_ns: r.fleet_queue.t[s],
            realtime_factor: 0.0,
            row,
            is_final: last_sample && id + 1 == n,
        });
    }
    out
}

/// A filesystem- and URL-safe run id from a scenario name: lowercase, runs of anything but
/// alphanumerics collapsed to one dash.
pub fn slug(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    for c in name.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    let out = out.trim_matches('-').to_string();
    if out.is_empty() {
        "run".into()
    } else {
        out
    }
}

/// A run id becomes a directory path, so it may nest (`1-routing/p2c`) but may not escape `runs/`.
fn check_run_id(run_id: &str) -> Result<(), String> {
    let ok = !run_id.is_empty()
        && !run_id.starts_with('/')
        && run_id.split('/').all(|seg| !seg.is_empty() && seg != "." && seg != "..");
    if ok {
        Ok(())
    } else {
        Err(format!("run id {run_id:?} must be a relative path with no empty, '.' or '..' segments"))
    }
}

pub fn export_run(result: &RunResult, run_id: &str, dir: &Path) -> Result<(), String> {
    export_run_from(result, run_id, None, dir).map(|_| ())
}

/// `export_run`, also recording which scenario file the run came from, and returning the run's
/// directory.
pub fn export_run_from(
    r: &RunResult,
    run_id: &str,
    scenario_file: Option<&str>,
    dir: &Path,
) -> Result<PathBuf, String> {
    check_run_id(run_id)?;
    let run_dir = dir.join("runs").join(run_id);
    std::fs::create_dir_all(&run_dir).map_err(|e| format!("{}: {e}", run_dir.display()))?;
    let write = |name: &str, body: &str| -> Result<(), String> {
        let p = run_dir.join(name);
        std::fs::write(&p, body).map_err(|e| format!("{}: {e}", p.display()))
    };

    write("status.json", &(wire::run_status_json(&status(r, run_id)) + "\n"))?;
    write("scenario.txt", &r.scenario.to_text())?;
    write("result.json", &(wire::run_result_json(&result(r, run_id)) + "\n"))?;

    let mut lines = String::new();
    for u in fleet_rows(r) {
        lines.push_str(&wire::subscription_update_json(&u));
        lines.push('\n');
    }
    write("fleet.jsonl", &lines)?;

    // A separate file rather than interleaved rows: the dashboard reads the fleet stream for every
    // chart and the replica stream only for the heatmap, and an older export without this file is
    // still a complete run.
    let mut lines = String::new();
    for s in 0..r.frames.len() {
        for u in replica_rows(r, s) {
            lines.push_str(&wire::subscription_update_json(&u));
            lines.push('\n');
        }
    }
    write("replicas.jsonl", &lines)?;

    merge_index(&dir.join("runs").join("index.json"), &index_entry(r, run_id, scenario_file))?;
    Ok(run_dir)
}

/// `export_run_from`, plus the run's sampled traces as `traces.jsonl`, within `trace_budget_bytes`.
///
/// The traces travel beside the result rather than inside it because `sim_leaf::RunResult` does not
/// carry them yet. Seam for the engine: once `RunResult` holds a `traces` field, `export_run_from`
/// calls this with it and this signature goes away.
pub fn export_run_with_traces(
    r: &RunResult,
    traces: &[RequestTrace],
    run_id: &str,
    scenario_file: Option<&str>,
    dir: &Path,
    trace_budget_bytes: u64,
) -> Result<PathBuf, String> {
    let run_dir = export_run_from(r, run_id, scenario_file, dir)?;
    export_traces(traces, &run_dir, trace_budget_bytes)?;
    Ok(run_dir)
}

/// Bytes `traces.jsonl` may take per run when the caller does not say. `sim_report::dump` gives a
/// run 100 MB by default and allots 65% to requests and 30% to series; the traces take the 5% it
/// left unallocated, so a run's telemetry stays inside the budget it was already held to.
pub const DEFAULT_TRACE_BUDGET_BYTES: u64 = 5 * 1024 * 1024;

/// What `export_traces` wrote, and the `manifest.json` beside it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TraceManifest {
    pub kept: usize,
    pub available: usize,
    pub bytes: u64,
    pub budget_bytes: u64,
    /// `complete`, `failures only`, or `1 in N by latency stride, all failures kept`.
    pub sampling: String,
}

/// Write `traces.jsonl` into `run_dir`, stratified to fit `budget_bytes` the way `sim_report::dump`
/// stratifies `requests.csv`: every failure is kept, since failures are rare and are what a reader
/// opens the traces to find, and successes are sorted by latency and taken at an even stride, so the
/// tail survives. Uniform sampling would keep almost nothing above p99, which is the part worth
/// reading. `manifest.json` records how many were kept of how many, so nobody downstream mistakes
/// the sample for the population.
pub fn export_traces(traces: &[RequestTrace], run_dir: &Path, budget_bytes: u64) -> Result<TraceManifest, String> {
    let write = |name: &str, body: &str| -> Result<(), String> {
        let p = run_dir.join(name);
        std::fs::write(&p, body).map_err(|e| format!("{}: {e}", p.display()))
    };
    let (lines, manifest) = stratify_traces(traces, budget_bytes);
    write("traces.jsonl", &lines)?;
    let mut j = wire::Json::new();
    j.begin_object()
        .field_str("file", "traces.jsonl")
        .field_int("kept", manifest.kept as i64)
        .field_int("available", manifest.available as i64)
        .field_u64("bytes", manifest.bytes)
        .field_u64("budget_bytes", manifest.budget_bytes)
        .field_str("sampling", &manifest.sampling)
        .end_object();
    write("manifest.json", &(j.finish() + "\n"))?;
    Ok(manifest)
}

/// The body of `traces.jsonl` and its manifest. Lines keep the input order, so the file reads in
/// completion order whatever was dropped.
fn stratify_traces(traces: &[RequestTrace], budget_bytes: u64) -> (String, TraceManifest) {
    let encoded: Vec<String> = traces.iter().map(|t| trace_wire::request_trace_json(t) + "\n").collect();
    let size = |i: &usize| encoded[*i].len() as u64;

    let (failures, mut successes): (Vec<usize>, Vec<usize>) =
        (0..traces.len()).partition(|i| !traces[*i].record.outcome.is_success());
    let mut chosen: Vec<usize> = Vec::new();
    let mut used = 0u64;
    for i in failures {
        if used + size(&i) > budget_bytes {
            break;
        }
        used += size(&i);
        chosen.push(i);
    }
    let room = budget_bytes - used;
    let success_bytes: u64 = successes.iter().map(size).sum();

    let sampling = if success_bytes <= room {
        chosen.extend(successes.iter().copied());
        "complete".to_string()
    } else {
        // Slowest first, so the stride is anchored at the far tail: whatever else is dropped, the
        // worst request a reader would want to open is in the file.
        successes.sort_by_key(|i| std::cmp::Reverse(traces[*i].latency_ns()));
        // The stride that fits by average size, then widened until the chosen lines actually fit,
        // since a tail trace with more spans is longer than a median one.
        let mean = success_bytes / successes.len() as u64;
        let mut stride = successes.len().div_ceil((room / mean.max(1)).max(1) as usize).max(1);
        let picked = loop {
            let picked: Vec<usize> = successes.iter().step_by(stride).copied().collect();
            let bytes: u64 = picked.iter().map(size).sum();
            if bytes <= room || picked.len() <= 1 {
                break if bytes <= room { picked } else { Vec::new() };
            }
            stride += 1;
        };
        let label = if picked.is_empty() {
            "failures only".to_string()
        } else {
            format!("1 in {stride} by latency stride, all failures kept")
        };
        chosen.extend(picked);
        label
    };
    chosen.sort_unstable();

    let mut body = String::new();
    for i in &chosen {
        body.push_str(&encoded[*i]);
    }
    let manifest = TraceManifest {
        kept: chosen.len(),
        available: traces.len(),
        bytes: body.len() as u64,
        budget_bytes,
        sampling,
    };
    (body, manifest)
}

// ---------------------------------------------------------------------------
// The index
// ---------------------------------------------------------------------------

/// One line of `index.json`. The file is an array written one entry per line, which is what lets
/// several invocations merge without a JSON parser: each line is one run, found by its `run_id`.
fn index_entry(r: &RunResult, run_id: &str, scenario_file: Option<&str>) -> String {
    let mut j = wire::Json::new();
    j.begin_object()
        .field_str("run_id", run_id)
        .field_str("name", &r.scenario.name)
        .field_str("routing", &r.routing_label);
    if let Some(f) = scenario_file {
        j.field_str("scenario_file", f);
    }
    j.field_u64("sim_start_unix_ns", sim_start(r))
        .field_u64("sim_end_unix_ns", r.measured_to)
        .field_f64("sample_interval_ms", r.scenario.sample_interval_ms)
        .field_int("replicas", r.scenario.replicas as i64)
        .end_object();
    j.finish()
}

/// Replace or add `entry` in the index at `path`, keeping every other run. Entries are sorted by
/// run id so the file does not depend on export order, which is what makes a re-export byte-identical.
fn merge_index(path: &Path, entry: &str) -> Result<(), String> {
    let new_id = json_string_field(entry, "run_id").ok_or("index entry has no run_id")?;
    let mut entries: Vec<(String, String)> = Vec::new();
    if let Ok(existing) = std::fs::read_to_string(path) {
        for raw in existing.lines() {
            let line = raw.trim().trim_end_matches(',');
            if line.is_empty() || line == "[" || line == "]" {
                continue;
            }
            let id = json_string_field(line, "run_id").ok_or_else(|| {
                format!("{}: cannot merge, line is not a one-run object: {raw}", path.display())
            })?;
            if id != new_id {
                entries.push((id, line.to_string()));
            }
        }
    }
    entries.push((new_id, entry.to_string()));
    entries.sort_by(|a, b| a.0.cmp(&b.0));
    let mut text = String::from("[\n");
    for (i, (_, line)) in entries.iter().enumerate() {
        text.push_str(line);
        text.push_str(if i + 1 < entries.len() { ",\n" } else { "\n" });
    }
    text.push_str("]\n");
    std::fs::write(path, text).map_err(|e| format!("{}: {e}", path.display()))
}

/// The value of a top-level string field in a compact JSON object, unescaped. Enough parser for the
/// index, whose lines this module wrote; a hand-edited index that breaks the one-object-per-line
/// rule is refused by `merge_index` rather than misread.
fn json_string_field(obj: &str, key: &str) -> Option<String> {
    let mut j = wire::Json::new();
    j.begin_object().key(key);
    let needle = &j.finish()[1..];
    let rest = &obj[obj.find(needle)? + needle.len()..];
    let rest = rest.strip_prefix('"')?;
    let mut out = String::new();
    let mut chars = rest.chars();
    while let Some(c) = chars.next() {
        match c {
            '"' => return Some(out),
            '\\' => match chars.next()? {
                'n' => out.push('\n'),
                't' => out.push('\t'),
                'r' => out.push('\r'),
                'b' => out.push('\u{08}'),
                'f' => out.push('\u{0c}'),
                'u' => {
                    let hex: String = chars.by_ref().take(4).collect();
                    out.push(char::from_u32(u32::from_str_radix(&hex, 16).ok()?)?);
                }
                other => out.push(other),
            },
            c => out.push(c),
        }
    }
    None
}

// ---------------------------------------------------------------------------
// The demo groups
// ---------------------------------------------------------------------------

/// One of `run-demos.sh`'s six groups: a comparison of several scenario files, or one file swept
/// over a key. Run ids are `<group>/<slug of name>` for a comparison and `<group>/<key>=<value>` for
/// a sweep, so the dashboard finds a group by its number.
pub struct Demo {
    pub group: &'static str,
    pub files: &'static [&'static str],
    pub sweep: Option<(&'static str, &'static [&'static str])>,
}

/// Mirrors `run-demos.sh` exactly: same files, same sweeps, seeds from the files. A change there is a
/// change here. `demos_table_mirrors_run_demos_sh` in `tests/wire_export.rs` guards the two against
/// drifting apart.
pub const DEMOS: &[Demo] = &[
    Demo {
        group: "1-routing",
        files: &["route_round_robin.txt", "route_p2c.txt", "route_least_requests.txt", "route_random.txt"],
        sweep: None,
    },
    Demo {
        group: "2-staleness",
        files: &["route_least_requests.txt"],
        sweep: Some(("telemetry_interval_ms", &["100", "250", "500", "1000", "2000", "4000"])),
    },
    Demo {
        group: "3-chunking",
        files: &["route_p2c.txt"],
        sweep: Some(("step_token_budget", &["512", "1024", "2048", "4096", "8192", "16384"])),
    },
    Demo {
        group: "4-load-curve",
        files: &["route_p2c.txt"],
        sweep: Some(("arrival_rps", &["30", "70", "110", "150", "190", "230"])),
    },
    Demo {
        group: "5-long-context",
        files: &["route_p2c.txt"],
        sweep: Some(("long_probability", &["0.0", "0.04", "0.08", "0.16", "0.32"])),
    },
    Demo {
        group: "6-retry",
        files: &["retry_none.txt", "retry_budget.txt", "retry_storm.txt"],
        sweep: None,
    },
    Demo {
        group: "7-no-decode",
        files: &["route_round_robin_no_decode.txt", "route_p2c_no_decode.txt"],
        sweep: None,
    },
    Demo {
        group: "8-admission",
        files: &["admit_accept_all.txt", "admit_deadline_aware.txt"],
        sweep: None,
    },
    Demo {
        group: "9-fair-share",
        files: &["admit_tenants_accept_all.txt", "admit_fair_share.txt"],
        sweep: None,
    },
    Demo {
        group: "10-probes",
        files: &["route_p2c.txt", "route_least_kv_probe.txt"],
        sweep: None,
    },
    Demo {
        group: "11-preemption",
        files: &["kv_spiral_never.txt", "kv_spiral_swap.txt"],
        sweep: None,
    },
    Demo {
        group: "12-spec-decode",
        files: &["spec_off.txt", "spec_n4.txt"],
        sweep: None,
    },
];

/// Set one key, through the text form so there is exactly one place that knows the key names. The
/// same round trip `sim_report::apply_override` does; repeated here because ingress sits beside the
/// report crate in the layering, not above it.
pub fn override_key(sc: &mut Scenario, key: &str, value: &str) -> Result<(), String> {
    let mut replaced = false;
    let text: Vec<String> = sc
        .to_text()
        .lines()
        .map(|l| {
            if l.split('=').next().map(str::trim) == Some(key) {
                replaced = true;
                format!("{key} = {value}")
            } else {
                l.to_string()
            }
        })
        .collect();
    if !replaced {
        return Err(format!("unknown key {key:?}"));
    }
    *sc = Scenario::parse(&text.join("\n"))?;
    Ok(())
}

/// Export every demo group. `overrides` apply to every run before a sweep sets its own key, which
/// is how a test shortens the runs. Returns the run ids in export order.
pub fn export_demos(
    scenarios_dir: &Path,
    dir: &Path,
    overrides: &[(String, String)],
) -> Result<Vec<String>, String> {
    let mut ids = Vec::new();
    for demo in DEMOS {
        for file in demo.files {
            let path = scenarios_dir.join(file);
            let text = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
            let mut base = Scenario::parse(&text)?;
            for (k, v) in overrides {
                override_key(&mut base, k, v)?;
            }
            let source = path.to_string_lossy().into_owned();
            match demo.sweep {
                None => {
                    let id = format!("{}/{}", demo.group, slug(&base.name));
                    let r = sim_leaf::run(&base)?;
                    export_run_from(&r, &id, Some(&source), dir)?;
                    ids.push(id);
                }
                Some((key, values)) => {
                    for v in values {
                        let mut sc = base.clone();
                        override_key(&mut sc, key, v)?;
                        // The name `sim-run sweep` gives it, so the exported scenario.txt is the
                        // sweep's own.
                        sc.name = format!("{key} = {v}");
                        let id = format!("{}/{key}={v}", demo.group);
                        let r = sim_leaf::run(&sc)?;
                        export_run_from(&r, &id, Some(&source), dir)?;
                        ids.push(id);
                    }
                }
            }
        }
    }
    Ok(ids)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugs_are_safe_and_stable() {
        assert_eq!(slug("least_requests"), "least-requests");
        assert_eq!(slug("telemetry_interval_ms = 250"), "telemetry-interval-ms-250");
        assert_eq!(slug("  P2C / v2  "), "p2c-v2");
        assert_eq!(slug("///"), "run");
    }

    #[test]
    fn run_ids_cannot_escape_the_runs_directory() {
        assert!(check_run_id("1-routing/p2c").is_ok());
        assert!(check_run_id("../etc").is_err());
        assert!(check_run_id("/abs").is_err());
        assert!(check_run_id("a//b").is_err());
        assert!(check_run_id("").is_err());
    }

    #[test]
    fn index_field_reader_unescapes() {
        let line = r#"{"run_id":"a\"b\\c\né","name":"x"}"#;
        assert_eq!(json_string_field(line, "run_id").unwrap(), "a\"b\\c\né");
        assert_eq!(json_string_field(line, "name").unwrap(), "x");
        assert!(json_string_field(line, "routing").is_none());
    }

    #[test]
    fn index_merge_replaces_by_run_id_and_sorts() {
        let dir = std::env::temp_dir().join(format!("lbsim-index-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("index.json");
        merge_index(&path, r#"{"run_id":"b","name":"1"}"#).unwrap();
        merge_index(&path, r#"{"run_id":"a","name":"2"}"#).unwrap();
        merge_index(&path, r#"{"run_id":"b","name":"3"}"#).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert_eq!(text, "[\n{\"run_id\":\"a\",\"name\":\"2\"},\n{\"run_id\":\"b\",\"name\":\"3\"}\n]\n");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
