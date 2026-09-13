//! Executable inventory of MXRB's public command surface against MXRS.
//!
//! This intentionally distinguishes "implemented" from parity: a row is
//! complete only after its user-visible contract has an explicit oracle.

use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Verified,
    Partial,
    Missing,
}

#[derive(Debug, Serialize)]
pub struct Row {
    pub mxrb_command: String,
    pub status: Status,
    pub mxrs_surface: &'static str,
    pub evidence: &'static str,
}

#[derive(Debug, Serialize)]
pub struct Report {
    pub rows: Vec<Row>,
    pub verified: usize,
    pub partial: usize,
    pub missing: usize,
}

impl Report {
    pub fn complete(&self) -> bool {
        self.partial == 0 && self.missing == 0
    }
}

pub fn build(mxrb_commands_output: &str) -> Result<Report, String> {
    let mut commands = parse_commands(mxrb_commands_output);
    if commands.is_empty() {
        return Err("could not parse any commands from mxrb --commands".into());
    }
    commands.sort();
    commands.dedup();
    let rows = commands
        .into_iter()
        .map(|command| {
            let (status, surface, evidence) = classify(&command);
            Row {
                mxrb_command: command,
                status,
                mxrs_surface: surface,
                evidence,
            }
        })
        .collect::<Vec<_>>();
    Ok(Report {
        verified: rows
            .iter()
            .filter(|row| row.status == Status::Verified)
            .count(),
        partial: rows
            .iter()
            .filter(|row| row.status == Status::Partial)
            .count(),
        missing: rows
            .iter()
            .filter(|row| row.status == Status::Missing)
            .count(),
        rows,
    })
}

fn parse_commands(output: &str) -> Vec<String> {
    output
        .lines()
        .filter_map(|line| {
            let trimmed = line.trim();
            let (command, description) = trimmed.split_once(char::is_whitespace)?;
            (!description.trim().is_empty()
                && command
                    .chars()
                    .all(|character| character.is_ascii_lowercase() || character == '-'))
            .then(|| command.to_string())
        })
        .collect()
}

fn classify(command: &str) -> (Status, &'static str, &'static str) {
    match command {
        // Byte/structural oracles exist for these storage-facing commands.
        "compare" => (
            Status::Verified,
            "mxrs compare",
            "structural comparison tests",
        ),
        "dump-unit" => (Status::Verified, "mxrs dump-unit", "raw unit tests"),
        "modules" => (Status::Verified, "mxrs modules", "module listing tests"),
        "sql" => (Status::Verified, "mxrs sql", "read-only MPR SQL tests"),
        // A corresponding MXRS surface exists, but full option/output/model
        // parity has not been proved and must remain visibly partial.
        "analyze" => (Status::Partial, "mxrs oql", "risk analyzer only"),
        "benchmark" => (
            Status::Partial,
            "cargo bench",
            "library benches, no CLI parity",
        ),
        "callees" | "callers" | "describe" | "refs" => (
            Status::Partial,
            "mxrs refs",
            "deterministic reference graph",
        ),
        "convert" => (
            Status::Partial,
            "mxrs import/export",
            "Cargo-native conversion",
        ),
        "diff" => (Status::Partial, "cargo mxrs diff", "typed project diff"),
        "export" => (Status::Partial, "mxrs export", "typed Rust export"),
        "find" | "search" => (
            Status::Partial,
            "mxrs search",
            "ranked name/reference search",
        ),
        "frontend" => (
            Status::Partial,
            "cargo mxrs frontend-dev",
            "pinned React shell",
        ),
        "generate" => (
            Status::Partial,
            "cargo mxrs build",
            "Cargo declaration to MPR",
        ),
        "impact" => (
            Status::Partial,
            "mxrs impact",
            "transitive dependency impact",
        ),
        "init" => (
            Status::Partial,
            "mxrs new",
            "transactional project scaffold",
        ),
        "inspect" => (
            Status::Partial,
            "mxrs inspect/units",
            "non-interactive inspection",
        ),
        "oql" => (
            Status::Partial,
            "mxrs oql",
            "catalog and logical SQL projection",
        ),
        "pack" | "portable" => (Status::Partial, "mxrs package", "reproducible MXRS archive"),
        "query" => (
            Status::Partial,
            "mxrs translate-oql",
            "safe OQL-to-SQL subset",
        ),
        "run" => (
            Status::Partial,
            "RuntimeHttp",
            "library adapter; orchestration missing",
        ),
        "serve" => (
            Status::Partial,
            "RuntimeHttp",
            "loopback-capable library; CLI missing",
        ),
        "validate" => (
            Status::Partial,
            "mxrs validate",
            "storage validation; scope differs",
        ),
        _ => (Status::Missing, "—", "no equivalent surface implemented"),
    }
}

pub fn print_table(report: &Report) {
    println!(
        "MXRB command capability matrix ({} rows)",
        report.rows.len()
    );
    println!("STATUS\tMXRB\tMXRS\tEVIDENCE");
    for row in &report.rows {
        println!(
            "{:?}\t{}\t{}\t{}",
            row.status, row.mxrb_command, row.mxrs_surface, row.evidence
        );
    }
    println!(
        "verified={} partial={} missing={}",
        report.verified, report.partial, report.missing
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_classifies_command_inventory_without_counting_headers() {
        let report = build(
            "Available MXRB commands (3):\n\n  compare  Compare projects\n  validate  Validate project\n  rename  Rename artifact\n",
        )
        .unwrap();
        assert_eq!(report.rows.len(), 3);
        assert_eq!(report.verified, 1);
        assert_eq!(report.partial, 1);
        assert_eq!(report.missing, 1);
        assert!(!report.complete());
    }

    #[test]
    fn empty_or_changed_help_format_fails_loudly() {
        assert!(build("Available commands: none").is_err());
    }
}
