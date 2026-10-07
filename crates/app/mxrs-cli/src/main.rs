//! Thin CLI dispatcher over `mxrs_cli`'s subcommand modules — mirrors
//! `bin/mxrb`'s `when "validate"`/`when "compare"`/`when "inspect"`/
//! `when "sql"`/... cases, narrowed the same way the library crate is (see
//! `lib.rs`'s doc comment for exactly what each command covers and what's
//! not ported yet — `run` boots the MXRS-owned runtime and `serve` the
//! loopback query server, while flow execution awaits the native
//! interpreter).
//! `inspect` has no `bin/mxrb` equivalent under that name — it's a new
//! single-file front end onto `compare`'s existing snapshot machinery.

use mxrs_cli::arguments::{take_flag, take_value, take_values, validate_options};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let name = args.next();
    let args: Vec<_> = args.collect();
    match name.as_deref() {
        Some("--help" | "-h") if args.is_empty() => run_help(vec![]),
        Some("--version" | "-V" | "-v") if args.is_empty() => {
            println!("mxrs {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        Some("--commands") => run_commands(args),
        Some(name) => match COMMANDS.iter().find(|command| command.name == name) {
            Some(command) if args == ["--help"] || args == ["-h"] => {
                print_help(command);
                ExitCode::SUCCESS
            }
            Some(command) => {
                let (values, flags, repeatable) = command_options(command.name);
                match validate_options(&args, values, flags, repeatable) {
                    Ok(()) => (command.run)(args),
                    Err(error) => {
                        eprintln!("[mxrs] error: {error}");
                        ExitCode::FAILURE
                    }
                }
            }
            None => {
                eprintln!("[mxrs] error: unknown command {name:?}");
                usage();
                ExitCode::FAILURE
            }
        },
        None => run_help(vec![]),
    }
}

struct Command {
    name: &'static str,
    arguments: &'static str,
    summary: &'static str,
    run: fn(Vec<String>) -> ExitCode,
}

fn command_options(
    name: &str,
) -> (
    &'static [&'static str],
    &'static [&'static str],
    &'static [&'static str],
) {
    match name {
        "benchmark" => (&["--iterations"], &["--json"], &[]),
        "changelog" | "evaluate" | "inspect" | "lint" | "preflight" | "report" | "validate" => {
            (&[], &["--json"], &[])
        }
        "portability" => (
            &[],
            &["--json", "--require-typed", "--verify-round-trip"],
            &[],
        ),
        "test" => (&[], &["--json", "--plan"], &[]),
        "functional-instrument" => (&[], &["--json"], &[]),
        "cache" => (&[], &["--json"], &[]),
        "db" => (
            &["--port", "--limit", "--save", "--compare"],
            &[
                "--json",
                "--write",
                "--analyze",
                "--allow-destructive-schema",
            ],
            &[],
        ),
        "serve" => (&["--port", "--db-port", "--oql-layout"], &["--no-up"], &[]),
        "run" => (
            &[
                "--host",
                "--server-port",
                "--api-port",
                "--client-port",
                "--port",
                "--environment",
            ],
            &["--no-frontend", "--allow-destructive-schema"],
            &[],
        ),
        "doctor" => (&[], &["--json"], &[]),
        "modules" => (&[], &["--json", "--names", "--no-progress"], &[]),
        "dump-unit" | "units" | "sql" => (&[], &["--no-progress"], &[]),
        "protocols" | "compare" | "diff" | "callees" | "callers" | "describe" | "impact"
        | "refs" | "tree" => (&[], &["--json", "--no-progress"], &[]),
        "export" => (&["-o"], &[], &[]),
        "convert" => (
            &["--output", "-o", "--mode", "--mxrs-workspace"],
            &["--release", "--offline"],
            &[],
        ),
        "env" => (&["--environment"], &["--json"], &[]),
        "import" => (&["--output", "-o", "--mode", "--mxrs-workspace"], &[], &[]),
        "javagen" => (&["--project-root"], &[], &[]),
        "new" => (
            &["--output", "-o", "--version", "--mxrs-workspace"],
            &["--no-atlas"],
            &[],
        ),
        "oql" => (&["--dialect"], &["--json"], &[]),
        "analyze" => (&["--dialect", "--sql", "--oql"], &["--json"], &[]),
        "pack" => (&["--output", "--deployment"], &["--force"], &[]),
        "portable" => (
            &["--output", "--deployment", "--mendix-home"],
            &["--force"],
            &[],
        ),
        "package" => (&["--web", "--output", "-o"], &[], &[]),
        "search" => (&["--limit", "--backend"], &["--json"], &[]),
        "find" => (&[], &["--semantic"], &[]),
        "translate-oql" => (&["--dialect"], &[], &[]),
        "page" => (
            &["--target", "--template", "--chain"],
            &["--dry-run", "--json", "--atlas"],
            &["--role"],
        ),
        "demo-user" => (
            &["--target", "--entity"],
            &["--dry-run", "--json"],
            &["--role"],
        ),
        "design" => (&["--target"], &["--apply", "--dry-run", "--json"], &[]),
        "diagram-er" => (&[], &["--apply", "--json"], &["--module"]),
        "update" => (&[], &["--check", "--changelog"], &[]),
        "widgets" => (&["--project"], &[], &[]),
        "ci" | "constant" | "consumed-rest" | "dto" | "entity" | "enumeration" | "evaluation"
        | "functional-test" | "integration" | "java-action" | "javascript-action" | "microflow"
        | "nanoflow" | "published-rest" | "repository" | "scheduled-event" | "security"
        | "use-case" | "validation" => (&["--target"], &["--dry-run", "--json"], &[]),
        "module" => (&["--target", "--registry"], &["--dry-run", "--json"], &[]),
        "presentation" => (
            &["--target"],
            &["--dry-run", "--json", "--no-progress"],
            &[],
        ),
        "scaffold" => (&["--target"], &[], &[]),
        "rename" | "remove" | "move" => (&[], &["--apply", "--json"], &[]),
        "marketplace" => (
            &[
                "--version",
                "--mendix-version",
                "--output",
                "-o",
                "--limit",
                "--target-root",
                "--mpr",
            ],
            &["--json", "--apply", "--allow-model-upgrade"],
            &[],
        ),
        "mda" => (&[], &["--json", "--no-progress"], &[]),
        "migrate" => (&[], &["--json"], &[]),
        "project" => (&[], &["--json", "--no-progress"], &[]),
        "team-server" => (&["--pat-file"], &["--json"], &[]),
        "uml" => (
            &[
                "--export",
                "--format",
                "--microflow",
                "--root",
                "--depth",
                "--port",
            ],
            &[],
            &["--module"],
        ),
        "upgrade" => (&["--mendix", "--target"], &["--apply", "--json"], &[]),
        _ => (&[], &[], &[]),
    }
}

macro_rules! commands {
    ($($name:literal, $arguments:literal, $summary:literal, $run:ident;)*) => {
        const COMMANDS: &[Command] = &[$(Command { name: $name, arguments: $arguments, summary: $summary, run: $run }),*];
    };
}

// Dispatch and discovery share a registry: help cannot advertise a stub or
// silently omit an implemented command.
commands! {
    "analyze", "<file.mpr> [--dialect postgresql|sql_server|ansi] [--json] | --sql QUERY | --oql QUERY", "Analyze OQL or SQL query risks with dialect-specific advice", run_analyze;
    "benchmark", "<file.mpr> [--iterations N] [--json]", "Measure model loading, semantic indexing, and validation performance", run_benchmark;
    "cache", "<status|warm|clear> <file.mpr> [--json]", "Inspect or manage the semantic index cache", run_cache;
    "ci", "init github [--target DIR] [--dry-run] [--json]", "Create a GitHub Actions workflow", run_ci;
    "callees", "<file.mpr> <artifact> [--json]", "List distinct directly called artifacts", run_callees;
    "callers", "<file.mpr> <artifact> [--json]", "List distinct direct callers", run_callers;
    "changelog", "[VERSION] [--json]", "Show mxrs release notes from GitHub", run_changelog;
    "compare", "<left.mpr> <right.mpr> [--json]", "Compare structural model snapshots", run_compare;
    "convert", "mendix-to-rust <file.mpr> --output <directory> [--mode axum|actix-web|rocket] [--mxrs-workspace <path>] | rust-to-mendix [directory] --output <file.mpr> [--release] [--offline]", "Convert between a Mendix MPR and a Cargo-native MXRS project", run_convert;
    "constant", "new <Module.Constant> [--target DIR] [--dry-run] [--json]", "Scaffold a string constant declaration", run_constant;
    "consumed-rest", "new <Module.Client> [--target DIR] [--dry-run] [--json]", "Scaffold a consumed REST adapter microflow", run_consumed_rest;
    "demo-user", "new <Name> [--entity Module.Entity] [--role ROLE] [--target DIR] [--dry-run] [--json]", "Create a local Mendix demo user backed by an ignored .env secret", run_demo_user;
    "describe", "<file.mpr> <artifact> [--json]", "Describe an artifact and its reference edges", run_describe;
    "design", "init [--target DIR] [--dry-run] [--json] | scan <file.mpr> [--json] | migrate <file.mpr> <literal> <token> [--apply] [--json]", "Initialize, inventory, or migrate the project design system", run_design;
    "db", "<status|up|down|destroy|credentials|url|shell> <file.mpr> [--port PORT] [--json] | sql <file.mpr> \"SELECT ...\" [--write] | explain <file.mpr> \"SELECT ...\" [--analyze] [--json] | workload <file.mpr> [--limit N] [--save FILE] [--compare FILE] [--json] | indexes <file.mpr> [--limit N] [--json] | sync <file.mpr> [--allow-destructive-schema] [--json]", "Manage, migrate, query and profile an isolated PostgreSQL workspace", run_db;
    "diagram-er", "<file.mpr> [--module NAME] [--json] | layout <file.mpr> <layout.json> [--apply] [--json]", "Project the domain ER diagram or apply audited visual layout", run_diagram_er;
    "diff", "<left.mpr> <right.mpr> [--json]", "List structural changes between two MPRs", run_diff;
    "doctor", "[DIR] [--json]", "Check a Cargo-native project and local toolchain", run_doctor;
    "dto", "new <Module.Dto> [--target DIR] [--dry-run] [--json]", "Scaffold a non-persistable entity (DTO) declaration", run_dto;
    "dump-unit", "<file.mpr> <unit_id> [--no-progress]", "Dump native unit metadata and bytes", run_dump_unit;
    "entity", "new <Module.Entity> [--target DIR] [--dry-run] [--json]", "Scaffold a domain entity declaration", run_entity;
    "enumeration", "new <Module.Enumeration> [--target DIR] [--dry-run] [--json]", "Scaffold an enumeration declaration", run_enumeration;
    "env", "[DIR] [--environment NAME] [--json]", "Inspect an environment profile without values", run_env;
    "evaluate", "<file.mpr> <evaluation.json> [--json]", "Run declarative static model checks", run_evaluate;
    "evaluation", "new <Name> [--target DIR] [--dry-run] [--json]", "Create declarative static model checks", run_evaluation;
    "export", "<file.mpr> [-o <out.rs>]", "Export complete editable Rust declarations", run_export;
    "find", "<file.mpr> <text> [--semantic]", "Find artifacts by name or semantic text", run_find;
    "functional-test", "new <Module.Flow> [--target DIR] [--dry-run] [--json]", "Create a declarative runtime test suite", run_functional_test;
    "functional-instrument", "<writable.mpr> <suite.json> [--json]", "Instrument a disposable MPR with a functional test runner", run_functional_instrument;
    "help", "[command]", "Show command usage", run_help;
    "impact", "<file.mpr> <artifact> [--json]", "Find transitive incoming dependencies", run_impact;
    "import", "<file.mpr> --output <directory> [--mode axum|actix-web|rocket] [--mxrs-workspace <path>]", "Import into a Cargo-native project", run_import;
    "inspect", "<file.mpr> [--json]", "Show a structural model snapshot", run_inspect;
    "integration", "new <Module.Adapter> [--target DIR] [--dry-run] [--json]", "Create an integration adapter microflow", run_integration;
    "java-action", "new <Module.Adapter> [--target DIR] [--dry-run] [--json]", "Scaffold a Java Action adapter microflow", run_java_action;
    "javascript-action", "new <Module.Action> [--target DIR] [--dry-run] [--json]", "Scaffold a JavaScript action and its JavaScript", run_javascript_action;
    "javagen", "<file.mpr> [--project-root <directory>]", "Generate Java entity proxies", run_javagen;
    "lint", "<file.mpr> [--json]", "Check explicit references and recursive call components", run_lint;
    "module", "new <Module> [--target DIR] [--dry-run] [--json] | search [query] --registry SOURCE [--json] | add <name|directory> --registry SOURCE [--target DIR] [--json]", "Scaffold an editable module declaration layer, or search/install from a private module catalog", run_module;
    "marketplace", "<search|show|versions|download> <name-or-id> [--version V] [--mendix-version V] [-o FILE] [--limit N] [--json] | install <package.mpk> <file.mpr> [--target-root DIR] [--allow-model-upgrade] [--apply] [--json] | list [--target-root DIR] [--json] | remove <name> [--target-root DIR] [--mpr FILE] [--apply] [--json] | dependencies <name> [--target-root DIR] [--mendix-version V] [--apply] [--json] | update <name[@version]> [--target-root DIR] [--mendix-version V] [--apply] [--json] | audit [--target-root DIR] [--mendix-version V] [--json] | verify [--target-root DIR] [--json]", "Search, download, install, list, remove, update, audit, verify, or resolve dependencies of official Marketplace content", run_marketplace;
    "mda", "<inspect|compare> ...", "Inspect or compare Mendix deployment archives", run_mda;
    "microflow", "new <Module.Flow> [--target DIR] [--dry-run] [--json]", "Scaffold a microflow in the service of what it is about", run_microflow;
    "migrate", "<check|plan> [DIR] [--json]", "Compare a Cargo-native build with its imported MPR snapshot", run_migrate;
    "modules", "<file.mpr> [--json | --names] [--no-progress]", "List modules with entity, page and microflow counts", run_modules;
    "move", "<file.mpr> <name> <container> [--apply] [--json]", "Preview or apply a unit move, composing rename across modules", run_move;
    "nanoflow", "new <Module.Flow> [--target DIR] [--dry-run] [--json]", "Scaffold a client nanoflow declaration", run_nanoflow;
    "new", "<name> --output <directory> [--version 11.12.1] [--mxrs-workspace <path>] [--no-atlas]", "Create a Cargo-native project with Atlas", run_new;
    "oql", "<file.mpr> [--dialect postgresql|sql_server|ansi] [--json]", "Catalog OQL and logical query risks", run_oql;
    "pack", "<file.mpr> [--output FILE.mda] [--deployment DIR] [--force]", "Package a materialized deployment into an MDA", run_pack;
    "package", "<file.mpr> --web <directory> --output <archive.tar>", "Create a deterministic MXRS archive", run_package;
    "portability", "<file.mpr> [--json] [--verify-round-trip] [--require-typed]", "Audit typed authoring versus lossless model preservation", run_portability;
    "portable", "<file.mpr> [--output runtime.zip] [--deployment DIR] [--mendix-home DIR] [--force]", "Build an executable portable Runtime ZIP", run_portable;
    "page", "new <Module.Page> [--template NAME] [--chain CHAIN] [--atlas] [--role Module.Role] [--target DIR] [--dry-run] [--json] | templates [--target DIR] [--json]", "Scaffold a page declaration, a page-led vertical slice, or list page templates", run_page;
    "preflight", "<file.mpr> [--json]", "Audit native compiler and runtime compatibility", run_preflight;
    "protocols", "<file.mpr> [--json]", "Audit imported Marketplace protocol connectors", run_protocols;
    "project", "inspect [DIR] [--json]", "Inspect a Cargo-native project workspace", run_project;
    "presentation", "init <Module> [--target DIR] [--dry-run] [--json]", "Initialize presentation and the application layout", run_presentation;
    "published-rest", "new <Module.Handler> [--target DIR] [--dry-run] [--json]", "Scaffold a published REST handler microflow", run_published_rest;
    "refs", "<file.mpr> <artifact> [--json]", "Show incoming references with their property paths", run_refs;
    "remove", "<file.mpr> <qualified-name> [--apply] [--json]", "Preview or apply a reference-safe removal", run_remove;
    "rename", "<file.mpr> <old-name> <new-name> [--apply] [--json]", "Preview or apply a model-wide rename", run_rename;
    "repository", "new <Module.Name> [--target DIR] [--dry-run] [--json]", "Scaffold a repository port and infrastructure adapter", run_repository;
    "report", "<file.mpr> [--json]", "Summarize explicit-reference lint and module dependencies", run_report;
    "run", "[DIR] [--host HOST] [--server-port PORT] [--client-port PORT] [--environment NAME] [--no-frontend] [--allow-destructive-schema]", "Run the built project on the MXRS runtime with its web shell", run_run;
    "scaffold", "<list|destroy> [<kind:name>] [--target DIR]", "List generators or remove a registered scaffold", run_scaffold;
    "scheduled-event", "new <Module.Event> [--target DIR] [--dry-run] [--json]", "Scaffold a scheduled event and its handler microflow", run_scheduled_event;
    "search", "<text> <file.mpr> [--backend auto|tfidf] [--limit N] [--json]", "Rank artifacts by similarity to a text", run_search;
    "security", "init <Module> [--target DIR] [--dry-run] [--json]", "Scaffold module roles and project security", run_security;
    "serve", "<file.mpr> [--port PORT] [--db-port PORT] [--no-up] [--oql-layout auto|physical|mendix]", "Serve loopback read-only SQL and OQL queries over the project database", run_serve;
    "sql", "<file.mpr> <query>", "Run read-only model-store SQL", run_sql;
    "translate-oql", "<query> [--dialect postgresql|sql_server|ansi]", "Translate the supported safe OQL subset", run_translate_oql;
    "team-server", "login --pat-file FILE [--json] | status DIR [--json]", "Configure a PAT pointer or inspect a local Team Server repository", run_team_server;
    "test", "<file.mpr> <suite.json> [--plan] [--json]", "Run a functional test suite on the MXRS runtime, or validate its plan", run_test;
    "tree", "<file.mpr> [module] [--json]", "Group indexed artifacts by module and kind", run_tree;
    "uml", "<file.mpr> --export class|activity|sequence [--format mermaid|plantuml] [--module NAME] [--microflow Module.Flow] [--root NAME] [--depth N]", "Export class, activity, or sequence diagrams as Mermaid or PlantUML", run_uml;
    "units", "<file.mpr>", "List native units and storage metadata", run_units;
    "update", "[--check | --changelog]", "Check for or install the latest published mxrs release", run_update;
    "upgrade", "[--mendix VERSION] [--target DIR] [--apply] [--json]", "Preview or apply a generated layout and optional version upgrade", run_upgrade;
    "use-case", "new <Module.Flow> [--target DIR] [--dry-run] [--json]", "Scaffold an application use-case microflow", run_use_case;
    "validate", "<file.mpr> [--json]", "Check storage-format integrity", run_validate;
    "validation", "new <Module.Flow> [--target DIR] [--dry-run] [--json]", "Create an application validation microflow", run_validation;
    "verify-package", "<archive.tar>", "Verify archive paths and content hashes", run_verify_package;
    "widgets", "new <Name> [DIR] | build <DIR> [--project MENDIX_PROJECT_DIR] | sync <project> <file.mpr>", "Create, build, or synchronize pluggable widget packages", run_widgets;
}

fn run_changelog(mut args: Vec<String>) -> ExitCode {
    let json = take_flag(&mut args, "--json");
    if args.len() > 1 {
        eprintln!("Usage: mxrs changelog [VERSION] [--json]");
        return ExitCode::FAILURE;
    }
    match mxrs_cli::changelog::fetch(args.first().map(String::as_str)) {
        Ok(release) => {
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&release).expect("release is serializable")
                );
            } else {
                match &release.published_at {
                    Some(date) => println!("{} ({date})", release.title),
                    None => println!("{}", release.title),
                }
                println!();
                println!("{}", release.body.as_deref().unwrap_or("No release notes."));
                println!();
                println!("{}", release.url);
            }
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("[mxrs] error: {error}");
            ExitCode::FAILURE
        }
    }
}

/// Hands the terminal to `psql`.
///
/// On Unix this replaces the process, as mxrb's `exec` does: psql needs the
/// real terminal for its pager, readline and `\!`, and an intermediate
/// parent would also swallow its exit status. Elsewhere it is spawned and
/// waited on, which costs the process replacement but keeps the status.
fn run_database_shell(command: &[String]) -> ExitCode {
    let Some((program, rest)) = command.split_first() else {
        eprintln!("[mxrs] error: empty database shell command");
        return ExitCode::FAILURE;
    };
    let mut process = std::process::Command::new(program);
    process.args(rest);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt as _;
        // `exec` only returns when it failed to replace this process.
        let error = process.exec();
        eprintln!("[mxrs] error: cannot start {program}: {error}");
        ExitCode::FAILURE
    }
    #[cfg(not(unix))]
    {
        match process.status() {
            Ok(status) if status.success() => ExitCode::SUCCESS,
            Ok(_) => ExitCode::FAILURE,
            Err(error) => {
                eprintln!("[mxrs] error: cannot start {program}: {error}");
                ExitCode::FAILURE
            }
        }
    }
}

/// `db workload`, including the baseline it can save and the one it can
/// compare against.
///
/// The snapshot is written before the report is printed: if saving fails, the
/// caller learns that instead of reading a report they believe was recorded.
fn run_db_workload(
    workspace: &mxrs_cli::database::DatabaseWorkspace,
    limit: u32,
    save: Option<&str>,
    compare: Option<&str>,
    json: bool,
) -> ExitCode {
    let report = match workspace.workload(limit) {
        Ok(report) => report,
        Err(error) => {
            eprintln!("[mxrs] error: {error}");
            return ExitCode::FAILURE;
        }
    };
    if let Some(path) = save {
        let snapshot = mxrs_oql::baseline::dump(&report, &mxrs_cli::db_reports::captured_at());
        if let Err(error) = std::fs::write(path, snapshot) {
            eprintln!("[mxrs] error: cannot write {path}: {error}");
            return ExitCode::FAILURE;
        }
    }
    let comparison = match compare {
        Some(path) => match std::fs::read_to_string(path)
            .map_err(|error| format!("cannot read workload baseline: {error}"))
            .and_then(|text| {
                mxrs_oql::baseline::compare(&report, &text).map_err(|error| error.to_string())
            }) {
            Ok(comparison) => Some(comparison),
            Err(message) => {
                eprintln!("[mxrs] error: {message}");
                return ExitCode::FAILURE;
            }
        },
        None => None,
    };
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&report).expect("a workload report is serializable")
        );
        return ExitCode::SUCCESS;
    }
    print!("{}", mxrs_cli::db_reports::render_workload_report(&report));
    if let Some(comparison) = &comparison {
        print!("{}", mxrs_cli::db_reports::render_comparison(comparison));
    }
    ExitCode::SUCCESS
}

fn run_db(mut args: Vec<String>) -> ExitCode {
    let json = take_flag(&mut args, "--json");
    let port = match take_value(&mut args, "--port")
        .as_deref()
        .unwrap_or("55432")
        .parse::<u16>()
    {
        Ok(port) if port > 0 => port,
        _ => {
            eprintln!("[mxrs] error: --port requires an integer from 1 to 65535");
            return ExitCode::FAILURE;
        }
    };
    let write = take_flag(&mut args, "--write");
    let analyze = take_flag(&mut args, "--analyze");
    let allow_destructive = take_flag(&mut args, "--allow-destructive-schema");
    let limit = take_value(&mut args, "--limit");
    let save = take_value(&mut args, "--save");
    let compare = take_value(&mut args, "--compare");
    // `sql` and `explain` are the actions that take a statement after the
    // model, so the arity check is per action rather than a single count.
    let expected = match args.first().map(String::as_str) {
        Some("sql" | "explain") => 3,
        _ => 2,
    };
    if args.len() != expected
        || !matches!(
            args[0].as_str(),
            "status"
                | "up"
                | "down"
                | "destroy"
                | "credentials"
                | "url"
                | "sql"
                | "explain"
                | "workload"
                | "indexes"
                | "sync"
                | "shell"
        )
    {
        eprintln!(
            "Usage: mxrs db <status|up|down|destroy|credentials|url|shell> <file.mpr> [--port PORT] [--json]\n       mxrs db sql <file.mpr> \"SELECT ...\" [--write] [--port PORT]\n       mxrs db explain <file.mpr> \"SELECT ...\" [--analyze] [--json] [--port PORT]\n       mxrs db workload <file.mpr> [--limit N] [--save FILE] [--compare FILE] [--json]\n       mxrs db indexes <file.mpr> [--limit N] [--json]\n       mxrs db sync <file.mpr> [--allow-destructive-schema] [--json]"
        );
        return ExitCode::FAILURE;
    }
    if write && !matches!(args[0].as_str(), "sql" | "shell") {
        eprintln!("[mxrs] error: --write applies only to db sql and db shell");
        return ExitCode::FAILURE;
    }
    if analyze && args[0].as_str() != "explain" {
        eprintln!("[mxrs] error: --analyze applies only to db explain");
        return ExitCode::FAILURE;
    }
    if allow_destructive && args[0].as_str() != "sync" {
        eprintln!("[mxrs] error: --allow-destructive-schema applies only to db sync");
        return ExitCode::FAILURE;
    }
    if limit.is_some() && !matches!(args[0].as_str(), "workload" | "indexes") {
        eprintln!("[mxrs] error: --limit applies only to db workload and db indexes");
        return ExitCode::FAILURE;
    }
    if (save.is_some() || compare.is_some()) && args[0].as_str() != "workload" {
        eprintln!("[mxrs] error: --save and --compare apply only to db workload");
        return ExitCode::FAILURE;
    }
    // mxrb's defaults: a workload listing is meant to be read, an index sweep
    // is meant to be exhaustive.
    let default_limit = if args[0] == "indexes" { 50 } else { 20 };
    let limit = match limit {
        Some(value) => match value.parse::<u32>() {
            Ok(limit) if (1..=1_000).contains(&limit) => limit,
            _ => {
                eprintln!("[mxrs] error: --limit requires an integer from 1 to 1000");
                return ExitCode::FAILURE;
            }
        },
        None => default_limit,
    };
    let workspace = match mxrs_cli::database::DatabaseWorkspace::open(&args[1], port) {
        Ok(workspace) => workspace,
        Err(error) => {
            eprintln!("[mxrs] error: {error}");
            return ExitCode::FAILURE;
        }
    };
    let action = args[0].as_str();
    if action == "sql" {
        return match workspace.execute(&args[2], write) {
            Ok(output) => {
                print!("{output}");
                ExitCode::SUCCESS
            }
            Err(error) => {
                eprintln!("[mxrs] error: {error}");
                ExitCode::FAILURE
            }
        };
    }
    if action == "explain" {
        return match workspace.explain(&args[2], analyze) {
            Ok(report) => {
                if json {
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&report)
                            .expect("a plan report is serializable")
                    );
                } else {
                    print!("{}", mxrs_cli::db_reports::render_plan_report(&report));
                }
                ExitCode::SUCCESS
            }
            Err(error) => {
                eprintln!("[mxrs] error: {error}");
                ExitCode::FAILURE
            }
        };
    }
    if action == "workload" {
        return run_db_workload(&workspace, limit, save.as_deref(), compare.as_deref(), json);
    }
    if action == "indexes" {
        return match workspace.index_advice(limit) {
            Ok(advice) => {
                if json {
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&advice)
                            .expect("index advice is serializable")
                    );
                } else {
                    print!("{}", mxrs_cli::db_reports::render_index_advice(&advice));
                }
                ExitCode::SUCCESS
            }
            Err(error) => {
                eprintln!("[mxrs] error: {error}");
                ExitCode::FAILURE
            }
        };
    }
    if action == "sync" {
        return match workspace.sync(allow_destructive) {
            Ok(report) => {
                if json {
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&report)
                            .expect("a sync report is serializable")
                    );
                } else {
                    print!("{}", mxrs_cli::db_reports::render_sync_report(&report));
                }
                ExitCode::SUCCESS
            }
            Err(error) => {
                eprintln!("[mxrs] error: {error}");
                ExitCode::FAILURE
            }
        };
    }
    if action == "shell" {
        return run_database_shell(&workspace.shell_command(write));
    }
    if matches!(action, "credentials" | "url") {
        return match workspace.credentials() {
            Ok(credentials) => {
                if action == "url" {
                    println!("{}", credentials.url);
                } else if json {
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&credentials)
                            .expect("database credentials are serializable")
                    );
                } else {
                    println!("Host     : {}", credentials.host);
                    println!("Port     : {}", credentials.port);
                    println!("Database : {}", credentials.database);
                    println!("Username : {}", credentials.username);
                    println!("Password : {}", credentials.password);
                }
                ExitCode::SUCCESS
            }
            Err(error) => {
                eprintln!("[mxrs] error: {error}");
                ExitCode::FAILURE
            }
        };
    }
    let result = match action {
        "status" => workspace.status(),
        "up" => workspace.up(),
        "down" => workspace.down(),
        "destroy" => workspace.destroy(),
        _ => unreachable!("validated above"),
    };
    match result {
        Ok(status) => {
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&status).expect("database status is serializable")
                );
            } else {
                println!("Source      : {}", status.source.display());
                println!("Container   : {}", status.container);
                println!("Volume      : {}", status.volume);
                println!("Image       : {}", status.image);
                println!("Port        : {}", status.port);
                println!("Initialized : {}", status.initialized);
                println!("State       : {}", status.container_state);
            }
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("[mxrs] error: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run_team_server(mut args: Vec<String>) -> ExitCode {
    let json = take_flag(&mut args, "--json");
    let pat_file = take_value(&mut args, "--pat-file");
    match args.as_slice() {
        [action] if action == "login" && pat_file.is_some() => {
            match mxrs_cli::team_server::Credentials::default()
                .configure_pat_file(pat_file.expect("validated above"))
            {
                Ok(report) => {
                    if json {
                        println!(
                            "{}",
                            serde_json::to_string_pretty(&report)
                                .expect("login report is serializable")
                        );
                    } else {
                        println!(
                            "[mxrs] configured {} to reference {}",
                            report.credentials_file.display(),
                            report.pat_file.display()
                        );
                    }
                    ExitCode::SUCCESS
                }
                Err(error) => {
                    eprintln!("[mxrs] error: {error}");
                    ExitCode::FAILURE
                }
            }
        }
        [action, root] if action == "status" && pat_file.is_none() => {
            match mxrs_cli::team_server::status(root) {
                Ok(report) => {
                    if json {
                        println!(
                            "{}",
                            serde_json::to_string_pretty(&report)
                                .expect("repository status is serializable")
                        );
                    } else {
                        println!("Repository : {}", report.root.display());
                        println!("Remote     : {}", report.repository_url);
                        print!("{}", report.status);
                    }
                    ExitCode::SUCCESS
                }
                Err(error) => {
                    eprintln!("[mxrs] error: {error}");
                    ExitCode::FAILURE
                }
            }
        }
        _ => {
            eprintln!("Usage: mxrs team-server login --pat-file FILE [--json]");
            eprintln!("       mxrs team-server status DIR [--json]");
            ExitCode::FAILURE
        }
    }
}

fn usage() {
    eprintln!(
        "Run `mxrs --commands` to list implemented commands or `mxrs help <command>` for usage."
    );
}

fn print_help(command: &Command) {
    println!(
        "Usage: mxrs {} {}\n{}",
        command.name, command.arguments, command.summary
    );
}

fn run_help(args: Vec<String>) -> ExitCode {
    match args.as_slice() {
        [] => {
            println!(
                "MXRS {} — Cargo-native Mendix tooling",
                env!("CARGO_PKG_VERSION")
            );
            println!(
                "Usage: mxrs <command> [arguments]\n       mxrs --commands [--json]\n       mxrs --version"
            );
            for command in COMMANDS {
                println!("  {:<18} {}", command.name, command.summary);
            }
            ExitCode::SUCCESS
        }
        [name] => match COMMANDS.iter().find(|command| command.name == name) {
            Some(command) => {
                print_help(command);
                ExitCode::SUCCESS
            }
            None => {
                eprintln!("[mxrs] error: unknown command {name:?}");
                ExitCode::FAILURE
            }
        },
        _ => {
            eprintln!("Usage: mxrs help [command]");
            ExitCode::FAILURE
        }
    }
}

fn run_commands(mut args: Vec<String>) -> ExitCode {
    let json = take_flag(&mut args, "--json");
    if !args.is_empty() {
        eprintln!("Usage: mxrs --commands [--json]");
        return ExitCode::FAILURE;
    }
    if json {
        let commands: Vec<_> = COMMANDS.iter().map(|command| serde_json::json!({"name": command.name, "usage": format!("mxrs {} {}", command.name, command.arguments), "summary": command.summary})).collect();
        println!(
            "{}",
            serde_json::to_string_pretty(&commands).expect("command catalog is serializable")
        );
    } else {
        println!("Available MXRS commands ({}):\n", COMMANDS.len());
        for command in COMMANDS {
            println!("  {:<18} {}", command.name, command.summary);
        }
    }
    ExitCode::SUCCESS
}

fn run_new(mut args: Vec<String>) -> ExitCode {
    let output = take_value(&mut args, "--output").or_else(|| take_value(&mut args, "-o"));
    let version = take_value(&mut args, "--version").unwrap_or_else(|| "11.12.1".to_string());
    let workspace = take_value(&mut args, "--mxrs-workspace");
    let plain = take_flag(&mut args, "--no-atlas");
    if args.len() != 1 || output.is_none() {
        eprintln!(
            "[mxrs] error: usage: mxrs new <name> --output <directory> [--version 11.12.1] [--mxrs-workspace <path>] [--no-atlas]"
        );
        return ExitCode::FAILURE;
    }
    // A new project looks the way Studio Pro's does: Atlas, from the
    // Marketplace. Without the packages, or asked not to, it is a plain one.
    if !plain {
        match mxrs_cli::atlas::packages(&version) {
            Ok(packages) => {
                // Absolute, as the plain scaffold makes it: what is run in the
                // project — rustfmt, Cargo — is given its path from anywhere.
                let destination =
                    std::path::PathBuf::from(output.as_deref().expect("validated above"));
                let destination = std::path::absolute(&destination).unwrap_or(destination);
                return match mxrs_cli::atlas::create(
                    &args[0],
                    &version,
                    &destination,
                    workspace.as_deref().map(std::path::Path::new),
                    &packages,
                ) {
                    Ok(()) => {
                        println!(
                            "[mxrs] created {} at {} with Atlas",
                            args[0],
                            destination.display()
                        );
                        ExitCode::SUCCESS
                    }
                    Err(error) => {
                        // Half a project is no project: what was written
                        // before the failure is removed.
                        let _ = std::fs::remove_dir_all(&destination);
                        eprintln!("[mxrs] error: {error}");
                        ExitCode::FAILURE
                    }
                };
            }
            Err(why) => eprintln!("[mxrs] warning: without Atlas: {why}"),
        }
    }
    let mut scaffold =
        mxrs_scaffold::ProjectScaffold::new(&args[0], version, output.expect("validated above"));
    if let Some(workspace) = workspace {
        scaffold = scaffold.dependency(mxrs_scaffold::MxrsDependency::Path(
            std::path::Path::new(&workspace).join("crates/app/mxrs"),
        ));
    }
    match mxrs_scaffold::generate_project(&scaffold) {
        Ok(report) => {
            println!(
                "[mxrs] created {} at {} ({} files)",
                report.package_name,
                report.destination.display(),
                report.files
            );
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("[mxrs] error: {error}");
            ExitCode::FAILURE
        }
    }
}

/// Every scaffold command shares one shape: parse, generate, render, and
/// surface a typed `mxrs-scaffold` error verbatim. Wrapping that once keeps
/// the twelve entry points below free of duplicated error plumbing.
fn reported(outcome: Result<(), String>) -> ExitCode {
    match outcome {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("[mxrs] error: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run_entity(args: Vec<String>) -> ExitCode {
    reported(mxrs_cli::scaffold::generate(
        mxrs_scaffold::ArtifactKind::Entity,
        args,
    ))
}

fn run_enumeration(args: Vec<String>) -> ExitCode {
    reported(mxrs_cli::scaffold::generate(
        mxrs_scaffold::ArtifactKind::Enumeration,
        args,
    ))
}

fn run_marketplace(args: Vec<String>) -> ExitCode {
    mxrs_cli::marketplace::run(args)
}

fn run_mda(mut args: Vec<String>) -> ExitCode {
    take_flag(&mut args, "--no-progress");
    let json = take_flag(&mut args, "--json");
    let result = match args.as_slice() {
        [action, path] if action == "inspect" => mxrs_packager::inspect_mda(path).map(|report| {
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&serde_json::json!({
                        "path": report.path,
                        "sha256": report.sha256,
                        "roots": report.roots(),
                        "files": report.files().count(),
                        "metadata": report.metadata,
                    }))
                    .expect("MDA inspection is serializable")
                );
            } else {
                println!(
                    "Runtime       : {}",
                    report.metadata["RuntimeVersion"]
                        .as_str()
                        .unwrap_or_default()
                );
                println!(
                    "Project       : {}",
                    report.metadata["ProjectName"].as_str().unwrap_or_default()
                );
                println!("Files         : {}", report.files().count());
                println!("Roots         : {}", report.roots().join(", "));
                println!("SHA-256       : {}", report.sha256);
            }
        }),
        [action, left, right] if action == "compare" && !json => {
            mxrs_packager::compare_mda(left, right).map(|differences| {
                for difference in &differences {
                    println!(
                        "{}\t{}",
                        match difference.status {
                            mxrs_packager::MdaDifferenceStatus::Added => "added",
                            mxrs_packager::MdaDifferenceStatus::Removed => "removed",
                            mxrs_packager::MdaDifferenceStatus::Changed => "changed",
                        },
                        difference.path
                    );
                }
                println!("[mxrs] {} difference(s)", differences.len());
            })
        }
        _ => {
            eprintln!("Usage: mxrs mda inspect <file.mda> [--json]");
            eprintln!("       mxrs mda compare <left.mda> <right.mda>");
            return ExitCode::FAILURE;
        }
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("[mxrs] error: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run_migrate(mut args: Vec<String>) -> ExitCode {
    let json = take_flag(&mut args, "--json");
    if args.is_empty() || args.len() > 2 || !matches!(args[0].as_str(), "check" | "plan") {
        eprintln!("Usage: mxrs migrate <check|plan> [DIR] [--json]");
        return ExitCode::FAILURE;
    }
    let action = args.remove(0);
    let root = args
        .first()
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| ".".into()));
    let manifest = root.join("Cargo.toml");
    let snapshot = root.join("model/imported");
    match mxrs_cli::cargo_project::diff(manifest, snapshot, false, true) {
        Ok(result) => {
            let clean = result.is_identical();
            if json {
                let changes = |operation| {
                    result
                        .changes
                        .iter()
                        .filter(|change| change.operation == operation)
                        .map(|change| {
                            serde_json::json!({
                                "path": change.path,
                                "before": change.before,
                                "after": change.after,
                            })
                        })
                        .collect::<Vec<_>>()
                };
                println!(
                    "{}",
                    serde_json::to_string_pretty(&serde_json::json!({
                        "clean": clean,
                        "added": changes(mxrs_cli::compare::Operation::Added),
                        "removed": changes(mxrs_cli::compare::Operation::Removed),
                        "changed": changes(mxrs_cli::compare::Operation::Changed),
                    }))
                    .expect("migration plan is serializable")
                );
            } else {
                let count = |operation| {
                    result
                        .changes
                        .iter()
                        .filter(|change| change.operation == operation)
                        .count()
                };
                println!("Added: {}", count(mxrs_cli::compare::Operation::Added));
                println!("Removed: {}", count(mxrs_cli::compare::Operation::Removed));
                println!("Changed: {}", count(mxrs_cli::compare::Operation::Changed));
                println!(
                    "[mxrs] {}",
                    if clean {
                        "No model drift"
                    } else {
                        "Model drift detected"
                    }
                );
            }
            if action == "check" && !clean {
                ExitCode::FAILURE
            } else {
                ExitCode::SUCCESS
            }
        }
        Err(error) => {
            eprintln!("[mxrs] error: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run_upgrade(mut args: Vec<String>) -> ExitCode {
    let version = take_value(&mut args, "--mendix");
    let target = take_value(&mut args, "--target").unwrap_or_else(|| ".".to_string());
    let apply = take_flag(&mut args, "--apply");
    let json = take_flag(&mut args, "--json");
    if !args.is_empty() {
        eprintln!("Usage: mxrs upgrade [--mendix VERSION] [--target DIR] [--apply] [--json]");
        return ExitCode::FAILURE;
    }
    let result = match version {
        Some(version) => mxrs_scaffold::lifecycle::upgrade_project(target, &version, apply),
        None => mxrs_scaffold::lifecycle::migrate_project_layers(target, apply),
    };
    match result {
        Ok(report) => {
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&serde_json::json!({
                        "root": report.root,
                        "from": report.from,
                        "to": report.to,
                        "created": report.created,
                        "updated": report.updated,
                        "files": report.files,
                        "migrated_layers": report.migrated_layers,
                        "applied": report.applied,
                    }))
                    .expect("upgrade report is serializable")
                );
            } else {
                println!(
                    "[mxrs] {} {}: {} -> {} ({} file(s), layers: {})",
                    if report.applied { "Updated" } else { "Preview" },
                    report.root.display(),
                    report.from,
                    report.to,
                    report.files.len(),
                    if report.migrated_layers {
                        "migrated"
                    } else {
                        "current"
                    }
                );
                for path in &report.created {
                    println!("  create  {}", path.display());
                }
                for path in &report.updated {
                    println!("  update  {}", path.display());
                }
            }
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("[mxrs] error: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run_rename(args: Vec<String>) -> ExitCode {
    mxrs_cli::refactor::run_rename(args)
}

fn run_remove(args: Vec<String>) -> ExitCode {
    mxrs_cli::refactor::run_remove(args)
}

fn run_move(args: Vec<String>) -> ExitCode {
    mxrs_cli::refactor::run_move(args)
}

fn run_constant(args: Vec<String>) -> ExitCode {
    reported(mxrs_cli::scaffold::generate(
        mxrs_scaffold::ArtifactKind::Constant,
        args,
    ))
}

fn run_scheduled_event(args: Vec<String>) -> ExitCode {
    reported(mxrs_cli::scaffold::generate(
        mxrs_scaffold::ArtifactKind::ScheduledEvent,
        args,
    ))
}

fn run_dto(args: Vec<String>) -> ExitCode {
    reported(mxrs_cli::scaffold::generate(
        mxrs_scaffold::ArtifactKind::Dto,
        args,
    ))
}

fn run_microflow(args: Vec<String>) -> ExitCode {
    reported(mxrs_cli::scaffold::generate(
        mxrs_scaffold::ArtifactKind::Microflow,
        args,
    ))
}

fn run_use_case(args: Vec<String>) -> ExitCode {
    reported(mxrs_cli::scaffold::generate(
        mxrs_scaffold::ArtifactKind::UseCase,
        args,
    ))
}

fn run_nanoflow(args: Vec<String>) -> ExitCode {
    reported(mxrs_cli::scaffold::generate(
        mxrs_scaffold::ArtifactKind::Nanoflow,
        args,
    ))
}

fn run_page(args: Vec<String>) -> ExitCode {
    reported(mxrs_cli::scaffold::generate(
        mxrs_scaffold::ArtifactKind::Page,
        args,
    ))
}

fn run_published_rest(args: Vec<String>) -> ExitCode {
    reported(mxrs_cli::scaffold::generate(
        mxrs_scaffold::ArtifactKind::PublishedRest,
        args,
    ))
}

fn run_consumed_rest(args: Vec<String>) -> ExitCode {
    reported(mxrs_cli::scaffold::generate(
        mxrs_scaffold::ArtifactKind::ConsumedRest,
        args,
    ))
}

fn run_javascript_action(args: Vec<String>) -> ExitCode {
    reported(mxrs_cli::scaffold::generate(
        mxrs_scaffold::ArtifactKind::JavaScriptAction,
        args,
    ))
}

fn run_java_action(args: Vec<String>) -> ExitCode {
    reported(mxrs_cli::scaffold::generate(
        mxrs_scaffold::ArtifactKind::JavaAction,
        args,
    ))
}

fn run_functional_test(args: Vec<String>) -> ExitCode {
    reported(mxrs_cli::scaffold::generate(
        mxrs_scaffold::ArtifactKind::FunctionalTest,
        args,
    ))
}

fn run_evaluation(args: Vec<String>) -> ExitCode {
    reported(mxrs_cli::scaffold::generate(
        mxrs_scaffold::ArtifactKind::Evaluation,
        args,
    ))
}

fn run_validation(args: Vec<String>) -> ExitCode {
    reported(mxrs_cli::scaffold::generate(
        mxrs_scaffold::ArtifactKind::Validation,
        args,
    ))
}

fn run_integration(args: Vec<String>) -> ExitCode {
    reported(mxrs_cli::scaffold::generate(
        mxrs_scaffold::ArtifactKind::Integration,
        args,
    ))
}

fn run_ci(args: Vec<String>) -> ExitCode {
    reported(mxrs_cli::scaffold::generate(
        mxrs_scaffold::ArtifactKind::Ci,
        args,
    ))
}

fn run_functional_instrument(mut args: Vec<String>) -> ExitCode {
    let json = take_flag(&mut args, "--json");
    let [project, definition] = args.as_slice() else {
        eprintln!("Usage: mxrs functional-instrument <writable.mpr> <suite.json> [--json]");
        return ExitCode::FAILURE;
    };
    match mxrs_cli::functional::instrument(project, definition) {
        Ok(report) => {
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&serde_json::json!({
                        "module": report.module,
                        "runner": report.runner,
                        "tests": report.tests,
                    }))
                    .expect("instrumentation report is serializable")
                );
            } else {
                println!(
                    "[mxrs] Instrumented {} tests; runner {}",
                    report.tests, report.runner
                );
            }
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("[mxrs] error: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run_security(args: Vec<String>) -> ExitCode {
    reported(mxrs_cli::scaffold::generate(
        mxrs_scaffold::ArtifactKind::Security,
        args,
    ))
}

fn run_demo_user(args: Vec<String>) -> ExitCode {
    reported(mxrs_cli::scaffold::generate(
        mxrs_scaffold::ArtifactKind::DemoUser,
        args,
    ))
}

fn run_design(mut args: Vec<String>) -> ExitCode {
    const USAGE: &str = "Usage: mxrs design init [--target DIR] [--dry-run] [--json] | \
        scan <file.mpr> [--json] | \
        migrate <file.mpr> <literal> <token> [--apply] [--json]";
    match args.first().map(String::as_str) {
        Some("init") => reported(mxrs_cli::scaffold::generate(
            mxrs_scaffold::ArtifactKind::Design,
            args,
        )),
        Some("scan") => {
            args.remove(0);
            let json = take_flag(&mut args, "--json");
            let [source] = args.as_slice() else {
                eprintln!("{USAGE}");
                return ExitCode::FAILURE;
            };
            match mxrs_cli::design::DesignSystem::scan(source) {
                Ok(design) => {
                    print_design_scan(&design, json);
                    ExitCode::SUCCESS
                }
                Err(error) => {
                    eprintln!("[mxrs] error: {error}");
                    ExitCode::FAILURE
                }
            }
        }
        Some("migrate") => {
            args.remove(0);
            let json = take_flag(&mut args, "--json");
            let apply = take_flag(&mut args, "--apply");
            let [source, literal, token] = args.as_slice() else {
                eprintln!("{USAGE}");
                return ExitCode::FAILURE;
            };
            let plan = match mxrs_cli::design::MigrationPlan::build(source, literal, token) {
                Ok(plan) => plan,
                Err(error) => {
                    eprintln!("[mxrs] error: {error}");
                    return ExitCode::FAILURE;
                }
            };
            if apply && let Err(error) = plan.apply() {
                eprintln!("[mxrs] error: {error}");
                return ExitCode::FAILURE;
            }
            if json {
                let payload = serde_json::json!({
                    "applied": apply,
                    "changes": plan.changes(),
                    "occurrences": plan.occurrences(),
                });
                println!(
                    "{}",
                    serde_json::to_string_pretty(&payload).expect("plan is serializable")
                );
            } else {
                for change in plan.changes() {
                    println!("{}\t{} replacements", change.path, change.occurrences);
                }
                let label = if apply { "Applied" } else { "Preview" };
                println!(
                    "[mxrs] {label}: {} replacements in {} files",
                    plan.occurrences(),
                    plan.changes().len()
                );
            }
            ExitCode::SUCCESS
        }
        Some(action) => {
            eprintln!("[mxrs] error: unknown design action {action:?}; use init, scan, or migrate");
            ExitCode::FAILURE
        }
        None => {
            eprintln!("{USAGE}");
            ExitCode::FAILURE
        }
    }
}

fn print_design_scan(design: &mxrs_cli::design::DesignSystem, json: bool) {
    if json {
        let catalogs: serde_json::Map<String, serde_json::Value> = design
            .catalogs()
            .iter()
            .map(|(path, parsed)| {
                (
                    path.clone(),
                    parsed.clone().unwrap_or(serde_json::Value::Null),
                )
            })
            .collect();
        let payload = serde_json::json!({
            "tokens": design.tokens(),
            "themes": design.themes(),
            "catalogs": catalogs,
            "unresolved_references": design.unresolved_references(),
            "literal_colors": design
                .literal_colors()
                .iter()
                .map(|token| token.name.clone())
                .collect::<Vec<_>>(),
        });
        println!(
            "{}",
            serde_json::to_string_pretty(&payload).expect("scan payload is serializable")
        );
    } else {
        println!("name\tvalue\tkind\ttheme\tlocation");
        for token in design.tokens() {
            println!(
                "{}\t{}\t{}\t{}\t{}:{}",
                token.name,
                token.value,
                token.kind,
                token.theme.as_deref().unwrap_or("-"),
                token.path,
                token.line
            );
        }
        println!(
            "[mxrs] {} tokens, {} themes, {} literal colors, {} unresolved references",
            design.tokens().len(),
            design.themes().len(),
            design.literal_colors().len(),
            design.unresolved_references().len()
        );
    }
}

fn run_widgets(mut args: Vec<String>) -> ExitCode {
    const USAGE: &str = "Usage: mxrs widgets new <Name> [DIR] | \
        build <DIR> [--project MENDIX_PROJECT_DIR] | \
        sync <project> <file.mpr>";
    match args.first().map(String::as_str) {
        Some("new") => {
            args.remove(0);
            let (name, directory) = match args.as_slice() {
                [name] => (name.clone(), std::env::current_dir().unwrap_or_default()),
                [name, directory] => (name.clone(), PathBuf::from(directory)),
                _ => {
                    eprintln!("{USAGE}");
                    return ExitCode::FAILURE;
                }
            };
            match mxrs_cli::widgets::WidgetDevelopment::new().create(&name, &directory) {
                Ok(created) => {
                    println!("[mxrs] Widget project: {}", created.display());
                    ExitCode::SUCCESS
                }
                Err(error) => {
                    eprintln!("[mxrs] error: {error}");
                    ExitCode::FAILURE
                }
            }
        }
        Some("build") => {
            args.remove(0);
            let project = take_value(&mut args, "--project").map(PathBuf::from);
            let [directory] = args.as_slice() else {
                eprintln!("{USAGE}");
                return ExitCode::FAILURE;
            };
            match mxrs_cli::widgets::WidgetDevelopment::new()
                .build(Path::new(directory), project.as_deref())
            {
                Ok(packages) => {
                    for package in packages {
                        println!("[mxrs] Built {}", package.display());
                    }
                    ExitCode::SUCCESS
                }
                Err(error) => {
                    eprintln!("[mxrs] error: {error}");
                    ExitCode::FAILURE
                }
            }
        }
        Some("sync") => {
            args.remove(0);
            let [source, target] = args.as_slice() else {
                eprintln!("{USAGE}");
                return ExitCode::FAILURE;
            };
            // The mxrs analog of mxrb's Ruby definition is the Cargo-native
            // project; accept its directory or its manifest directly.
            let source = Path::new(source);
            let manifest = if source.is_dir() {
                source.join("Cargo.toml")
            } else {
                source.to_path_buf()
            };
            // mxrb generates twice so the second pass sees every MPK the
            // first one put next to the target; the writer's package-schema
            // lookup does the rest.
            for _ in 0..2 {
                if let Err(error) = mxrs_cli::cargo_project::build(&manifest, target, false, false)
                {
                    eprintln!("[mxrs] error: {error}");
                    return ExitCode::FAILURE;
                }
            }
            println!("[mxrs] Synchronized widget schemas with native MPK schemas");
            println!("[mxrs] Generated {target}");
            ExitCode::SUCCESS
        }
        Some(action) => {
            eprintln!("[mxrs] error: unknown widgets action {action:?}; use new, build, or sync");
            ExitCode::FAILURE
        }
        None => {
            eprintln!("{USAGE}");
            ExitCode::FAILURE
        }
    }
}

fn run_diagram_er(mut args: Vec<String>) -> ExitCode {
    const USAGE: &str = "Usage: mxrs diagram-er <file.mpr> [--module NAME] [--json] | \
        layout <file.mpr> <layout.json> [--apply] [--json]";
    match args.first().map(String::as_str) {
        Some("up" | "down" | "status" | "destroy" | "__serve") => {
            eprintln!(
                "[mxrs] error: the browser ER-diagram server lifecycle is not ported; use \
                 `mxrs diagram-er <file.mpr> --json` for the diagram data and \
                 `mxrs diagram-er layout` to apply visual changes"
            );
            ExitCode::FAILURE
        }
        Some("layout") => {
            args.remove(0);
            let json = take_flag(&mut args, "--json");
            let apply = take_flag(&mut args, "--apply");
            let [source, layout_path] = args.as_slice() else {
                eprintln!("{USAGE}");
                return ExitCode::FAILURE;
            };
            let payload = match std::fs::read_to_string(layout_path)
                .map_err(|error| format!("cannot read {layout_path}: {error}"))
                .and_then(|source| {
                    serde_json::from_str::<serde_json::Value>(&source)
                        .map_err(|error| format!("{layout_path} is not valid JSON: {error}"))
                }) {
                Ok(payload) => payload,
                Err(error) => {
                    eprintln!("[mxrs] error: {error}");
                    return ExitCode::FAILURE;
                }
            };
            let plan = match mxrs_cli::diagram_er::plan_layout(Path::new(source), &payload) {
                Ok(plan) => plan,
                Err(error) => {
                    eprintln!("[mxrs] error: {error}");
                    return ExitCode::FAILURE;
                }
            };
            let changes = if apply {
                match mxrs_cli::diagram_er::apply_layout(Path::new(source), plan) {
                    Ok(changes) => changes,
                    Err(error) => {
                        eprintln!("[mxrs] error: {error}");
                        return ExitCode::FAILURE;
                    }
                }
            } else {
                plan.changes()
            };
            if json {
                let payload = serde_json::json!({ "applied": apply, "changes": changes });
                println!(
                    "{}",
                    serde_json::to_string_pretty(&payload).expect("plan is serializable")
                );
            } else {
                let label = if apply { "Applied" } else { "Preview" };
                println!("[mxrs] {label}: {changes} visual changes");
            }
            ExitCode::SUCCESS
        }
        Some(_) => {
            let json = take_flag(&mut args, "--json");
            let modules = take_values(&mut args, "--module");
            let [source] = args.as_slice() else {
                eprintln!("{USAGE}");
                return ExitCode::FAILURE;
            };
            let payload = match mxrs_cli::diagram_er::document(Path::new(source), &modules) {
                Ok(payload) => payload,
                Err(error) => {
                    eprintln!("[mxrs] error: {error}");
                    return ExitCode::FAILURE;
                }
            };
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&payload).expect("payload is serializable")
                );
            } else {
                let empty = Vec::new();
                let module_payloads = payload["modules"].as_array().unwrap_or(&empty);
                let mut entities = 0;
                let mut associations = 0;
                println!("module\tentity\tkind\tlocation");
                for module in module_payloads {
                    for entity in module["entities"].as_array().unwrap_or(&empty) {
                        entities += 1;
                        println!(
                            "{}\t{}\t{}\t{};{}",
                            module["name"].as_str().unwrap_or("-"),
                            entity["name"].as_str().unwrap_or("-"),
                            entity["kind"].as_str().unwrap_or("-"),
                            entity["x"],
                            entity["y"]
                        );
                    }
                    associations += module["associations"]
                        .as_array()
                        .map(Vec::len)
                        .unwrap_or_default();
                }
                println!(
                    "[mxrs] {} modules, {entities} entities, {associations} associations",
                    module_payloads.len()
                );
            }
            ExitCode::SUCCESS
        }
        None => {
            eprintln!("{USAGE}");
            ExitCode::FAILURE
        }
    }
}

fn run_update(mut args: Vec<String>) -> ExitCode {
    let check = take_flag(&mut args, "--check");
    let show_changelog = take_flag(&mut args, "--changelog");
    if check && show_changelog {
        eprintln!("[mxrs] error: use only one of --check or --changelog");
        return ExitCode::FAILURE;
    }
    if !args.is_empty() {
        eprintln!("Usage: mxrs update [--check | --changelog]");
        return ExitCode::FAILURE;
    }
    if show_changelog {
        return match mxrs_cli::changelog::fetch(None) {
            Ok(release) => {
                println!(
                    "{}\n\n{}\n\n{}",
                    release.title,
                    release.body.unwrap_or_default(),
                    release.url
                );
                ExitCode::SUCCESS
            }
            Err(error) => {
                eprintln!("[mxrs] error: could not load the changelog: {error}");
                ExitCode::FAILURE
            }
        };
    }
    let installed = env!("CARGO_PKG_VERSION");
    let status = match mxrs_cli::update::status(installed) {
        Ok(status) => status,
        Err(error) => {
            eprintln!("[mxrs] error: could not check the latest published version: {error}");
            return ExitCode::FAILURE;
        }
    };
    if check {
        if status.available() {
            println!("[mxrs] Update available: {installed} -> {}", status.latest);
            println!("[mxrs] Review it with `mxrs changelog`; install it with `mxrs update`.");
        } else {
            println!("[mxrs] {installed} is the latest published version.");
        }
        return ExitCode::SUCCESS;
    }
    if !status.available() {
        println!("[mxrs] {installed} is already the latest published version.");
        return ExitCode::SUCCESS;
    }
    match mxrs_cli::update::Updater::new().install(&status) {
        Ok(()) => {
            println!("[mxrs] Updated mxrs from {installed} to {}.", status.latest);
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("[mxrs] error: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run_module(args: Vec<String>) -> ExitCode {
    match args.first().map(String::as_str) {
        Some("search" | "add") => mxrs_cli::module_catalog::run(args),
        _ => reported(mxrs_cli::scaffold::generate(
            mxrs_scaffold::ArtifactKind::Module,
            args,
        )),
    }
}

fn run_repository(args: Vec<String>) -> ExitCode {
    reported(mxrs_cli::scaffold::generate(
        mxrs_scaffold::ArtifactKind::Repository,
        args,
    ))
}

fn run_scaffold(args: Vec<String>) -> ExitCode {
    reported(mxrs_cli::scaffold::registry_command(args))
}

fn run_project(args: Vec<String>) -> ExitCode {
    reported(mxrs_cli::scaffold::project_command(args))
}

fn run_env(mut args: Vec<String>) -> ExitCode {
    let requested = take_value(&mut args, "--environment");
    let json = take_flag(&mut args, "--json");
    if args.len() > 1 {
        eprintln!("Usage: mxrs env [DIR] [--environment NAME] [--json]");
        return ExitCode::FAILURE;
    }
    let root = args.first().map_or_else(
        || std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from(".")),
        std::path::PathBuf::from,
    );
    match mxrs_cli::environment::EnvironmentProfile::load(&root, requested.as_deref()) {
        Ok(profile) => {
            if json {
                let payload = serde_json::json!({
                    "environment": profile.environment,
                    "requested": profile.requested,
                    "root": profile.root,
                    "sources": profile.sources,
                    "keys": profile.keys().collect::<Vec<_>>(),
                });
                println!(
                    "{}",
                    serde_json::to_string_pretty(&payload)
                        .expect("environment profile is serializable")
                );
            } else {
                println!("Environment : {}", profile.environment);
                println!("Root        : {}", profile.root.display());
                for source in &profile.sources {
                    println!("Source      : {}", source.display());
                }
                println!(
                    "Keys        : {}",
                    profile.keys().collect::<Vec<_>>().join(", ")
                );
            }
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("[mxrs] error: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run_doctor(mut args: Vec<String>) -> ExitCode {
    let json = take_flag(&mut args, "--json");
    if args.len() > 1 {
        eprintln!("Usage: mxrs doctor [DIR] [--json]");
        return ExitCode::FAILURE;
    }
    let root = args.first().map_or_else(
        || std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from(".")),
        std::path::PathBuf::from,
    );
    let report = mxrs_cli::doctor::diagnose(root);
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&report).expect("doctor report is serializable")
        );
    } else {
        for check in &report.checks {
            println!(
                "[{}] {}: {}",
                format!("{:?}", check.status).to_uppercase(),
                check.name,
                check.message
            );
        }
        println!(
            "[mxrs] {} error(s), {} warning(s)",
            report.errors(),
            report.warnings()
        );
    }
    if report.valid {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

fn run_preflight(mut args: Vec<String>) -> ExitCode {
    let json = take_flag(&mut args, "--json");
    if args.len() != 1 {
        eprintln!("Usage: mxrs preflight <file.mpr> [--json]");
        return ExitCode::FAILURE;
    }
    match mxrs_cli::preflight::analyze(&args[0]) {
        Ok(report) => {
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&report)
                        .expect("preflight report is serializable")
                );
            } else {
                println!("Project : {}", report.path.display());
                println!(
                    "Mendix  : {}",
                    report.mendix_version.as_deref().unwrap_or("unknown")
                );
                println!("Units   : {}", report.stats.units);
                for finding in &report.findings {
                    println!(
                        "[{}] {} {} at {}: {}",
                        format!("{:?}", finding.severity).to_uppercase(),
                        finding.category,
                        finding.artifact_type,
                        finding.location,
                        finding.message
                    );
                }
                println!(
                    "[mxrs] {} error(s), {} warning(s)",
                    report.errors(),
                    report.warnings()
                );
            }
            if report.compatible {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            }
        }
        Err(error) => {
            eprintln!("[mxrs] error: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run_evaluate(mut args: Vec<String>) -> ExitCode {
    let json = take_flag(&mut args, "--json");
    let [project, definition] = args.as_slice() else {
        eprintln!("Usage: mxrs evaluate <file.mpr> <evaluation.json> [--json]");
        return ExitCode::FAILURE;
    };
    match mxrs_cli::evaluation::evaluate(project, definition) {
        Ok(result) => {
            if json {
                println!("{}", serde_json::to_string_pretty(&result).unwrap());
            } else {
                for check in &result.checks {
                    let status = if check.passed {
                        "pass"
                    } else {
                        match check.severity {
                            mxrs_cli::evaluation::Severity::Error => "error",
                            mxrs_cli::evaluation::Severity::Warning => "warning",
                        }
                    };
                    println!("[{status}] {}: {}", check.name, check.message);
                }
                let passed = result.checks.iter().filter(|check| check.passed).count();
                println!(
                    "[mxrs] score {:.2}% ({passed}/{})",
                    result.score,
                    result.checks.len()
                );
            }
            if result.passed {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            }
        }
        Err(error) => {
            eprintln!("[mxrs] error: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run_protocols(mut args: Vec<String>) -> ExitCode {
    take_flag(&mut args, "--no-progress");
    let json = take_flag(&mut args, "--json");
    let [path] = args.as_slice() else {
        eprintln!("Usage: mxrs protocols <file.mpr> [--json]");
        return ExitCode::FAILURE;
    };
    match mxrs_cli::protocols::audit(path) {
        Ok(audit) => {
            if json {
                println!("{}", serde_json::to_string_pretty(&audit).unwrap());
            } else {
                if audit.connectors.is_empty() {
                    println!("No known protocol connectors found.");
                }
                for connector in &audit.connectors {
                    println!(
                        "{}\t{}\tmarketplace={}\tprotected={}",
                        connector.protocol,
                        connector.module_name,
                        connector.metadata.marketplace_id,
                        connector.protected
                    );
                    if !connector.entities.is_empty() {
                        println!("  entities: {}", connector.entities.join(", "));
                    }
                    if !connector.microflows.is_empty() {
                        println!("  microflows: {}", connector.microflows.join(", "));
                    }
                }
                if !audit.unknown_marketplace_modules.is_empty() {
                    println!(
                        "Unrecognized marketplace modules ({}): {}",
                        audit.unknown_marketplace_modules.len(),
                        audit.unknown_marketplace_modules.join(", ")
                    );
                    println!("Run `mxrs modules {path}` to list every module.");
                }
            }
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("[mxrs] error: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run_test(mut args: Vec<String>) -> ExitCode {
    let json = take_flag(&mut args, "--json");
    let plan_only = take_flag(&mut args, "--plan");
    let [project, definition] = args.as_slice() else {
        eprintln!("Usage: mxrs test <file.mpr> <suite.json> [--plan] [--json]");
        return ExitCode::FAILURE;
    };
    if !plan_only {
        return match mxrs_cli::functional::execute(project, definition) {
            Ok(report) => {
                if json {
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&report)
                            .expect("functional report is serializable")
                    );
                } else {
                    print!("{}", report.transcript);
                    for test in report.tests.iter().filter(|test| !test.passed) {
                        println!("- {}: {}", test.name, test.message);
                    }
                    println!(
                        "[mxrs] {}/{} test(s) passed",
                        report.tests.iter().filter(|test| test.passed).count(),
                        report.tests.len()
                    );
                }
                if report.passed {
                    ExitCode::SUCCESS
                } else {
                    ExitCode::FAILURE
                }
            }
            Err(error) => {
                eprintln!("[mxrs] error: {error}");
                ExitCode::FAILURE
            }
        };
    }
    match mxrs_cli::functional::plan(project, definition) {
        Ok(plan) => {
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&plan).expect("functional plan is serializable")
                );
            } else {
                println!("Project   : {}", plan.project.display());
                println!(
                    "Mendix    : {}",
                    plan.mendix_version.as_deref().unwrap_or("unknown")
                );
                println!("Tests     : {}", plan.tests.len());
                for test in &plan.tests {
                    println!("- {} -> {}", test.name, test.target);
                }
                println!("Execution : plan-only");
            }
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("[mxrs] error: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run_portability(mut args: Vec<String>) -> ExitCode {
    let json = take_flag(&mut args, "--json");
    let require_typed = take_flag(&mut args, "--require-typed");
    let verify_round_trip = take_flag(&mut args, "--verify-round-trip");
    let [path] = args.as_slice() else {
        eprintln!(
            "Usage: mxrs portability <file.mpr> [--json] [--verify-round-trip] [--require-typed]"
        );
        return ExitCode::FAILURE;
    };
    let report = match mxrs_exporter::audit_portability(path) {
        Ok(report) => report,
        Err(error) => {
            eprintln!("[mxrs] error: {error}");
            return ExitCode::FAILURE;
        }
    };
    let verification = if verify_round_trip {
        match mxrs_exporter::verify_editable_document_round_trip(path) {
            Ok(verification) => Some(verification),
            Err(error) => {
                eprintln!("[mxrs] error: round-trip verification failed: {error}");
                return ExitCode::FAILURE;
            }
        }
    } else {
        None
    };
    if json {
        let output = match &verification {
            Some(verification) => serde_json::json!({
                "portability": report,
                "round_trip": verification,
            }),
            None => serde_json::to_value(&report).expect("portability report is serializable"),
        };
        println!("{}", serde_json::to_string_pretty(&output).unwrap());
    } else {
        println!("Project       : {}", report.path.display());
        println!(
            "Version       : {}",
            report.mendix_version.as_deref().unwrap_or("")
        );
        println!(
            "Model lossless: {}",
            if report.model_lossless { "yes" } else { "no" }
        );
        println!(
            "Fully typed   : {}",
            if report.fully_typed { "yes" } else { "no" }
        );
        println!(
            "Units         : {} typed, {} partial, {} preserved ({} total)",
            report.summary.typed_units,
            report.summary.partial_units,
            report.summary.preserved_units,
            report.summary.total_units
        );
        println!(
            "Typed export gaps: {}",
            report.summary.typed_round_trip_gaps
        );
        println!();
        println!("STATUS\tTYPED\tPARTIAL\tPRESERVED\tTYPE");
        for family in &report.families {
            println!(
                "{:?}\t{}\t{}\t{}\t{}",
                family.status, family.typed, family.partial, family.preserved, family.native_type
            );
        }
        if let Some(verification) = &verification {
            println!();
            println!(
                "Round trip    : {} of {} editable document(s) byte-identical",
                verification.byte_identical_units, verification.candidate_units
            );
            for failure in &verification.failures {
                println!("[FAIL] {failure}");
            }
        }
    }
    if verification
        .as_ref()
        .is_some_and(|verification| !verification.passed)
    {
        eprintln!("[mxrs] error: editable document round trip is not lossless");
        ExitCode::FAILURE
    } else if require_typed && !report.fully_typed {
        eprintln!(
            "[mxrs] error: {} partial and {} preserved unit(s) still require model/imported",
            report.summary.partial_units, report.summary.preserved_units
        );
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

fn semantic_index(path: &str) -> Result<mxrs_semantic::SemanticIndex, String> {
    let project = mxrs_model::Project::open(path, true).map_err(|error| error.to_string())?;
    mxrs_semantic::cache::cached_or_build(&project).map_err(|error| error.to_string())
}

#[derive(serde::Serialize)]
struct BenchmarkResult {
    iterations: u16,
    open_seconds: f64,
    index_seconds: f64,
    validate_seconds: f64,
    units: usize,
}

/// MXRB's `Benchmark#measure` rounds every average to six decimals before it
/// ever reaches stdout, so that precision — not the raw clock reading — is the
/// contract a `--json` consumer sees. Matching it keeps the two JSON documents
/// comparable field for field.
fn rounded_seconds(seconds: f64) -> f64 {
    (seconds * 1e6).round() / 1e6
}

/// Measures the same three read-only operations as MXRB's `benchmark`: open
/// and enumerate units, build a fresh semantic index, then validate storage.
/// The index deliberately bypasses MXRS's derivative cache: benchmark results
/// must describe the operation itself rather than the state of a prior command.
///
/// One rendering divergence is deliberate: the human lines format every
/// average with six decimals, while MXRB prints Ruby's `Float#to_s` of the
/// rounded value (`0.0s`, `0.0005s`, `1.0e-06s`). The labels and the JSON
/// fields match; the human value spelling does not, and reproducing Ruby's
/// float notation would buy nothing a reader of a timing report wants.
fn run_benchmark(mut args: Vec<String>) -> ExitCode {
    let json = take_flag(&mut args, "--json");
    let iterations = match take_value(&mut args, "--iterations") {
        Some(value) => match value.parse::<u16>() {
            Ok(value @ 1..=100) => value,
            _ => {
                eprintln!("[mxrs] error: --iterations must be an integer between 1 and 100");
                return ExitCode::FAILURE;
            }
        },
        None => 3,
    };
    let [path] = args.as_slice() else {
        eprintln!("Usage: mxrs benchmark <file.mpr> [--iterations N] [--json]");
        return ExitCode::FAILURE;
    };

    let measure = |operation: &str, mut run: Box<dyn FnMut() -> Result<usize, String>>| {
        let started = std::time::Instant::now();
        let mut value = 0;
        for _ in 0..iterations {
            value = run().map_err(|error| format!("{operation}: {error}"))?;
        }
        Ok::<_, String>((
            rounded_seconds(started.elapsed().as_secs_f64() / f64::from(iterations)),
            value,
        ))
    };

    let (open_seconds, units) = match measure(
        "open",
        Box::new(|| {
            mxrs_model::Project::open(path, true)
                .and_then(|project| project.all_units())
                .map(|units| units.len())
                .map_err(|error| error.to_string())
        }),
    ) {
        Ok(result) => result,
        Err(error) => {
            eprintln!("[mxrs] error: {error}");
            return ExitCode::FAILURE;
        }
    };
    let (index_seconds, _) = match measure(
        "semantic index",
        Box::new(|| {
            let project =
                mxrs_model::Project::open(path, true).map_err(|error| error.to_string())?;
            mxrs_semantic::SemanticIndex::build(&project)
                .map(|index| index.artifacts().count())
                .map_err(|error| error.to_string())
        }),
    ) {
        Ok(result) => result,
        Err(error) => {
            eprintln!("[mxrs] error: {error}");
            return ExitCode::FAILURE;
        }
    };
    let (validate_seconds, _) = match measure(
        "validate",
        Box::new(|| {
            mxrs_cli::validate::validate(path)
                .map(|report| usize::from(report.is_valid()))
                .map_err(|error| error.to_string())
        }),
    ) {
        Ok(result) => result,
        Err(error) => {
            eprintln!("[mxrs] error: {error}");
            return ExitCode::FAILURE;
        }
    };
    let result = BenchmarkResult {
        iterations,
        open_seconds,
        index_seconds,
        validate_seconds,
        units,
    };
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&result).expect("benchmark result is serializable")
        );
    } else {
        println!("Units            : {}", result.units);
        println!("Open average     : {:.6}s", result.open_seconds);
        println!("Index average    : {:.6}s", result.index_seconds);
        println!("Validate average : {:.6}s", result.validate_seconds);
    }
    ExitCode::SUCCESS
}

fn run_cache(mut args: Vec<String>) -> ExitCode {
    let json = take_flag(&mut args, "--json");
    if args.len() != 2 || !matches!(args[0].as_str(), "status" | "warm" | "clear") {
        eprintln!("Usage: mxrs cache <status|warm|clear> <file.mpr> [--json]");
        return ExitCode::FAILURE;
    }
    let started = std::time::Instant::now();
    let project = match mxrs_model::Project::open(&args[1], true) {
        Ok(project) => project,
        Err(error) => {
            eprintln!("[mxrs] error: {error}");
            return ExitCode::FAILURE;
        }
    };
    let cache = mxrs_semantic::cache::SemanticCache::default();
    let result = match args[0].as_str() {
        "status" => cache.status(&project),
        "warm" => cache.warm(&project),
        "clear" => cache.clear(&project),
        _ => unreachable!("validated above"),
    };
    match result {
        Ok(info) => {
            let elapsed = started.elapsed().as_secs_f64();
            if json {
                let mut payload = serde_json::to_value(&info).expect("cache info is serializable");
                payload["elapsed_seconds"] = serde_json::json!(elapsed);
                println!(
                    "{}",
                    serde_json::to_string_pretty(&payload).expect("cache info is serializable")
                );
            } else {
                println!("Present     : {}", info.present);
                println!("Current hit : {}", info.hit);
                println!("Entries     : {}", info.entries);
                println!("Bytes       : {}", info.bytes);
                println!("Fingerprint : {}", info.current_fingerprint);
                if let Some(removed) = info.removed {
                    println!("Removed     : {removed}");
                }
                println!("Elapsed     : {elapsed:.6}s");
            }
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("[mxrs] error: {error}");
            ExitCode::FAILURE
        }
    }
}

fn document_index(path: &str) -> Result<mxrs_semantic::documents::DocumentIndex, String> {
    let project = mxrs_model::Project::open(path, true).map_err(|error| error.to_string())?;
    mxrs_semantic::documents::DocumentIndex::build(&project).map_err(|error| error.to_string())
}

fn run_uml(mut args: Vec<String>) -> ExitCode {
    const USAGE: &str = "Usage: mxrs uml <file.mpr> --export class|activity|sequence \
        [--format mermaid|plantuml] [--module NAME] [--microflow Module.Flow] \
        [--root NAME] [--depth N]";
    let export = take_value(&mut args, "--export");
    let format = take_value(&mut args, "--format").unwrap_or_else(|| "mermaid".to_string());
    let microflow = take_value(&mut args, "--microflow");
    let root = take_value(&mut args, "--root");
    let depth = take_value(&mut args, "--depth").unwrap_or_else(|| "2".to_string());
    // MXRB accepts --port for its interactive viewer and ignores it during
    // `--export`; the viewer itself is not ported.
    take_value(&mut args, "--port");
    let modules = take_values(&mut args, "--module");
    if args.len() != 1 {
        eprintln!("{USAGE}");
        return ExitCode::FAILURE;
    }
    if !matches!(format.as_str(), "mermaid" | "plantuml") {
        eprintln!("[mxrs] error: --format requires mermaid or plantuml");
        return ExitCode::FAILURE;
    }
    let Some(export) = export else {
        eprintln!(
            "[mxrs] error: the interactive UML viewer is not ported; use --export class|activity|sequence"
        );
        return ExitCode::FAILURE;
    };
    if !matches!(export.as_str(), "class" | "activity" | "sequence") {
        eprintln!("[mxrs] error: --export requires class, activity, or sequence");
        return ExitCode::FAILURE;
    }
    match uml_diagram(
        &args[0], &export, &format, &modules, microflow, root, &depth,
    ) {
        Ok(diagram) => {
            print!("{diagram}");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("[mxrs] error: {error}");
            ExitCode::FAILURE
        }
    }
}

fn uml_diagram(
    source: &str,
    export: &str,
    format: &str,
    modules: &[String],
    microflow: Option<String>,
    root: Option<String>,
    depth: &str,
) -> Result<String, String> {
    use mxrs_cli::uml::{ActivityDiagram, ClassDiagram, SequenceDiagram};

    let mermaid = format == "mermaid";
    match export {
        "class" => {
            let project =
                mxrs_model::Project::open(source, true).map_err(|error| error.to_string())?;
            let all = project.modules().map_err(|error| error.to_string())?;
            let selected: Vec<&mxrs_model::Module> = if modules.is_empty() {
                all.iter().collect()
            } else {
                let known: std::collections::HashSet<&str> = all
                    .iter()
                    .filter_map(|module| module.name.as_deref())
                    .collect();
                let unknown: Vec<&str> = modules
                    .iter()
                    .map(String::as_str)
                    .filter(|name| !known.contains(name))
                    .collect();
                if !unknown.is_empty() {
                    return Err(format!("unknown modules: {}", unknown.join(", ")));
                }
                all.iter()
                    .filter(|module| {
                        module
                            .name
                            .as_deref()
                            .is_some_and(|name| modules.iter().any(|wanted| wanted == name))
                    })
                    .collect()
            };
            let diagram = ClassDiagram::new(selected);
            Ok(if mermaid {
                diagram.to_mermaid()
            } else {
                diagram.to_plantuml()
            })
        }
        "activity" => {
            let reference = microflow
                .filter(|name| !name.is_empty())
                .ok_or_else(|| "--microflow is required for activity export".to_string())?;
            let (module_name, flow_name) = reference
                .split_once('.')
                .ok_or_else(|| format!("microflow not found: {reference}"))?;
            let project =
                mxrs_model::Project::open(source, true).map_err(|error| error.to_string())?;
            let all = project.modules().map_err(|error| error.to_string())?;
            let flow = all
                .iter()
                .find(|module| module.name.as_deref() == Some(module_name))
                .and_then(|module| {
                    module
                        .microflows
                        .iter()
                        .find(|flow| flow.name.as_deref() == Some(flow_name))
                })
                .ok_or_else(|| format!("microflow not found: {reference}"))?;
            let diagram = ActivityDiagram::new(flow);
            Ok(if mermaid {
                diagram.to_mermaid()
            } else {
                diagram.to_plantuml()
            })
        }
        "sequence" => {
            if root.is_some() && !modules.is_empty() {
                return Err("use either --root or --module for sequence export".to_string());
            }
            if root.is_none() && modules.len() != 1 {
                return Err("--root or --module is required for sequence export".to_string());
            }
            let depth: i64 = depth
                .parse()
                .map_err(|_| format!("invalid value for depth: {depth:?}"))?;
            let index = document_index(source)?;
            let diagram = SequenceDiagram::new(
                &index,
                root.as_deref(),
                modules.first().map(String::as_str),
                depth,
            )?;
            Ok(if mermaid {
                diagram.to_mermaid()
            } else {
                diagram.to_plantuml()
            })
        }
        _ => unreachable!("validated by run_uml"),
    }
}

fn run_callers(args: Vec<String>) -> ExitCode {
    run_call_graph(args, true)
}
fn run_callees(args: Vec<String>) -> ExitCode {
    run_call_graph(args, false)
}

fn graph_command(
    mut args: Vec<String>,
    command: &str,
    query: impl FnOnce(&mxrs_semantic::documents::DocumentIndex, &str, bool) -> Result<(), String>,
) -> ExitCode {
    take_flag(&mut args, "--no-progress");
    let json = take_flag(&mut args, "--json");
    if args.len() != 2 {
        eprintln!("Usage: mxrs {command} <file.mpr> <artifact> [--json]");
        return ExitCode::FAILURE;
    }
    match document_index(&args[0]).and_then(|index| query(&index, &args[1], json)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("[mxrs] error: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run_call_graph(args: Vec<String>, incoming: bool) -> ExitCode {
    graph_command(
        args,
        if incoming { "callers" } else { "callees" },
        |index, name, json| {
            let artifacts = if incoming {
                index.calls(name, true)
            } else {
                index.calls(name, false)
            }
            .map_err(|error| error.to_string())?;
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&artifacts).expect("artifacts are serializable")
                );
            } else {
                for artifact in artifacts {
                    println!("{}\t{}", artifact.qualified_name, artifact.kind.as_str());
                }
            }
            Ok(())
        },
    )
}

fn run_describe(args: Vec<String>) -> ExitCode {
    graph_command(args, "describe", |index, name, json| {
        let artifact = index.require(name).map_err(|error| error.to_string())?;
        let incoming = index.incoming(name).map_err(|error| error.to_string())?;
        let outgoing = index.outgoing(name).map_err(|error| error.to_string())?;
        if json {
            println!("{}", serde_json::to_string_pretty(&serde_json::json!({"artifact": artifact, "incoming": incoming, "outgoing": outgoing, "fingerprint": index.fingerprint()})).expect("artifact details are serializable"));
        } else {
            println!(
                "{}\t{}\nIncoming:",
                artifact.qualified_name,
                artifact.kind.as_str()
            );
            for reference in incoming {
                println!(
                    "  {}\t{}",
                    index.artifact(&reference.from).qualified_name,
                    reference.relation
                );
            }
            println!("Outgoing:");
            for reference in outgoing {
                println!(
                    "  {}\t{}",
                    index.artifact(&reference.to).qualified_name,
                    reference.relation
                );
            }
        }
        Ok(())
    })
}

fn run_tree(mut args: Vec<String>) -> ExitCode {
    take_flag(&mut args, "--no-progress");
    let json = take_flag(&mut args, "--json");
    if !(1..=2).contains(&args.len()) || args.iter().any(|arg| arg.starts_with('-')) {
        eprintln!("Usage: mxrs tree <file.mpr> [module] [--json]");
        return ExitCode::FAILURE;
    }
    let index = match document_index(&args[0]) {
        Ok(index) => index,
        Err(error) => {
            eprintln!("[mxrs] error: {error}");
            return ExitCode::FAILURE;
        }
    };
    let selected = args.get(1).map(String::as_str);
    let mut tree: std::collections::BTreeMap<
        Option<&str>,
        std::collections::BTreeMap<&str, Vec<&str>>,
    > = std::collections::BTreeMap::new();
    for artifact in index.artifacts() {
        if selected.is_some_and(|module| artifact.module_name.as_deref() != Some(module)) {
            continue;
        }
        let kinds = tree.entry(artifact.module_name.as_deref()).or_default();
        if artifact.kind != "module" {
            kinds
                .entry(artifact.kind.as_str())
                .or_default()
                .push(&artifact.qualified_name);
        }
    }
    for kinds in tree.values_mut() {
        for names in kinds.values_mut() {
            names.sort();
        }
    }
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(
                &tree
                    .iter()
                    .map(|(module, kinds)| (module.unwrap_or("(project)"), kinds))
                    .collect::<std::collections::BTreeMap<_, _>>()
            )
            .expect("artifact tree is serializable")
        );
    } else {
        for (module, kinds) in tree {
            println!("{}", module.unwrap_or("(project)"));
            for (kind, names) in kinds {
                println!("  {kind}");
                for name in names {
                    println!("    {name}");
                }
            }
        }
    }
    ExitCode::SUCCESS
}

fn run_lint(args: Vec<String>) -> ExitCode {
    run_analysis(args, false)
}
fn run_report(args: Vec<String>) -> ExitCode {
    run_analysis(args, true)
}

fn run_analysis(mut args: Vec<String>, summary: bool) -> ExitCode {
    let json = take_flag(&mut args, "--json");
    if args.len() != 1 {
        eprintln!(
            "Usage: mxrs {} <file.mpr> [--json]",
            if summary { "report" } else { "lint" }
        );
        return ExitCode::FAILURE;
    }
    let index = match semantic_index(&args[0]) {
        Ok(index) => index,
        Err(error) => {
            eprintln!("[mxrs] error: {error}");
            return ExitCode::FAILURE;
        }
    };
    let report = index.analyze();
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&report).expect("analysis is serializable")
        );
    } else {
        println!(
            "Scope: {} (not a full Studio Pro consistency check)",
            report.scope
        );
        if summary {
            println!(
                "Call cycles: {}\nCross-module dependencies: {}",
                report.call_cycles.len(),
                report.module_dependencies.len()
            );
            for dependency in &report.module_dependencies {
                println!(
                    "{}\t->\t{}\t{} references\t{} artifacts",
                    dependency.from,
                    dependency.to,
                    dependency.references.len(),
                    dependency.source_artifacts.len()
                );
            }
        }
        for diagnostic in &report.diagnostics {
            println!(
                "[{}] {}: {}",
                diagnostic.severity, diagnostic.code, diagnostic.message
            );
        }
        for limitation in report.limitations {
            println!("Not covered: {limitation}");
        }
    }
    if report.valid() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

fn run_refs(args: Vec<String>) -> ExitCode {
    graph_command(args, "refs", |index, name, json| {
        let incoming = index.incoming(name).map_err(|error| error.to_string())?;
        if json {
            println!("{}", serde_json::to_string_pretty(&serde_json::json!({"artifact": name, "incoming": incoming, "fingerprint": index.fingerprint()})).expect("serializable reference report"));
        } else {
            for reference in incoming {
                let source = index.artifact(&reference.from);
                println!(
                    "{}\t{}\t{}\t{}",
                    source.qualified_name,
                    source.kind,
                    reference.relation,
                    reference.path.join(".")
                );
            }
        }
        Ok(())
    })
}

fn run_impact(args: Vec<String>) -> ExitCode {
    graph_command(args, "impact", |index, name, json| {
        let affected = index.impact(name).map_err(|error| error.to_string())?;
        if json {
            println!(
                "{}",
                serde_json::to_string_pretty(&affected).expect("serializable impact report")
            );
        } else {
            for artifact in affected {
                println!("{}\t{}", artifact.qualified_name, artifact.kind);
            }
        }
        Ok(())
    })
}

/// One artifact a search ranked, as `search --json` lists it.
#[derive(serde::Serialize)]
struct RankedArtifact<'a> {
    rank: usize,
    qualified_name: &'a str,
    kind: &'a str,
    distance: f64,
}

fn run_search(mut args: Vec<String>) -> ExitCode {
    let json = take_flag(&mut args, "--json");
    let limit = match take_value(&mut args, "--limit")
        .as_deref()
        .unwrap_or("10")
        .parse::<usize>()
    {
        Ok(limit) if limit > 0 => limit,
        _ => {
            eprintln!("[mxrs] error: --limit requires a positive integer");
            return ExitCode::FAILURE;
        }
    };
    // mxrb ranks with an ONNX model when one is installed and with hashed
    // term frequencies otherwise; mxrs ranks with the term frequencies.
    match take_value(&mut args, "--backend").as_deref() {
        None | Some("auto" | "tfidf") => {}
        Some("onnx") => {
            eprintln!(
                "[mxrs] error: the onnx backend is not available; mxrs ranks by term frequency (--backend tfidf)"
            );
            return ExitCode::FAILURE;
        }
        Some(other) => {
            eprintln!("[mxrs] error: unknown embedding backend: {other}");
            return ExitCode::FAILURE;
        }
    }
    let [text, path] = args.as_slice() else {
        eprintln!(
            "[mxrs] error: usage: mxrs search <text> <file.mpr> [--backend auto|tfidf] [--limit N] [--json]"
        );
        return ExitCode::FAILURE;
    };
    let index = match document_index(path) {
        Ok(index) => index,
        Err(error) => {
            eprintln!("[mxrs] error: {error}");
            return ExitCode::FAILURE;
        }
    };
    let hits = index.search(text, limit);
    if json {
        let ranked: Vec<RankedArtifact<'_>> = hits
            .iter()
            .enumerate()
            .map(|(index, (artifact, distance))| RankedArtifact {
                rank: index + 1,
                qualified_name: &artifact.qualified_name,
                kind: &artifact.kind,
                distance: *distance,
            })
            .collect();
        println!(
            "{}",
            serde_json::to_string_pretty(&ranked).expect("serializable search report")
        );
    } else {
        println!("rank\tdistance\tqualified_name\tkind");
        for (index, (artifact, distance)) in hits.iter().enumerate() {
            println!(
                "{}\t{distance:.6}\t{}\t{}",
                index + 1,
                artifact.qualified_name,
                artifact.kind
            );
        }
    }
    ExitCode::SUCCESS
}

fn run_find(mut args: Vec<String>) -> ExitCode {
    let semantic = take_flag(&mut args, "--semantic");
    let [path, text] = args.as_slice() else {
        eprintln!("[mxrs] error: usage: mxrs find <file.mpr> <text> [--semantic]");
        return ExitCode::FAILURE;
    };
    let index = match document_index(path) {
        Ok(index) => index,
        Err(error) => {
            eprintln!("[mxrs] error: {error}");
            return ExitCode::FAILURE;
        }
    };
    // By name, the artifacts whose name holds the text; by meaning, the ten
    // a search ranks nearest.
    let found: Vec<&mxrs_semantic::documents::Artifact> = if semantic {
        index
            .search(text, 10)
            .into_iter()
            .map(|(artifact, _)| artifact)
            .collect()
    } else {
        index.find(text)
    };
    for artifact in found {
        println!("{}\t{}", artifact.qualified_name, artifact.kind);
    }
    ExitCode::SUCCESS
}

fn run_analyze(mut args: Vec<String>) -> ExitCode {
    let json = take_flag(&mut args, "--json");
    let dialect = match take_value(&mut args, "--dialect")
        .as_deref()
        .map_or(Ok(mxrs_oql::Dialect::PostgreSql), mxrs_oql::Dialect::parse)
    {
        Ok(dialect) => dialect,
        Err(error) => {
            eprintln!("[mxrs] error: {error}");
            return ExitCode::FAILURE;
        }
    };
    let sql = take_value(&mut args, "--sql");
    let oql = take_value(&mut args, "--oql");
    if sql.is_some() && oql.is_some() || (sql.is_some() || oql.is_some()) && !args.is_empty() {
        eprintln!("[mxrs] error: use exactly one of --sql, --oql, or <file.mpr>");
        return ExitCode::FAILURE;
    }
    let reports: Vec<mxrs_oql::Report> = if let Some(source) = &sql {
        vec![mxrs_oql::analyze_source(source, mxrs_oql::QueryKind::Sql)]
    } else if let Some(source) = &oql {
        vec![mxrs_oql::analyze_source(source, mxrs_oql::QueryKind::Oql)]
    } else {
        if args.len() != 1 {
            eprintln!(
                "[mxrs] error: usage: mxrs analyze <file.mpr> [--dialect postgresql|sql_server|ansi] [--json] | --sql QUERY | --oql QUERY"
            );
            return ExitCode::FAILURE;
        }
        let project = match mxrs_model::Project::open(&args[0], true) {
            Ok(project) => project,
            Err(error) => {
                eprintln!("[mxrs] error: {error}");
                return ExitCode::FAILURE;
            }
        };
        match mxrs_oql::catalog(&project) {
            Ok(queries) => queries.iter().map(mxrs_oql::analyze_query).collect(),
            Err(error) => {
                eprintln!("[mxrs] error: {error}");
                return ExitCode::FAILURE;
            }
        }
    };
    if json {
        let payload = reports
            .iter()
            .map(|report| {
                let findings: Vec<serde_json::Value> = report
                    .findings
                    .iter()
                    .map(|finding| {
                        serde_json::json!({
                            "rule": finding.rule,
                            "severity": finding.severity,
                            "fragment": finding.fragment,
                            "message": finding.message,
                            "suggestions": finding.suggestions,
                            "selected_suggestion": finding.suggestions.get(dialect),
                        })
                    })
                    .collect();
                serde_json::json!({
                    "name": report.name,
                    "kind": report.kind.as_str(),
                    "source": report.source,
                    "clean": report.clean(),
                    "warnings": report.warnings(),
                    "findings": findings,
                })
            })
            .collect::<Vec<_>>();
        println!(
            "{}",
            serde_json::to_string_pretty(&payload).expect("serializable OQL analysis")
        );
    } else if reports.iter().all(|report| report.findings.is_empty()) {
        println!("[mxrs] No analysis findings");
    } else {
        let dialect_name = match dialect {
            mxrs_oql::Dialect::Ansi => "ANSI",
            mxrs_oql::Dialect::PostgreSql => "PostgreSQL",
            mxrs_oql::Dialect::SqlServer => "SQL Server",
        };
        for report in &reports {
            for finding in &report.findings {
                println!(
                    "{}  [{}] {}",
                    report.name,
                    finding.severity.to_ascii_uppercase(),
                    finding.rule
                );
                println!(
                    "  {}: {}",
                    report.kind.as_str().to_ascii_uppercase(),
                    finding.fragment
                );
                println!("  {}", finding.message);
                println!("  {dialect_name} -> {}", finding.suggestions.get(dialect));
            }
        }
    }
    ExitCode::SUCCESS
}

fn run_oql(mut args: Vec<String>) -> ExitCode {
    let json = take_flag(&mut args, "--json");
    let dialect = match take_value(&mut args, "--dialect")
        .as_deref()
        .map_or(Ok(mxrs_oql::Dialect::PostgreSql), mxrs_oql::Dialect::parse)
    {
        Ok(dialect) => dialect,
        Err(error) => {
            eprintln!("[mxrs] error: {error}");
            return ExitCode::FAILURE;
        }
    };
    if args.len() != 1 {
        eprintln!(
            "[mxrs] error: usage: mxrs oql <file.mpr> [--dialect postgresql|sql_server|ansi] [--json]"
        );
        return ExitCode::FAILURE;
    }
    let project = match mxrs_model::Project::open(&args[0], true) {
        Ok(project) => project,
        Err(error) => {
            eprintln!("[mxrs] error: {error}");
            return ExitCode::FAILURE;
        }
    };
    let queries = match mxrs_oql::catalog(&project) {
        Ok(queries) => queries,
        Err(error) => {
            eprintln!("[mxrs] error: {error}");
            return ExitCode::FAILURE;
        }
    };
    let mut rows = Vec::with_capacity(queries.len());
    for query in &queries {
        let projection = match mxrs_oql::translate_project(&query.oql, dialect, &project) {
            Ok(projection) => projection,
            Err(error) => {
                eprintln!("[mxrs] error: {error}");
                return ExitCode::FAILURE;
            }
        };
        rows.push((query, projection));
    }
    if json {
        let values = rows
            .iter()
            .map(|(query, projection)| {
                serde_json::json!({
                    "query": query,
                    "projection": projection,
                    "findings": mxrs_oql::analyze(&query.oql),
                })
            })
            .collect::<Vec<_>>();
        println!(
            "{}",
            serde_json::to_string_pretty(&values).expect("serializable OQL report")
        );
    } else if rows.is_empty() {
        println!("[mxrs] no native OQL queries found");
    } else {
        for (query, projection) in rows {
            println!("{}", query.qualified_name);
            if let Some(sql) = projection.sql {
                println!("{sql}");
            } else {
                println!("unsupported: {}", projection.warnings.join("; "));
            }
        }
    }
    ExitCode::SUCCESS
}

fn run_run(mut args: Vec<String>) -> ExitCode {
    let root = if args
        .first()
        .is_some_and(|argument| !argument.starts_with("--"))
    {
        PathBuf::from(args.remove(0))
    } else {
        PathBuf::from(".")
    };
    let environment = take_value(&mut args, "--environment");
    let host = take_value(&mut args, "--host").unwrap_or_else(|| "127.0.0.1".to_string());
    let server_port = match exclusive_port(&mut args, &["--server-port", "--api-port"], 9292) {
        Ok(port) => port,
        Err(error) => {
            eprintln!("[mxrs] error: {error}");
            return ExitCode::FAILURE;
        }
    };
    let client_port = match exclusive_port(&mut args, &["--client-port", "--port"], 5173) {
        Ok(port) => port,
        Err(error) => {
            eprintln!("[mxrs] error: {error}");
            return ExitCode::FAILURE;
        }
    };
    let frontend = !take_flag(&mut args, "--no-frontend");
    let allow_destructive_schema = take_flag(&mut args, "--allow-destructive-schema");
    if !args.is_empty() {
        eprintln!("[mxrs] error: unknown arguments: {}", args.join(" "));
        return ExitCode::FAILURE;
    }
    let options = mxrs_cli::run::RunOptions {
        root,
        host,
        server_port,
        client_port,
        frontend,
        environment,
        allow_destructive_schema,
    };
    match mxrs_cli::run::start(&options) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("[mxrs] error: {error}");
            ExitCode::FAILURE
        }
    }
}

/// mxrb's `run` accepts compatibility aliases for both ports but refuses a
/// command line that names the same port twice.
fn exclusive_port(args: &mut Vec<String>, names: &[&str], fallback: u16) -> Result<u16, String> {
    let mut found: Option<(&str, String)> = None;
    for name in names {
        if let Some(value) = take_value(args, name) {
            if found.is_some() {
                return Err(format!("use only one of {}", names.join(" or ")));
            }
            found = Some((name, value));
        }
    }
    match found {
        None => Ok(fallback),
        Some((flag, value)) => value
            .parse::<u16>()
            .ok()
            .filter(|port| *port > 0)
            .ok_or_else(|| format!("{flag} requires an integer from 1 to 65535")),
    }
}

fn run_serve(mut args: Vec<String>) -> ExitCode {
    let port = match take_value(&mut args, "--port")
        .as_deref()
        .unwrap_or("4567")
        .parse::<u16>()
    {
        Ok(port) if port > 0 => port,
        _ => {
            eprintln!("[mxrs] error: --port requires an integer from 1 to 65535");
            return ExitCode::FAILURE;
        }
    };
    let database_port = match take_value(&mut args, "--db-port")
        .as_deref()
        .unwrap_or("55432")
        .parse::<u16>()
    {
        Ok(port) if port > 0 => port,
        _ => {
            eprintln!("[mxrs] error: --db-port requires an integer from 1 to 65535");
            return ExitCode::FAILURE;
        }
    };
    let prepare = !take_flag(&mut args, "--no-up");
    let layout = match take_value(&mut args, "--oql-layout")
        .as_deref()
        .unwrap_or("auto")
        .parse::<mxrs_cli::serve::OqlLayout>()
    {
        Ok(layout) => layout,
        Err(error) => {
            eprintln!("[mxrs] error: {error}");
            return ExitCode::FAILURE;
        }
    };
    if args.len() != 1 {
        eprintln!(
            "Usage: mxrs serve <file.mpr> [--port PORT] [--db-port PORT] [--no-up] [--oql-layout auto|physical|mendix]"
        );
        return ExitCode::FAILURE;
    }
    let workspace = match mxrs_cli::database::DatabaseWorkspace::open(&args[0], database_port) {
        Ok(workspace) => workspace,
        Err(error) => {
            eprintln!("[mxrs] error: {error}");
            return ExitCode::FAILURE;
        }
    };
    if prepare && let Err(error) = workspace.up() {
        eprintln!("[mxrs] error: {error}");
        return ExitCode::FAILURE;
    }
    let (translator, chosen) = match mxrs_cli::serve::oql_translator(layout, &workspace, &args[0]) {
        Ok(resolved) => resolved,
        Err(error) => {
            eprintln!("[mxrs] error: {error}");
            return ExitCode::FAILURE;
        }
    };
    let server = match mxrs_query_server::QueryServer::new(
        std::sync::Arc::new(mxrs_cli::serve::WorkspaceQueries(workspace)),
        translator,
        "127.0.0.1",
        port,
    ) {
        Ok(server) => server,
        Err(error) => {
            eprintln!("[mxrs] error: {error}");
            return ExitCode::FAILURE;
        }
    };
    println!("[mxrs] Read-only query server: http://127.0.0.1:{port}/query");
    println!("[mxrs] Accepts POST JSON with an sql or oql field");
    println!("[mxrs] OQL layout: {chosen}");
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("[mxrs] error: cannot start the async runtime: {error}");
            return ExitCode::FAILURE;
        }
    };
    match runtime.block_on(server.serve_with_shutdown(async {
        let _ = tokio::signal::ctrl_c().await;
    })) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("[mxrs] error: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run_translate_oql(mut args: Vec<String>) -> ExitCode {
    let dialect = match take_value(&mut args, "--dialect")
        .as_deref()
        .map_or(Ok(mxrs_oql::Dialect::PostgreSql), mxrs_oql::Dialect::parse)
    {
        Ok(dialect) => dialect,
        Err(error) => {
            eprintln!("[mxrs] error: {error}");
            return ExitCode::FAILURE;
        }
    };
    if args.len() != 1 {
        eprintln!(
            "[mxrs] error: usage: mxrs translate-oql <query> [--dialect postgresql|sql_server|ansi]"
        );
        return ExitCode::FAILURE;
    }
    let projection = mxrs_oql::translate(&args[0], dialect);
    if let Some(sql) = projection.sql {
        println!("{sql}");
        ExitCode::SUCCESS
    } else {
        eprintln!("[mxrs] unsupported: {}", projection.warnings.join("; "));
        ExitCode::FAILURE
    }
}

/// `mxrs pack` is the container half of MXRB's `pack`: it archives a
/// deployment directory that something else already materialized. It does not
/// compile a model into one, and says so rather than producing an archive
/// that is structurally valid and semantically empty.
///
/// The default output mirrors MXRB's: `<project>/build/<name>.mda`.
fn run_pack(mut args: Vec<String>) -> ExitCode {
    let output = take_value(&mut args, "--output");
    let deployment = take_value(&mut args, "--deployment");
    let force = take_flag(&mut args, "--force");
    let [path] = args.as_slice() else {
        eprintln!(
            "[mxrs] error: usage: mxrs pack <file.mpr> [--output FILE.mda] [--deployment DIR] [--force]"
        );
        return ExitCode::FAILURE;
    };
    let model = Path::new(path);
    let output = output.map(PathBuf::from).unwrap_or_else(|| {
        let name = model.file_stem().unwrap_or_default();
        model
            .parent()
            .unwrap_or(Path::new("."))
            .join("build")
            .join(format!("{}.mda", name.to_string_lossy()))
    });
    match mxrs_packager::pack_mda(model, deployment.as_deref().map(Path::new), &output, force) {
        Ok(report) => {
            println!(
                "[mxrs] Packed {} files for Mendix {}",
                report.files, report.mendix_version
            );
            println!("[mxrs] SHA-256 {}", report.sha256);
            println!("[mxrs] {}", report.path.display());
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("[mxrs] error: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run_portable(mut args: Vec<String>) -> ExitCode {
    let output = take_value(&mut args, "--output");
    let deployment = take_value(&mut args, "--deployment");
    let mendix_home = take_value(&mut args, "--mendix-home");
    let force = take_flag(&mut args, "--force");
    let [path] = args.as_slice() else {
        eprintln!(
            "[mxrs] error: usage: mxrs portable <file.mpr> [--output runtime.zip] [--deployment DIR] [--mendix-home DIR] [--force]"
        );
        return ExitCode::FAILURE;
    };
    let model = Path::new(path);
    // MXRB's default: `<project>/build/runtime.zip` beside the model.
    let output = output.map(PathBuf::from).unwrap_or_else(|| {
        model
            .parent()
            .unwrap_or(Path::new("."))
            .join("build")
            .join("runtime.zip")
    });
    match mxrs_packager::pack_portable(
        model,
        deployment.as_deref().map(Path::new),
        mendix_home.as_deref().map(Path::new),
        &output,
        force,
    ) {
        Ok(report) => {
            println!(
                "[mxrs] Packed portable Runtime with {} files for Mendix {}",
                report.files, report.mendix_version
            );
            println!("[mxrs] SHA-256 {}", report.sha256);
            println!("[mxrs] {}", report.path.display());
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("[mxrs] error: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run_package(mut args: Vec<String>) -> ExitCode {
    let web = take_value(&mut args, "--web");
    let output = take_value(&mut args, "--output").or_else(|| take_value(&mut args, "-o"));
    if args.len() != 1 || web.is_none() || output.is_none() {
        eprintln!(
            "[mxrs] error: usage: mxrs package <file.mpr> --web <directory> --output <archive.tar>"
        );
        return ExitCode::FAILURE;
    }
    let options = mxrs_packager::PackageOptions::new(
        &args[0],
        web.expect("validated above"),
        output.expect("validated above"),
    );
    match mxrs_packager::package(&options) {
        Ok(report) => {
            println!(
                "[mxrs] packaged {} file(s) into {} ({} bytes, sha256 {})",
                report.payload_files,
                report.output.display(),
                report.archive_bytes,
                report.archive_sha256
            );
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("[mxrs] error: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run_verify_package(args: Vec<String>) -> ExitCode {
    if args.len() != 1 {
        eprintln!("[mxrs] error: usage: mxrs verify-package <archive.tar>");
        return ExitCode::FAILURE;
    }
    match mxrs_packager::verify_package(&args[0]) {
        Ok(manifest) => {
            println!(
                "[mxrs] OK: {} {} ({} payload files)",
                manifest.application,
                manifest.mendix_version,
                manifest.files.len()
            );
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("[mxrs] error: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run_validate(mut args: Vec<String>) -> ExitCode {
    let json = take_flag(&mut args, "--json");
    let [path] = args.as_slice() else {
        eprintln!("[mxrs] error: usage: mxrs validate <file.mpr> [--json]");
        return ExitCode::FAILURE;
    };

    let report = match mxrs_cli::validate::validate(path) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("[mxrs] error: {e}");
            return ExitCode::FAILURE;
        }
    };

    if json {
        println!(
            "{}",
            serde_json::json!({ "valid": report.is_valid(), "errors": report.errors, "warnings": report.warnings })
        );
    } else {
        for warning in &report.warnings {
            eprintln!("[mxrs] warning: {warning}");
        }
        if report.is_valid() {
            println!("[mxrs] OK");
        } else {
            for error in &report.errors {
                eprintln!("[mxrs] error: {error}");
            }
        }
    }

    if report.is_valid() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

fn run_compare(args: Vec<String>) -> ExitCode {
    run_comparison(args, false)
}
fn run_diff(args: Vec<String>) -> ExitCode {
    run_comparison(args, true)
}

fn run_comparison(mut args: Vec<String>, tabular: bool) -> ExitCode {
    let command = if tabular { "diff" } else { "compare" };
    take_flag(&mut args, "--no-progress");
    let json = take_flag(&mut args, "--json");
    if args.len() != 2 {
        eprintln!("[mxrs] error: usage: mxrs {command} <left.mpr> <right.mpr> [--json]");
        return ExitCode::FAILURE;
    }

    let result = match mxrs_cli::compare::compare(&args[0], &args[1]) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("[mxrs] error: {e}");
            return ExitCode::FAILURE;
        }
    };

    if json {
        let changes: Vec<_> = result
            .changes
            .iter()
            .map(|c| {
                serde_json::json!({
                    "operation": format!("{:?}", c.operation),
                    "path": c.json_path(),
                    "before": c.before,
                    "after": c.after,
                })
            })
            .collect();
        println!(
            "{}",
            serde_json::json!({ "identical": result.is_identical(), "changes": changes })
        );
    } else if tabular {
        for change in &result.changes {
            println!("{}", change.format_diff());
        }
    } else if result.is_identical() {
        println!("[mxrs] OK");
    } else {
        for change in &result.changes {
            println!("[mxrs] diff: {}", change.format());
        }
    }

    if result.is_identical() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

fn run_inspect(mut args: Vec<String>) -> ExitCode {
    let json = take_flag(&mut args, "--json");
    let [path] = args.as_slice() else {
        eprintln!("[mxrs] error: usage: mxrs inspect <file.mpr> [--json]");
        return ExitCode::FAILURE;
    };

    let snapshot = match mxrs_cli::inspect::inspect(path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("[mxrs] error: {e}");
            return ExitCode::FAILURE;
        }
    };

    if json {
        println!("{snapshot}");
    } else {
        print!("{}", mxrs_cli::inspect::summarize(&snapshot));
    }

    ExitCode::SUCCESS
}

fn run_units(mut args: Vec<String>) -> ExitCode {
    take_flag(&mut args, "--no-progress");
    let [path] = args.as_slice() else {
        eprintln!("[mxrs] error: usage: mxrs units <file.mpr>");
        return ExitCode::FAILURE;
    };
    let report = match mxrs_cli::browse::units(path) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("[mxrs] error: {e}");
            return ExitCode::FAILURE;
        }
    };
    println!("Project : {}", report.project_name.as_deref().unwrap_or(""));
    println!(
        "Version : {}",
        report.mendix_version.as_deref().unwrap_or("")
    );
    println!("Tables  : {}", report.tables.join(", "));
    println!();
    println!("Unit types found:");
    for t in &report.unit_types {
        println!("  {t}");
    }
    println!();
    println!("All units:");
    for u in &report.units {
        println!(
            "  [{}] {} (container={}, name={})",
            u.unit_id, u.type_name, u.container_id, u.containment_name
        );
    }
    ExitCode::SUCCESS
}

fn run_dump_unit(mut args: Vec<String>) -> ExitCode {
    take_flag(&mut args, "--no-progress");
    if args.len() != 2 {
        eprintln!("[mxrs] error: usage: mxrs dump-unit <file.mpr> <unit_id>");
        return ExitCode::FAILURE;
    }
    let dump = match mxrs_cli::browse::dump_unit(&args[0], &args[1]) {
        Ok(Some(d)) => d,
        Ok(None) => {
            eprintln!("[mxrs] error: unit {} not found", args[1]);
            return ExitCode::FAILURE;
        }
        Err(e) => {
            eprintln!("[mxrs] error: {e}");
            return ExitCode::FAILURE;
        }
    };
    println!("UnitID           : {}", dump.unit_id);
    println!("ContainerID      : {}", dump.container_id);
    println!("ContainmentName  : {}", dump.containment_name);
    println!("TypeName         : {}", dump.type_name);
    println!(
        "ContentsHash     : {}",
        dump.contents_hash.unwrap_or_default()
    );
    println!("Contents (hex)   :");
    match &dump.bytes {
        Some(bytes) => print!("{}", mxrs_cli::browse::format_hex_dump(bytes)),
        None => println!("  (empty)"),
    }
    ExitCode::SUCCESS
}

fn run_sql(mut args: Vec<String>) -> ExitCode {
    take_flag(&mut args, "--no-progress");
    if args.len() != 2 {
        eprintln!("[mxrs] error: usage: mxrs sql <file.mpr> \"<query>\"");
        return ExitCode::FAILURE;
    }
    let result = match mxrs_cli::browse::sql(&args[0], &args[1]) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("[mxrs] error: {e}");
            return ExitCode::FAILURE;
        }
    };
    for row in &result.rows {
        let cells: Vec<String> = row.iter().map(mxrs_cli::browse::format_sql_cell).collect();
        println!("[{}]", cells.join(", "));
    }
    println!("({} rows)", result.rows.len());
    ExitCode::SUCCESS
}

fn run_modules(mut args: Vec<String>) -> ExitCode {
    let json = take_flag(&mut args, "--json");
    let names_only = take_flag(&mut args, "--names");
    take_flag(&mut args, "--no-progress");
    if json && names_only {
        eprintln!("[mxrs] error: use only one of --json or --names");
        return ExitCode::FAILURE;
    }
    let [path] = args.as_slice() else {
        eprintln!(
            "[mxrs] error: usage: mxrs modules <file.mpr> [--json | --names] [--no-progress]"
        );
        return ExitCode::FAILURE;
    };
    if names_only {
        return match mxrs_cli::browse::list_modules(path) {
            Ok(names) => {
                for name in names {
                    println!("{name}");
                }
                ExitCode::SUCCESS
            }
            Err(error) => {
                eprintln!("[mxrs] error: {error}");
                ExitCode::FAILURE
            }
        };
    }
    match mxrs_cli::browse::module_summaries(path) {
        Ok(modules) => {
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&modules)
                        .expect("module summaries are serializable")
                );
            } else {
                for module in modules {
                    let name =
                        serde_json::to_string(&module.name).expect("module name is serializable");
                    println!(
                        "{name}: entities={} pages={} microflows={}",
                        module.entities, module.pages, module.microflows
                    );
                }
            }
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("[mxrs] error: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run_export(mut args: Vec<String>) -> ExitCode {
    let out_path = take_value(&mut args, "-o");
    let [path] = args.as_slice() else {
        eprintln!("[mxrs] error: usage: mxrs export <file.mpr> [-o <out.rs>]");
        return ExitCode::FAILURE;
    };

    let exported = mxrs_exporter::export_project(path).map_err(|error| {
        let mut message = error.to_string();
        for gap in error.gaps() {
            message.push_str(&format!("\n  - {}: {}", gap.path, gap.reason));
        }
        message
    });
    let source = match exported {
        Ok(s) => s,
        Err(e) => {
            eprintln!("[mxrs] error: {e}");
            return ExitCode::FAILURE;
        }
    };

    match out_path {
        Some(out_path) => {
            if let Err(e) = std::fs::write(&out_path, source) {
                eprintln!("[mxrs] error: writing {out_path}: {e}");
                return ExitCode::FAILURE;
            }
            eprintln!("[mxrs] wrote {out_path}");
        }
        None => print!("{source}"),
    }
    ExitCode::SUCCESS
}

fn run_convert(mut args: Vec<String>) -> ExitCode {
    let Some(direction) = args.first().cloned() else {
        eprintln!(
            "[mxrs] error: use `mxrs convert mendix-to-rust` or `mxrs convert rust-to-mendix`"
        );
        return ExitCode::FAILURE;
    };
    args.remove(0);
    match direction.as_str() {
        "mendix-to-rust" => run_import(args),
        "rust-to-mendix" => {
            let output = take_value(&mut args, "--output").or_else(|| take_value(&mut args, "-o"));
            let release = take_flag(&mut args, "--release");
            let offline = take_flag(&mut args, "--offline");
            let root = match args.as_slice() {
                [] => PathBuf::from("."),
                [root] => PathBuf::from(root),
                _ => {
                    eprintln!(
                        "[mxrs] error: usage: mxrs convert rust-to-mendix [directory] --output <file.mpr> [--release] [--offline]"
                    );
                    return ExitCode::FAILURE;
                }
            };
            let Some(output) = output else {
                eprintln!("[mxrs] error: rust-to-mendix requires --output <file.mpr>");
                return ExitCode::FAILURE;
            };
            match mxrs_cli::cargo_project::build(root.join("Cargo.toml"), &output, release, offline)
            {
                Ok(path) => {
                    println!("[mxrs] converted Cargo project to {}", path.display());
                    ExitCode::SUCCESS
                }
                Err(error) => {
                    eprintln!("[mxrs] error: {error}");
                    ExitCode::FAILURE
                }
            }
        }
        _ => {
            eprintln!("[mxrs] error: direction must be mendix-to-rust or rust-to-mendix");
            ExitCode::FAILURE
        }
    }
}

fn run_import(mut args: Vec<String>) -> ExitCode {
    let output = take_value(&mut args, "--output").or_else(|| take_value(&mut args, "-o"));
    let workspace = take_value(&mut args, "--mxrs-workspace").map(std::path::PathBuf::from);
    let mode = take_value(&mut args, "--mode").unwrap_or_else(|| "axum".to_string());
    let Some(mode) = mxrs_exporter::ApiMode::parse(&mode) else {
        eprintln!("[mxrs] error: --mode must be axum, actix-web, or rocket");
        return ExitCode::FAILURE;
    };
    if args.len() != 1 || output.is_none() {
        eprintln!(
            "[mxrs] error: usage: mxrs import <file.mpr> --output <directory> [--mode axum|actix-web|rocket] [--mxrs-workspace <path>]"
        );
        return ExitCode::FAILURE;
    }

    let output = output.expect("validated above");
    let imported = match mxrs_exporter::import_cargo_project_with_mode(
        &args[0],
        &output,
        workspace.as_deref(),
        mode,
    ) {
        Ok(imported) => imported,
        Err(error) => {
            eprintln!("[mxrs] error: {error}");
            return ExitCode::FAILURE;
        }
    };

    println!(
        "[mxrs] imported {} as Cargo package {} ({} model units, {} assets)",
        imported.project_name,
        imported.package_name,
        imported.imported_units,
        imported.imported_assets
    );
    if !imported.typed_round_trip_gaps.is_empty() {
        println!(
            "[mxrs] {} feature(s) remain losslessly backed by model/imported until typed support is added",
            imported.typed_round_trip_gaps.len()
        );
    }
    if imported.page_export.typed_candidates > 0 {
        println!(
            "[mxrs] {} page(s) detected as buildable from mxrs-dsl's native widget vocabulary; see src/ui/pages/mod.rs",
            imported.page_export.typed_candidates
        );
    }
    println!("[mxrs] next: cd {output} && cargo check");
    ExitCode::SUCCESS
}

fn run_javagen(mut args: Vec<String>) -> ExitCode {
    let project_root = take_value(&mut args, "--project-root");
    if args.len() != 1 {
        eprintln!("[mxrs] error: usage: mxrs javagen <file.mpr> [--project-root <directory>]");
        return ExitCode::FAILURE;
    }
    let mpr_path = std::path::Path::new(&args[0]);
    let root = project_root
        .map(std::path::PathBuf::from)
        .or_else(|| mpr_path.parent().map(std::path::Path::to_path_buf))
        .unwrap_or_else(|| std::path::PathBuf::from("."));
    let generator = match mxrs_javagen::JavaProxyGenerator::new(mpr_path, &root) {
        Ok(generator) => generator,
        Err(error) => {
            eprintln!("[mxrs] error: {error}");
            return ExitCode::FAILURE;
        }
    };
    match generator.generate() {
        Ok(count) => {
            println!("[mxrs] generated {count} Java file(s)");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("[mxrs] error: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run_presentation(mut args: Vec<String>) -> ExitCode {
    take_flag(&mut args, "--no-progress");
    reported(mxrs_cli::scaffold::generate(
        mxrs_scaffold::ArtifactKind::Presentation,
        args,
    ))
}
