//! Thin CLI dispatcher over `mxrs_cli`'s subcommand modules — mirrors
//! `bin/mxrb`'s `when "validate"`/`when "compare"`/`when "inspect"`/
//! `when "sql"`/... cases, narrowed the same way the library crate is (see
//! `lib.rs`'s doc comment for exactly what each command covers and what's
//! not ported yet — many of the 76 audited MXRB commands depend on engines
//! mxrs hasn't built yet, e.g. `db`/`run` need runtime orchestration and
//! `rename`/`move` need semantic mutation planning).
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
        "callees" | "callers" | "compare" | "describe" | "impact" | "inspect" | "lint" | "refs"
        | "report" | "tree" | "validate" => (&[], &["--json"], &[]),
        "export" => (&["-o"], &["--allow-lossy"], &[]),
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
        "constant" | "consumed-rest" | "entity" | "enumeration" | "java-action" | "module"
        | "nanoflow" | "published-rest" | "scheduled-event" | "security" | "use-case" => {
            (&["--target"], &["--dry-run", "--json"], &[])
        }
        "scaffold" => (&["--target"], &[], &[]),
        "project" => (&[], &["--json"], &[]),
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
    "callees", "<file.mpr> <artifact> [--json]", "List distinct directly called artifacts", run_callees;
    "callers", "<file.mpr> <artifact> [--json]", "List distinct direct callers", run_callers;
    "compare", "<left.mpr> <right.mpr> [--json]", "Compare structural model snapshots", run_compare;
    "constant", "new <Module.Constant> [--target DIR] [--dry-run] [--json]", "Scaffold a string constant declaration", run_constant;
    "consumed-rest", "new <Module.Client> [--target DIR] [--dry-run] [--json]", "Scaffold a consumed REST adapter microflow", run_consumed_rest;
    "describe", "<file.mpr> <artifact> [--json]", "Describe an artifact and its reference edges", run_describe;
    "dump-unit", "<file.mpr> <unit_id>", "Dump native unit bytes", run_dump_unit;
    "entity", "new <Module.Entity> [--target DIR] [--dry-run] [--json]", "Scaffold a domain entity declaration", run_entity;
    "enumeration", "new <Module.Enumeration> [--target DIR] [--dry-run] [--json]", "Scaffold an enumeration declaration", run_enumeration;
    "export", "<file.mpr> [-o <out.rs>] [--allow-lossy]", "Export editable Rust declarations", run_export;
    "help", "[command]", "Show command usage", run_help;
    "impact", "<file.mpr> <artifact> [--json]", "Find transitive incoming dependencies", run_impact;
    "import", "<file.mpr> --output <directory> [--mxrs-workspace <path>]", "Import into a Cargo-native project", run_import;
    "inspect", "<file.mpr> [--json]", "Show a structural model snapshot", run_inspect;
    "java-action", "new <Module.Adapter> [--target DIR] [--dry-run] [--json]", "Scaffold a Java Action adapter microflow", run_java_action;
    "javagen", "<file.mpr> [--project-root <directory>]", "Generate Java entity proxies", run_javagen;
    "lint", "<file.mpr> [--json]", "Check explicit references and recursive call components", run_lint;
    "module", "new <Module> [--target DIR] [--dry-run] [--json]", "Scaffold an editable module declaration layer", run_module;
    "modules", "<file.mpr>", "List module names", run_modules;
    "nanoflow", "new <Module.Flow> [--target DIR] [--dry-run] [--json]", "Scaffold a client nanoflow declaration", run_nanoflow;
    "new", "<name> --output <directory> [--version 11.12.1] [--mxrs-workspace <path>]", "Create a Cargo-native project", run_new;
    "oql", "<file.mpr> [--dialect postgresql|sql_server|ansi] [--json]", "Catalog OQL and logical query risks", run_oql;
    "package", "<file.mpr> --web <directory> --output <archive.tar>", "Create a deterministic MXRS archive", run_package;
    "page", "new <Module.Page> [--template NAME] [--chain CHAIN] [--role Module.Role] [--target DIR] [--dry-run] [--json] | templates [--json]", "Scaffold a page declaration, a page-led vertical slice, or list page templates", run_page;
    "project", "inspect [DIR] [--json]", "Inspect a Cargo-native project workspace", run_project;
    "published-rest", "new <Module.Handler> [--target DIR] [--dry-run] [--json]", "Scaffold a published REST handler microflow", run_published_rest;
    "refs", "<file.mpr> <artifact> [--json]", "Show incoming and outgoing references", run_refs;
    "report", "<file.mpr> [--json]", "Summarize explicit-reference lint and module dependencies", run_report;
    "scaffold", "<list|destroy> [<kind:name>] [--target DIR]", "List generators or remove a registered scaffold", run_scaffold;
    "scheduled-event", "new <Module.Event> [--target DIR] [--dry-run] [--json]", "Scaffold a scheduled event and its handler microflow", run_scheduled_event;
    "search", "<file.mpr> <query> [--limit N] [--json]", "Search artifact names and documentation", run_semantic_search;
    "security", "init <Module> [--target DIR] [--dry-run] [--json]", "Scaffold module roles and project security", run_security;
    "sql", "<file.mpr> <query>", "Run read-only model-store SQL", run_sql;
    "translate-oql", "<query> [--dialect postgresql|sql_server|ansi]", "Translate the supported safe OQL subset", run_translate_oql;
    "tree", "<file.mpr> [module] [--json]", "Group indexed artifacts by module and kind", run_tree;
    "units", "<file.mpr>", "List native units and storage metadata", run_units;
    "use-case", "new <Module.Flow> [--target DIR] [--dry-run] [--json]", "Scaffold an application use-case microflow", run_use_case;
    "validate", "<file.mpr> [--json]", "Check storage-format integrity", run_validate;
    "verify-package", "<archive.tar>", "Verify archive paths and content hashes", run_verify_package;
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

fn run_scaffold(args: Vec<String>) -> ExitCode {
    reported(mxrs_cli::scaffold::registry_command(args))
}

fn run_project(args: Vec<String>) -> ExitCode {
    reported(mxrs_cli::scaffold::project_command(args))
}

fn semantic_index(path: &str) -> Result<mxrs_semantic::SemanticIndex, String> {
    let project = mxrs_model::Project::open(path, true).map_err(|error| error.to_string())?;
    mxrs_semantic::SemanticIndex::build(&project).map_err(|error| error.to_string())
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

fn run_modules(args: Vec<String>) -> ExitCode {
    let [path] = args.as_slice() else {
        eprintln!("[mxrs] error: usage: mxrs modules <file.mpr>");
        return ExitCode::FAILURE;
    };
    match mxrs_cli::browse::list_modules(path) {
        Ok(names) => {
            for name in names {
                println!("{name}");
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
    let allow_lossy = take_flag(&mut args, "--allow-lossy");
    let [path] = args.as_slice() else {
        eprintln!("[mxrs] error: usage: mxrs export <file.mpr> [-o <out.rs>] [--allow-lossy]");
        return ExitCode::FAILURE;
    };

    let exported = if allow_lossy {
        mxrs_exporter::export_project_lossy(path).map_err(|error| error.to_string())
    } else {
        mxrs_exporter::export_project(path).map_err(|error| {
            let mut message = error.to_string();
            for gap in error.gaps() {
                message.push_str(&format!("\n  - {}: {}", gap.path, gap.reason));
            }
            message
        })
    };
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
            "[mxrs] {} page(s) detected as buildable from mxrs-dsl's native widget vocabulary; see src/domain/pages/mod.rs",
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
