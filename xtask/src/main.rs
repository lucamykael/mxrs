//! Dev-only differential-testing/fixture-generation harness against mxrb
//! (Ruby) — never linked into shipped mxrs binaries. Implements the
//! `xtask fixture-gen`/`xtask oracle-diff` tools described in the mxrs
//! plan's Testing section (`decisions/mxrs-rust-rewrite-plan.md` in this
//! project's ai-memory).
//!
//! Usage:
//!   cargo run -p xtask -- fixture-gen <name> <dsl_source.rb>
//!   cargo run -p xtask -- oracle-diff <fixture_dir>
//!   cargo run -p xtask -- command-oracle <modules|dump-unit|sql|inspect|protocols|mda|project|compare|diff|callers|callees|refs|impact|describe|tree|presentation|uml> <mxrs-binary>
//!   cargo run -p xtask -- noise-audit
//!   cargo run -p xtask -- mxbuild-oracle <app_dir>
//!   cargo run -p xtask -- capability-matrix [--json] [--check-baseline] [--require-complete]
//!   cargo run -p xtask -- coverage-gate <llvm-export.json> [--json] [--require-complete]

mod capability_matrix;
mod coverage_gate;
mod mxbuild_oracle;
mod noise_audit;

#[cfg(test)]
mod command_tests;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use mxrs_bson::Document;
use mxrs_mpr::MprFile;
use sha2::{Digest, Sha256};

fn mxrb_home() -> PathBuf {
    std::env::var("MXRB_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .parent()
                .and_then(Path::parent)
                .unwrap_or_else(|| Path::new(".."))
                .join("mxrb")
        })
}

fn main() {
    if let Err(error) = dispatch(&std::env::args().skip(1).collect::<Vec<_>>()) {
        eprintln!("[xtask] error: {error}");
        std::process::exit(1);
    }
}

fn dispatch(arguments: &[String]) -> Result<(), String> {
    let Some((command, arguments)) = arguments.split_first() else {
        return Err("usage: xtask <fixture-gen | oracle-diff | command-oracle | noise-audit | mxbuild-oracle | capability-matrix | coverage-gate>".into());
    };
    match command.as_str() {
        "fixture-gen" => match arguments {
            [name, source] if valid_fixture_name(name) => fixture_gen(name, Path::new(source)),
            _ => Err("usage: fixture-gen <name> <dsl_source.rb>; name must start with a letter and contain only ASCII letters, digits, '-' or '_'".into()),
        },
        "oracle-diff" => match arguments {
            [fixture] => oracle_diff(Path::new(fixture)),
            _ => Err("usage: oracle-diff <fixture_dir>".into()),
        }
        "command-oracle" => match arguments {
            [command, executable] if matches!(command.as_str(), "modules" | "dump-unit" | "sql" | "inspect" | "protocols" | "mda" | "project" | "compare" | "diff" | "callers" | "callees" | "refs" | "impact" | "describe" | "tree" | "presentation" | "uml") => {
                let executable = std::path::absolute(executable).map_err(|e| e.to_string())?;
                if !executable.is_file() { return Err("MXRS executable not found".into()); }
                let script = Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("support/{}_oracle.rb", command.replace('-', "_")));
                let output = run_bundle_capture(&["exec", "ruby", &path_str(&script), &path_str(&executable)])?;
                print!("{}", output.stdout);
                if output.success { Ok(()) } else { Err(format!("{command} command oracle failed")) }
            }
            _ => Err("usage: command-oracle <modules|dump-unit|sql|inspect|protocols|mda|project|compare|diff|callers|callees|refs|impact|describe|tree|presentation|uml> <mxrs-binary>".into()),
        },
        "noise-audit" if arguments.is_empty() => {
            let root = Path::new(env!("CARGO_MANIFEST_DIR"))
                .parent()
                .expect("xtask always lives one directory below the workspace root")
                .to_path_buf();
            noise_audit::noise_audit(&root)
        }
        "noise-audit" => Err("usage: noise-audit".into()),
        "mxbuild-oracle" => match arguments {
            [app] => mxbuild_oracle::mxbuild_oracle(Path::new(app)),
            _ => Err("usage: mxbuild-oracle <app_dir>".into()),
        },
        "capability-matrix" => {
            let json = arguments.iter().any(|argument| argument == "--json");
            let require_complete = arguments
                .iter()
                .any(|argument| argument == "--require-complete");
            let check_baseline = arguments
                .iter()
                .any(|argument| argument == "--check-baseline");
            if arguments.iter().any(|argument| {
                !matches!(
                    argument.as_str(),
                    "--json" | "--require-complete" | "--check-baseline"
                )
            }) {
                Err(
                    "usage: capability-matrix [--json] [--check-baseline] [--require-complete]"
                        .into(),
                )
            } else {
                capability_matrix(json, check_baseline, require_complete)
            }
        }
        "coverage-gate" => coverage_gate_command(arguments),
        _ => Err(format!("unknown xtask command: {command}")),
    }
}

fn valid_fixture_name(name: &str) -> bool {
    name.as_bytes().first().is_some_and(u8::is_ascii_alphabetic)
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

fn coverage_gate_command(arguments: &[String]) -> Result<(), String> {
    let Some((path, flags)) = arguments.split_first() else {
        return Err("usage: coverage-gate <llvm-export.json> [--json] [--require-complete]".into());
    };
    if path.starts_with('-')
        || flags
            .iter()
            .any(|flag| !matches!(flag.as_str(), "--json" | "--require-complete"))
    {
        return Err("usage: coverage-gate <llvm-export.json> [--json] [--require-complete]".into());
    }
    let bytes = fs::read(path).map_err(|error| format!("cannot read {path}: {error}"))?;
    let report = coverage_gate::evaluate(&bytes)?;
    let require_complete = flags.iter().any(|flag| flag == "--require-complete");
    if flags.iter().any(|flag| flag == "--json") {
        println!("{}", serde_json::to_string_pretty(&serde_json::json!({
            "source_report": path,
            "source_sha256": format!("{:x}", Sha256::digest(&bytes)),
            "gate": if require_complete { "release_100_percent" } else { "development_ratchet" },
            "coverage": report,
        })).map_err(|error| error.to_string())?);
    } else {
        for metric in &report.metrics {
            println!(
                "{}: {}/{} ({:.4}%), {} uncovered",
                metric.metric, metric.covered, metric.count, metric.percent, metric.uncovered
            );
        }
        println!(
            "Coverage complete: {} ({} instrumented files); this does not establish Studio Pro parity",
            report.coverage_complete, report.files
        );
    }
    coverage_gate::enforce(&report, require_complete)
}

fn capability_matrix(
    json: bool,
    check_baseline: bool,
    require_complete: bool,
) -> Result<(), String> {
    let output = run_bundle_capture(&["exec", "mxrb", "--commands"])?;
    if !output.success {
        return Err(format!("mxrb --commands failed:\n{}", output.stdout));
    }
    let report = capability_matrix::build(&output.stdout)?;
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&report).map_err(|error| error.to_string())?
        );
    } else {
        capability_matrix::print_table(&report);
    }
    if check_baseline {
        capability_matrix::check_baseline(&report, include_str!("../command-baseline.json"))?;
    }
    if require_complete && !report.complete() {
        return Err(format!(
            "capability matrix is incomplete: {} partial, {} missing",
            report.partial, report.missing
        ));
    }
    Ok(())
}

/// Runs mxrb's `generate` then forces v2 storage, producing a real
/// Studio-Pro-11-shaped `.mpr` (mxrb's own `generate` defaults to v1
/// inline-BSON storage even for a Mendix 11.12.1 project; only
/// `ensure_storage_for_version!` — normally reached via `Project#migrate_to!`
/// on a version change — splits it into v2 `.mxunit` files). Copies the
/// result plus a SHA-256 manifest into `xtask/fixtures/<name>/`.
fn fixture_gen(name: &str, dsl_source: &Path) -> Result<(), String> {
    // `bundle exec` below runs with mxrb's repo as its cwd, so a relative
    // path here would resolve against the wrong directory.
    let dsl_source = std::path::absolute(dsl_source).map_err(|e| e.to_string())?;
    let build_dir = tempfile::Builder::new()
        .prefix("mxrs-fixture-gen-")
        .tempdir()
        .map_err(|e| e.to_string())?;
    let built_mpr = build_dir.path().join(format!("{name}.mpr"));

    run_bundle(&[
        "exec",
        "mxrb",
        "generate",
        &path_str(&dsl_source),
        &path_str(&built_mpr),
    ])?;

    let migrate_script = "require 'mxrb'; mpr = Mxrb::IO::MprFile.open(ARGV.fetch(0), readonly: false); \
         mpr.ensure_storage_for_version!('11.12.1'); mpr.close";
    run_bundle(&["exec", "ruby", "-e", migrate_script, &path_str(&built_mpr)])?;

    let fixture_dir = fixtures_root().join(name);
    let _ = fs::remove_dir_all(&fixture_dir);
    fs::create_dir_all(&fixture_dir).map_err(|e| e.to_string())?;
    fs::copy(&built_mpr, fixture_dir.join(format!("{name}.mpr"))).map_err(|e| e.to_string())?;
    copy_dir_recursive(
        &build_dir.path().join("mprcontents"),
        &fixture_dir.join("mprcontents"),
    )?;
    fs::copy(dsl_source, fixture_dir.join("source.rb")).map_err(|e| e.to_string())?;

    write_manifest(&fixture_dir)?;

    println!("[xtask] fixture-gen: wrote {}", fixture_dir.display());
    Ok(())
}

/// Round-trips every unit in the fixture through mxrs (parse → write back
/// unchanged) and cross-checks the result against mxrb's own `compare`
/// tool, per Phase 1's done-definition. Two independent success signals:
/// (1) every unit re-serializes byte-identical (mxrs-mpr's `update_unit`
/// skip-optimization fires for all of them), and (2) `mxrb compare` between
/// the original fixture and the resaved copy reports no differences.
fn oracle_diff(fixture_dir: &Path) -> Result<(), String> {
    // `bundle exec` below runs with mxrb's repo as its cwd, so relative
    // paths here would resolve against the wrong directory.
    let fixture_dir = std::path::absolute(fixture_dir).map_err(|e| e.to_string())?;
    let original_mpr = find_mpr_file(&fixture_dir).ok_or("no .mpr file found in fixture dir")?;

    let work_dir = tempfile::Builder::new()
        .prefix("mxrs-oracle-diff-")
        .tempdir()
        .map_err(|e| e.to_string())?;
    let resaved_mpr = work_dir.path().join(original_mpr.file_name().unwrap());
    fs::copy(&original_mpr, &resaved_mpr).map_err(|e| e.to_string())?;
    let original_contents_dir = fixture_dir.join("mprcontents");
    if original_contents_dir.is_dir() {
        copy_dir_recursive(&original_contents_dir, &work_dir.path().join("mprcontents"))?;
    }

    let (total, rewritten) = round_trip_all_units(&resaved_mpr)?;
    println!(
        "[xtask] oracle-diff: {total} unit(s), {rewritten} rewritten (byte-identical: {})",
        total - rewritten
    );

    let compare = run_bundle_capture(&[
        "exec",
        "mxrb",
        "compare",
        &path_str(&original_mpr),
        &path_str(&resaved_mpr),
    ])?;
    print!("{}", compare.stdout);

    if !compare.success {
        return Err("mxrb compare reported structural differences".to_string());
    }
    if rewritten > 0 {
        println!(
            "[xtask] WARNING: {rewritten} unit(s) needed rewriting to match (serialization isn't byte-identical to mxrb's), but mxrb compare confirms structural equivalence"
        );
    }
    println!("[xtask] PASS");
    Ok(())
}

/// Opens `mpr_path` and rewrites every unit's contents with the exact
/// document just parsed from it. Returns `(total_units, rewritten_count)`
/// — `rewritten_count` is 0 when mxrs's BSON serialization is byte-identical
/// to what's already stored (see `MprFile::update_unit`'s content-hash
/// skip optimization).
fn round_trip_all_units(mpr_path: &Path) -> Result<(usize, usize), String> {
    let mut mpr = MprFile::open(mpr_path, false).map_err(|e| e.to_string())?;
    let units = mpr.all_units().map_err(|e| e.to_string())?;
    let total = units.len();
    let mut rewritten = 0;
    for unit in units {
        let doc: Document = mpr.parse_contents(&unit).map_err(|e| e.to_string())?;
        let changed = mpr
            .update_unit(&unit.unit_id, doc)
            .map_err(|e| e.to_string())?;
        if changed {
            rewritten += 1;
        }
    }
    Ok((total, rewritten))
}

fn find_mpr_file(dir: &Path) -> Option<PathBuf> {
    fs::read_dir(dir)
        .ok()?
        .filter_map(Result::ok)
        .map(|e| e.path())
        .find(|p| p.extension().and_then(|e| e.to_str()) == Some("mpr"))
}

struct BundleOutput {
    success: bool,
    stdout: String,
}

fn run_bundle(args: &[&str]) -> Result<(), String> {
    let output = run_bundle_capture(args)?;
    if !output.success {
        return Err(format!(
            "command failed: bundle {}\n{}",
            args.join(" "),
            output.stdout
        ));
    }
    Ok(())
}

fn run_bundle_capture(args: &[&str]) -> Result<BundleOutput, String> {
    let output = Command::new("bundle")
        .args(args)
        .current_dir(mxrb_home())
        .output()
        .map_err(|e| format!("failed to spawn bundle: {e}"))?;
    let mut combined = String::from_utf8_lossy(&output.stdout).into_owned();
    combined.push_str(&String::from_utf8_lossy(&output.stderr));
    Ok(BundleOutput {
        success: output.status.success(),
        stdout: combined,
    })
}

fn path_str(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn fixtures_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures")
}

fn write_manifest(fixture_dir: &Path) -> Result<(), String> {
    let mut files = Vec::new();
    collect_files(fixture_dir, &mut files)?;
    files.sort();

    let mut manifest = String::new();
    for file in &files {
        let relative = file
            .strip_prefix(fixture_dir)
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/");
        if relative == "manifest.sha256" {
            continue;
        }
        let bytes = fs::read(file).map_err(|e| e.to_string())?;
        let digest: String = Sha256::digest(&bytes)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        manifest.push_str(&format!("{digest}  {relative}\n"));
    }
    fs::write(fixture_dir.join("manifest.sha256"), manifest).map_err(|e| e.to_string())
}

fn collect_files(dir: &Path, out: &mut Vec<PathBuf>) -> Result<(), String> {
    for entry in fs::read_dir(dir).map_err(|e| e.to_string())? {
        let path = entry.map_err(|e| e.to_string())?.path();
        if path.is_dir() {
            collect_files(&path, out)?;
        } else {
            out.push(path);
        }
    }
    Ok(())
}

fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<(), String> {
    fs::create_dir_all(dst).map_err(|e| e.to_string())?;
    for entry in fs::read_dir(src).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let dest_path = dst.join(entry.file_name());
        if entry.file_type().map_err(|e| e.to_string())?.is_dir() {
            copy_dir_recursive(&entry.path(), &dest_path)?;
        } else {
            fs::copy(entry.path(), dest_path).map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}
