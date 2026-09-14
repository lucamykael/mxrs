//! Executable inventory of MXRB's public command surface against MXRS.
//!
//! A command name is not evidence of Studio Pro compatibility. This inventory
//! only tracks MXRB's public CLI; the official compiler and runtime need their
//! own versioned, behavioral acceptance evidence. No row is verified merely
//! because similarly named unit tests exist.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
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
    pub scope: &'static str,
    pub studio_pro_parity: &'static str,
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
    let mut commands = parse_commands(mxrb_commands_output)?;
    commands.sort();
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
        scope: "mxrb_top_level_commands",
        studio_pro_parity: "not_established_by_command_inventory",
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

/// Matches the catalog emitted by mxrb/lib/mxrb/cli/help.rb:379-386, including
/// its declared count. Fail closed when the oracle format changes: silently
/// accepting a truncated inventory can make missing commands disappear.
fn parse_commands(output: &str) -> Result<Vec<String>, String> {
    let mut lines = output.lines().filter(|line| !line.trim().is_empty());
    let count = lines
        .next()
        .and_then(|header| header.strip_prefix("Available MXRB commands ("))
        .and_then(|header| header.strip_suffix("):"))
        .and_then(|count| count.parse::<usize>().ok())
        .filter(|count| *count > 0)
        .ok_or("invalid or empty mxrb --commands header")?;
    let mut commands = Vec::new();
    let mut names = BTreeSet::new();
    for _ in 0..count {
        let line = lines.next().ok_or("truncated mxrb command inventory")?;
        let (name, description) = line
            .strip_prefix("  ")
            .and_then(|line| line.split_once("  "))
            .ok_or_else(|| format!("invalid command inventory row: {line:?}"))?;
        if !valid_command_name(name) || description.trim().is_empty() {
            return Err(format!("invalid command inventory row: {line:?}"));
        }
        if !names.insert(name.to_owned()) {
            return Err(format!("duplicate command in inventory: {name}"));
        }
        commands.push(name.to_owned());
    }
    if lines.next() != Some("Run `mxrb COMMAND --help` for usage and an example.")
        || lines.next().is_some()
    {
        return Err("unexpected footer or count mismatch in mxrb command inventory".into());
    }
    Ok(commands)
}

fn valid_command_name(name: &str) -> bool {
    name.split('-').all(|segment| {
        !segment.is_empty()
            && segment
                .bytes()
                .all(|character| character.is_ascii_lowercase())
    })
}

pub fn check_baseline(report: &Report, baseline: &str) -> Result<(), String> {
    let baseline: BTreeMap<String, Status> =
        serde_json::from_str(baseline).map_err(|error| format!("invalid baseline: {error}"))?;
    if baseline.is_empty() {
        return Err("command baseline must not be empty".into());
    }
    let current = report
        .rows
        .iter()
        .map(|row| (row.mxrb_command.as_str(), row.status))
        .collect::<BTreeMap<_, _>>();
    let mut regressions = Vec::new();
    for (command, previous) in &baseline {
        match current.get(command.as_str()) {
            None => regressions.push(format!("command disappeared from oracle: {command}")),
            Some(status) if status.rank() < previous.rank() => regressions.push(format!(
                "command regressed: {command}: {previous:?} -> {status:?}"
            )),
            _ => {}
        }
    }
    for command in current.keys() {
        if !baseline.contains_key(*command) {
            regressions.push(format!("new command needs baseline review: {command}"));
        }
    }
    if regressions.is_empty() {
        Ok(())
    } else {
        Err(regressions.join("\n"))
    }
}

impl Status {
    fn rank(self) -> u8 {
        match self {
            Self::Missing => 0,
            Self::Partial => 1,
            Self::Verified => 2,
        }
    }
}

fn classify(command: &str) -> (Status, &'static str, &'static str) {
    match command {
        "compare" => (
            Status::Partial,
            "mxrs compare",
            "structural comparison tests; full CLI contract not differentially verified",
        ),
        "dump-unit" => (
            Status::Partial,
            "mxrs dump-unit",
            "raw unit tests; CLI oracle missing",
        ),
        "modules" => (
            Status::Partial,
            "mxrs modules",
            "module listing tests; CLI oracle missing",
        ),
        "sql" => (
            Status::Partial,
            "mxrs sql",
            "read-only MPR SQL tests; CLI oracle missing",
        ),
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
            "mxrs callees/callers/describe/refs",
            "deterministic reference graph",
        ),
        "tree" => (
            Status::Partial,
            "mxrs tree",
            "semantic artifact hierarchy; full CLI oracle missing",
        ),
        "lint" | "report" => (
            Status::Partial,
            "mxrs lint/report",
            "unresolved references and flow call cycles; full CLI oracle missing",
        ),
        "convert" => (
            Status::Partial,
            "mxrs import/export",
            "Cargo-native conversion",
        ),
        // MXRB's source generators. MXRS writes Rust declarations where MXRB
        // writes Ruby, so the generated content is not comparable by
        // construction and no differential oracle can exist for it; the
        // argument grammar, rendering and registry behavior are ported and
        // covered by mxrs-cli's scaffold_commands tests, which is exactly
        // "partial", not "verified".
        "consumed-rest" | "entity" | "enumeration" | "java-action" | "nanoflow"
        | "published-rest" | "use-case" => (
            Status::Partial,
            "mxrs <kind> new",
            "transactional Rust declaration scaffold; generated content is not MXRB-comparable",
        ),
        "page" => (
            Status::Partial,
            "mxrs page new",
            "page and module layout scaffold with --role; MXRB's --chain slices and template catalog not ported",
        ),
        "security" => (
            Status::Partial,
            "mxrs security init",
            "module roles and project security scaffold; MXRB's access_rule guidance has no DSL surface",
        ),
        "module" => (
            Status::Partial,
            "mxrs module new",
            "module declaration layer; MXRB's marketplace search/add subcommands not ported",
        ),
        "scaffold" => (
            Status::Partial,
            "mxrs scaffold list/destroy",
            "generator catalog and digest-checked removal",
        ),
        "project" => (
            Status::Partial,
            "mxrs project inspect",
            "workspace inventory; MXRB has no other project subcommand",
        ),
        // Named rather than left to the catch-all: these two are the model
        // authoring commands that were deliberately not implemented, and the
        // reason is a missing declaration surface, not a missing CLI. Their
        // MXRB templates emit `constant`/`scheduled_event` declarations, which
        // in Rust would have to compile against builders that do not exist.
        "constant" => (
            Status::Missing,
            "—",
            "mxrs-ir has no ConstantDecl and mxrs-writer persists no Constants$Constant document",
        ),
        "scheduled-event" => (
            Status::Missing,
            "—",
            "mxrs-ir has no ScheduledEventDecl and mxrs-writer persists no ScheduledEvents$ScheduledEvent document",
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
    println!("Studio Pro parity: NOT established by this inventory");
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
            "Available MXRB commands (3):\n\n  compare  Compare projects\n  validate  Validate project\n  rename  Rename artifact\n\nRun `mxrb COMMAND --help` for usage and an example.\n",
        )
        .unwrap();
        assert_eq!(report.rows.len(), 3);
        assert_eq!(report.verified, 0);
        assert_eq!(report.partial, 2);
        assert_eq!(report.missing, 1);
        assert!(!report.complete());
    }

    #[test]
    fn empty_or_changed_help_format_fails_loudly() {
        assert!(build("Available commands: none").is_err());
    }

    fn inventory(commands: &[&str]) -> String {
        format!(
            "Available MXRB commands ({}):\n{}\nRun `mxrb COMMAND --help` for usage and an example.\n",
            commands.len(),
            commands
                .iter()
                .map(|command| format!("  {command}  Description\n"))
                .collect::<String>()
        )
    }

    #[test]
    fn malformed_truncated_duplicated_and_extra_inventory_rows_fail_closed() {
        let valid = inventory(&["modules", "dump-unit"]);
        for invalid in [
            String::new(),
            inventory(&[]),
            inventory(&["modules", "modules"]),
            inventory(&["-modules"]),
            inventory(&["modules-"]),
            inventory(&["dump--unit"]),
            inventory(&["Modules"]),
            valid.replace("(2)", "(3)"),
            valid.replace("(2)", "(1)"),
            valid.replace("  modules  Description", "modules  Description"),
            valid.replace("  modules  Description", "  modules Description"),
            valid.replace("Description", ""),
            valid.replace("Run `mxrb COMMAND --help` for usage and an example.\n", ""),
            format!("{valid}unexpected warning\n"),
        ] {
            assert!(build(&invalid).is_err(), "accepted {invalid:?}");
        }
        assert!(build(&valid).is_ok());
    }

    #[test]
    fn every_existing_command_is_individually_ratchet_checked() {
        let report = build(&inventory(&["modules", "rename"])).unwrap();
        assert!(check_baseline(&report, r#"{"modules":"partial","rename":"missing"}"#).is_ok());
        assert!(check_baseline(&report, r#"{"modules":"missing","rename":"missing"}"#).is_ok());
        let regression =
            check_baseline(&report, r#"{"modules":"missing","rename":"partial"}"#).unwrap_err();
        assert!(regression.contains("command regressed: rename"));
        assert!(check_baseline(&report, r#"{"modules":"verified","rename":"missing"}"#).is_err());
        assert!(
            check_baseline(&report, r#"{"modules":"partial"}"#)
                .unwrap_err()
                .contains("new command")
        );
        assert!(
            check_baseline(
                &report,
                r#"{"modules":"partial","rename":"missing","sql":"partial"}"#
            )
            .unwrap_err()
            .contains("disappeared")
        );
        assert!(check_baseline(&report, "{}").is_err());
        assert!(check_baseline(&report, "not json").is_err());
    }

    #[test]
    fn a_named_implementation_never_implies_verified_behavior() {
        let report = build(&inventory(&["compare", "dump-unit", "modules", "sql"])).unwrap();
        assert_eq!(report.verified, 0);
        assert!(!report.complete());
        assert_eq!(
            report.studio_pro_parity,
            "not_established_by_command_inventory"
        );
    }
}
