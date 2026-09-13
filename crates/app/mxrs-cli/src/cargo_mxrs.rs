use std::process::ExitCode;

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1).collect::<Vec<_>>();
    if args.first().is_some_and(|argument| argument == "mxrs") {
        args.remove(0);
    }
    match args.first().map(String::as_str) {
        Some("build") => {
            args.remove(0);
            run_build(args)
        }
        Some("diff") => {
            args.remove(0);
            run_diff(args)
        }
        Some("frontend-dev") => {
            args.remove(0);
            run_frontend_dev(args)
        }
        Some("package") => {
            args.remove(0);
            run_package(args)
        }
        _ => {
            usage();
            ExitCode::FAILURE
        }
    }
}

fn usage() {
    eprintln!(
        "Usage: cargo mxrs build --output <file.mpr> [--web-output <directory>] [--manifest-path <Cargo.toml>] [--release] [--offline]"
    );
    eprintln!(
        "       cargo mxrs diff [--manifest-path <Cargo.toml>] [--snapshot <model/imported>] [--json] [--release] [--offline]"
    );
    eprintln!("       cargo mxrs frontend-dev [--output <directory>]");
    eprintln!(
        "       cargo mxrs package --mpr <file.mpr> --web <directory> --output <archive.tar>"
    );
}

fn run_package(mut args: Vec<String>) -> ExitCode {
    let mpr = take_value(&mut args, "--mpr");
    let web = take_value(&mut args, "--web");
    let output = take_value(&mut args, "--output").or_else(|| take_value(&mut args, "-o"));
    if !args.is_empty() || mpr.is_none() || web.is_none() || output.is_none() {
        eprintln!(
            "[mxrs] error: usage: cargo mxrs package --mpr <file.mpr> --web <directory> --output <archive.tar>"
        );
        return ExitCode::FAILURE;
    }
    let options = mxrs_packager::PackageOptions::new(
        mpr.expect("validated above"),
        web.expect("validated above"),
        output.expect("validated above"),
    );
    match mxrs_packager::package(&options) {
        Ok(report) => {
            println!(
                "[mxrs] packaged {} file(s) into {} (sha256 {})",
                report.payload_files,
                report.output.display(),
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

fn run_build(mut args: Vec<String>) -> ExitCode {
    let output = take_value(&mut args, "--output").or_else(|| take_value(&mut args, "-o"));
    let manifest = take_value(&mut args, "--manifest-path").unwrap_or_else(|| "Cargo.toml".into());
    let web_output = take_value(&mut args, "--web-output");
    let release = take_flag(&mut args, "--release");
    let offline = take_flag(&mut args, "--offline");
    let Some(output) = output else {
        eprintln!("[mxrs] error: --output <file.mpr> is required");
        return ExitCode::FAILURE;
    };
    if !args.is_empty() {
        eprintln!("[mxrs] error: unexpected arguments: {}", args.join(" "));
        return ExitCode::FAILURE;
    }
    let result = match web_output {
        Some(web) => {
            mxrs_cli::cargo_project::build_with_web_output(manifest, output, web, release, offline)
        }
        None => mxrs_cli::cargo_project::build(manifest, output, release, offline),
    };
    match result {
        Ok(path) => {
            println!("[mxrs] built and validated {}", path.display());
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("[mxrs] error: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run_frontend_dev(mut args: Vec<String>) -> ExitCode {
    let output = take_value(&mut args, "--output").unwrap_or_else(|| "frontend".into());
    if !args.is_empty() {
        eprintln!("[mxrs] error: unexpected arguments: {}", args.join(" "));
        return ExitCode::FAILURE;
    }
    match mxrs_cli::cargo_project::frontend_sources(output) {
        Ok(path) => {
            println!(
                "[mxrs] materialized pinned frontend sources at {}",
                path.display()
            );
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("[mxrs] error: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run_diff(mut args: Vec<String>) -> ExitCode {
    let manifest = take_value(&mut args, "--manifest-path").unwrap_or_else(|| "Cargo.toml".into());
    let snapshot = take_value(&mut args, "--snapshot").unwrap_or_else(|| {
        std::path::Path::new(&manifest)
            .parent()
            .unwrap_or_else(|| std::path::Path::new("."))
            .join("model/imported")
            .display()
            .to_string()
    });
    let release = take_flag(&mut args, "--release");
    let offline = take_flag(&mut args, "--offline");
    let json = take_flag(&mut args, "--json");
    if !args.is_empty() {
        eprintln!("[mxrs] error: unexpected arguments: {}", args.join(" "));
        return ExitCode::FAILURE;
    }
    match mxrs_cli::cargo_project::diff(manifest, snapshot, release, offline) {
        Ok(result) if json => {
            let changes = result
                .changes
                .iter()
                .map(|change| {
                    serde_json::json!({
                        "operation": format!("{:?}", change.operation),
                        "path": change.path,
                        "before": change.before,
                        "after": change.after,
                    })
                })
                .collect::<Vec<_>>();
            println!("{}", serde_json::json!({ "changes": changes }));
            ExitCode::SUCCESS
        }
        Ok(result) => {
            for change in &result.changes {
                println!("{}", change.format());
            }
            println!("[mxrs] {} change(s)", result.changes.len());
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("[mxrs] error: {error}");
            ExitCode::FAILURE
        }
    }
}

fn take_value(args: &mut Vec<String>, flag: &str) -> Option<String> {
    let position = args.iter().position(|argument| argument == flag)?;
    if position + 1 >= args.len() {
        return None;
    }
    args.remove(position);
    Some(args.remove(position))
}

fn take_flag(args: &mut Vec<String>, flag: &str) -> bool {
    if let Some(position) = args.iter().position(|argument| argument == flag) {
        args.remove(position);
        true
    } else {
        false
    }
}
