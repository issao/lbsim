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
use std::path::Path;

fn workspace() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

/// A fresh directory per test, so tests can run in parallel and a stale index cannot leak between
/// them. The guard itself, the sweep of stale leftovers, and its unit test now live in
/// `tests/common` (U110b), shared by every test file that scratches `/tmp`; this keeps the
/// `lbsim-wire-export-*` naming so old leftovers from before that move still get swept.
fn fresh_dir(name: &str) -> common::ScratchDir {
    common::scratch(&format!("wire-export-{name}"))
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

/// An `f64` field of a small JSON object slice, like the ones `metric_distribution` returns —
/// `mean`, `min`, `max`, unquoted doubles unlike `count`.
fn field_f64(obj: &str, name: &str) -> f64 {
    let key = format!(r#""{name}":"#);
    let start = obj.find(&key).unwrap_or_else(|| panic!("{name} missing in {obj}")) + key.len();
    let rest = &obj[start..];
    let end = rest.find(',').unwrap();
    rest[..end].parse().unwrap_or_else(|e| panic!("{}: {e}", &rest[..end]))
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

/// U27c: the prefix hit rate only appears on the wire once a scenario has a prefix model.
/// `small_run()`'s scenario has `prefix_roots = 0`, so no fleet row ever carries metric 48; a
/// variant with a prefix model turned on shows the ratio in range, and positive somewhere once the
/// cache has had a chance to warm.
#[test]
fn prefix_hit_rate_is_absent_without_a_prefix_model_and_in_range_with_one() {
    let dir = fresh_dir("prefix-off");
    let r = small_run();
    let run_dir = export::export_run_from(&r, "prefix-off", None, &dir).unwrap();
    let fleet_lines: Vec<String> = read(&run_dir.join("fleet.jsonl")).lines().map(String::from).collect();
    assert!(!fleet_lines.is_empty());
    for line in &fleet_lines {
        assert!(metric_value(line, wire::METRIC_PREFIX_HIT_RATE).is_none(), "{line}");
    }

    let dir = fresh_dir("prefix-on");
    let mut sc = common::at_load(0.9);
    sc.name = "wire \"export\" / small-prefix".into();
    sc.prefix_roots = 1;
    sc.prefix_cache_tokens = 1e6;
    let r = sim::run(&sc).expect("small prefix scenario runs");
    let run_dir = export::export_run_from(&r, "prefix-on", None, &dir).unwrap();
    let fleet_lines: Vec<String> = read(&run_dir.join("fleet.jsonl")).lines().map(String::from).collect();
    assert!(!fleet_lines.is_empty());
    let mut hit_rate_positive = false;
    for line in &fleet_lines {
        if let Some(rate) = metric_value(line, wire::METRIC_PREFIX_HIT_RATE) {
            assert!((0.0..=1.0).contains(&rate), "{line}");
            hit_rate_positive |= rate > 0.0;
        }
    }
    assert!(hit_rate_positive, "no fleet row shows any prefix hit rate");
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
fn replica_ttft_is_a_windowed_mean_with_no_percentiles() {
    // U31a's failure injection is not wired into this scenario, so every replica in a small,
    // failure-free run is READY at true speed 1.0 — the two-line emission this test does not
    // cover, because there is no scenario handle here to force a failure. `sim-model`'s own tests
    // hold `state()` to READY/DEGRADED/EJECTED directly.
    let dir = fresh_dir("ttft");
    let r = small_run();
    let run_dir = export::export_run_from(&r, "ttft", None, &dir).unwrap();
    let lines: Vec<String> = read(&run_dir.join("replicas.jsonl")).lines().map(String::from).collect();
    assert!(!lines.is_empty());

    let mut any_ttft = false;
    for (i, line) in lines.iter().enumerate() {
        assert_eq!(metric_value(line, wire::METRIC_REPLICA_STATE), Some(1.0), "line {i}: {line}");
        assert_eq!(metric_value(line, wire::METRIC_TRUE_SPEED_MULTIPLIER), Some(1.0), "line {i}: {line}");
        if let Some(dist) = metric_distribution(line, wire::METRIC_TTFT) {
            assert!(field_u64(dist, "count") > 0, "line {i}: a carried distribution has a count: {line}");
            assert!(field_f64(dist, "mean") > 0.0, "line {i}: a carried distribution has a positive mean: {line}");
            assert!(field_percentiles(dist).is_empty(), "line {i}: no percentiles on a windowed mean: {line}");
            any_ttft = true;
        }
    }
    assert!(any_ttft, "at least one replica row should carry a TTFT distribution somewhere in the run");
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
fn preemptions_and_retries_are_windowed_rates() {
    // U115: Issao saw no preemptions on the KV panel at a full cache. Metrics 46 and 47 had table
    // rows but no server or exporter wrote them, so the panel read a gap. A cache-starved fleet of
    // multi-turn sessions under `swap_to_dram` must show a positive preemption rate on some fleet
    // row and on some replica row; a fleet with headroom shows an explicit zero, not an absence.
    let dir = fresh_dir("preempt-off");
    let r = small_run();
    let run_dir = export::export_run_from(&r, "preempt-off", None, &dir).unwrap();
    let fleet_lines: Vec<String> = read(&run_dir.join("fleet.jsonl")).lines().map(String::from).collect();
    assert!(!fleet_lines.is_empty());
    for line in &fleet_lines {
        assert_eq!(metric_value(line, wire::METRIC_PREEMPTIONS_PER_S), Some(0.0), "{line}");
        assert_eq!(metric_value(line, wire::METRIC_RETRIES_PER_S), Some(0.0), "{line}");
    }

    let dir = fresh_dir("preempt-on");
    let mut sc = common::at_load(0.9);
    sc.name = "wire \"export\" / small-preempt".into();
    // kv_spiral_swap.txt's shape: sessions that hold context between turns, a cache that cannot
    // hold them all, and eviction rather than waiting.
    sc.kv_capacity_tokens = 4000.0;
    sc.session_turns_mean = 8.0;
    sc.session_think_s = 2.0;
    sc.preemption = "swap_to_dram".into();
    sc.preemption_victim = "newest".into();
    let r = sim::run(&sc).expect("small preempting scenario runs");
    let run_dir = export::export_run_from(&r, "preempt-on", None, &dir).unwrap();
    let fleet_lines: Vec<String> = read(&run_dir.join("fleet.jsonl")).lines().map(String::from).collect();
    let mut fleet_positive = false;
    for line in &fleet_lines {
        let v = metric_value(line, wire::METRIC_PREEMPTIONS_PER_S).unwrap_or_else(|| panic!("no 46 on {line}"));
        assert!(v >= 0.0 && v.is_finite(), "{line}");
        fleet_positive |= v > 0.0;
    }
    assert!(fleet_positive, "no fleet row shows a preemption under swap_to_dram at a 4k cache");
    let replica_lines: Vec<String> = read(&run_dir.join("replicas.jsonl")).lines().map(String::from).collect();
    let replica_sum: f64 = replica_lines
        .iter()
        .map(|l| metric_value(l, wire::METRIC_PREEMPTIONS_PER_S).unwrap_or_else(|| panic!("no 46 on {l}")))
        .sum();
    assert!(replica_sum > 0.0, "no replica row shows a preemption");
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
    assert_eq!(count("13-herd-fleet/"), 5);
    assert_eq!(count("14-affinity/"), 3);
    assert_eq!(count("15-gray-failure/"), 2);
    assert_eq!(count("16-scheduling/"), 3);
    assert_eq!(count("17-cascade/"), 2);
    assert_eq!(count("18-bode/perturb_frequency_hz="), 7);
    assert_eq!(ids.len(), 64);
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
    assert_eq!(index.lines().filter(|l| l.starts_with('{')).count(), 64);
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

/// U27d: a crashed replica keeps its slot in `frame.replicas`, so counting the slots (as
/// `fleet_rows` used to, reading `scenario.replicas`) never notices it left. The ready count must
/// drop for every sample after the crash instant, and only then.
#[test]
fn ready_replicas_excludes_a_crashed_one() {
    let mut sc = common::at_load(0.9);
    sc.name = "wire export crash".into();
    let crash_s = 15.0; // small()'s run is 30 s with 3 s of warmup; this is comfortably mid-run.
    sc.failures = format!("t={crash_s},replica=0,kind=crash");
    let r = sim::run(&sc).expect("crash scenario runs");
    let replicas = r.scenario.replicas as f64;
    let crash = lbsim::EPOCH_BASE + (crash_s * 1e9) as lbsim::Nanos;

    let rows = export::fleet_rows(&r);
    assert!(!rows.is_empty(), "the crash scenario must produce fleet samples");
    let (mut before, mut after) = (false, false);
    for u in &rows {
        let ready = u
            .row
            .values
            .iter()
            .find(|(m, _)| *m == wire::METRIC_READY_REPLICAS)
            .map(|(_, v)| *v)
            .expect("every fleet row carries a ready-replica count");
        if u.sim_time_unix_ns < crash {
            assert_eq!(ready, replicas, "sample at {} precedes the crash instant", u.sim_time_unix_ns);
            before = true;
        } else {
            assert_eq!(
                ready,
                replicas - 1.0,
                "sample at {} follows the crash instant",
                u.sim_time_unix_ns
            );
            after = true;
        }
    }
    assert!(before && after, "need fleet samples on both sides of the crash instant to prove anything");
}

/// U30: the memory-tier gauges at fleet scope. 27 and 28 each carry a value and a distribution whose
/// `percentile` slots are tier ids, 1 DRAM and 2 SSD, every reading in [0, 1]; a scenario with an
/// SSD pool carries both tiers, one without carries DRAM alone.
#[test]
fn fleet_rows_carry_the_tier_gauges_keyed_by_tier_id() {
    let mut sc = common::at_load(0.9);
    sc.preemption = "swap_to_dram".into();
    sc.dram_pool_tokens = 1000.0;
    sc.ssd_pool_tokens = 1000.0;
    let r = sim::run(&sc).expect("tiered scenario runs");
    let rows = export::fleet_rows(&r);
    assert!(!rows.is_empty());
    for u in &rows {
        for m in [wire::METRIC_TIER_UTILIZATION, wire::METRIC_TIER_BANDWIDTH_UTILIZATION] {
            let v = u.row.values.iter().find(|(k, _)| *k == m).map(|(_, v)| *v).unwrap_or_else(|| panic!("metric {m} value missing"));
            assert!((0.0..=1.0).contains(&v), "metric {m} value {v} outside [0, 1]");
            let d = &u.row.distributions.iter().find(|(k, _)| *k == m).unwrap_or_else(|| panic!("metric {m} distribution missing")).1;
            assert_eq!(d.percentile, vec![1.0, 2.0], "slots are tier ids, DRAM then SSD");
            assert_eq!(d.count, 2);
            assert!(d.value.iter().all(|v| (0.0..=1.0).contains(v)), "{:?}", d.value);
            assert!(!d.from_merged_histogram);
        }
    }
    let untiered = export::fleet_rows(&small_run());
    let d = &untiered[0].row.distributions.iter().find(|(k, _)| *k == wire::METRIC_TIER_UTILIZATION).unwrap().1;
    assert_eq!(d.percentile, vec![1.0], "no SSD pool, no tier 2");
}
