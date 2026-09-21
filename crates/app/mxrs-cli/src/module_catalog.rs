//! Front end for `mxrs-module-catalog` — mxrb's `module search|add`, the
//! private (pre-official) module distribution mechanism. Distinct from
//! `marketplace`, which fronts the official Mendix Content API.
//!
//! mxrb bundles a default catalog inside its own gem
//! (`marketplace/catalog.json`); mxrs ships no such tree, so `--registry`
//! is required here rather than optional. `add` installs immediately, with
//! no preview/`--apply` step — mxrb's own `module add` has none either.

use std::path::Path;
use std::process::ExitCode;

use mxrs_module_catalog::{Catalog, ModuleCatalogError};

use crate::arguments::{take_flag, take_value};

pub const USAGE: &str = "usage: mxrs module <new|search|add> [arguments]";
const SEARCH_ADD_USAGE: &str = "usage: mxrs module search [query] --registry SOURCE [--json] | \
     mxrs module add <name|directory> --registry SOURCE [--target DIR] [--json]";

pub fn run(mut args: Vec<String>) -> ExitCode {
    let json = take_flag(&mut args, "--json");
    let registry = take_value(&mut args, "--registry");
    let target = take_value(&mut args, "--target");

    let Some((action, rest)) = args.split_first() else {
        eprintln!("[mxrs] error: {SEARCH_ADD_USAGE}");
        return ExitCode::FAILURE;
    };

    let outcome = match (action.as_str(), rest) {
        ("search", query) if query.len() <= 1 => {
            search(query.first().map(String::as_str), registry.as_deref(), json)
        }
        ("add", [identifier]) => add(
            identifier,
            registry.as_deref(),
            target.as_deref().map(Path::new),
            json,
        ),
        _ => {
            eprintln!("[mxrs] error: {SEARCH_ADD_USAGE}");
            return ExitCode::FAILURE;
        }
    };

    match outcome {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("[mxrs] error: {error}");
            ExitCode::FAILURE
        }
    }
}

fn catalog(registry: Option<&str>) -> Result<Catalog, ModuleCatalogError> {
    let Some(source) = registry else {
        // mxrb falls back to a catalog bundled inside its own gem; mxrs
        // ships no equivalent tree, so a source is mandatory here.
        return Err(ModuleCatalogError::NotFound(
            "--registry is required: mxrs bundles no default module catalog".to_string(),
        ));
    };
    Catalog::read(source, &mxrs_module_catalog::catalog::UreqCatalogTransport)
}

fn search(
    query: Option<&str>,
    registry: Option<&str>,
    json: bool,
) -> Result<(), ModuleCatalogError> {
    let catalog = catalog(registry)?;
    let entries = catalog.search(query);
    if json {
        print_json(&serde_json::Value::Array(
            entries.iter().map(|entry| entry_json(entry)).collect(),
        ));
    } else {
        for entry in &entries {
            println!("{}\t{}\t{}", entry.name, entry.version, entry.description);
        }
        println!("[mxrs] {} module(s)", entries.len());
    }
    Ok(())
}

fn add(
    identifier: &str,
    registry: Option<&str>,
    target: Option<&Path>,
    json: bool,
) -> Result<(), ModuleCatalogError> {
    let target = target.map(Path::to_path_buf).unwrap_or_else(|| {
        std::env::current_dir().unwrap_or_else(|_| Path::new(".").to_path_buf())
    });
    let mut installer = mxrs_module_catalog::Installer::new(target);
    // A local directory needs no catalog at all; only resolve one (and
    // require `--registry`) when the identifier isn't a path on disk.
    if !Path::new(identifier).is_dir() {
        installer = installer.with_catalog(catalog(registry)?);
    }
    let installation = installer.install(identifier, None)?;
    if json {
        print_json(&serde_json::json!({
            "name": installation.entry.name,
            "version": installation.entry.version,
            "moduleName": installation.module_name,
            "destination": installation.destination.display().to_string(),
            "sha256": installation.digest,
        }));
    } else {
        println!(
            "[mxrs] Installed {} {} as {}",
            installation.entry.name, installation.entry.version, installation.module_name
        );
        println!("[mxrs] {}", installation.destination.display());
        println!("[mxrs] SHA-256 {}", installation.digest);
    }
    Ok(())
}

fn entry_json(entry: &mxrs_module_catalog::Entry) -> serde_json::Value {
    serde_json::json!({
        "name": entry.name,
        "version": entry.version,
        "description": entry.description,
        "source": entry.source,
        "ref": entry.git_ref,
    })
}

fn print_json(value: &serde_json::Value) {
    println!(
        "{}",
        serde_json::to_string_pretty(value).expect("module catalog output is serializable")
    );
}
