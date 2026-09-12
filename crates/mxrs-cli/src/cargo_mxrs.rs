use std::process::ExitCode;

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1).collect::<Vec<_>>();
    if args.first().is_some_and(|argument| argument == "mxrs") {
        args.remove(0);
    }
    if args.first().is_none_or(|argument| argument != "build") {
        eprintln!(
            "Usage: cargo mxrs build --output <file.mpr> [--manifest-path <Cargo.toml>] [--release] [--offline]"
        );
        return ExitCode::FAILURE;
    }
    args.remove(0);
    run_build(args)
}

fn run_build(mut args: Vec<String>) -> ExitCode {
    let output = take_value(&mut args, "--output").or_else(|| take_value(&mut args, "-o"));
    let manifest = take_value(&mut args, "--manifest-path").unwrap_or_else(|| "Cargo.toml".into());
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
    match mxrs_cli::cargo_project::build(manifest, output, release, offline) {
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
