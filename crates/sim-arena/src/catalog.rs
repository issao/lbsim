//! Appending arena-authored policies to `docs/policy-catalog.md`.
//!
//! Issao: *"Load and latency forecasting should be added as potential policies to evaluate (populate
//! an md with all policy ideas we have had so far and instruct the arena policy generator to populate
//! that as well with any that it authors)."* The document is housekeeping's; this is the generator's
//! pen. The one contract between them is [`HEADER`], which `tests/arena_catalog.rs` checks against
//! every table in the committed file so the two cannot drift apart silently.
//!
//! Rows go into the table under the `## <Family>` heading whose text contains the family name; a family
//! with no section yet gets a new one at the end of the file, same header. The `added` date is the
//! caller's: there is no clock in simulation code, and the generator is the one that knows when it
//! wrote the policy.

use std::path::Path;

/// The header every table in `docs/policy-catalog.md` carries, verbatim. Change it there and here in
/// the same commit or the catalog test fails.
pub const HEADER: &str = "| Name | Family | Idea | Status | Source | Score | Rule set | Added |";

/// The alignment row that follows [`HEADER`] in a new table.
const SEPARATOR: &str = "|---|---|---|---|---|---|---|---|";

/// The status every row written by this module carries. The other statuses are for hand-written rows.
const STATUS: &str = "arena-authored";

/// One arena-authored policy, as the catalog records it.
#[derive(Clone, Debug, PartialEq)]
pub struct CatalogRow {
    /// Policy name as the registry resolves it.
    pub name: String,
    /// Short family name as the catalog's Family column uses it: `routing`, `load forecasting`, ...
    pub family: String,
    /// One line.
    pub idea: String,
    /// Path of the generated file.
    pub source: String,
    /// Minimum gated goodput share over the in-scope loads, from the round that scored it.
    pub score: f64,
    /// [`crate::RULE_SET`] of that round.
    pub rule_set: String,
    /// `YYYY-MM-DD`, supplied by the caller.
    pub added: String,
}

/// A pipe inside a cell would split the row; markdown's escape keeps the cell whole.
fn cell(text: &str) -> String {
    text.replace('|', "\\|")
}

/// The row as one markdown table line, with [`STATUS`] filled in.
pub fn render_row(row: &CatalogRow) -> String {
    format!(
        "| `{}` | {} | {} | {} | `{}` | {:.3} | {} | {} |",
        cell(&row.name),
        cell(&row.family),
        cell(&row.idea),
        STATUS,
        cell(&row.source),
        row.score,
        cell(&row.rule_set),
        cell(&row.added),
    )
}

/// Title-case for a new `## <Family>` heading: `load forecasting` becomes `Load forecasting`, matching
/// the hand-written headings' style.
fn heading_for(family: &str) -> String {
    let mut chars = family.trim().chars();
    match chars.next() {
        Some(c) => format!("## {}{}", c.to_uppercase(), chars.as_str()),
        None => "## ".to_string(),
    }
}

/// Index of the heading line for `family`, if the catalog has one. Substring match on the lowercased
/// heading, because the headings are longer than the family names (`## Admission and shaping` holds the
/// `admission` family) and the Family column is the short form.
fn find_section(lines: &[&str], family: &str) -> Option<usize> {
    let needle = family.trim().to_lowercase();
    if needle.is_empty() {
        return None;
    }
    lines.iter().position(|l| {
        l.strip_prefix("## ").map_or(false, |h| h.trim().to_lowercase().contains(&needle))
    })
}

/// Append `row` to the catalog at `path`. Writing an identical row twice is a no-op, so a generator
/// that re-runs a round does not duplicate its own entries.
pub fn append(path: &Path, row: &CatalogRow) -> Result<(), String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let rendered = render_row(row);
    let heading = heading_for(&row.family);
    let mut lines: Vec<&str> = text.lines().collect();

    let insert_at = match find_section(&lines, &row.family) {
        Some(h) => {
            // The table is the header after the heading, its separator, then every consecutive row.
            // Stop at the next heading so a family without a table yet gets one rather than a row
            // stranded in prose.
            let next_heading = lines[h + 1..]
                .iter()
                .position(|l| l.starts_with("## "))
                .map_or(lines.len(), |i| h + 1 + i);
            match lines[h..next_heading].iter().position(|l| l.trim() == HEADER) {
                Some(off) => {
                    let header = h + off;
                    let mut end = header + 1;
                    while end < next_heading && lines[end].starts_with('|') {
                        if lines[end].trim() == rendered {
                            return Ok(());
                        }
                        end += 1;
                    }
                    end
                }
                None => {
                    // A section with no table: give it one, directly before the next heading and
                    // after the section's prose.
                    let mut at = next_heading;
                    while at > h + 1 && lines[at - 1].trim().is_empty() {
                        at -= 1;
                    }
                    lines.insert(at, "");
                    lines.insert(at + 1, HEADER);
                    lines.insert(at + 2, SEPARATOR);
                    at + 3
                }
            }
        }
        None => {
            while lines.last().map_or(false, |l| l.trim().is_empty()) {
                lines.pop();
            }
            lines.push("");
            lines.push(&heading);
            lines.push("");
            lines.push(HEADER);
            lines.push(SEPARATOR);
            lines.len()
        }
    };

    lines.insert(insert_at, &rendered);
    let mut out = lines.join("\n");
    out.push('\n');
    std::fs::write(path, out).map_err(|e| format!("{}: {e}", path.display()))
}
