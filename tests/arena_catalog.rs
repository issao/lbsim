//! `sim_arena::catalog` against the committed `docs/policy-catalog.md`. Issao: *"populate an md with
//! all policy ideas we have had so far and instruct the arena policy generator to populate that as
//! well with any that it authors."* The document is hand-written; the generator appends; the header
//! is the contract, and the first test is what stops the two from drifting apart.

use lbsim::arena::catalog::{append, render_row, CatalogRow, HEADER};
use std::path::{Path, PathBuf};

const CATALOG: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/docs/policy-catalog.md");

fn row(name: &str, family: &str) -> CatalogRow {
    CatalogRow {
        name: name.into(),
        family: family.into(),
        idea: "an idea".into(),
        source: format!("arena/generated/{name}.rs"),
        score: 0.8125,
        rule_set: lbsim::arena::RULE_SET.into(),
        added: "2026-09-06".into(),
    }
}

/// A private copy of the committed catalog, so a test never writes housekeeping's file.
fn temp_copy(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("lbsim-arena-catalog-{}-{tag}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("policy-catalog.md");
    std::fs::copy(CATALOG, &path).unwrap();
    path
}

fn tables_with_header(text: &str) -> Vec<usize> {
    text.lines().enumerate().filter(|(_, l)| l.starts_with("| Name")).map(|(i, _)| i).collect()
}

#[test]
fn row_matches_the_committed_catalog_header() {
    let text = std::fs::read_to_string(CATALOG).expect("docs/policy-catalog.md at the workspace root");
    let headers = tables_with_header(&text);
    assert!(!headers.is_empty(), "the catalog has no tables");
    let lines: Vec<&str> = text.lines().collect();
    for i in headers {
        assert_eq!(lines[i].trim(), HEADER, "line {}: table header differs from catalog::HEADER", i + 1);
    }
    // The row has as many cells as the header, so it lands under the right columns.
    let cells = |l: &str| l.trim().trim_matches('|').split('|').count();
    assert_eq!(cells(&render_row(&row("x", "routing"))), cells(HEADER));
    // And the fixed-status column is the one the Format section reserves for the generator.
    assert!(render_row(&row("x", "routing")).contains("| arena-authored |"));
}

#[test]
fn append_into_an_existing_family_table() {
    let path = temp_copy("existing");
    let before = std::fs::read_to_string(&path).unwrap();
    let r = row("forecast_p2c", "load forecasting");
    append(&path, &r).unwrap();
    let after = std::fs::read_to_string(&path).unwrap();
    assert_eq!(after.lines().count(), before.lines().count() + 1, "exactly one line added");
    assert_eq!(tables_with_header(&after).len(), tables_with_header(&before).len(), "no new table");

    // The new row sits inside the Load forecasting table: after its header, before the next heading,
    // contiguous with the other rows.
    let lines: Vec<&str> = after.lines().collect();
    let heading = lines.iter().position(|l| *l == "## Load forecasting").unwrap();
    let next = heading + 1 + lines[heading + 1..].iter().position(|l| l.starts_with("## ")).unwrap();
    let at = lines.iter().position(|l| l.trim() == render_row(&r)).unwrap();
    assert!(heading < at && at < next, "row at {at} outside section {heading}..{next}");
    assert!(lines[at - 1].starts_with('|'), "row must extend the table, not float below it");
    assert!(at + 1 == next || !lines[at + 1].starts_with('|') || lines[at + 1].trim().is_empty());
}

#[test]
fn append_creates_a_new_family_section_when_absent() {
    let path = temp_copy("newfamily");
    let r = row("thermal_aware", "thermal");
    append(&path, &r).unwrap();
    let after = std::fs::read_to_string(&path).unwrap();
    let lines: Vec<&str> = after.lines().collect();
    let heading = lines.iter().position(|l| *l == "## Thermal").expect("a new ## Thermal section");
    assert_eq!(lines[heading + 1], "");
    assert_eq!(lines[heading + 2], HEADER);
    assert!(lines[heading + 3].starts_with("|---"));
    assert_eq!(lines[heading + 4].trim(), render_row(&r));
    assert!(after.ends_with('\n'));

    // Every table, including the new one, still carries the contract header.
    for i in tables_with_header(&after) {
        assert_eq!(lines[i].trim(), HEADER);
    }
    // A second family in the same file gets its own section too, and the first is untouched.
    append(&path, &row("other", "thermal")).unwrap();
    let again = std::fs::read_to_string(&path).unwrap();
    assert_eq!(again.matches("## Thermal").count(), 1, "one section per family");
}

#[test]
fn append_is_idempotent() {
    let path = temp_copy("idem");
    let r = row("twice", "routing");
    append(&path, &r).unwrap();
    let once = std::fs::read_to_string(&path).unwrap();
    append(&path, &r).unwrap();
    let twice = std::fs::read_to_string(&path).unwrap();
    assert_eq!(once, twice, "an identical row must not be written twice");
    assert_eq!(once.matches(&render_row(&r)).count(), 1);
}

#[test]
fn pipes_in_text_are_escaped() {
    let mut r = row("piped", "scheduling");
    r.idea = "a | b".into();
    r.source = "gen/a|b.rs".into();
    let line = render_row(&r);
    assert!(line.contains("a \\| b"), "{line}");
    assert!(line.contains("gen/a\\|b.rs"), "{line}");
    // An escaped pipe is not a cell boundary, so the column count is unchanged.
    let cells = |l: &str| l.replace("\\|", "").trim().trim_matches('|').split('|').count();
    assert_eq!(cells(&line), cells(HEADER));
    let _ = Path::new(CATALOG);
}
