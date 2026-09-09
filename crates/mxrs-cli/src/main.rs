//! Thin CLI dispatcher over `mxrs_cli`'s `validate`/`compare` — mirrors
//! `bin/mxrb`'s `when "validate"`/`when "compare"` cases (`mxrb validate`,
//! `mxrb compare`), narrowed the same way the library crate is (see
//! `lib.rs`'s doc comment for what's not ported yet: `export`, `--json`
//! output is supported but no `preflight`/`pack`/`portable`/`mda`/
//! `diagram-er` — those depend on Phase 5 packaging, which doesn't exist).

use std::process::ExitCode;

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        Some("validate") => run_validate(args.collect()),
        Some("compare") => run_compare(args.collect()),
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

fn take_flag(args: &mut Vec<String>, flag: &str) -> bool {
    if let Some(pos) = args.iter().position(|a| a == flag) {
        args.remove(pos);
        true
    } else {
        false
    }
}
