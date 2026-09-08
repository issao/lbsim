//! The wire encoder and the static export, held to WIRE.md.
//!
//! The documents under `runs/` are what the dashboard reads before a live server exists, and what
//! the live server will send once it does. Three things are worth proving rather than trusting: the
//! field names are the proto's, a re-export is byte-identical, and the windowed rows add up to the
//! scorecard, so a chart of the run and the headline number cannot disagree.

mod common;

use lbsim::sim::{self, RunResult};
use sim_ingress::{export, wire};
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

fn workspace() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

/// A fresh directory per test, so tests can run in parallel and a stale index cannot leak between
/// them.
fn fresh_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("lbsim-wire-export-{}-{name}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn small_run() -> RunResult {
    let mut sc = common::at_load(0.9);
    // A name with characters that must be escaped and slugged.
    sc.name = "wire \"export\" / small".into();
    sim::run(&sc).expect("small scenario runs")
}

fn read(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// Every `"key":` in a JSON document. Keys are the only strings immediately followed by a colon,
/// and the documents here never contain a colon inside a string value that is followed by a quote
/// and colon, so a scan is enough.
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
    let text = read(&workspace().join("proto/lbsim/v1").join(file));
    text.lines()
        .filter_map(|l| {
            let l = l.split("//").next()?.trim();
            let (decl, _) = l.split_once('=')?;
            let name = decl.split_whitespace().last()?;
            Some(name.to_string())
        })
        .collect()
}

#[test]
fn emitted_field_names_exist_in_the_protos() {
    let dir = fresh_dir("names");
    let r = small_run();
    let run_dir = export::export_run_from(&r, "names", None, &dir).unwrap();

    // `result.json` is `GetResult`'s `RunResult`, whose message lives in metrics.proto, which
    // ingress.proto imports for exactly that RPC.
    let mut allowed = proto_field_names("ingress.proto");
    allowed.extend(proto_field_names("subscription.proto"));
    allowed.extend(proto_field_names("metrics.proto"));

    let mut offenders = Vec::new();
    for doc in ["status.json", "result.json", "fleet.jsonl", "replicas.jsonl"] {
        for key in json_keys(&read(&run_dir.join(doc))) {
            // Enum-keyed maps use the enum number as the key; WIRE.md rule 4.
            if key.chars().all(|c| c.is_ascii_digit()) {
                continue;
            }
            if !allowed.contains(&key) {
                offenders.push(format!("{doc}: {key}"));
            }
        }
    }
    assert!(offenders.is_empty(), "field names not in any proto:\n  {}", offenders.join("\n  "));

    // The one deviation, and it is exactly the documented one.
    let index_keys = json_keys(&read(&dir.join("runs/index.json")));
    let expected: BTreeSet<String> = [
        "run_id", "name", "routing", "sim_start_unix_ns", "sim_end_unix_ns", "sample_interval_ms", "replicas",
    ]
    .into_iter()
    .map(String::from)
    .collect();
    assert_eq!(index_keys, expected);
}

#[test]
fn export_is_byte_identical_on_rerun() {
    let a = fresh_dir("rerun-a");
    let b = fresh_dir("rerun-b");
    let r = small_run();
    export::export_run(&r, "x/y", &a).unwrap();
    export::export_run(&small_run(), "x/y", &b).unwrap();
    // Exporting into the directory a second time must leave it unchanged too: that exercises the
    // index merge, which replaces rather than appends.
    export::export_run(&r, "x/y", &a).unwrap();

    for name in [
        "index.json",
        "x/y/status.json",
        "x/y/scenario.txt",
        "x/y/result.json",
        "x/y/fleet.jsonl",
        "x/y/replicas.jsonl",
    ] {
        let pa = a.join("runs").join(name);
        let pb = b.join("runs").join(name);
        assert_eq!(fs::read(&pa).unwrap(), fs::read(&pb).unwrap(), "{name} differs between exports");
    }
}

#[test]
fn fleet_rows_have_one_line_per_sample_and_a_final_flag() {
    let dir = fresh_dir("rows");
    let r = small_run();
    let run_dir = export::export_run_from(&r, "rows", None, &dir).unwrap();
    let text = read(&run_dir.join("fleet.jsonl"));
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), r.fleet_queue.t.len(), "one line per sample instant");
    assert!(lines.len() > 50, "the small scenario samples more than this");

    for (i, line) in lines.iter().enumerate() {
        let last = i + 1 == lines.len();
        assert!(line.ends_with(if last { r#","final":true}"# } else { r#","final":false}"# }), "line {i}: {line}");
        assert!(line.starts_with(r#"{"subscription_id":"export","sim_time_unix_ns":""#));
        // WIRE.md rule 2: the instant is the engine's own, as a decimal string.
        assert!(line.contains(&format!(r#""sim_time_unix_ns":"{}""#, r.fleet_queue.t[i])), "line {i}");
        assert!(line.contains(r#""target":{"scope":"SCOPE_FLEET"}"#));
        assert!(!line.contains("NaN") && !line.contains("inf"), "non-finite number on the wire: {line}");
    }

    let status = read(&run_dir.join("status.json"));
    assert!(status.contains(r#""state":"STATE_COMPLETE""#));
    assert!(status.contains(&format!(r#""sim_end_unix_ns":"{}""#, r.measured_to)));
    assert_eq!(read(&run_dir.join("scenario.txt")), r.scenario.to_text());
}

/// A metric's value on one `SubscriptionUpdate` line, or None when the row omitted it.
fn metric_value(line: &str, metric: i32) -> Option<f64> {
    let key = format!(r#""{metric}":"#);
    let start = line.find(&key)? + key.len();
    let rest = &line[start..];
    let end = rest.find([',', '}']).unwrap_or(rest.len());
    Some(rest[..end].parse().unwrap_or_else(|e| panic!("{}: {e}", &rest[..end])))
}

/// A metric's `Distribution` sub-object on one line's `distributions` map, or None when absent. The
/// key `"<metric>":{` is unambiguous: a `values` entry for the same number is always followed by a
/// number, never a `{`.
fn metric_distribution(line: &str, metric: i32) -> Option<&str> {
    let key = format!(r#""{metric}":{{"#);
    let start = line.find(&key)?;
    let obj_start = start + key.len() - 1;
    let bytes = line.as_bytes();
    let mut depth = 0i32;
    let mut i = obj_start;
    loop {
        match bytes[i] {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(&line[obj_start..=i]);
                }
            }
            _ => {}
        }
        i += 1;
    }
}

/// A `u64` field of a small JSON object slice, like the ones `metric_distribution` returns. WIRE.md
/// rule: a `uint64` crosses as a quoted string, unlike the `double` fields alongside it.
fn field_u64(obj: &str, name: &str) -> u64 {
    let key = format!(r#""{name}":""#);
    let start = obj.find(&key).unwrap_or_else(|| panic!("{name} missing in {obj}")) + key.len();
    let rest = &obj[start..];
    let end = rest.find('"').unwrap();
    rest[..end].parse().unwrap_or_else(|e| panic!("{}: {e}", &rest[..end]))
}

/// The `percentile` array of a `Distribution` object slice.
fn field_percentiles(obj: &str) -> Vec<f64> {
    let key = r#""percentile":["#;
    let start = obj.find(key).unwrap_or_else(|| panic!("percentile missing in {obj}")) + key.len();
    let rest = &obj[start..];
    let end = rest.find(']').unwrap();
    rest[..end].split(',').filter(|s| !s.is_empty()).map(|s| s.parse().unwrap()).collect()
}

/// U94b part (2): the fleet row carries GPU utilization as a mean plus a distribution over
/// replicas at 50/90/99, and the same distribution now exists for KV utilization; the replica row
/// carries the plain scalar. Both scopes' values stay in [0, 1], and the small scenario's fleet is
/// busy enough that both scopes see a positive reading somewhere.
#[test]
fn gpu_utilization_is_a_mean_and_a_distribution_over_replicas() {
    let dir = fresh_dir("gpu");
    let r = small_run();
    let run_dir = export::export_run_from(&r, "gpu", None, &dir).unwrap();
    let replicas = r.scenario.replicas as u64;

    let fleet_lines: Vec<String> = read(&run_dir.join("fleet.jsonl")).lines().map(String::from).collect();
    assert!(!fleet_lines.is_empty());
    let mut fleet_gpu_positive = false;
    for line in &fleet_lines {
        let gpu = metric_value(line, wire::METRIC_GPU_UTILIZATION).expect("fleet gpu utilization value");
        assert!((0.0..=1.0).contains(&gpu), "{line}");
        fleet_gpu_positive |= gpu > 0.0;

        let dist = metric_distribution(line, wire::METRIC_GPU_UTILIZATION).expect("gpu distribution");
        assert_eq!(field_percentiles(dist), vec![50.0, 90.0, 99.0], "{line}");
        assert_eq!(field_u64(dist, "count"), replicas, "{line}");

        assert!(metric_distribution(line, wire::METRIC_KV_UTILIZATION).is_some(), "{line}");
    }
    assert!(fleet_gpu_positive, "no fleet row shows any GPU busy");

    let replica_lines: Vec<String> = read(&run_dir.join("replicas.jsonl")).lines().map(String::from).collect();
    assert!(!replica_lines.is_empty());
    let mut replica_gpu_positive = false;
    for line in &replica_lines {
        let gpu = metric_value(line, wire::METRIC_GPU_UTILIZATION).expect("replica gpu utilization value");
        assert!((0.0..=1.0).contains(&gpu), "{line}");
        replica_gpu_positive |= gpu > 0.0;
    }
    assert!(replica_gpu_positive, "no replica row shows any GPU busy");
}

#[test]
fn replica_rows_follow_the_frames() {
    let dir = fresh_dir("replicas");
    let r = small_run();
    let run_dir = export::export_run_from(&r, "replicas", None, &dir).unwrap();
    let fleet: Vec<String> = read(&run_dir.join("fleet.jsonl")).lines().map(String::from).collect();
    let lines: Vec<String> = read(&run_dir.join("replicas.jsonl")).lines().map(String::from).collect();

    // The replica rows index frames by sample; that is only right while the two are the same
    // series of instants.
    assert_eq!(r.frames.len(), r.fleet_queue.t.len(), "one frame per fleet sample");
    for (s, f) in r.frames.iter().enumerate() {
        assert_eq!(f.t, r.fleet_queue.t[s], "frame {s} closes at the fleet sample's instant");
    }
    let replicas = r.scenario.replicas as usize;
    assert!(replicas > 1, "the small scenario has a fleet to break down");
    assert_eq!(lines.len(), replicas * fleet.len(), "replicas x samples lines");

    for (s, fleet_line) in fleet.iter().enumerate() {
        let t = format!(r#""sim_time_unix_ns":"{}""#, r.fleet_queue.t[s]);
        let (mut queued, mut running) = (0.0, 0.0);
        for id in 0..replicas {
            let line = &lines[s * replicas + id];
            assert!(line.contains(&t), "sample {s} replica {id}: {line}");
            assert!(
                line.contains(&format!(r#""target":{{"scope":"SCOPE_REPLICA","replica_id":"{id}"}}"#)),
                "sample {s} replica {id}: {line}"
            );
            queued += metric_value(line, wire::METRIC_QUEUED_SEQS).unwrap();
            running += metric_value(line, wire::METRIC_RUNNING_SEQS).unwrap();
            let kv = metric_value(line, wire::METRIC_KV_UTILIZATION).unwrap();
            assert!((0.0..=1.0).contains(&kv), "sample {s} replica {id}: kv {kv}");
            assert!(metric_value(line, wire::METRIC_KV_TOKENS_RESIDENT).is_some());
            assert!(metric_value(line, wire::METRIC_STEP_TIME).unwrap() >= 0.0);
            assert!(!line.contains("NaN") && !line.contains("inf"), "non-finite number on the wire: {line}");
        }
        assert_eq!(queued, metric_value(fleet_line, wire::METRIC_QUEUED_SEQS).unwrap(), "sample {s}: queued");
        assert_eq!(running, metric_value(fleet_line, wire::METRIC_RUNNING_SEQS).unwrap(), "sample {s}: running");
    }
    for (i, line) in lines.iter().enumerate() {
        let last = i + 1 == lines.len();
        assert!(line.ends_with(if last { r#","final":true}"# } else { r#","final":false}"# }), "line {i}: {line}");
    }
}

#[test]
fn window_counts_sum_to_the_scorecard() {
    let r = small_run();
    let rows = export::fleet_rows(&r);
    let iv_s = r.scenario.sample_interval_ms / 1000.0;
    let measured_s = (r.measured_to - r.measured_from) as f64 / 1e9;

    let mut e2e_count = 0u64;
    let mut queue_wait_count = 0u64;
    let mut completed = 0.0;
    let mut goodput_tokens = 0.0;
    let mut output_tokens = 0.0;
    for u in &rows {
        for (m, d) in &u.row.distributions {
            if *m == wire::METRIC_E2E {
                e2e_count += d.count;
            }
            if *m == wire::METRIC_QUEUE_WAIT {
                queue_wait_count += d.count;
            }
        }
        for (m, v) in &u.row.values {
            match *m {
                wire::METRIC_COMPLETED_RPS => completed += v * iv_s,
                wire::METRIC_GOODPUT_TOKENS_PER_S => goodput_tokens += v * iv_s,
                wire::METRIC_OUTPUT_TOKENS_PER_S => output_tokens += v * iv_s,
                _ => {}
            }
        }
    }
    assert!(r.completed() > 100, "the scenario must complete something to be worth checking");
    // The windows partition the measured records, so the sums are exact, not approximate.
    assert_eq!(e2e_count, r.completed());
    assert_eq!(queue_wait_count, r.records.len() as u64);
    assert!((completed - r.completed() as f64).abs() < 1e-6, "{completed} vs {}", r.completed());
    assert!(
        (goodput_tokens - r.goodput_tokens_s() * measured_s).abs() < 1e-3,
        "{goodput_tokens} vs {}",
        r.goodput_tokens_s() * measured_s
    );
    assert!((output_tokens - r.throughput_tokens_s() * measured_s).abs() < 1e-3);

    // And the scorecard document carries the same totals.
    let card = export::result(&r, "sum");
    let outcome_total: u64 = card.overall.outcome_counts.iter().map(|(_, n)| n).sum();
    assert_eq!(outcome_total, r.records.len() as u64);
    assert_eq!(card.state_checksum, r.fingerprint);
    assert_eq!(card.event_count, r.events);
}

#[test]
fn demos_export_writes_every_group() {
    let dir = fresh_dir("demos");
    let overrides = vec![("duration_s".to_string(), "20".to_string()), ("warmup_s".to_string(), "5".to_string())];
    let ids = export::export_demos(&workspace().join("scenarios"), &dir, &overrides).unwrap();

    let count = |prefix: &str| ids.iter().filter(|id| id.starts_with(prefix)).count();
    assert_eq!(count("1-routing/"), 4);
    assert_eq!(count("2-staleness/telemetry_interval_ms="), 6);
    assert_eq!(count("3-chunking/step_token_budget="), 6);
    assert_eq!(count("4-load-curve/arrival_rps="), 6);
    assert_eq!(count("5-long-context/long_probability="), 5);
    assert_eq!(count("6-retry/"), 3);
    assert_eq!(count("7-no-decode/"), 2);
    assert_eq!(count("8-admission/"), 2);
    assert_eq!(count("9-fair-share/"), 2);
    assert_eq!(count("10-probes/"), 2);
    assert_eq!(count("11-preemption/"), 2);
    assert_eq!(count("12-spec-decode/"), 2);
    assert_eq!(ids.len(), 42);
    assert!(ids.contains(&"1-routing/round-robin".to_string()), "{ids:?}");
    assert!(ids.contains(&"2-staleness/telemetry_interval_ms=250".to_string()));

    for id in &ids {
        let run_dir = dir.join("runs").join(id);
        for doc in ["status.json", "scenario.txt", "result.json", "fleet.jsonl"] {
            assert!(run_dir.join(doc).is_file(), "{id}/{doc} missing");
        }
        // The override reached the run, and the sweep's own key won over it where they met.
        let scenario = read(&run_dir.join("scenario.txt"));
        assert!(scenario.contains("duration_s = 20\n"), "{id}");
        assert!(scenario.contains("warmup_s = 5\n"), "{id}");
    }
    let index = read(&dir.join("runs/index.json"));
    assert_eq!(index.lines().filter(|l| l.starts_with('{')).count(), 42);
    assert!(index.contains(r#""scenario_file":""#));
    assert!(read(&dir.join("runs/3-chunking/step_token_budget=4096/scenario.txt")).contains("step_token_budget = 4096\n"));
}

/// `run-demos.sh` is the source of truth for what a demo shows; `DEMOS` is what the dashboard's
/// export can replay. Parse the script's own `compare` and `--out` lines and check the two cannot
/// silently drift apart: every scenario file the script compares must show up in some `DEMOS` entry,
/// and every `DEMOS` group name must be one the script actually writes to `out/`.
#[test]
fn demos_table_mirrors_run_demos_sh() {
    let script = read(&workspace().join("run-demos.sh"));

    let mut out_groups: BTreeSet<String> = BTreeSet::new();
    let mut compare_files: BTreeSet<String> = BTreeSet::new();

    for block in script.split("\n\n") {
        if !block.contains("$S ") {
            continue;
        }
        if let Some(out_line) = block.lines().find(|l| l.contains("--out out/")) {
            if let Some(start) = out_line.find("out/") {
                let rest = &out_line[start + "out/".len()..];
                if let Some(end) = rest.find(".html") {
                    out_groups.insert(rest[..end].to_string());
                }
            }
        }
        if block.contains("$S compare") {
            for token in block.split_whitespace() {
                if let Some(file) = token.strip_prefix("scenarios/") {
                    if file.ends_with(".txt") {
                        compare_files.insert(file.to_string());
                    }
                }
            }
        }
    }

    assert!(!out_groups.is_empty() && !compare_files.is_empty(), "parsed nothing out of run-demos.sh");

    for demo in export::DEMOS {
        assert!(
            out_groups.contains(demo.group),
            "DEMOS group {:?} has no matching `--out out/{{group}}.html` in run-demos.sh (found: {out_groups:?})",
            demo.group
        );
    }

    for file in &compare_files {
        assert!(
            export::DEMOS.iter().any(|d| d.files.contains(&file.as_str())),
            "run-demos.sh compares scenarios/{file} but no DEMOS entry lists it"
        );
    }
}
