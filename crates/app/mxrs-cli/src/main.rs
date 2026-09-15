//! Thin CLI dispatcher over `mxrs_cli`'s subcommand modules — mirrors
//! `bin/mxrb`'s `when "validate"`/`when "compare"`/`when "inspect"`/
//! `when "sql"`/... cases, narrowed the same way the library crate is (see
//! `lib.rs`'s doc comment for exactly what each command covers and what's
//! not ported yet — many of the 76 audited MXRB commands depend on engines
//! mxrs hasn't built yet, e.g. `db`/`run` need runtime orchestration).
//! `inspect` has no `bin/mxrb` equivalent under that name — it's a new
//! single-file front end onto `compare`'s existing snapshot machinery.

use mxrs_cli::arguments::{take_flag, take_value, validate_options};
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
        "callees" | "callers" | "changelog" | "compare" | "describe" | "evaluate" | "impact"
        | "inspect" | "lint" | "protocols" | "refs" | "preflight" | "report" | "tree"
        | "validate" => (&[], &["--json"], &[]),
        "portability" => (
            &[],
            &["--json", "--require-typed", "--verify-round-trip"],
            &[],
        ),
        "test" => (&[], &["--json", "--plan"], &[]),
        "functional-instrument" => (&[], &["--json"], &[]),
        "cache" => (&[], &["--json"], &[]),
        "db" => (&["--port"], &["--json"], &[]),
        "doctor" => (&[], &["--json"], &[]),
        "modules" => (&[], &["--json", "--names", "--no-progress"], &[]),
        "export" => (&["-o"], &[], &[]),
        "env" => (&["--environment"], &["--json"], &[]),
        "import" => (&["--output", "-o", "--mxrs-workspace"], &[], &[]),
        "javagen" => (&["--project-root"], &[], &[]),
        "new" => (
            &["--output", "-o", "--version", "--mxrs-workspace"],
            &[],
            &[],
        ),
        "oql" => (&["--dialect"], &["--json"], &[]),
        "package" => (&["--web", "--output", "-o"], &[], &[]),
        "search" => (&["--limit"], &["--json"], &[]),
        "translate-oql" => (&["--dialect"], &[], &[]),
        "page" => (
            &["--target", "--template", "--chain"],
            &["--dry-run", "--json"],
            &["--role"],
        ),
        "ci" | "constant" | "consumed-rest" | "entity" | "enumeration" | "evaluation"
        | "functional-test" | "integration" | "java-action" | "module" | "nanoflow"
        | "published-rest" | "repository" | "scheduled-event" | "security" | "use-case"
        | "validation" => (&["--target"], &["--dry-run", "--json"], &[]),
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
            ],
            &["--json", "--apply", "--allow-model-upgrade"],
            &[],
        ),
        "mda" => (&[], &["--json"], &[]),
        "migrate" => (&[], &["--json"], &[]),
        "project" => (&[], &["--json"], &[]),
        "team-server" => (&["--pat-file"], &["--json"], &[]),
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
    "cache", "<status|warm|clear> <file.mpr> [--json]", "Inspect or manage the semantic index cache", run_cache;
    "ci", "init github [--target DIR] [--dry-run] [--json]", "Create a GitHub Actions workflow", run_ci;
    "callees", "<file.mpr> <artifact> [--json]", "List distinct directly called artifacts", run_callees;
    "callers", "<file.mpr> <artifact> [--json]", "List distinct direct callers", run_callers;
    "changelog", "[VERSION] [--json]", "Show mxrs release notes from GitHub", run_changelog;
    "compare", "<left.mpr> <right.mpr> [--json]", "Compare structural model snapshots", run_compare;
    "constant", "new <Module.Constant> [--target DIR] [--dry-run] [--json]", "Scaffold a string constant declaration", run_constant;
    "consumed-rest", "new <Module.Client> [--target DIR] [--dry-run] [--json]", "Scaffold a consumed REST adapter microflow", run_consumed_rest;
    "describe", "<file.mpr> <artifact> [--json]", "Describe an artifact and its reference edges", run_describe;
    "db", "<status|up|down|destroy|credentials|url> <file.mpr> [--port PORT] [--json]", "Manage an isolated PostgreSQL workspace", run_db;
    "doctor", "[DIR] [--json]", "Check a Cargo-native project and local toolchain", run_doctor;
    "dump-unit", "<file.mpr> <unit_id>", "Dump native unit bytes", run_dump_unit;
    "entity", "new <Module.Entity> [--target DIR] [--dry-run] [--json]", "Scaffold a domain entity declaration", run_entity;
    "enumeration", "new <Module.Enumeration> [--target DIR] [--dry-run] [--json]", "Scaffold an enumeration declaration", run_enumeration;
    "env", "[DIR] [--environment NAME] [--json]", "Inspect an environment profile without values", run_env;
    "evaluate", "<file.mpr> <evaluation.json> [--json]", "Run declarative static model checks", run_evaluate;
    "evaluation", "new <Name> [--target DIR] [--dry-run] [--json]", "Create declarative static model checks", run_evaluation;
    "export", "<file.mpr> [-o <out.rs>]", "Export complete editable Rust declarations", run_export;
    "functional-test", "new <Module.Flow> [--target DIR] [--dry-run] [--json]", "Create a declarative runtime test suite", run_functional_test;
    "functional-instrument", "<writable.mpr> <suite.json> [--json]", "Instrument a disposable MPR with a functional test runner", run_functional_instrument;
    "help", "[command]", "Show command usage", run_help;
    "impact", "<file.mpr> <artifact> [--json]", "Find transitive incoming dependencies", run_impact;
    "import", "<file.mpr> --output <directory> [--mxrs-workspace <path>]", "Import into a Cargo-native project", run_import;
    "inspect", "<file.mpr> [--json]", "Show a structural model snapshot", run_inspect;
    "integration", "new <Module.Adapter> [--target DIR] [--dry-run] [--json]", "Create an integration adapter microflow", run_integration;
    "java-action", "new <Module.Adapter> [--target DIR] [--dry-run] [--json]", "Scaffold a Java Action adapter microflow", run_java_action;
    "javagen", "<file.mpr> [--project-root <directory>]", "Generate Java entity proxies", run_javagen;
    "lint", "<file.mpr> [--json]", "Check explicit references and recursive call components", run_lint;
    "module", "new <Module> [--target DIR] [--dry-run] [--json]", "Scaffold an editable module declaration layer", run_module;
    "marketplace", "<search|show|versions|download> <name-or-id> [--version V] [--mendix-version V] [-o FILE] [--limit N] [--json] | install <package.mpk> <file.mpr> [--target-root DIR] [--allow-model-upgrade] [--apply] [--json]", "Search, download, or install official Marketplace content", run_marketplace;
    "mda", "<inspect|compare> ...", "Inspect or compare Mendix deployment archives", run_mda;
    "migrate", "<check|plan> [DIR] [--json]", "Compare a Cargo-native build with its imported MPR snapshot", run_migrate;
    "modules", "<file.mpr> [--json | --names] [--no-progress]", "List modules with entity, page and microflow counts", run_modules;
    "move", "<file.mpr> <name> <container> [--apply] [--json]", "Preview or apply a same-module unit move", run_move;
    "nanoflow", "new <Module.Flow> [--target DIR] [--dry-run] [--json]", "Scaffold a client nanoflow declaration", run_nanoflow;
    "new", "<name> --output <directory> [--version 11.12.1] [--mxrs-workspace <path>]", "Create a Cargo-native project", run_new;
    "oql", "<file.mpr> [--dialect postgresql|sql_server|ansi] [--json]", "Catalog OQL and logical query risks", run_oql;
    "package", "<file.mpr> --web <directory> --output <archive.tar>", "Create a deterministic MXRS archive", run_package;
    "portability", "<file.mpr> [--json] [--verify-round-trip] [--require-typed]", "Audit typed authoring versus lossless model preservation", run_portability;
    "page", "new <Module.Page> [--template NAME] [--chain CHAIN] [--role Module.Role] [--target DIR] [--dry-run] [--json] | templates [--json]", "Scaffold a page declaration, a page-led vertical slice, or list page templates", run_page;
    "preflight", "<file.mpr> [--json]", "Audit native compiler and runtime compatibility", run_preflight;
    "protocols", "<file.mpr> [--json]", "Audit imported Marketplace protocol connectors", run_protocols;
    "project", "inspect [DIR] [--json]", "Inspect a Cargo-native project workspace", run_project;
    "published-rest", "new <Module.Handler> [--target DIR] [--dry-run] [--json]", "Scaffold a published REST handler microflow", run_published_rest;
    "refs", "<file.mpr> <artifact> [--json]", "Show incoming and outgoing references", run_refs;
    "remove", "<file.mpr> <qualified-name> [--apply] [--json]", "Preview or apply a reference-safe removal", run_remove;
    "rename", "<file.mpr> <old-name> <new-name> [--apply] [--json]", "Preview or apply a model-wide rename", run_rename;
    "repository", "new <Module.Name> [--target DIR] [--dry-run] [--json]", "Scaffold a repository port and infrastructure adapter", run_repository;
    "report", "<file.mpr> [--json]", "Summarize explicit-reference lint and module dependencies", run_report;
    "scaffold", "<list|destroy> [<kind:name>] [--target DIR]", "List generators or remove a registered scaffold", run_scaffold;
    "scheduled-event", "new <Module.Event> [--target DIR] [--dry-run] [--json]", "Scaffold a scheduled event and its handler microflow", run_scheduled_event;
    "search", "<file.mpr> <query> [--limit N] [--json]", "Search artifact names and documentation", run_semantic_search;
    "security", "init <Module> [--target DIR] [--dry-run] [--json]", "Scaffold module roles and project security", run_security;
    "sql", "<file.mpr> <query>", "Run read-only model-store SQL", run_sql;
    "translate-oql", "<query> [--dialect postgresql|sql_server|ansi]", "Translate the supported safe OQL subset", run_translate_oql;
    "team-server", "login --pat-file FILE [--json] | status DIR [--json]", "Configure a PAT pointer or inspect a local Team Server repository", run_team_server;
    "test", "<file.mpr> <suite.json> --plan [--json]", "Validate a functional runtime test plan", run_test;
    "tree", "<file.mpr> [module] [--json]", "Group indexed artifacts by module and kind", run_tree;
    "units", "<file.mpr>", "List native units and storage metadata", run_units;
    "upgrade", "[--mendix VERSION] [--target DIR] [--apply] [--json]", "Preview or apply a generated layout and optional version upgrade", run_upgrade;
    "use-case", "new <Module.Flow> [--target DIR] [--dry-run] [--json]", "Scaffold an application use-case microflow", run_use_case;
    "validate", "<file.mpr> [--json]", "Check storage-format integrity", run_validate;
    "validation", "new <Module.Flow> [--target DIR] [--dry-run] [--json]", "Create an application validation microflow", run_validation;
    "verify-package", "<archive.tar>", "Verify archive paths and content hashes", run_verify_package;
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
    if args.len() != 2
        || !matches!(
            args[0].as_str(),
            "status" | "up" | "down" | "destroy" | "credentials" | "url"
        )
    {
        eprintln!(
            "Usage: mxrs db <status|up|down|destroy|credentials|url> <file.mpr> [--port PORT] [--json]"
        );
        return ExitCode::FAILURE;
    }
    let workspace = match mxrs_cli::database::DatabaseWorkspace::open(&args[1], port) {
        Ok(workspace) => workspace,
        Err(error) => {
            eprintln!("[mxrs] error: {error}");
            return ExitCode::FAILURE;
        }
    };
    let action = args[0].as_str();
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
    if args.len() != 1 || output.is_none() {
        eprintln!(
            "[mxrs] error: usage: mxrs new <name> --output <directory> [--version 11.12.1] [--mxrs-workspace <path>]"
        );
        return ExitCode::FAILURE;
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
                        .unwrap_or("unknown")
                );
                println!(
                    "Project       : {}",
                    report.metadata["ProjectName"].as_str().unwrap_or("unknown")
                );
                println!("Files         : {}", report.files().count());
                println!("Roots         : {}", report.roots().join(", "));
                println!("SHA-256       : {}", report.sha256);
            }
        }),
        [action, left, right] if action == "compare" && !json => {
            mxrs_packager::compare_mda(left, right).map(|differences| {
                for difference in &differences {
                    println!("{:?}\t{}", difference.status, difference.path);
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

fn run_module(args: Vec<String>) -> ExitCode {
    reported(mxrs_cli::scaffold::generate(
        mxrs_scaffold::ArtifactKind::Module,
        args,
    ))
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
                }
                if !audit.unknown_marketplace_modules.is_empty() {
                    println!(
                        "Unrecognized marketplace modules ({}): {}",
                        audit.unknown_marketplace_modules.len(),
                        audit.unknown_marketplace_modules.join(", ")
                    );
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
        eprintln!("Usage: mxrs test <file.mpr> <suite.json> --plan [--json]");
        return ExitCode::FAILURE;
    };
    if !plan_only {
        eprintln!(
            "[mxrs] error: functional flow execution is not implemented; use --plan to validate the suite"
        );
        return ExitCode::FAILURE;
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

fn run_callers(args: Vec<String>) -> ExitCode {
    run_call_graph(args, true)
}
fn run_callees(args: Vec<String>) -> ExitCode {
    run_call_graph(args, false)
}

fn graph_command(
    mut args: Vec<String>,
    command: &str,
    query: impl FnOnce(&mxrs_semantic::SemanticIndex, &str, bool) -> Result<(), String>,
) -> ExitCode {
    let json = take_flag(&mut args, "--json");
    if args.len() != 2 {
        eprintln!("Usage: mxrs {command} <file.mpr> <artifact> [--json]");
        return ExitCode::FAILURE;
    }
    match semantic_index(&args[0]).and_then(|index| query(&index, &args[1], json)) {
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
                index.callers(name)
            } else {
                index.callees(name)
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
                    index
                        .require(&reference.from)
                        .map_err(|error| error.to_string())?
                        .qualified_name,
                    reference.relation
                );
            }
            println!("Outgoing:");
            for reference in outgoing {
                println!(
                    "  {}\t{}",
                    index
                        .require(&reference.to)
                        .map_err(|error| error.to_string())?
                        .qualified_name,
                    reference.relation
                );
            }
        }
        Ok(())
    })
}

fn run_tree(mut args: Vec<String>) -> ExitCode {
    let json = take_flag(&mut args, "--json");
    if !(1..=2).contains(&args.len()) || args.iter().any(|arg| arg.starts_with('-')) {
        eprintln!("Usage: mxrs tree <file.mpr> [module] [--json]");
        return ExitCode::FAILURE;
    }
    let index = match semantic_index(&args[0]) {
        Ok(index) => index,
        Err(error) => {
            eprintln!("[mxrs] error: {error}");
            return ExitCode::FAILURE;
        }
    };
    let selected = args.get(1).map(String::as_str);
    if let Some(module) = selected
        && !index.artifacts().any(|artifact| {
            artifact.kind == mxrs_semantic::ArtifactKind::Module && artifact.name == module
        })
    {
        eprintln!("[mxrs] error: unknown module {module:?}");
        return ExitCode::FAILURE;
    }
    let mut tree: std::collections::BTreeMap<&str, std::collections::BTreeMap<&str, Vec<&str>>> =
        std::collections::BTreeMap::new();
    for artifact in index.artifacts() {
        if artifact.kind == mxrs_semantic::ArtifactKind::Module {
            if selected.is_none_or(|module| module == artifact.name) {
                tree.entry(&artifact.name).or_default();
            }
            continue;
        }
        if selected.is_some_and(|module| artifact.module.as_deref() != Some(module)) {
            continue;
        }
        tree.entry(artifact.module.as_deref().unwrap_or("(project)"))
            .or_default()
            .entry(artifact.kind.as_str())
            .or_default()
            .push(&artifact.qualified_name);
    }
    for kinds in tree.values_mut() {
        for names in kinds.values_mut() {
            names.sort();
        }
    }
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&tree).expect("artifact tree is serializable")
        );
    } else {
        for (module, kinds) in tree {
            println!("{module}");
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

fn run_refs(mut args: Vec<String>) -> ExitCode {
    let json = take_flag(&mut args, "--json");
    if args.len() != 2 {
        eprintln!("[mxrs] error: usage: mxrs refs <file.mpr> <artifact> [--json]");
        return ExitCode::FAILURE;
    }
    let index = match semantic_index(&args[0]) {
        Ok(index) => index,
        Err(error) => {
            eprintln!("[mxrs] error: {error}");
            return ExitCode::FAILURE;
        }
    };
    let incoming = match index.incoming(&args[1]) {
        Ok(references) => references,
        Err(error) => {
            eprintln!("[mxrs] error: {error}");
            return ExitCode::FAILURE;
        }
    };
    let outgoing = index
        .outgoing(&args[1])
        .expect("resolution already validated");
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "artifact": args[1], "incoming": incoming, "outgoing": outgoing,
                "fingerprint": index.fingerprint(),
            }))
            .expect("serializable reference report")
        );
    } else {
        for reference in incoming {
            println!("<- {} ({})", reference.from, reference.relation);
        }
        for reference in outgoing {
            println!("-> {} ({})", reference.to, reference.relation);
        }
    }
    ExitCode::SUCCESS
}

fn run_impact(mut args: Vec<String>) -> ExitCode {
    let json = take_flag(&mut args, "--json");
    if args.len() != 2 {
        eprintln!("[mxrs] error: usage: mxrs impact <file.mpr> <artifact> [--json]");
        return ExitCode::FAILURE;
    }
    let index = match semantic_index(&args[0]) {
        Ok(index) => index,
        Err(error) => {
            eprintln!("[mxrs] error: {error}");
            return ExitCode::FAILURE;
        }
    };
    let impact = match index.impact(&args[1]) {
        Ok(impact) => impact,
        Err(error) => {
            eprintln!("[mxrs] error: {error}");
            return ExitCode::FAILURE;
        }
    };
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&impact).expect("serializable impact report")
        );
    } else {
        for artifact in impact {
            println!("{:?} {}", artifact.kind, artifact.qualified_name);
        }
    }
    ExitCode::SUCCESS
}

fn run_semantic_search(mut args: Vec<String>) -> ExitCode {
    let json = take_flag(&mut args, "--json");
    let limit = match take_value(&mut args, "--limit")
        .as_deref()
        .unwrap_or("20")
        .parse::<usize>()
    {
        Ok(limit) if limit > 0 => limit,
        _ => {
            eprintln!("[mxrs] error: --limit requires a positive integer");
            return ExitCode::FAILURE;
        }
    };
    if args.len() != 2 {
        eprintln!("[mxrs] error: usage: mxrs search <file.mpr> <query> [--limit N] [--json]");
        return ExitCode::FAILURE;
    }
    let index = match semantic_index(&args[0]) {
        Ok(index) => index,
        Err(error) => {
            eprintln!("[mxrs] error: {error}");
            return ExitCode::FAILURE;
        }
    };
    let hits = index.search(&args[1], limit);
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&hits).expect("serializable search report")
        );
    } else {
        for hit in hits {
            println!(
                "{}\t{:?}\t{}",
                hit.score, hit.artifact.kind, hit.artifact.qualified_name
            );
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
    let rows = queries
        .iter()
        .map(|query| (query, mxrs_oql::translate(&query.oql, dialect)))
        .collect::<Vec<_>>();
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

fn run_compare(mut args: Vec<String>) -> ExitCode {
    let json = take_flag(&mut args, "--json");
    if args.len() != 2 {
        eprintln!("[mxrs] error: usage: mxrs compare <left.mpr> <right.mpr> [--json]");
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
                    "path": c.path,
                    "before": c.before,
                    "after": c.after,
                })
            })
            .collect();
        println!(
            "{}",
            serde_json::json!({ "identical": result.is_identical(), "changes": changes })
        );
    } else if result.is_identical() {
        println!("[mxrs] OK");
    } else {
        for change in &result.changes {
            println!("{}", change.format());
        }
        println!("[mxrs] {} difference(s)", result.changes.len());
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

fn run_units(args: Vec<String>) -> ExitCode {
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

fn run_dump_unit(args: Vec<String>) -> ExitCode {
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
        Some(bytes) if !bytes.is_empty() => print!("{}", mxrs_cli::browse::format_hex_dump(bytes)),
        _ => println!("  (empty)"),
    }
    ExitCode::SUCCESS
}

fn run_sql(args: Vec<String>) -> ExitCode {
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
        let cells: Vec<String> = row.iter().map(|c| c.to_string()).collect();
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

fn run_import(mut args: Vec<String>) -> ExitCode {
    let output = take_value(&mut args, "--output").or_else(|| take_value(&mut args, "-o"));
    let workspace = take_value(&mut args, "--mxrs-workspace").map(std::path::PathBuf::from);
    if args.len() != 1 || output.is_none() {
        eprintln!(
            "[mxrs] error: usage: mxrs import <file.mpr> --output <directory> [--mxrs-workspace <path>]"
        );
        return ExitCode::FAILURE;
    }

    let output = output.expect("validated above");
    let imported =
        match mxrs_exporter::import_cargo_project(&args[0], &output, workspace.as_deref()) {
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
            "[mxrs] {} page(s) detected as buildable from mxrs-dsl's native widget vocabulary; see src/presentation/pages/mod.rs",
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
