//! Thin CLI dispatcher over `mxrs_cli`'s subcommand modules — mirrors
//! `bin/mxrb`'s `when "validate"`/`when "compare"`/`when "inspect"`/
//! `when "sql"`/... cases, narrowed the same way the library crate is (see
//! `lib.rs`'s doc comment for exactly what each command covers and what's
//! not ported yet — most of `bin/mxrb`'s ~45 subcommands depend on engines
//! mxrs hasn't built yet, e.g. `db`/`run` need runtime orchestration and
//! `rename`/`move` need semantic mutation planning).
//! `inspect` has no `bin/mxrb` equivalent under that name — it's a new
//! single-file front end onto `compare`'s existing snapshot machinery.

use std::process::ExitCode;

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        Some("validate") => run_validate(args.collect()),
        Some("compare") => run_compare(args.collect()),
        Some("inspect") => run_inspect(args.collect()),
        Some("units") => run_units(args.collect()),
        Some("dump-unit") => run_dump_unit(args.collect()),
        Some("sql") => run_sql(args.collect()),
        Some("modules") => run_modules(args.collect()),
        Some("import") => run_import(args.collect()),
        Some("export") => run_export(args.collect()),
        Some("javagen") => run_javagen(args.collect()),
        Some("package") => run_package(args.collect()),
        Some("verify-package") => run_verify_package(args.collect()),
        Some("oql") => run_oql(args.collect()),
        Some("translate-oql") => run_translate_oql(args.collect()),
        Some("refs") => run_refs(args.collect()),
        Some("impact") => run_impact(args.collect()),
        Some("search") => run_semantic_search(args.collect()),
        Some("new") => run_new(args.collect()),
        Some(other) => {
            eprintln!("[mxrs] error: unknown command {other:?}");
            usage();
            ExitCode::FAILURE
        }
        None => {
            usage();
            ExitCode::FAILURE
        }
    }
}

fn usage() {
    eprintln!("Usage: mxrs validate <file.mpr> [--json]");
    eprintln!("       mxrs compare <left.mpr> <right.mpr> [--json]");
    eprintln!("       mxrs inspect <file.mpr> [--json]");
    eprintln!("       mxrs units <file.mpr>");
    eprintln!("       mxrs dump-unit <file.mpr> <unit_id>");
    eprintln!("       mxrs sql <file.mpr> \"<query>\"");
    eprintln!("       mxrs modules <file.mpr>");
    eprintln!("       mxrs import <file.mpr> --output <directory> [--mxrs-workspace <path>]");
    eprintln!("       mxrs export <file.mpr> [-o <out.rs>] [--allow-lossy]");
    eprintln!("       mxrs javagen <file.mpr> [--project-root <directory>]");
    eprintln!("       mxrs package <file.mpr> --web <directory> --output <archive.tar>");
    eprintln!("       mxrs verify-package <archive.tar>");
    eprintln!("       mxrs oql <file.mpr> [--dialect postgresql|sql_server|ansi] [--json]");
    eprintln!("       mxrs translate-oql <query> [--dialect postgresql|sql_server|ansi]");
    eprintln!("       mxrs refs <file.mpr> <artifact> [--json]");
    eprintln!("       mxrs impact <file.mpr> <artifact> [--json]");
    eprintln!("       mxrs search <file.mpr> <query> [--limit N] [--json]");
    eprintln!(
        "       mxrs new <name> --output <directory> [--version 11.12.1] [--mxrs-workspace <path>]"
    );
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

fn semantic_index(path: &str) -> Result<mxrs_semantic::SemanticIndex, String> {
    let project = mxrs_model::Project::open(path, true).map_err(|error| error.to_string())?;
    mxrs_semantic::SemanticIndex::build(&project).map_err(|error| error.to_string())
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
    let limit = take_value(&mut args, "--limit")
        .as_deref()
        .unwrap_or("20")
        .parse::<usize>()
        .unwrap_or(20);
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
    let Some(path) = args.first() else {
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
    let Some(path) = args.first() else {
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
    let Some(path) = args.first() else {
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
    let Some(path) = args.first() else {
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
    let Some(path) = args.first() else {
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

fn take_value(args: &mut Vec<String>, flag: &str) -> Option<String> {
    let pos = args.iter().position(|a| a == flag)?;
    if pos + 1 >= args.len() {
        return None;
    }
    args.remove(pos);
    Some(args.remove(pos))
}

fn take_flag(args: &mut Vec<String>, flag: &str) -> bool {
    if let Some(pos) = args.iter().position(|a| a == flag) {
        args.remove(pos);
        true
    } else {
        false
    }
}
