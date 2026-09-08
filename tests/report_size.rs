//! A report that draws every replica costs the same per replica whether or not the picture actually
//! changes, so its size scales with fleet size unless something caps it. At the project's 256-replica
//! default, `sim-run compare`'s HTML grew to 29 MB (measured on lbsim-00017) and Cloud Run 500s a
//! non-chunked response over 32 MiB — so two of four scenarios in that comparison came back empty.
//! This is the regression guard on the fix: the report for the same 256-replica compare must stay
//! under 8 MB, well inside Cloud Run's limit and small enough to actually open.
//!
//! ~15 s in `--release` for the four `route_*.txt` scenarios at 256 replicas over 120 simulated
//! seconds. An unoptimized build spends far longer on the same run than a `cargo test` default should
//! cost, so this is `#[ignore]`d. Run it explicitly and in release:
//! `cargo test --release --test report_size -- --ignored`.

mod common;

use lbsim::report::{self, Opts};
use lbsim::scenario::Scenario;
use lbsim::sim;

const SCENARIOS: [&str; 4] = [
    "scenarios/route_round_robin.txt",
    "scenarios/route_p2c.txt",
    "scenarios/route_least_requests.txt",
    "scenarios/route_random.txt",
];

#[test]
#[ignore]
fn routing_compare_report_stays_under_8mb_at_256_replicas() {
    let runs: Vec<_> = SCENARIOS
        .iter()
        .map(|path| {
            let text = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{path}: {e}"));
            let sc = Scenario::parse(&text).unwrap_or_else(|e| panic!("{path}: {e}"));
            assert_eq!(
                sc.replicas, 256,
                "{path} is no longer the 256-replica fixture this test's size budget targets"
            );
            sim::run(&sc).unwrap_or_else(|e| panic!("{path}: {e}"))
        })
        .collect();

    let dir = common::scratch("report-size");
    let html_path = dir.join("compare.html");
    let telemetry_dir = dir.join("telemetry");
    let opts = Opts {
        out: None,
        telemetry: Some(telemetry_dir.to_string_lossy().to_string()),
        budget_mb: 100,
        overrides: Vec::new(),
    };
    report::emit(&runs, &html_path.to_string_lossy(), &opts).expect("emit failed");

    let bytes = std::fs::metadata(&html_path).unwrap().len();
    assert!(
        bytes < 8 * 1024 * 1024,
        "report is {:.2} MB at 256 replicas ({} bytes, {}); Cloud Run 500s a non-chunked response \
         over 32 MiB and this project's own budget is 8 MB",
        bytes as f64 / 1e6,
        bytes,
        html_path.display()
    );

    // `summary_csv` is a different function from the HTML renderer the size fix above changes; a
    // rendering change should be structurally unable to reach it. This is a smoke check, through the
    // same public `emit` path, that every run still gets its full scorecard rather than a truncated
    // one — telemetry is written by the same `emit` call the HTML came from.
    for (i, r) in runs.iter().enumerate() {
        let slug: String =
            r.scenario.name.chars().map(|c| if c.is_alphanumeric() { c } else { '_' }).collect();
        let csv_path = telemetry_dir.join(format!("{i:02}-{slug}.summary.csv"));
        let csv = std::fs::read_to_string(&csv_path)
            .unwrap_or_else(|e| panic!("{}: {e}", csv_path.display()));
        assert!(csv.starts_with("metric,value\n"), "{}: lost its header", csv_path.display());
        assert!(csv.contains("fingerprint,"), "{}: lost the fingerprint row", csv_path.display());
        let rows = csv.lines().count();
        assert!(rows > 10, "{}: only {rows} lines, expected one per metric", csv_path.display());
    }
}
