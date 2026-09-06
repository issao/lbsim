//! Command line interface and the static HTML report.
//!
//! Self-contained HTML with inline SVG and no external assets, so a report can be opened from disk,
//! attached to a message, or committed. No chart library: every panel here is a polyline, a grid of
//! rectangles, or a table.

use crate::metrics::Histogram;
use crate::scenario::Scenario;
use crate::sim::{self, RunResult};
use crate::Nanos;
use std::fmt::Write as _;

pub fn cli(args: Vec<String>) -> Result<(), String> {
    let mut args = args.into_iter();
    let cmd = args.next().unwrap_or_else(|| "help".into());
    let rest: Vec<String> = args.collect();
    match cmd.as_str() {
        "run" => {
            if rest.is_empty() {
                return Err("usage: sim-run run <scenario.txt> [--out FILE] [--set k=v ...]".into());
            }
            let (paths, out, overrides) = split_args(&rest)?;
            let runs = run_all(&paths, &overrides)?;
            emit(&runs, out.as_deref().unwrap_or("out/report.html"))
        }
        "compare" => {
            let (paths, out, overrides) = split_args(&rest)?;
            if paths.len() < 2 {
                return Err("compare needs at least two scenarios".into());
            }
            let runs = run_all(&paths, &overrides)?;
            check_comparable(&runs)?;
            emit(&runs, out.as_deref().unwrap_or("out/compare.html"))
        }
        "sweep" => {
            // One scenario, one key, several values. The cheapest way to see a trend.
            if rest.len() < 2 {
                return Err("usage: sim-run sweep <scenario.txt> --over key=v1,v2,v3".into());
            }
            let base = &rest[0];
            let mut key = String::new();
            let mut values: Vec<String> = Vec::new();
            let mut out = None;
            let mut i = 1;
            while i < rest.len() {
                match rest[i].as_str() {
                    "--over" => {
                        let spec = rest.get(i + 1).ok_or("--over needs key=v1,v2")?;
                        let (k, vs) = spec.split_once('=').ok_or("--over needs key=v1,v2")?;
                        key = k.to_string();
                        values = vs.split(',').map(|s| s.to_string()).collect();
                        i += 2;
                    }
                    "--out" => {
                        out = rest.get(i + 1).cloned();
                        i += 2;
                    }
                    other => return Err(format!("unexpected argument {other:?}")),
                }
            }
            let text = std::fs::read_to_string(base).map_err(|e| format!("{base}: {e}"))?;
            let mut runs = Vec::new();
            for v in &values {
                let mut sc = Scenario::parse(&text)?;
                apply_override(&mut sc, &key, v)?;
                sc.name = format!("{} = {}", key, v);
                runs.push(sim::run(&sc)?);
            }
            emit(&runs, out.as_deref().unwrap_or("out/sweep.html"))
        }
        _ => {
            println!("sim-run <command>");
            println!("  run     <scenario.txt> [...]      one or more scenarios into one report");
            println!("  compare <a.txt> <b.txt> [...]     same load, different policies, checked");
            println!("  sweep   <s.txt> --over key=v1,v2  one parameter across several values");
            println!("  options: --out FILE, --set key=value");
            Ok(())
        }
    }
}

fn split_args(rest: &[String]) -> Result<(Vec<String>, Option<String>, Vec<(String, String)>), String> {
    let mut paths = Vec::new();
    let mut out = None;
    let mut overrides = Vec::new();
    let mut i = 0;
    while i < rest.len() {
        match rest[i].as_str() {
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
    Ok((paths, out, overrides))
}

fn apply_override(sc: &mut Scenario, key: &str, value: &str) -> Result<(), String> {
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

fn run_all(paths: &[String], overrides: &[(String, String)]) -> Result<Vec<RunResult>, String> {
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
fn check_comparable(runs: &[RunResult]) -> Result<(), String> {
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

fn emit(runs: &[RunResult], out: &str) -> Result<(), String> {
    if let Some(dir) = std::path::Path::new(out).parent() {
        std::fs::create_dir_all(dir).ok();
    }
    let html = render(runs);
    std::fs::write(out, html).map_err(|e| format!("{out}: {e}"))?;
    println!();
    print_summary(runs);
    println!("\nreport: {out}");
    Ok(())
}

fn ms(v: Nanos) -> String {
    format!("{:.0}", v as f64 / 1e6)
}

fn print_summary(runs: &[RunResult]) {
    println!(
        "{:<26} {:>9} {:>9} {:>8} {:>8} {:>8} {:>7} {:>8}",
        "scenario", "goodput", "thruput", "ttft99", "itl99", "e2e99", "slo", "imbal"
    );
    println!("{}", "-".repeat(92));
    for r in runs {
        println!(
            "{:<26} {:>9.0} {:>9.0} {:>8} {:>8} {:>8} {:>6.1}% {:>8.2}",
            truncate(&r.scenario.name, 26),
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

fn render(runs: &[RunResult]) -> String {
    let mut h = String::new();
    let _ = write!(h, r##"<!doctype html><meta charset="utf-8"><title>lbsim report</title>
<style>
:root{{color-scheme:light}}
body{{font:14px/1.55 -apple-system,BlinkMacSystemFont,"Segoe UI",Roboto,sans-serif;margin:0;background:#fbfbfa;color:#1a1a1a}}
main{{max-width:1180px;margin:0 auto;padding:32px 24px 64px}}
h1{{font-size:24px;margin:0 0 4px}}
h2{{font-size:17px;margin:36px 0 10px;padding-bottom:6px;border-bottom:1px solid #e3e3e0}}
h3{{font-size:14px;margin:22px 0 8px;color:#444}}
p.note{{color:#5a5a56;margin:6px 0 16px;max-width:72ch}}
table{{border-collapse:collapse;font-size:13px;margin:8px 0 4px}}
th,td{{padding:5px 11px;text-align:right;border-bottom:1px solid #ececea}}
th:first-child,td:first-child{{text-align:left}}
th{{font-weight:600;color:#444;background:#f4f4f2}}
tr.best td{{background:#f0f7f0}}
code{{background:#f0f0ee;padding:1px 4px;border-radius:3px;font-size:12px}}
.legend{{font-size:12px;margin:4px 0 10px}}
.legend span{{display:inline-block;margin-right:14px}}
.sw{{display:inline-block;width:10px;height:10px;border-radius:2px;margin-right:5px;vertical-align:middle}}
.grid{{display:grid;gap:22px}}
figure{{margin:0}}
figcaption{{font-size:12px;color:#5a5a56;margin-top:4px}}
</style><main>"##);
    let _ = write!(h, "<h1>lbsim report</h1>");
    let _ = write!(
        h,
        r##"<p class="note">Package B of <code>docs/scope-today.md</code>. Queueing and control dynamics with
two-phase request timing. Key-value cache capacity, preemption, prefix caching, tiering and
autoscaling are deliberately absent; that document says why each was cut.</p>"##
    );

    summary_table(&mut h, runs);
    slo_table(&mut h, runs);
    latency_table(&mut h, runs);
    outcome_table(&mut h, runs);

    let _ = write!(h, "<h2>Fleet queue depth over time</h2>");
    let _ = write!(h, r##"<p class="note">The signature of a load-balancing failure is here rather than in an average.
A rising line under one policy and a flat one under another, at identical offered load and seed, is
the whole result. Sustained ringing at a frequency related to the telemetry period is the control
loop oscillating.</p>"##);
    legend(&mut h, runs);
    let _ = write!(h, "{}", multi_line_chart(runs, |r| &r.fleet_queue, 1000, 240));

    let _ = write!(h, "<h2>Offered load</h2>");
    legend(&mut h, runs);
    let _ = write!(h, "{}", multi_line_chart(runs, |r| &r.offered_rps, 1000, 130));

    for (i, r) in runs.iter().enumerate() {
        let _ = write!(h, "<h2>Per-replica load: {}</h2>", esc(&r.scenario.name));
        let _ = write!(h, r##"<p class="note">One row per replica, time left to right. Dark means a deep queue. Under
round-robin with heterogeneous request sizes the dark patches move between replicas even though the
fleet is below its rated capacity: the rolling hotspot. A policy that samples rather than cycles
shows a flat field instead.</p>"##);
        let _ = write!(h, "{}", heatmap(r, 1000, 8, i));
    }

    let _ = write!(h, "<h2>Oscillation</h2>");
    let _ = write!(h, r##"<p class="note">Dominant frequency in fleet queue depth, from a discrete Fourier transform of
the mean-removed signal, beside the telemetry sampling frequency. A peak near or below the sampling
frequency is the sampled-delayed feedback loop ringing, which is the thing to predict rather than
merely notice.</p>"##);
    oscillation_table(&mut h, runs);

    let _ = write!(h, "<h2>Scenarios, verbatim</h2>");
    let _ = write!(h, r##"<p class="note">Every number above is reproducible from these. Same seed, same output, byte for
byte; the fingerprint is an event-count-and-checksum pair asserted by the test suite.</p>"##);
    for r in runs {
        let _ = write!(
            h,
            "<h3>{}</h3><pre style=\"font-size:11.5px;background:#f4f4f2;padding:12px;overflow-x:auto\">{}</pre>",
            esc(&r.scenario.name),
            esc(&r.scenario.to_text())
        );
    }
    let _ = write!(h, "</main>");
    h
}

fn legend(h: &mut String, runs: &[RunResult]) {
    let _ = write!(h, r##"<div class="legend">"##);
    for (i, r) in runs.iter().enumerate() {
        let _ = write!(
            h,
            r##"<span><i class="sw" style="background:{}"></i>{}</span>"##,
            PALETTE[i % PALETTE.len()],
            esc(&r.scenario.name)
        );
    }
    let _ = write!(h, "</div>");
}

fn summary_table(h: &mut String, runs: &[RunResult]) {
    let _ = write!(h, "<h2>Scorecard</h2>");
    let _ = write!(h, r##"<p class="note">Goodput leads deliberately: a fleet can have excellent throughput and near-zero
goodput by making everyone slightly too slow, so ranking on throughput picks the wrong policy. The
imbalance column is the coefficient of variation of per-replica load, time-averaged, which is the
direct measure of whether the balancer is doing its job. Inspected is replicas examined per routing
decision, and anything proportional to fleet size does not hold at scale.</p>"##);
    let best = runs
        .iter()
        .enumerate()
        .max_by(|a, b| a.1.goodput_tokens_s().partial_cmp(&b.1.goodput_tokens_s()).unwrap())
        .map(|(i, _)| i);
    let _ = write!(h, "<table><tr><th>scenario</th><th>routing</th><th>offered rps</th><th>rated rps</th><th>completed rps</th><th>goodput tok/s</th><th>throughput tok/s</th><th>imbalance CV</th><th>inspected</th></tr>");
    for (i, r) in runs.iter().enumerate() {
        let cls = if Some(i) == best { " class=\"best\"" } else { "" };
        let _ = write!(
            h,
            "<tr{}><td>{}</td><td>{}</td><td>{:.0}</td><td>{:.0}</td><td>{:.1}</td><td>{:.0}</td><td>{:.0}</td><td>{:.2}</td><td>{}</td></tr>",
            cls,
            esc(&r.scenario.name),
            esc(&r.routing_label),
            r.scenario.arrival_rps,
            r.rated_rps,
            r.completed_rps(),
            r.goodput_tokens_s(),
            r.throughput_tokens_s(),
            r.load_imbalance_cv(),
            r.replicas_inspected_per_decision
        );
    }
    let _ = write!(h, "</table>");
}

fn slo_table(h: &mut String, runs: &[RunResult]) {
    let _ = write!(h, "<h3>Service level</h3>");
    let _ = write!(h, "<table><tr><th>scenario</th><th>attainment</th><th>ttft within SLO</th><th>worst-gap within SLO</th><th>retries</th></tr>");
    for r in runs {
        let sc = &r.scenario;
        let _ = write!(
            h,
            "<tr><td>{}</td><td>{:.2}%</td><td>{:.2}%</td><td>{:.2}%</td><td>{}</td></tr>",
            esc(&sc.name),
            r.slo_attainment() * 100.0,
            r.ttft.fraction_below((sc.ttft_slo_ms * 1e6) as u64) * 100.0,
            r.itl_max.fraction_below((sc.itl_slo_ms * 1e6) as u64) * 100.0,
            r.retries
        );
    }
    let _ = write!(h, "</table>");
}

fn latency_table(h: &mut String, runs: &[RunResult]) {
    let _ = write!(h, "<h2>Latency</h2>");
    let _ = write!(h, r##"<p class="note">Time-to-first-token and inter-token latency are separate quantities, which is
the observable that makes this domain different from a web service. The worst-gap column is the
largest pause between consecutive tokens within a single request: one long stall is what a user
perceives, and a mean hides it entirely.</p>"##);
    for (label, pick) in [
        ("time to first token", 0usize),
        ("worst gap between tokens", 1),
        ("end to end", 2),
        ("queue wait", 3),
    ] {
        let _ = write!(h, "<h3>{label} (ms)</h3><table><tr><th>scenario</th><th>count</th><th>mean</th><th>p50</th><th>p90</th><th>p99</th><th>p99.9</th><th>max</th></tr>");
        for r in runs {
            let hist: &Histogram = match pick {
                0 => &r.ttft,
                1 => &r.itl_max,
                2 => &r.e2e,
                _ => &r.queue_wait,
            };
            let _ = write!(
                h,
                "<tr><td>{}</td><td>{}</td><td>{:.0}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td></tr>",
                esc(&r.scenario.name),
                hist.count(),
                hist.mean() / 1e6,
                ms(hist.percentile(50.0)),
                ms(hist.percentile(90.0)),
                ms(hist.percentile(99.0)),
                ms(hist.percentile(99.9)),
                ms(hist.max())
            );
        }
        let _ = write!(h, "</table>");
    }
}

fn outcome_table(h: &mut String, runs: &[RunResult]) {
    let _ = write!(h, "<h3>Outcomes</h3>");
    let _ = write!(h, r##"<p class="note">When a request failed matters more than that it failed. Shedding early is cheap;
timing out after running has already burned device time on tokens nobody will read, which under
overload is most of the fleet's capacity.</p>"##);
    let _ = write!(h, "<table><tr><th>scenario</th><th>ok</th><th>ok but late</th><th>rejected early</th><th>timeout queued</th><th>timeout running</th></tr>");
    for r in runs {
        let _ = write!(
            h,
            "<tr><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td></tr>",
            esc(&r.scenario.name),
            r.outcome("ok"),
            r.outcome("ok_slo_violated"),
            r.outcome("rejected"),
            r.outcome("timeout_queued"),
            r.outcome("timeout_running")
        );
    }
    let _ = write!(h, "</table>");
}

fn oscillation_table(h: &mut String, runs: &[RunResult]) {
    let _ = write!(h, "<table><tr><th>scenario</th><th>telemetry period</th><th>telemetry delay</th><th>dominant freq</th><th>relative amplitude</th></tr>");
    for r in runs {
        let iv = r.scenario.sample_interval_ms / 1000.0;
        let (f, amp) = r.fleet_queue.dominant_frequency(iv);
        let _ = write!(
            h,
            "<tr><td>{}</td><td>{:.0} ms</td><td>{:.0} ms</td><td>{:.3} Hz</td><td>{:.2}</td></tr>",
            esc(&r.scenario.name),
            r.scenario.telemetry_interval_ms,
            r.scenario.telemetry_delay_ms,
            f,
            amp
        );
    }
    let _ = write!(h, "</table>");
}

fn multi_line_chart(
    runs: &[RunResult],
    pick: fn(&RunResult) -> &crate::metrics::Series,
    w: usize,
    hgt: usize,
) -> String {
    let pad = 44.0;
    let mut ymax = 0.0f64;
    let mut n = 0usize;
    for r in runs {
        let s = pick(r);
        ymax = ymax.max(s.max());
        n = n.max(s.v.len());
    }
    if ymax <= 0.0 {
        ymax = 1.0;
    }
    let ymax = ymax * 1.08;
    let mut svg = format!(
        r##"<figure><svg viewBox="0 0 {w} {hgt}" width="100%" height="{hgt}" role="img"><rect width="{w}" height="{hgt}" fill="#fff" stroke="#e3e3e0"/>"##
    );
    // Gridlines and y labels.
    for k in 0..=4 {
        let y = pad / 2.0 + (hgt as f64 - pad) * (k as f64 / 4.0);
        let val = ymax * (1.0 - k as f64 / 4.0);
        let _ = write!(
            svg,
            r##"<line x1="{pad}" y1="{y:.1}" x2="{}" y2="{y:.1}" stroke="#f0f0ee"/><text x="{}" y="{:.1}" font-size="10" fill="#8a8a86" text-anchor="end">{:.0}</text>"##,
            w as f64 - 8.0,
            pad - 6.0,
            y + 3.0,
            val
        );
    }
    for (i, r) in runs.iter().enumerate() {
        let s = pick(r);
        if s.v.is_empty() {
            continue;
        }
        let mut pts = String::new();
        for (j, v) in s.v.iter().enumerate() {
            let x = pad + (w as f64 - pad - 8.0) * (j as f64 / (n.max(2) - 1) as f64);
            let y = pad / 2.0 + (hgt as f64 - pad) * (1.0 - (v / ymax).clamp(0.0, 1.0));
            let _ = write!(pts, "{x:.1},{y:.1} ");
        }
        let _ = write!(
            svg,
            r##"<polyline points="{}" fill="none" stroke="{}" stroke-width="1.6"/>"##,
            pts.trim(),
            PALETTE[i % PALETTE.len()]
        );
    }
    let secs = runs
        .first()
        .map(|r| r.scenario.duration_s)
        .unwrap_or(0.0);
    let _ = write!(
        svg,
        r##"<text x="{pad}" y="{}" font-size="10" fill="#8a8a86">0 s</text><text x="{}" y="{}" font-size="10" fill="#8a8a86" text-anchor="end">{:.0} s</text></svg><figcaption>simulated time</figcaption></figure>"##,
        hgt as f64 - 6.0,
        w as f64 - 8.0,
        hgt as f64 - 6.0,
        secs
    );
    svg
}

fn heatmap(r: &RunResult, w: usize, row_h: usize, _idx: usize) -> String {
    let rows = r.replica_load.len();
    if rows == 0 {
        return String::new();
    }
    let cols = r.replica_load[0].v.len().max(1);
    let hgt = rows * row_h + 26;
    let pad = 44.0;
    let cw = (w as f64 - pad - 8.0) / cols as f64;
    let mut vmax = 1.0f64;
    for s in &r.replica_load {
        vmax = vmax.max(s.max());
    }
    let mut svg = format!(
        r##"<figure><svg viewBox="0 0 {w} {hgt}" width="100%" height="{hgt}" role="img"><rect width="{w}" height="{hgt}" fill="#fff" stroke="#e3e3e0"/>"##
    );
    for (ri, s) in r.replica_load.iter().enumerate() {
        let y = ri * row_h + 4;
        for (ci, v) in s.v.iter().enumerate() {
            if *v <= 0.0 {
                continue;
            }
            let t = (v / vmax).clamp(0.0, 1.0);
            // A single-hue ramp: light for idle, dark for a deep queue. One hue because the value is
            // a magnitude, not a category.
            let l = 96.0 - 62.0 * t;
            let x = pad + ci as f64 * cw;
            let _ = write!(
                svg,
                r##"<rect x="{x:.2}" y="{y}" width="{:.2}" height="{}" fill="hsl(212 62% {l:.0}%)"/>"##,
                cw.max(0.6),
                row_h - 1
            );
        }
    }
    let _ = write!(
        svg,
        r##"<text x="{}" y="14" font-size="10" fill="#8a8a86" text-anchor="end">replica 0</text><text x="{}" y="{}" font-size="10" fill="#8a8a86" text-anchor="end">replica {}</text>"##,
        pad - 6.0,
        pad - 6.0,
        rows * row_h,
        rows - 1
    );
    let _ = write!(
        svg,
        r##"<text x="{pad}" y="{}" font-size="10" fill="#8a8a86">0 s</text><text x="{}" y="{}" font-size="10" fill="#8a8a86" text-anchor="end">{:.0} s, darkest = {:.0} queued+running</text></svg><figcaption>{}</figcaption></figure>"##,
        hgt - 6,
        w as f64 - 8.0,
        hgt - 6,
        r.scenario.duration_s,
        vmax,
        esc(&r.routing_label)
    );
    svg
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}
