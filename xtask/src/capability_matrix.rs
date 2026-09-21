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
            Status::Verified,
            "mxrs compare",
            "native snapshots, ordered changes and CLI: xtask command-oracle compare; explicit mappings in docs/commands/compare.md",
        ),
        "dump-unit" => (
            Status::Verified,
            "mxrs dump-unit",
            "exact metadata/hex/ASCII stdout, v1/v2, empty/absent contents and errors: xtask command-oracle dump-unit; contract in docs/commands/dump-unit.md",
        ),
        "modules" => (
            Status::Verified,
            "mxrs modules",
            "ordered names/entity/page/microflow counts, v1/v2, folders, empty models and errors: xtask command-oracle modules; presentation normalization and extensions in docs/commands/modules.md",
        ),
        "sql" => (
            Status::Verified,
            "mxrs sql",
            "lossless dynamic SQL values and bytes, read-only restrictions, v1/v2 and errors: xtask command-oracle sql; presentation contract in docs/commands/sql.md",
        ),
        // A corresponding MXRS surface exists, but full option/output/model
        // parity has not been proved and must remain visibly partial.
        "analyze" => (Status::Partial, "mxrs oql", "risk analyzer only"),
        "benchmark" => (
            Status::Partial,
            "cargo bench",
            "library benches, no CLI parity",
        ),
        "cache" => (
            Status::Partial,
            "mxrs cache status/warm/clear",
            "external derivative cache with source-fingerprint invalidation, atomic writes and typed-analysis consumption; full CLI oracle missing",
        ),
        "callees" | "callers" | "describe" | "refs" => (
            Status::Verified,
            "mxrs callees/callers/describe/refs",
            "native named-document graph and exact CLI oracles; docs/commands/document-queries.md",
        ),
        "tree" => (
            Status::Verified,
            "mxrs tree",
            "full native document inventory, ancestry and CLI: xtask command-oracle tree; docs/commands/document-queries.md",
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
            "mxrs page new/templates",
            "page scaffold with --role, --template and --chain vertical slices, plus the template catalog; MXRB additionally writes a navigation entry per page, which has no mxrs aggregator to write into",
        ),
        "security" => (
            Status::Partial,
            "mxrs security init",
            "module roles and project security scaffold, plus marker-checked entity access rules on EntityBuilder; no command-contract oracle",
        ),
        "module" => (
            Status::Partial,
            "mxrs module new/search/add",
            "module declaration layer plus the private (pre-official) catalog install path (local-directory and git sources, offline-tested); `builtin:` sources are refused honestly since mxrs bundles no built-in module tree, and CLI `update`/`remove` are not exposed because bin/mxrb itself never exposes them for this catalog either (Mxrs::ModuleCatalog::Installer supports both as a library API, matching Mxrb::Marketplace::Installer)",
        ),
        "scaffold" => (
            Status::Partial,
            "mxrs scaffold list/destroy",
            "generator catalog and digest-checked removal",
        ),
        "project" => (
            Status::Verified,
            "mxrs project inspect",
            "native workspace inventory with explicit Cargo path/field mapping, text/JSON and errors: xtask command-oracle project; docs/commands/project.md",
        ),
        "preflight" => (
            Status::Partial,
            "mxrs preflight",
            "read-only MPR, page/layout, flow/code-action and nanoflow compatibility audit; unversioned-corpus and CLI oracles remain incomplete",
        ),
        "portability" => (
            Status::Partial,
            "mxrs portability",
            "per-unit typed/partial/preserved inventory, fail-closed --require-typed gate, and byte-exact editable-document round-trip verification; not every native family has typed authoring yet",
        ),
        "changelog" => (
            Status::Partial,
            "mxrs changelog",
            "credential-free GitHub Releases reader with optional validated version and explicit HTTP, transport and JSON failures; live release availability remains external",
        ),
        "test" => (
            Status::Partial,
            "mxrs test",
            "declarative JSON suites run on the MXRS-owned native flow interpreter (mxrb Native::Executor contract: shared store, setup/cleanup hooks, return and count expectations, [MXRS_TEST] transcript); --plan keeps the validation-only mode; the Mendix-Runtime-toolchain executor is not ported",
        ),
        "evaluate" => (
            Status::Partial,
            "mxrs evaluate",
            "declarative JSON checks for artifacts, references, cycles, unresolved references and module dependency rules; arbitrary Ruby check blocks are intentionally not executed",
        ),
        "presentation" => (
            Status::Verified,
            "mxrs presentation init",
            "paired native/Cargo scaffold CLI and complete native application layout: xtask command-oracle presentation; docs/commands/presentation.md",
        ),
        "protocols" => (
            Status::Verified,
            "mxrs protocols",
            "native fail-closed GUID registry audit, human/JSON output and errors: xtask command-oracle protocols; no currently evidenced connector GUIDs; docs/commands/protocols.md",
        ),
        "evaluation" => (
            Status::Partial,
            "mxrs evaluation new",
            "transactional JSON evaluation scaffold consumed by mxrs evaluate; arbitrary Ruby check blocks are intentionally absent",
        ),
        "validation" => (
            Status::Partial,
            "mxrs validation new",
            "transactional Rust validation-microflow scaffold with compiled model reachability; no command-contract oracle",
        ),
        "integration" => (
            Status::Partial,
            "mxrs integration new",
            "transactional Rust integration-adapter microflow scaffold with compiled model reachability; connector operations remain native",
        ),
        "ci" => (
            Status::Partial,
            "mxrs ci init github",
            "transactional GitHub Actions workflow for fmt, clippy and the complete Cargo test suite; no hosted-run oracle",
        ),
        "functional-test" => (
            Status::Partial,
            "mxrs functional-test new",
            "transactional JSON suite scaffold consumed directly by mxrs test --plan; runtime execution is not ported",
        ),
        "functional-instrument" => (
            Status::Partial,
            "mxrs functional-instrument",
            "atomic MPR-v2 instrumentation with isolated wrappers, assertions, logging and AfterStartup runner; Mendix Runtime execution remains outside this command",
        ),
        "repository" => (
            Status::Partial,
            "mxrs repository new",
            "transactional Cargo-native application port plus infrastructure adapter scaffold; generated Rust differs from MXRB Ruby",
        ),
        // Named rather than left to the catch-all: these two were the model
        // authoring commands blocked on a missing declaration surface rather
        // than a missing CLI. That surface now exists end to end (ConstantDecl
        // / ScheduledEventDecl, DSL builders, writer lowering, generators), and
        // the scaffolded project compiles and reads back. They stay Partial,
        // not Verified: neither has an executable oracle comparing mxrs's
        // command contract against mxrb's, which is what Verified requires.
        "constant" => (
            Status::Partial,
            "mxrs constant new",
            "string-constant declaration scaffold persisting Constants$Constant; no command-contract oracle",
        ),
        "scheduled-event" => (
            Status::Partial,
            "mxrs scheduled-event new",
            "scheduled event and handler scaffold plus a complete typed event/schedule IR covering all eight IntervalType values and four modern schedule shapes; command-contract oracle still missing",
        ),
        "diff" => (
            Status::Verified,
            "mxrs diff",
            "native MPR change records and CLI: xtask command-oracle diff; docs/commands/diff.md",
        ),
        "export" => (Status::Partial, "mxrs export", "typed Rust export"),
        "env" => (
            Status::Partial,
            "mxrs env",
            "safe layered dotenv/profile inspection with value-free output; full CLI oracle missing",
        ),
        "doctor" => (
            Status::Partial,
            "mxrs doctor",
            "Cargo-native project, MPR and local toolchain diagnostics; full CLI oracle missing",
        ),
        "db" => (
            Status::Partial,
            "mxrs db status/up/down/destroy/credentials/url",
            "owned, labeled, loopback-only PostgreSQL Docker workspace with private external state; Mendix Runtime boot, schema sync, SQL tooling and workload analysis are not ported",
        ),
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
            Status::Verified,
            "mxrs impact",
            "native ordered transitive incoming dependencies: xtask command-oracle impact; docs/commands/document-queries.md",
        ),
        "init" => (
            Status::Partial,
            "mxrs new",
            "transactional project scaffold",
        ),
        "inspect" => (
            Status::Verified,
            "mxrs units",
            "native metadata, sorted types and ordered unit inventory, v1/v2 and BSON errors: xtask command-oracle inspect; docs/commands/inspect.md",
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
            "mxrs run",
            "boots the MXRS-owned runtime in-process from the built model: store schema, fail-closed security policy (XPath-guarded rules deny), SQLite persistence, static web shell, mxrb's Supervisor contract for the optional Vite frontend, and every named flow registered on the native interpreter (POST /api/microflow/<Module.Flow> executes model logic transactionally with effects and log); adapters for Java/client/web-service activities remain injectable seams",
        ),
        "serve" => (
            Status::Partial,
            "mxrs serve",
            "full port of mxrb's loopback-only JSON query contract (sql XOR oql, translated OQL parameter checks, psql-variable binding, CSV-derived rows, 405/413/400/422 statuses) over the owned Docker database workspace; raw-sql params are additionally bound instead of mxrb's silent discard; offline-tested end to end, live command oracle still missing",
        ),
        "validate" => (
            Status::Partial,
            "mxrs validate",
            "storage validation; scope differs",
        ),
        "marketplace" => (
            Status::Partial,
            "mxrs marketplace search/show/versions/download/install/list/remove/dependencies/update/audit/verify",
            "official Content API client plus .mpk module install (live-API-verified), a marketplace lockfile, blocker-guarded transactional removal, recursive dependency resolution from unresolved model references, standalone official widget install (envelope-kind dispatch, asset-owner collision guard), reference-safe official update, offline integrity verification of every locked package, and a live vulnerability/staleness audit of official components — all offline-tested end to end; `login` (this CLI reads credentials from the environment instead) and the `pull`/`import` command-name split (folded into `install`/`update`/`dependencies`) are the only remaining surface differences",
        ),
        "mda" => (
            Status::Verified,
            "mxrs mda inspect/compare",
            "native ZIP inventory, metadata and content differences, directories and errors: xtask command-oracle mda; stricter archive validation in docs/commands/mda.md",
        ),
        "migrate" => (
            Status::Partial,
            "mxrs migrate check/plan",
            "offline Cargo-native rebuild versus lossless imported snapshot with drift-sensitive check exit status; full CLI oracle missing",
        ),
        "upgrade" => (
            Status::Partial,
            "mxrs upgrade",
            "transactional preview/apply migrates pre-layered generated source and optionally updates generated Cargo-native version markers; incomplete layouts and inconsistent declarations fail closed; full CLI oracle missing",
        ),
        "team-server" => (
            Status::Partial,
            "mxrs team-server login/status",
            "private PAT-file pointer configuration and validated offline Git status; remote APIs and mutating Git operations are not ported",
        ),
        // Semantic refactoring. All three preview by default and mutate only
        // under `--apply`, like MXRB's own; none has a command-contract
        // oracle, so none is verified.
        "rename" => (
            Status::Partial,
            "mxrs rename",
            "model-wide rename with a per-string preview; substitution-based like MXRB's, so it cannot see references in document types nobody models",
        ),
        "remove" => (
            Status::Partial,
            "mxrs remove",
            "reference-checked removal, blocked by incoming references or child units as MXRB blocks it; modules/entities/attributes/associations refused as typed domain-model mutations",
        ),
        "move" => (
            Status::Partial,
            "mxrs move",
            "same-module unit relocation plus MXRB's cross-module composition (rename with cross_module + relocation in one transaction, reference rewrites verified by test); folder destinations are refused because folders are not indexed artifacts",
        ),
        "design" => (
            Status::Verified,
            "mxrs design",
            "scan/migrate compared over the same theme assets (structured facts, applied files byte-identical) and init paired native/Cargo with byte-identical theme kits: xtask command-oracle design; the design_system Ruby DSL policy block is not ported; docs/commands/design.md",
        ),
        "demo-user" => (
            Status::Partial,
            "mxrs demo-user new",
            "transactional demo-user declaration over the typed security surface: generated password in a private .env (0600), writer resolves it from the environment and preserves stored passwords/identities/opaque entries; role and entity references validated structurally; no command-contract oracle against mxrb's Ruby-project recipe",
        ),
        "uml" => (
            Status::Verified,
            "mxrs uml",
            "byte-identical class/activity/sequence Mermaid and PlantUML exports and shared refusals: xtask command-oracle uml; the interactive browser viewer is not ported and is refused explicitly; docs/commands/uml.md",
        ),
        "diagram-er" => (
            Status::Partial,
            "mxrs diagram-er",
            "ports the framework-neutral core (the diagram JSON projection and the audited visual layout writer, cross-module anchors interoperable through mxrb's own sidecar table); the browser server and its db-style lifecycle are refused explicitly, not pretended",
        ),
        "update" => (
            Status::Partial,
            "mxrs update",
            "full port of mxrb's check/changelog/install contract onto lucamykael/mxrs GitHub releases (source checkouts refused, cargo-install runner seam tested); live-verified to report the real HTTP failure while the repository has no published releases, so no success path is verifiable yet",
        ),
        "widgets" => (
            Status::Partial,
            "mxrs widgets",
            "sync ports write-time MPK schema synchronization (mxrs-widget-package + the writer's packages_root, tested against fixture packages); new/build drive Mendix's official npm generator with honest tool detection and are not offline-verifiable against mxrb",
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

    /// Stands in for "a command with no mxrs surface". Deliberately not a
    /// real MXRB command: naming one here couples these tests to that
    /// command's status, and they broke the day `rename` was implemented.
    const UNIMPLEMENTED: &str = "notacommand";

    #[test]
    fn parses_and_classifies_command_inventory_without_counting_headers() {
        let report = build(&format!(
            "Available MXRB commands (3):\n\n  cache  Cache operations\n  validate  Validate project\n  {UNIMPLEMENTED}  Something unported\n\nRun `mxrb COMMAND --help` for usage and an example.\n",
        ))
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
        let report = build(&inventory(&["modules", UNIMPLEMENTED])).unwrap();
        let baseline = |modules: &str, other: &str| {
            format!(r#"{{"modules":"{modules}","{UNIMPLEMENTED}":"{other}"}}"#)
        };
        assert!(check_baseline(&report, &baseline("partial", "missing")).is_ok());
        assert!(check_baseline(&report, &baseline("missing", "missing")).is_ok());
        let regression = check_baseline(&report, &baseline("missing", "partial")).unwrap_err();
        assert!(regression.contains(&format!("command regressed: {UNIMPLEMENTED}")));
        assert!(check_baseline(&report, &baseline("verified", "missing")).is_ok());
        let mut regressed = build(&inventory(&["modules", UNIMPLEMENTED])).unwrap();
        regressed
            .rows
            .iter_mut()
            .find(|row| row.mxrb_command == "modules")
            .unwrap()
            .status = Status::Partial;
        assert!(check_baseline(&regressed, &baseline("verified", "missing")).is_err());
        assert!(
            check_baseline(&report, r#"{"modules":"partial"}"#)
                .unwrap_err()
                .contains("new command")
        );
        assert!(
            check_baseline(
                &report,
                &format!(
                    r#"{{{}, "sql":"partial"}}"#,
                    baseline("partial", "missing").trim_matches(['{', '}'])
                )
            )
            .unwrap_err()
            .contains("disappeared")
        );
        assert!(check_baseline(&report, "{}").is_err());
        assert!(check_baseline(&report, "not json").is_err());
    }

    #[test]
    fn a_named_implementation_never_implies_verified_behavior() {
        let report = build(&inventory(&["lint", "report", "search", "cache"])).unwrap();
        assert_eq!(report.verified, 0);
        assert!(!report.complete());
        assert_eq!(
            report.studio_pro_parity,
            "not_established_by_command_inventory"
        );
    }
}
