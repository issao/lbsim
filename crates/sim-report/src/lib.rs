//! The static HTML report, the text scorecard, and budgeted telemetry. The command line that drives
//! these lives in the `sim-run` binary.
//!
//! Self-contained HTML with inline SVG and no external assets, so a report can be opened from disk,
//! attached to a message, or committed. No chart library: every panel here is a polyline, a grid of
//! rectangles, or a table.

use sim_metrics::{Histogram, RequestRecord, Series};
use sim_scenario::Scenario;
use sim_leaf::{self as sim, RunResult};
use sim_core::Nanos;
use std::cmp::Ordering;
use std::fmt::{Display, Write as _};

/// Parsed command-line options shared by every subcommand.
pub struct Opts {
    pub out: Option<String>,
    pub telemetry: Option<String>,
    pub budget_mb: u64,
    pub overrides: Vec<(String, String)>,
}

pub fn split_args(rest: &[String]) -> Result<(Vec<String>, Opts), String> {
    let mut paths = Vec::new();
    let mut out = None;
    let mut telemetry = None;
    let mut budget_mb = DEFAULT_TELEMETRY_BUDGET_BYTES / (1024 * 1024);
    let mut overrides = Vec::new();
    let mut i = 0;
    while i < rest.len() {
        match rest[i].as_str() {
            "--telemetry" => {
                telemetry = rest.get(i + 1).cloned();
                i += 2;
            }
            "--telemetry-budget-mb" => {
                budget_mb = rest
                    .get(i + 1)
                    .and_then(|v| v.parse().ok())
                    .ok_or("--telemetry-budget-mb needs a number")?;
                i += 2;
            }
            "--out" => {
                out = rest.get(i + 1).cloned();
                i += 2;
            }
            "--set" => {
                let spec = rest.get(i + 1).ok_or("--set needs key=value")?;
                let (k, v) = spec.split_once('=').ok_or("--set needs key=value")?;
                overrides.push((k.to_string(), v.to_string()));
                i += 2;
            }
            p => {
                paths.push(p.to_string());
                i += 1;
            }
        }
    }
    Ok((paths, Opts { out, telemetry, budget_mb, overrides }))
}

pub fn apply_override(sc: &mut Scenario, key: &str, value: &str) -> Result<(), String> {
    // Round-trip through the text form so there is exactly one place that knows the key names.
    let mut text = sc.to_text();
    let mut replaced = false;
    text = text
        .lines()
        .map(|l| {
            if l.split('=').next().map(str::trim) == Some(key) {
                replaced = true;
                format!("{key} = {value}")
            } else {
                l.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    if !replaced {
        return Err(format!("unknown key {key:?}"));
    }
    *sc = Scenario::parse(&text)?;
    Ok(())
}

pub fn run_all(paths: &[String], overrides: &[(String, String)]) -> Result<Vec<RunResult>, String> {
    let mut runs = Vec::new();
    for p in paths {
        let text = std::fs::read_to_string(p).map_err(|e| format!("{p}: {e}"))?;
        let mut sc = Scenario::parse(&text)?;
        for (k, v) in overrides {
            apply_override(&mut sc, k, v)?;
        }
        eprintln!(
            "running {:<28} routing={:<18} offered={:.0} rps  rated={:.0} rps",
            sc.name, sc.routing, sc.arrival_rps, sc.rated_rps()
        );
        runs.push(sim::run(&sc)?);
    }
    Ok(runs)
}

/// Refuse to compare runs that differ in anything but policy.
///
/// Named independent random streams mean the workload does not shift when the policy changes, so a
/// difference in outcome is a difference in policy. That only holds if the seed and the workload
/// actually match, and letting someone compare two unlike runs is worse than refusing.
pub fn check_comparable(runs: &[RunResult]) -> Result<(), String> {
    let a = &runs[0].scenario;
    for r in &runs[1..] {
        let b = &r.scenario;
        if a.seed != b.seed {
            return Err(format!(
                "not comparable: seeds differ ({} vs {}). Same seed, or the difference you measure is luck.",
                a.seed, b.seed
            ));
        }
        let same_load = a.arrival_rps == b.arrival_rps
            && a.prompt_mean == b.prompt_mean
            && a.output_mean == b.output_mean
            && a.long_probability == b.long_probability
            && a.replicas == b.replicas;
        if !same_load {
            return Err("not comparable: the workload or fleet differs, not just the policy".into());
        }
    }
    Ok(())
}

/// The four latency distributions every artefact reports, as (csv key, prose label, accessor). One
/// list so the summary CSV and the HTML can never disagree about which distributions exist.
const HISTOGRAMS: [(&str, &str, fn(&RunResult) -> &Histogram); 4] = [
    ("ttft", "time to first token", |r| &r.ttft),
    ("itl_max", "worst gap between tokens", |r| &r.itl_max),
    ("e2e", "end to end", |r| &r.e2e),
    ("queue_wait", "queue wait", |r| &r.queue_wait),
];

/// Outcome labels in reporting order, shared by the summary CSV and the outcomes table.
const OUTCOMES: [&str; 5] = ["ok", "ok_slo_violated", "rejected", "timeout_queued", "timeout_running"];

/// How much telemetry a run may leave behind, in bytes.
///
/// Issao's constraint: a run must leave enough to analyse afterwards, but must not violate the
/// observability throughput rule the whole design rests on. So this is a subscription with a budget
/// rather than a firehose: whoever wants telemetry says so, says how much they can take, and gets a
/// stratified sample that fits with a manifest saying what was dropped. An unlabelled sample is worse
/// than no sample.
pub const DEFAULT_TELEMETRY_BUDGET_BYTES: u64 = 100 * 1024 * 1024;
const BYTES_PER_REQUEST_ROW: u64 = 160;
const BYTES_PER_SERIES_ROW: u64 = 48;

/// Write telemetry for later analysis by a human or an agent, inside a byte budget.
///
/// Per Issao: a run must leave enough detail to be analysed afterwards by a human or by an agent
/// generating policies or loads, and that rules out the HTML report as the only artefact, because an
/// agent should not have to scrape a chart. It must also not violate the throughput rule the rest of
/// the design rests on, so this is budgeted rather than complete.
///
/// When the full set does not fit, requests are **stratified** rather than truncated: every failure is
/// kept, since failures are rare and are what an analysis is usually looking for, and successes are
/// sampled evenly across the latency distribution so the tail survives. Uniform sampling would keep
/// almost nothing above the 99th percentile, which is the part worth reading. Series are decimated by
/// a stride. `manifest.csv` records the sampling rate, so nothing downstream can mistake a sample for
/// a census.
fn dump(runs: &[RunResult], dir: &str, budget_bytes: u64) -> Result<(), String> {
    use std::io::Write as _;
    std::fs::create_dir_all(dir).map_err(|e| format!("{dir}: {e}"))?;

    // Split the budget: most to requests, which is the file an agent reasons over, the rest to series.
    let per_run = budget_bytes / runs.len().max(1) as u64;
    let max_req_rows = (per_run * 65 / 100 / BYTES_PER_REQUEST_ROW).max(1_000) as usize;
    let max_ser_rows = (per_run * 30 / 100 / BYTES_PER_SERIES_ROW).max(1_000) as usize;

    let mut manifest = std::fs::File::create(format!("{dir}/manifest.csv"))
        .map_err(|e| format!("{dir}/manifest.csv: {e}"))?;
    writeln!(manifest, "run,file,rows_written,rows_available,sampling,note").ok();
    let write = |path: String, body: &str| std::fs::write(&path, body).map_err(|e| format!("{path}: {e}"));

    for (i, r) in runs.iter().enumerate() {
        let slug: String = r.scenario.name.chars().map(|c| if c.is_alphanumeric() { c } else { '_' }).collect();
        let stem = format!("{i:02}-{slug}");
        let base = format!("{dir}/{stem}");

        // Reproducibility starts at the resolved scenario, and it is tiny.
        write(format!("{base}.scenario.txt"), &r.scenario.to_text())?;
        write(format!("{base}.summary.csv"), &summary_csv(r))?;

        let (body, written, sampling) = requests_csv(r, max_req_rows);
        write(format!("{base}.requests.csv"), &body)?;
        writeln!(
            manifest,
            "{},{stem}.requests.csv,{written},{},{sampling},per-request detail",
            r.scenario.name, r.records.len()
        )
        .ok();

        let (body, written, available, sampling) = series_csv(r, max_ser_rows);
        write(format!("{base}.series.csv"), &body)?;
        writeln!(
            manifest,
            "{},{stem}.series.csv,{written},{available},{sampling},time series including per-replica load",
            r.scenario.name
        )
        .ok();
    }

    let used: u64 = std::fs::read_dir(dir)
        .map_err(|e| format!("{dir}: {e}"))?
        .filter_map(|e| e.ok())
        .filter_map(|e| e.metadata().ok())
        .map(|m| m.len())
        .sum();
    println!(
        "telemetry: {dir}/  {:.1} MB of a {:.0} MB budget, see manifest.csv for sampling",
        used as f64 / 1e6,
        budget_bytes as f64 / 1e6
    );
    if used > budget_bytes {
        return Err(format!(
            "telemetry wrote {:.1} MB against a {:.0} MB budget; the row-size estimates in \
             report.rs are wrong",
            used as f64 / 1e6,
            budget_bytes as f64 / 1e6
        ));
    }
    Ok(())
}

/// Run-level metrics including the determinism fingerprint. Bounded, so never sampled.
fn summary_csv(r: &RunResult) -> String {
    let mut s = String::from("metric,value\n");
    let mut row = |k: &str, v: String| {
        let _ = writeln!(s, "{k},{v}");
    };
    row("name", r.scenario.name.clone());
    row("routing", r.routing_label.clone());
    row("seed", r.scenario.seed.to_string());
    row("fingerprint", r.fingerprint.to_string());
    row("events", r.events.to_string());
    row("offered_rps", format!("{:.3}", r.scenario.arrival_rps));
    row("rated_rps", format!("{:.3}", r.rated_rps));
    row("effective_batch_limit", format!("{:.1}", r.scenario.effective_batch()));
    row("completed_rps", format!("{:.3}", r.completed_rps()));
    row("goodput_tokens_s", format!("{:.1}", r.goodput_tokens_s()));
    row("throughput_tokens_s", format!("{:.1}", r.throughput_tokens_s()));
    row("slo_attainment", format!("{:.6}", r.slo_attainment()));
    row("load_imbalance_cv", format!("{:.4}", r.load_imbalance_cv()));
    row("replicas_inspected_per_decision", r.replicas_inspected_per_decision.to_string());
    row("retries", r.retries.to_string());
    row("first_attempts", r.first_attempts.to_string());
    for q in [50.0, 90.0, 99.0, 99.9] {
        for (key, _, hist) in HISTOGRAMS {
            row(&format!("{key}_p{q}_ns"), hist(r).percentile(q).to_string());
        }
    }
    for label in OUTCOMES {
        row(&format!("outcome_{label}"), r.outcome(label).to_string());
    }
    if let Some((pre, post, ok)) = r.recovery() {
        row("recovery_queue_before", format!("{pre:.2}"));
        row("recovery_queue_after", format!("{post:.2}"));
        row("recovered", ok.to_string());
    }
    s
}

/// One row per request, where it went and what it experienced, so a policy generator can find
/// *which* requests suffered rather than only that a percentile moved. Stratified to fit `max_rows`;
/// returns the body, the rows kept and the sampling label for the manifest.
fn requests_csv(r: &RunResult, max_rows: usize) -> (String, usize, String) {
    let (failures, mut successes): (Vec<_>, Vec<_>) =
        r.records.iter().partition(|x| !x.outcome.is_success());
    // Every failure. They are rare and they are what an analysis looks for first.
    let mut chosen: Vec<&RequestRecord> = failures.iter().copied().take(max_rows).collect();
    let room = max_rows.saturating_sub(chosen.len());
    let sampling = if successes.len() <= room {
        chosen.extend(successes.iter().copied());
        "complete".to_string()
    } else if room == 0 {
        "failures only".to_string()
    } else {
        // Sort by end-to-end latency and take an even stride, which keeps the shape of the
        // distribution including its tail. Uniform random sampling would keep almost nothing
        // above the 99th percentile.
        successes.sort_by_key(|x| x.e2e().unwrap_or(0));
        let stride = successes.len().div_ceil(room);
        chosen.extend(successes.iter().step_by(stride).copied());
        format!("1 in {stride} by latency stride, all failures kept")
    };

    let mut s = String::from(
        "id,outcome,replica,attempts,arrived_at_unix_ns,admitted_at_unix_ns,\
         first_token_at_unix_ns,finished_at_unix_ns,prompt_tokens,output_tokens,\
         queue_wait_ns,ttft_ns,e2e_ns,mean_itl_ns,max_itl_ns\n",
    );
    for rec in &chosen {
        let _ = writeln!(
            s,
            "{},{},{},{},{},{},{},{},{},{},{},{},{},{},{}",
            rec.id, rec.outcome.label(), rec.replica, rec.attempts,
            rec.arrived_at, rec.admitted_at, rec.first_token_at, rec.finished_at,
            rec.prompt_tokens, rec.output_tokens,
            rec.queue_wait(),
            rec.ttft().map(|v| v.to_string()).unwrap_or_default(),
            rec.e2e().map(|v| v.to_string()).unwrap_or_default(),
            rec.mean_itl, rec.max_itl
        );
    }
    (s, chosen.len(), sampling)
}

/// Every fleet series plus per-replica load in long format, so the hotspot is analysable rather than
/// only visible. Decimated by one stride to fit `max_rows`; returns the body, rows written, rows
/// available and the sampling label.
fn series_csv(r: &RunResult, max_rows: usize) -> (String, usize, usize, String) {
    let all: Vec<&Series> = [&r.fleet_queue, &r.fleet_running, &r.fleet_kv_utilization, &r.offered_rps]
        .into_iter()
        .chain(r.replica_load.iter())
        .collect();
    let available: usize = all.iter().map(|s| s.t.len()).sum();
    let stride = available.div_ceil(max_rows).max(1);
    let mut s = String::from("series,t_unix_ns,value\n");
    let mut written = 0usize;
    for series in &all {
        for (k, (t, v)) in series.t.iter().zip(series.v.iter()).enumerate() {
            if k % stride == 0 {
                let _ = writeln!(s, "{},{},{:.4}", series.name, t, v);
                written += 1;
            }
        }
    }
    let sampling = if stride == 1 { "complete".into() } else { format!("1 in {stride}") };
    (s, written, available, sampling)
}

pub fn emit(runs: &[RunResult], out: &str, opts: &Opts) -> Result<(), String> {
    if let Some(dir) = std::path::Path::new(out).parent() {
        std::fs::create_dir_all(dir).ok();
    }
    let html = render(runs);
    std::fs::write(out, html).map_err(|e| format!("{out}: {e}"))?;
    // Telemetry is opt-in and budgeted; see `dump`. Opt-in rather than automatic because it is a
    // subscription: whoever wants it says so and says how much they can take.
    if let Some(dir) = &opts.telemetry {
        dump(runs, dir, opts.budget_mb * 1024 * 1024)?;
    }
    println!();
    print_summary(runs);
    println!("\nreport: {out}");
    Ok(())
}

fn ms(v: Nanos) -> String {
    format!("{:.0}", v as f64 / 1e6)
}

fn print_summary(runs: &[RunResult]) {
    for r in runs {
        if let Some((pre, post, ok)) = r.recovery() {
            println!(
                "recovery {:<26} attempts={} budget={:.0}% retries={:<6} queue {:.0} -> {:.0}  {}",
                truncate(&r.scenario.name, 26), r.scenario.max_attempts,
                r.scenario.retry_budget_fraction * 100.0, r.retries, pre, post,
                if ok { "recovered" } else { "STAYED COLLAPSED" }
            );
        }
    }

    println!(
        "{:<32} {:>9} {:>9} {:>8} {:>8} {:>8} {:>7} {:>8}",
        "scenario", "goodput", "thruput", "ttft99", "itl99", "e2e99", "slo", "imbal"
    );
    println!("{}", "-".repeat(98));
    for r in runs {
        println!(
            "{:<32} {:>9.0} {:>9.0} {:>8} {:>8} {:>8} {:>6.1}% {:>8.2}",
            truncate(&r.scenario.name, 32),
            r.goodput_tokens_s(),
            r.throughput_tokens_s(),
            ms(r.ttft.percentile(99.0)),
            ms(r.itl_max.percentile(99.0)),
            ms(r.e2e.percentile(99.0)),
            r.slo_attainment() * 100.0,
            r.load_imbalance_cv()
        );
    }
    println!("\ngoodput and throughput in output tokens/s; latencies in ms; imbal = per-replica load CV");
}

fn truncate(s: &str, n: usize) -> String {
    if s.len() <= n {
        s.to_string()
    } else {
        s[..n].to_string()
    }
}

// ---------------------------------------------------------------------------
// HTML
// ---------------------------------------------------------------------------

const PALETTE: [&str; 6] = ["#1565C0", "#C62828", "#2E7D32", "#F9A825", "#6A1B9A", "#00838F"];
/// Every chart shares one width so the time axes line up down the page; heights vary by panel.
const CHART_W: usize = 1000;
/// Left gutter that holds the axis labels.
const PAD: f64 = 44.0;
/// Heatmap row height: thin, because a fleet of dozens of replicas still has to fit on one screen.
const ROW_H: usize = 8;

fn render(runs: &[RunResult]) -> String {
    let mut h = String::from(r##"<!doctype html><meta charset="utf-8"><title>lbsim report</title>
<style>
:root{color-scheme:light}
body{font:14px/1.55 -apple-system,BlinkMacSystemFont,"Segoe UI",Roboto,sans-serif;margin:0;background:#fbfbfa;color:#1a1a1a}
main{max-width:1180px;margin:0 auto;padding:32px 24px 64px}
h1{font-size:24px;margin:0 0 4px}
h2{font-size:17px;margin:36px 0 10px;padding-bottom:6px;border-bottom:1px solid #e3e3e0}
h3{font-size:14px;margin:22px 0 8px;color:#444}
p.note{color:#5a5a56;margin:6px 0 16px;max-width:72ch}
table{border-collapse:collapse;font-size:13px;margin:8px 0 4px}
th,td{padding:5px 11px;text-align:right;border-bottom:1px solid #ececea}
th:first-child,td:first-child{text-align:left}
th{font-weight:600;color:#444;background:#f4f4f2}
tr.best td{background:#f0f7f0}
code{background:#f0f0ee;padding:1px 4px;border-radius:3px;font-size:12px}
.legend{font-size:12px;margin:4px 0 10px}
.legend span{display:inline-block;margin-right:14px}
.sw{display:inline-block;width:10px;height:10px;border-radius:2px;margin-right:5px;vertical-align:middle}
.grid{display:grid;gap:22px}
figure{margin:0}
figcaption{font-size:12px;color:#5a5a56;margin-top:4px}
</style><main>"##);
    heading(&mut h, "h1", "lbsim report", r##"Package B of <code>docs/scope-today.md</code>. Queueing and control dynamics with
two-phase request timing. Key-value cache capacity, preemption, prefix caching, tiering and
autoscaling are deliberately absent; that document says why each was cut."##);

    findings(&mut h, runs);
    summary_table(&mut h, runs);
    slo_table(&mut h, runs);
    latency_table(&mut h, runs);
    outcome_table(&mut h, runs);

    heading(&mut h, "h2", "Fleet queue depth over time", r##"The signature of a load-balancing failure is here rather than in an average.
A rising line under one policy and a flat one under another, at identical offered load and seed, is
the whole result. Sustained ringing at a frequency related to the telemetry period is the control
loop oscillating."##);
    legend(&mut h, runs);
    multi_line_chart(&mut h, runs, |r| &r.fleet_queue, 240);

    heading(&mut h, "h2", "Key-value cache utilization", r##"Mean across the fleet, as a percentage of each replica's token budget.
Capacity here is a token budget rather than a request count, which is the central departure from a
stateless service: one 24,000-token context consumes what eight chat turns consume, so a queue of long
prompts blocks admission that a request count would have allowed."##);
    legend(&mut h, runs);
    multi_line_chart(&mut h, runs, |r| &r.fleet_kv_utilization, 170);

    heading(&mut h, "h2", "Offered load", "");
    legend(&mut h, runs);
    multi_line_chart(&mut h, runs, |r| &r.offered_rps, 130);

    for r in runs {
        heading(&mut h, "h2", &format!("Per-replica load: {}", esc(&r.scenario.name)), r##"One row per replica, time left to right. Dark means a deep queue. Under
round-robin with heterogeneous request sizes the dark patches move between replicas even though the
fleet is below its rated capacity: the rolling hotspot. A policy that samples rather than cycles
shows a flat field instead."##);
        heatmap(&mut h, r);
    }

    recovery_table(&mut h, runs);

    heading(&mut h, "h2", "Oscillation", r##"Dominant frequency in fleet queue depth, from a discrete Fourier transform of
the mean-removed signal, beside the telemetry sampling frequency. A peak near or below the sampling
frequency is the sampled-delayed feedback loop ringing, which is the thing to predict rather than
merely notice."##);
    oscillation_table(&mut h, runs);

    heading(&mut h, "h2", "Scenarios, verbatim", r##"Every number above is reproducible from these. Same seed, same output, byte for
byte; the fingerprint is an event-count-and-checksum pair asserted by the test suite."##);
    for r in runs {
        let _ = write!(
            h,
            "<h3>{}</h3><pre style=\"font-size:11.5px;background:#f4f4f2;padding:12px;overflow-x:auto\">{}</pre>",
            esc(&r.scenario.name),
            esc(&r.scenario.to_text())
        );
    }
    h.push_str("</main>");
    h
}

/// A heading and the note a reader needs to interpret what follows it. A section whose meaning is
/// self-evident passes an empty note and gets only the heading.
fn heading(h: &mut String, tag: &str, title: &str, note: &str) {
    let _ = write!(h, "<{tag}>{title}</{tag}>");
    if !note.is_empty() {
        let _ = write!(h, r##"<p class="note">{note}</p>"##);
    }
}

/// Header cells are raw HTML, because some carry a `<small>` qualifier; rows arrive fully rendered so
/// each table keeps its own number formatting and row attributes.
fn table(h: &mut String, headers: &[&str], rows: impl IntoIterator<Item = String>) {
    h.push_str("<table><tr>");
    for th in headers {
        let _ = write!(h, "<th>{th}</th>");
    }
    h.push_str("</tr>");
    for row in rows {
        h.push_str(&row);
    }
    h.push_str("</table>");
}

fn goodput_cmp(a: &RunResult, b: &RunResult) -> Ordering {
    a.goodput_tokens_s().partial_cmp(&b.goodput_tokens_s()).unwrap()
}

/// The runs at either end of an integer metric. Ties resolve as `min_by_key` and `max_by_key` do:
/// first minimum, last maximum.
fn extremes<K: Ord>(runs: &[RunResult], key: impl Fn(&RunResult) -> K) -> (&RunResult, &RunResult) {
    (runs.iter().min_by_key(|r| key(r)).unwrap(), runs.iter().max_by_key(|r| key(r)).unwrap())
}

/// Findings, computed from the run rather than written by hand.
///
/// Computed on purpose: prose asserting a ratio goes stale the moment a parameter changes, and a
/// report whose text contradicts its own tables is worse than one with no text at all.
fn findings(h: &mut String, runs: &[RunResult]) {
    if runs.len() < 2 {
        return;
    }
    heading(h, "h2", "What this run shows", "");
    h.push_str("<ul style=\"max-width:78ch;color:#333\">");

    let load_frac = runs[0].scenario.arrival_rps / runs[0].rated_rps;
    let _ = write!(
        h,
        "<li>Offered load is <b>{:.0} requests/s against a rated {:.0}</b>, so the fleet is at \
         <b>{:.0}% of capacity</b>. Everything below happens with capacity to spare, which is the \
         point: these are not overload results.</li>",
        runs[0].scenario.arrival_rps, runs[0].rated_rps, load_frac * 100.0
    );

    let best = runs.iter().max_by(|a, b| goodput_cmp(a, b)).unwrap();
    let worst = runs.iter().min_by(|a, b| goodput_cmp(a, b)).unwrap();
    if worst.goodput_tokens_s() > 0.0 {
        let _ = write!(
            h,
            "<li><b>Goodput spans {:.1}x</b> across these runs, from {:.0} tokens/s under \
             <i>{}</i> to {:.0} under <i>{}</i>, at identical offered load and seed. Throughput \
             barely moves ({:.0} to {:.0}), which is exactly why ranking on throughput picks the \
             wrong policy: the work gets done either way, but under the worse policy it arrives too \
             late to count.</li>",
            best.goodput_tokens_s() / worst.goodput_tokens_s(),
            worst.goodput_tokens_s(), esc(&worst.scenario.name),
            best.goodput_tokens_s(), esc(&best.scenario.name),
            worst.throughput_tokens_s(), best.throughput_tokens_s()
        );
    }

    let (t_best, t_worst) = extremes(runs, |r| r.ttft.percentile(99.0));
    let (lo, hi) = (t_best.ttft.percentile(99.0), t_worst.ttft.percentile(99.0));
    if lo > 0 {
        let _ = write!(
            h,
            "<li><b>Tail time-to-first-token spans {:.1}x</b>, {} ms under <i>{}</i> against {} ms \
             under <i>{}</i>. Per-replica load spread moves with it, {:.2} against {:.2}, which is \
             the mechanism rather than a coincidence: an uneven fleet has deep queues somewhere even \
             when its average queue is shallow.</li>",
            hi as f64 / lo as f64,
            ms(lo), esc(&t_best.scenario.name),
            ms(hi), esc(&t_worst.scenario.name),
            t_best.load_imbalance_cv(), t_worst.load_imbalance_cv()
        );
    }

    // The result worth stating loudest, when it is present: a policy that reads a stale global view
    // does worse than one that samples at random.
    let stale_loser = runs.iter().find(|r| r.replicas_inspected_per_decision > 4);
    let sampler = runs.iter().find(|r| r.replicas_inspected_per_decision <= 4);
    if let (Some(l), Some(s)) = (stale_loser, sampler) {
        if l.load_imbalance_cv() > s.load_imbalance_cv() {
            let _ = write!(
                h,
                "<li><b>Reading the whole fleet is worse than sampling two of it.</b> <i>{}</i> \
                 inspects all {} replicas per decision and reaches {:.0}% attainment with a load \
                 spread of {:.2}; <i>{}</i> inspects {} and reaches {:.0}% with {:.2}. The snapshot \
                 is up to {:.0} ms old, so every router sees the same apparently idle replica and \
                 sends to it at once. Sampling bounds that by construction, because only a fraction \
                 of decisions consider any one replica at a time.</li>",
                esc(&l.scenario.name), l.replicas_inspected_per_decision,
                l.slo_attainment() * 100.0, l.load_imbalance_cv(),
                esc(&s.scenario.name), s.replicas_inspected_per_decision,
                s.slo_attainment() * 100.0, s.load_imbalance_cv(),
                l.scenario.telemetry_interval_ms + l.scenario.telemetry_delay_ms
            );
        }
    }

    // A sweep over one key: state the direction rather than leaving the reader to infer it.
    let (a, b) = extremes(runs, |r| r.itl_max.percentile(99.0));
    let spread = b.itl_max.percentile(99.0) as f64 / a.itl_max.percentile(99.0).max(1) as f64;
    if spread > 2.0 {
        let _ = write!(
            h,
            "<li><b>The worst gap between tokens spans {:.1}x</b>, {} ms against {} ms, while tail \
             time-to-first-token moves the other way ({} against {} ms). That is prefill and decode \
             contending for one device: a larger prefill chunk gets the first token out sooner and \
             inserts a longer stall into every stream already running. There is no setting that \
             wins both.</li>",
            spread, ms(a.itl_max.percentile(99.0)), ms(b.itl_max.percentile(99.0)),
            ms(a.ttft.percentile(99.0)), ms(b.ttft.percentile(99.0))
        );
    }

    h.push_str("</ul>");
}

fn legend(h: &mut String, runs: &[RunResult]) {
    h.push_str(r##"<div class="legend">"##);
    for (i, r) in runs.iter().enumerate() {
        let _ = write!(
            h,
            r##"<span><i class="sw" style="background:{}"></i>{}</span>"##,
            PALETTE[i % PALETTE.len()],
            esc(&r.scenario.name)
        );
    }
    h.push_str("</div>");
}

fn summary_table(h: &mut String, runs: &[RunResult]) {
    heading(h, "h2", "Scorecard", r##"Goodput leads deliberately: a fleet can have excellent throughput and near-zero
goodput by making everyone slightly too slow, so ranking on throughput picks the wrong policy. The
imbalance column is the coefficient of variation of per-replica load, time-averaged, which is the
direct measure of whether the balancer is doing its job. Inspected is replicas examined per routing
decision, and anything proportional to fleet size does not hold at scale."##);
    let best = runs.iter().enumerate().max_by(|a, b| goodput_cmp(a.1, b.1)).map(|(i, _)| i);
    let headers = ["scenario", "routing", "offered rps", "rated rps", "completed rps", "goodput tok/s",
        "throughput tok/s", "imbalance CV", "batch limit", "inspected"];
    table(h, &headers, runs.iter().enumerate().map(|(i, r)| {
        format!(
            "<tr{}><td>{}</td><td>{}</td><td>{:.0}</td><td>{:.0}</td><td>{:.1}</td><td>{:.0}</td><td>{:.0}</td><td>{:.2}</td><td>{:.0}</td><td>{}</td></tr>",
            if Some(i) == best { " class=\"best\"" } else { "" },
            esc(&r.scenario.name),
            esc(&r.routing_label),
            r.scenario.arrival_rps,
            r.rated_rps,
            r.completed_rps(),
            r.goodput_tokens_s(),
            r.throughput_tokens_s(),
            r.load_imbalance_cv(),
            r.scenario.effective_batch(),
            r.replicas_inspected_per_decision
        )
    }));
}

fn slo_table(h: &mut String, runs: &[RunResult]) {
    heading(h, "h3", "Service level", "");
    let headers = ["scenario", "attainment<br><small>all requests</small>", "of those served",
        "ttft within SLO", "worst-gap within SLO", "retries"];
    table(h, &headers, runs.iter().map(|r| {
        let sc = &r.scenario;
        format!(
            "<tr><td>{}</td><td>{:.2}%</td><td>{:.2}%</td><td>{:.2}%</td><td>{:.2}%</td><td>{}</td></tr>",
            esc(&sc.name),
            r.slo_attainment() * 100.0,
            r.served_attainment() * 100.0,
            r.ttft.fraction_below((sc.ttft_slo_ms * 1e6) as u64) * 100.0,
            r.itl_max.fraction_below((sc.itl_slo_ms * 1e6) as u64) * 100.0,
            r.retries
        )
    }));
}

fn latency_table(h: &mut String, runs: &[RunResult]) {
    heading(h, "h2", "Latency", r##"Time-to-first-token and inter-token latency are separate quantities, which is
the observable that makes this domain different from a web service. The worst-gap column is the
largest pause between consecutive tokens within a single request: one long stall is what a user
perceives, and a mean hides it entirely."##);
    for (_, label, hist) in HISTOGRAMS {
        heading(h, "h3", &format!("{label} (ms)"), "");
        let headers = ["scenario", "count", "mean", "p50", "p90", "p99", "p99.9", "max"];
        table(h, &headers, runs.iter().map(|r| {
            let hist = hist(r);
            format!(
                "<tr><td>{}</td><td>{}</td><td>{:.0}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td></tr>",
                esc(&r.scenario.name),
                hist.count(),
                hist.mean() / 1e6,
                ms(hist.percentile(50.0)),
                ms(hist.percentile(90.0)),
                ms(hist.percentile(99.0)),
                ms(hist.percentile(99.9)),
                ms(hist.max())
            )
        }));
    }
}

fn outcome_table(h: &mut String, runs: &[RunResult]) {
    heading(h, "h3", "Outcomes", r##"When a request failed matters more than that it failed. Shedding early is cheap;
timing out after running has already burned device time on tokens nobody will read, which under
overload is most of the fleet's capacity."##);
    let headers = ["scenario", "ok", "ok but late", "rejected early", "timeout queued", "timeout running"];
    table(h, &headers, runs.iter().map(|r| {
        let mut row = format!("<tr><td>{}</td>", esc(&r.scenario.name));
        for label in OUTCOMES {
            let _ = write!(row, "<td>{}</td>", r.outcome(label));
        }
        row + "</tr>"
    }));
}

fn recovery_table(h: &mut String, runs: &[RunResult]) {
    if !runs.iter().any(|r| r.recovery().is_some()) {
        return;
    }
    heading(h, "h2", "Recovery after the spike", r##"The question is not whether latency rose during the spike, it is
whether the fleet came back afterwards. A metastable collapse is one it stays in once offered load
returns to normal, so only the tail of the run answers it. Queue depth is compared before the spike
against the final quarter."##);
    let headers = ["scenario", "attempts", "retry budget", "retries", "queue before", "queue after", "recovered"];
    table(h, &headers, runs.iter().filter_map(|r| {
        r.recovery().map(|(pre, post, ok)| {
            format!(
                "<tr><td>{}</td><td>{}</td><td>{:.0}%</td><td>{}</td><td>{:.0}</td><td>{:.0}</td><td><b>{}</b></td></tr>",
                esc(&r.scenario.name),
                r.scenario.max_attempts,
                r.scenario.retry_budget_fraction * 100.0,
                r.retries,
                pre, post,
                if ok { "yes" } else { "NO" }
            )
        })
    }));
}

fn oscillation_table(h: &mut String, runs: &[RunResult]) {
    let headers = ["scenario", "telemetry period", "telemetry delay", "dominant freq", "relative amplitude"];
    table(h, &headers, runs.iter().map(|r| {
        let iv = r.scenario.sample_interval_ms / 1000.0;
        let (f, amp) = r.fleet_queue.dominant_frequency(iv);
        format!(
            "<tr><td>{}</td><td>{:.0} ms</td><td>{:.0} ms</td><td>{:.3} Hz</td><td>{:.2}</td></tr>",
            esc(&r.scenario.name),
            r.scenario.telemetry_interval_ms,
            r.scenario.telemetry_delay_ms,
            f,
            amp
        )
    }));
}

fn frame(h: &mut String, hgt: usize) {
    let _ = write!(
        h,
        r##"<figure><svg viewBox="0 0 {CHART_W} {hgt}" width="100%" height="{hgt}" role="img"><rect width="{CHART_W}" height="{hgt}" fill="#fff" stroke="#e3e3e0"/>"##
    );
}

fn label(h: &mut String, x: impl Display, y: impl Display, anchor_end: bool, text: impl Display) {
    let anchor = if anchor_end { r##" text-anchor="end""## } else { "" };
    let _ = write!(h, r##"<text x="{x}" y="{y}" font-size="10" fill="#8a8a86"{anchor}>{text}</text>"##);
}

/// The time axis and caption every chart ends with, so the panels read as one system.
fn close_figure(h: &mut String, hgt: f64, right: impl Display, caption: &str) {
    label(h, PAD, hgt - 6.0, false, "0 s");
    label(h, CHART_W as f64 - 8.0, hgt - 6.0, true, right);
    let _ = write!(h, "</svg><figcaption>{caption}</figcaption></figure>");
}

fn multi_line_chart(h: &mut String, runs: &[RunResult], pick: fn(&RunResult) -> &Series, hgt: usize) {
    let n = runs.iter().map(|r| pick(r).v.len()).max().unwrap_or(0);
    let ymax = runs.iter().map(|r| pick(r).max()).fold(0.0, f64::max);
    let ymax = if ymax <= 0.0 { 1.0 } else { ymax } * 1.08;
    frame(h, hgt);
    for k in 0..=4 {
        let y = PAD / 2.0 + (hgt as f64 - PAD) * (k as f64 / 4.0);
        let _ = write!(
            h,
            r##"<line x1="{PAD}" y1="{y:.1}" x2="{}" y2="{y:.1}" stroke="#f0f0ee"/>"##,
            CHART_W as f64 - 8.0
        );
        label(h, PAD - 6.0, format_args!("{:.1}", y + 3.0), true, format_args!("{:.0}", ymax * (1.0 - k as f64 / 4.0)));
    }
    for (i, r) in runs.iter().enumerate() {
        let s = pick(r);
        if s.v.is_empty() {
            continue;
        }
        let pts: Vec<String> = s.v.iter().enumerate().map(|(j, v)| {
            let x = PAD + (CHART_W as f64 - PAD - 8.0) * (j as f64 / (n.max(2) - 1) as f64);
            let y = PAD / 2.0 + (hgt as f64 - PAD) * (1.0 - (v / ymax).clamp(0.0, 1.0));
            format!("{x:.1},{y:.1}")
        }).collect();
        let _ = write!(
            h,
            r##"<polyline points="{}" fill="none" stroke="{}" stroke-width="1.6"/>"##,
            pts.join(" "),
            PALETTE[i % PALETTE.len()]
        );
    }
    let secs = runs.first().map(|r| r.scenario.duration_s).unwrap_or(0.0);
    close_figure(h, hgt as f64, format_args!("{secs:.0} s"), "simulated time");
}

fn heatmap(h: &mut String, r: &RunResult) {
    let rows = r.replica_load.len();
    if rows == 0 {
        return;
    }
    let cols = r.replica_load[0].v.len().max(1);
    let hgt = rows * ROW_H + 26;
    let cw = (CHART_W as f64 - PAD - 8.0) / cols as f64;
    let vmax = r.replica_load.iter().map(Series::max).fold(1.0, f64::max);
    frame(h, hgt);
    for (ri, s) in r.replica_load.iter().enumerate() {
        let y = ri * ROW_H + 4;
        for (ci, v) in s.v.iter().enumerate() {
            if *v <= 0.0 {
                continue;
            }
            let t = (v / vmax).clamp(0.0, 1.0);
            // A single-hue ramp: light for idle, dark for a deep queue. One hue because the value is
            // a magnitude, not a category.
            let l = 96.0 - 62.0 * t;
            let x = PAD + ci as f64 * cw;
            let _ = write!(
                h,
                r##"<rect x="{x:.2}" y="{y}" width="{:.2}" height="{}" fill="hsl(212 62% {l:.0}%)"/>"##,
                cw.max(0.6),
                ROW_H - 1
            );
        }
    }
    label(h, PAD - 6.0, 14, true, "replica 0");
    label(h, PAD - 6.0, rows * ROW_H, true, format_args!("replica {}", rows - 1));
    close_figure(
        h,
        hgt as f64,
        format_args!("{:.0} s, darkest = {:.0} queued+running", r.scenario.duration_s, vmax),
        &esc(&r.routing_label),
    );
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}
