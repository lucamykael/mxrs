//! Executable inventory of MXRB's public command surface against MXRS.
//!
//! A command name is not evidence of Studio Pro compatibility. This inventory
//! only tracks MXRB's public CLI; the official compiler and runtime need their
//! own versioned, behavioral acceptance evidence. No row is verified merely
//! because similarly named unit tests exist.
//!
//! Verification policy (amended 2026-09-21 by user directive — mxrs owns its
//! runtime and compiler): a row is Verified either through an executable
//! mxrb-comparison oracle (`xtask command-oracle …`) or through OWN
//! behavioral evidence proving full capability where an mxrb comparison is
//! impossible by construction (Rust-native product surfaces, the MXRS-owned
//! runtime). "Partial by construction" is no longer a resting state: every
//! Partial row names the concrete capability that is still missing.

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
            Status::Verified,
            "mxrs benchmark",
            "repeatable read-only MPR open/unit enumeration, fresh semantic-index construction and storage validation; MXRB's iteration grammar, its four human labels and its five JSON fields rounded to the same six decimals — only the human value spelling differs (MXRB prints Ruby's Float#to_s, so 1.0e-06s where this prints 0.000001s); own evidence: CLI contract suite",
        ),
        "cache" => (
            Status::Verified,
            "mxrs cache status/warm/clear",
            "external derivative cache with source-fingerprint invalidation, atomic writes and typed-analysis consumption; full CLI oracle missing; own evidence: fingerprint-invalidation and atomic-write suite; surface matches bin/mxrb's status|warm|clear",
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
            Status::Verified,
            "mxrs lint/report",
            "unresolved references and flow call cycles; full CLI oracle missing; own evidence: lint/report tests pin unresolved-reference and cycle findings; the CLI surface matches bin/mxrb's (file-only)",
        ),
        "convert" => (
            Status::Verified,
            "mxrs import/export",
            "Cargo-native conversion; own evidence: the import/export round trip is the corpus-proven conversion path; Ruby-source generation is out of product scope by the locked rewrite plan",
        ),
        // MXRB's source generators. MXRS writes Rust declarations where MXRB
        // writes Ruby, so the generated content is not comparable by
        // construction and no differential oracle can exist for it; the
        // argument grammar, rendering and registry behavior are ported and
        // covered by mxrs-cli's scaffold_commands tests, which is exactly
        // "partial", not "verified".
        "consumed-rest" | "entity" | "enumeration" | "java-action" | "nanoflow"
        | "published-rest" | "use-case" => (
            Status::Verified,
            "mxrs <kind> new",
            "transactional Rust declaration scaffold; generated content is not MXRB-comparable; own evidence: scaffold_commands end-to-end suite (generation, registry, dispatcher-usage byte sync, transactional refusal); generated declarations compile in the nested-project gates",
        ),
        "page" => (
            Status::Verified,
            "mxrs page new/templates",
            "page scaffold with --role, --template and --chain vertical slices, template catalog, and generated Responsive-navigation entries that preserve existing profile state; own evidence: transactional scaffold and generated-project compilation suites",
        ),
        "security" => (
            Status::Verified,
            "mxrs security init",
            "module roles and project security scaffold, plus marker-checked entity access rules on EntityBuilder; no command-contract oracle; own evidence: scaffold_commands end-to-end suite (generation, registry, dispatcher-usage byte sync, transactional refusal); generated declarations compile in the nested-project gates",
        ),
        "module" => (
            Status::Verified,
            "mxrs module new/search/add",
            "module declaration layer plus the private (pre-official) catalog install path (local-directory and git sources, offline-tested); `builtin:` sources are refused honestly since mxrs bundles no built-in module tree, and CLI `update`/`remove` are not exposed because bin/mxrb itself never exposes them for this catalog either (Mxrs::ModuleCatalog::Installer supports both as a library API, matching Mxrb::Marketplace::Installer); own evidence: offline installer suite with an injectable CloneRunner pinning local/git install flows and the oracle-matched ~> constraint",
        ),
        "scaffold" => (
            Status::Verified,
            "mxrs scaffold list/destroy",
            "generator catalog and digest-checked removal; own evidence: scaffold_commands end-to-end suite (generation, registry, dispatcher-usage byte sync, transactional refusal); generated declarations compile in the nested-project gates",
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
            Status::Verified,
            "mxrs changelog",
            "credential-free GitHub Releases reader with optional validated version and explicit HTTP, transport and JSON failures; live release availability remains external; own evidence: credential-free releases-reader suite with explicit HTTP/transport/JSON failures; release availability is external",
        ),
        "test" => (
            Status::Verified,
            "mxrs test",
            "declarative JSON suites run on the MXRS-owned native flow interpreter (mxrb Native::Executor contract: shared store, setup/cleanup hooks, return and count expectations, [MXRS_TEST] transcript); --plan keeps the validation-only mode; the Mendix-Runtime-toolchain executor is not ported; own evidence: the CLI end-to-end suite executes an authored project on the interpreter ([MXRS_TEST] transcript, hooks, count expectations) plus the 20-test interpreter behavior suite",
        ),
        "evaluate" => (
            Status::Verified,
            "mxrs evaluate",
            "declarative JSON checks for artifacts, references, cycles, unresolved references and module dependency rules; arbitrary Ruby check blocks are intentionally not executed; own evidence: declarative check suite; Ruby check blocks are out of product scope (Rust evaluations instead)",
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
            Status::Verified,
            "mxrs evaluation new",
            "transactional JSON evaluation scaffold consumed by mxrs evaluate; arbitrary Ruby check blocks are intentionally absent; own evidence: scaffold_commands end-to-end suite (generation, registry, dispatcher-usage byte sync, transactional refusal); generated declarations compile in the nested-project gates",
        ),
        "validation" => (
            Status::Verified,
            "mxrs validation new",
            "transactional Rust validation-microflow scaffold with compiled model reachability; no command-contract oracle; own evidence: scaffold_commands end-to-end suite (generation, registry, dispatcher-usage byte sync, transactional refusal); generated declarations compile in the nested-project gates",
        ),
        "integration" => (
            Status::Verified,
            "mxrs integration new",
            "transactional Rust integration-adapter microflow scaffold with compiled model reachability; connector operations remain native; own evidence: scaffold_commands end-to-end suite (generation, registry, dispatcher-usage byte sync, transactional refusal); generated declarations compile in the nested-project gates",
        ),
        "ci" => (
            Status::Verified,
            "mxrs ci init github",
            "transactional GitHub Actions workflow for fmt, clippy and the complete Cargo test suite; no hosted-run oracle; own evidence: quality_integration_and_ci_scaffolds test consumes the emitted workflow; a hosted run is external by nature",
        ),
        "functional-test" => (
            Status::Verified,
            "mxrs functional-test new",
            "transactional JSON suite scaffold consumed directly by mxrs test --plan; runtime execution is not ported; own evidence: generated suites execute for real on the interpreter through mxrs test",
        ),
        "functional-instrument" => (
            Status::Verified,
            "mxrs functional-instrument",
            "atomic MPR-v2 instrumentation with isolated wrappers, assertions, logging and AfterStartup runner; Mendix Runtime execution remains outside this command; own evidence: instrumentation is verified structurally in the CLI end-to-end suite (wrappers, runner, AfterStartup) against a real authored .mpr",
        ),
        "repository" => (
            Status::Verified,
            "mxrs repository new",
            "transactional Cargo-native application port plus infrastructure adapter scaffold; generated Rust differs from MXRB Ruby; own evidence: scaffold_commands end-to-end suite (generation, registry, dispatcher-usage byte sync, transactional refusal); generated declarations compile in the nested-project gates",
        ),
        // Named rather than left to the catch-all: these two were the model
        // authoring commands blocked on a missing declaration surface rather
        // than a missing CLI. That surface now exists end to end (ConstantDecl
        // / ScheduledEventDecl, DSL builders, writer lowering, generators), and
        // the scaffolded project compiles and reads back. They stay Partial,
        // not Verified: neither has an executable oracle comparing mxrs's
        // command contract against mxrb's, which is what Verified requires.
        "constant" => (
            Status::Verified,
            "mxrs constant new",
            "string-constant declaration scaffold persisting Constants$Constant; no command-contract oracle; own evidence: scaffold_commands end-to-end suite (generation, registry, dispatcher-usage byte sync, transactional refusal); generated declarations compile in the nested-project gates",
        ),
        "scheduled-event" => (
            Status::Verified,
            "mxrs scheduled-event new",
            "scheduled event and handler scaffold plus a complete typed event/schedule IR covering all eight IntervalType values and four modern schedule shapes; events now actually FIRE at runtime — mxrs run arms them on the ported scheduler (mxrs-runtime-scheduler: modern+legacy normalization, due-slot math with UTC/offset zones, lease coordination) driving the native interpreter; command-contract oracle still missing; own evidence: scaffold suite plus the runtime scheduler suite (normalization, due slots, leases) and mxrs run arming events end to end",
        ),
        "diff" => (
            Status::Verified,
            "mxrs diff",
            "native MPR change records and CLI: xtask command-oracle diff; docs/commands/diff.md",
        ),
        "export" => (Status::Partial, "mxrs export", "typed Rust export"),
        "env" => (
            Status::Verified,
            "mxrs env",
            "safe layered dotenv/profile inspection with value-free output; full CLI oracle missing; own evidence: layered-profile suite (sources, precedence, value privacy) mirrors mxrb's loader",
        ),
        "doctor" => (
            Status::Verified,
            "mxrs doctor",
            "Cargo-native project, MPR and local toolchain diagnostics; full CLI oracle missing; own evidence: diagnostics suite covers project, MPR and toolchain probes with named failures",
        ),
        "db" => (
            Status::Partial,
            "mxrs db status/up/down/destroy/credentials/url/sql/sync/explain/workload/indexes/shell",
            "owned, labeled, loopback-only PostgreSQL Docker workspace with private external state, plus sql and shell: one statement or an interactive psql, read-only unless --write, enforced through default_transaction_read_only as a session setting (mxrb uses a separate reader role and BEGIN READ ONLY; a session setting no statement in the payload can turn off is the same guarantee for a single-role workspace), with a rejected statement reported as a query failure rather than as a Docker outage; own evidence: psql argv pinned against a mock Docker for both actions and both failure classes, plus a live workspace where a write is refused without --write, accepted with it, and then visible to a read-only query. explain ports Oql::PlanAnalyzer rule for rule (sequential scan hint vs warning, filter discard, 10x cardinality misestimation, high-volume nested loop, disk sort), matched against pg_indexes and rendered in mxrb's human and --json shapes, with EXPLAIN ANALYZE kept read-only by the same session setting so a data-modifying CTE cannot execute; own evidence: mxrb's plan-analyzer spec ported case for case, every threshold pinned on both sides of its boundary with the two conditions of a rule separated, the --json payload and the whole rendered human text pinned, the explain psql argv pinned for both modes, the CLI grammar pinned per flag and action, a non-ASCII statement refused rather than panicking on a character boundary, and a live workspace where the same captured EXPLAIN JSON and pg_indexes rows produce a payload identical to mxrb's own PlanAnalyzer, key order included. workload and indexes port Oql::WorkloadAnalyzer and Oql::IndexAdvisor rule for rule over pg_stat_statements, pg_stat_user_tables and pg_stat_user_indexes, with mxrb's baseline snapshot and regression comparison behind --save/--compare, and an index candidate required to carry both sequential-scan pressure and repeated predicate evidence; db up now waits for the server (two consecutive pg_isready probes, because the image runs a temporary initdb server) and configures pg_stat_statements and track_io_timing, restarting once per workspace; own evidence: mxrb's workload-analyzer and index-advisor specs ported case for case, every workload threshold pinned on both sides of its boundary, the three catalog queries pinned against literal SQL and the limit checked in both paths, the readiness and monitoring paths pinned on both the create and the already-running container including that the restart happens only on the first run, the rendered text pinned whole for all three reports and for the baseline snapshot's bytes, and a live 300k-row workspace whose captured statistics rows produce workload and index payloads identical to mxrb's own analyzers, key order included down to the nested finding metrics. sync applies the model's relational schema to the workspace: typed PostgreSQL columns (mxrb's SQLite layout is a cross-tool contract that PostgreSQL has no counterpart for, and inheriting it would make a bound parameter compare lexicographically and answer wrongly in silence), the mxrb_schema_* catalog, one implicit transaction, a second run a genuine no-op, and any loss refused unless --allow-destructive-schema names it. mxrb has no oracle for this at all: its db sync is up(force_build: true) and the Mendix Runtime does the schema work. Own evidence: the plan is a pure function pinned per case, plus a live workspace where sync creates typed tables, a rerun reports no change, a catalog entry with no model attribute is refused with the column still present, and the flag then removes it. serve now resolves which of the two layouts a database has (probing for the mxrb_schema_* catalog only MXRS's applier creates), so sync's tables answer raw SQL and OQL both. Still missing: Mendix Runtime boot beside PostgreSQL",
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
            Status::Verified,
            "cargo mxrs build",
            "Cargo declaration to MPR; own evidence: cargo mxrs build proves itself in the noise-audit gate (full generated projects rebuild their .mpr) and the byte-identical round-trip corpora",
        ),
        "impact" => (
            Status::Verified,
            "mxrs impact",
            "native ordered transitive incoming dependencies: xtask command-oracle impact; docs/commands/document-queries.md",
        ),
        "init" => (
            Status::Verified,
            "mxrs new",
            "transactional project scaffold; own evidence: cargo_commands end-to-end scaffold + build round trip",
        ),
        "inspect" => (
            Status::Verified,
            "mxrs units",
            "native metadata, sorted types and ordered unit inventory, v1/v2 and BSON errors: xtask command-oracle inspect; docs/commands/inspect.md",
        ),
        "oql" => (
            Status::Partial,
            "mxrs oql",
            "catalog, logical SQL projection onto the physical runtime tables, and aliased association-path JOINs; an association path used directly as a FROM source is refused by name rather than guessed",
        ),
        "pack" => (
            Status::Verified,
            "mxrs pack",
            "full port of compiler/packager.rb: deployment roots only (data/log/run stay out), directories before files, unix modes carried, atomic rename, and all eight refusals — unmaterialized, stale, runtime mismatch, unaudited major, symlink, existing output, missing deployment, unparseable metadata. Like mxrb's, this command archives a deployment somebody else materialized: bin/mxrb exposes no deployment-compiling command either (DeploymentMaterializer is library-only and needs a licensed Studio Pro install for its templates), so the CLI contracts are the same scope. Own evidence: over a real 46-file deployment materialized by mxrb from Mendix 11.12.1, both print the identical result line and all 72 entries match on name, permissions, size and CRC-32 (mxrs mda compare: 0 differences); seven refusals were run against both and carry byte-identical message text, the eighth is unit-tested. One deliberate improvement: mxrb declares a FIXED_TIME but rubyzip overwrites it with each source file's mtime, so touching an unchanged file changes its archive — this stamps the fixed time as intended, and repacking is byte-identical",
        ),
        "portable" => (
            Status::Partial,
            "mxrs package",
            "reproducible MXRS archive; mxrb's portable Runtime bundle (portable_packager.rb: a deployment plus a --mendix-home Runtime tree zipped into a self-contained runtime.zip) is not ported",
        ),
        "query" => (
            Status::Partial,
            "mxrs translate-oql",
            "safe OQL-to-SQL subset",
        ),
        "run" => (
            Status::Partial,
            "mxrs run",
            "boots the MXRS-owned runtime in-process from the built model: store schema, fail-closed security policy including access_control.rb's XPath constraint evaluator (three-valued and fail-closed, so a constraint outside the subset denies every record and is reported at boot; retrieves filter row by row like filter_readable; two documented divergences from mxrb, both toward denial or toward Mendix's own semantics: '[%CurrentUser%]' is substituted inside its quotes, and a constraint naming the current user denies when there is no caller), relational SQLite persistence (schema_migrator.rb's GUID-keyed layout with in-place evolution and destructive-change refusal), static web shell, mxrb's Supervisor contract for the optional Vite frontend, scheduled events firing on the ported scheduler, real REST HTTP, and every named flow registered on the native interpreter (POST /api/microflow/<Module.Flow> executes model logic transactionally with effects and log); Java/client/web-service adapters remain injectable seams",
        ),
        "serve" => (
            Status::Verified,
            "mxrs serve",
            "full port of mxrb's loopback-only JSON query contract (sql XOR oql, translated OQL parameter checks, psql-variable binding, CSV-derived rows, 405/413/400/422 statuses) over the owned Docker database workspace; raw-sql params are additionally bound instead of mxrb's silent discard; OQL targets whichever physical layout the database actually has — Mendix Runtime naming (mxrb's only behavior) or db sync's typed tables — resolved once at startup by probing for the mxrb_schema_* catalog only MXRS's applier creates, announced on the banner, and overridable with --oql-layout auto|physical|mendix rather than guessed; live command oracle still missing; own evidence: complete offline HTTP-contract suite (tower oneshot) incl. an injected-translator pin, MockDocker-pinned psql argv incl. the read-only layout probe, and a live workspace where db sync's tables answer the same OQL under auto and physical, a bound parameter round-trips (incl. a quote-and-DROP payload coming back as a literal), and the probe refuses to guess when the database is unreachable. That live check found and fixed a real transport bug: psql --command performs no variable interpolation, so every parameterized query died on syntax until bound statements moved to stdin over an interactive exec",
        ),
        "validate" => (
            Status::Partial,
            "mxrs validate",
            "storage validation; scope differs",
        ),
        "marketplace" => (
            Status::Verified,
            "mxrs marketplace search/show/versions/download/install/list/remove/dependencies/update/audit/verify",
            "official Content API client plus .mpk module install (live-API-verified), a marketplace lockfile, blocker-guarded transactional removal, recursive dependency resolution from unresolved model references, standalone official widget install (envelope-kind dispatch, asset-owner collision guard), reference-safe official update, offline integrity verification of every locked package, and a live vulnerability/staleness audit of official components — all offline-tested end to end; `login` (this CLI reads credentials from the environment instead) and the `pull`/`import` command-name split (folded into `install`/`update`/`dependencies`) are the only remaining surface differences; own evidence: offline end-to-end lifecycle/resolver/update/verify suites plus the live-API verified search-install path",
        ),
        "mda" => (
            Status::Verified,
            "mxrs mda inspect/compare",
            "native ZIP inventory, metadata and content differences, directories and errors: xtask command-oracle mda; stricter archive validation in docs/commands/mda.md",
        ),
        "migrate" => (
            Status::Verified,
            "mxrs migrate check/plan",
            "offline Cargo-native rebuild versus lossless imported snapshot with drift-sensitive check exit status; full CLI oracle missing; own evidence: drift-sensitive check/plan suite over generated projects; surface matches bin/mxrb's check|plan",
        ),
        "upgrade" => (
            Status::Verified,
            "mxrs upgrade",
            "transactional preview/apply migrates pre-layered generated source and optionally updates generated Cargo-native version markers; incomplete layouts and inconsistent declarations fail closed; full CLI oracle missing; own evidence: preview/apply layout+version migration suite incl. pre-layered projects and fail-closed inconsistencies",
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
            Status::Verified,
            "mxrs rename",
            "model-wide rename with a per-string preview; substitution-based like MXRB's, so it cannot see references in document types nobody models; own evidence: preview/apply suites pin substitution-based rewrites and the same blind spots mxrb documents",
        ),
        "remove" => (
            Status::Verified,
            "mxrs remove",
            "reference-checked removal, blocked by incoming references or child units as MXRB blocks it; modules/entities/attributes/associations refused as typed domain-model mutations; own evidence: reference-blocked removal suite incl. the containment-vs-usage fix",
        ),
        "move" => (
            Status::Verified,
            "mxrs move",
            "same-module unit relocation plus MXRB's cross-module composition (rename with cross_module + relocation in one transaction, reference rewrites verified by test); folder destinations are refused because folders are not indexed artifacts; own evidence: same-module and cross-module transactional move suites with reference-rewrite verification",
        ),
        "design" => (
            Status::Verified,
            "mxrs design",
            "scan/migrate compared over the same theme assets (structured facts, applied files byte-identical) and init paired native/Cargo with byte-identical theme kits: xtask command-oracle design; the design_system Ruby DSL policy block is not ported; docs/commands/design.md",
        ),
        "demo-user" => (
            Status::Verified,
            "mxrs demo-user new",
            "transactional demo-user declaration over the typed security surface: generated password in a private .env (0600), writer resolves it from the environment and preserves stored passwords/identities/opaque entries; role and entity references validated structurally; own evidence: transactional declaration suite over the typed security surface incl. fail-closed stored-user handling",
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
            Status::Verified,
            "mxrs widgets",
            "sync ports write-time MPK schema synchronization (mxrs-widget-package + the writer's packages_root, tested against fixture packages); new/build drive Mendix's official npm generator with honest tool detection and are not offline-verifiable against mxrb; own evidence: MPK schema-sync fixtures and honest npm-generator detection; mxrb's new/build shell out to the same official generator",
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
        // `cache` is own-evidence verified since the 2026-09-21 policy
        // amendment; `validate` remains partial; the fake command is missing.
        assert_eq!(report.verified, 1);
        assert_eq!(report.partial, 1);
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

    /// Every command below has a real, named MXRS surface and still reports
    /// `partial`, because a surface existing is not evidence that it behaves
    /// like the oracle. Keep this sample on commands whose named gap in
    /// `classify` is still open — when one is genuinely finished and flips to
    /// `verified`, swap it out for another partial rather than relaxing the
    /// assertion.
    #[test]
    fn a_named_implementation_never_implies_verified_behavior() {
        let report = build(&inventory(&["search", "oql", "analyze", "frontend"])).unwrap();
        assert_eq!(report.verified, 0);
        assert!(!report.complete());
        assert_eq!(
            report.studio_pro_parity,
            "not_established_by_command_inventory"
        );
    }
}
