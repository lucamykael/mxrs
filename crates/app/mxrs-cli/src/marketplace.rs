//! Front end for `mxrs-marketplace` — the search/inspect/download half of
//! mxrb's `marketplace` command.
//!
//! Every subcommand needs a Mendix Personal Access Token. It is read from
//! `MXRS_MENDIX_PAT` or from the file named by `MXRS_MENDIX_PAT_FILE`, never
//! from a command-line argument: an argument lands in shell history and in
//! every process listing on the machine.
//!
//! `install` needs no credential: it works on a `.mpk` already on disk, so a
//! package obtained any way at all can be installed. Like the refactoring
//! commands it previews by default and writes only under `--apply`.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use mxrs_marketplace::{
    ContentApi, Credentials, MarketplaceError, Package, SearchQuery, lifecycle, plan_install,
    ureq_transport::UreqTransport,
};

use crate::arguments::{take_flag, take_value};

pub const USAGE: &str = "usage: mxrs marketplace <search|show|versions|download|install|list|remove|dependencies|update> [arguments]";

pub fn run(mut args: Vec<String>) -> ExitCode {
    let json = take_flag(&mut args, "--json");
    let version = take_value(&mut args, "--version");
    let mendix_version = take_value(&mut args, "--mendix-version");
    let output = take_value(&mut args, "--output").or_else(|| take_value(&mut args, "-o"));
    let limit = take_value(&mut args, "--limit");
    let apply = take_flag(&mut args, "--apply");
    let allow_model_upgrade = take_flag(&mut args, "--allow-model-upgrade");
    let target_root = take_value(&mut args, "--target-root");
    let mpr_option = take_value(&mut args, "--mpr");

    let Some((action, rest)) = args.split_first() else {
        eprintln!("[mxrs] error: {USAGE}");
        return ExitCode::FAILURE;
    };

    let outcome = match (action.as_str(), rest) {
        ("search", [query]) => search(query, limit.as_deref(), json),
        ("show", [identifier]) => show(identifier, json),
        ("versions", [identifier]) => versions(identifier, mendix_version.as_deref(), json),
        ("download", [identifier]) => download(
            identifier,
            version.as_deref(),
            mendix_version.as_deref(),
            output.as_deref(),
            json,
        ),
        ("install", [package, mpr]) => install(
            Path::new(package),
            Path::new(mpr),
            target_root.as_deref().map(Path::new),
            allow_model_upgrade,
            apply,
            json,
        ),
        ("list", []) => list(target_root.as_deref().map(Path::new), json),
        ("dependencies", [identifier]) => dependencies(
            identifier,
            target_root.as_deref().map(Path::new),
            mendix_version.as_deref(),
            apply,
            json,
        ),
        ("remove", [identifier]) => remove(
            identifier,
            target_root.as_deref().map(Path::new),
            mpr_option.as_deref().map(Path::new),
            apply,
            json,
        ),
        ("update", [identifier]) => update(
            identifier,
            target_root.as_deref().map(Path::new),
            mendix_version.as_deref(),
            apply,
            json,
        ),
        _ => {
            eprintln!("[mxrs] error: {USAGE}");
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

/// Builds a client, resolving the credential first so a missing token fails
/// with an actionable message rather than an HTTP 401.
fn client() -> Result<ContentApi<UreqTransport>, MarketplaceError> {
    let pat = Credentials::default().resolve()?;
    Ok(ContentApi::new(UreqTransport::new(), pat))
}

fn search(query: &str, limit: Option<&str>, json: bool) -> Result<(), MarketplaceError> {
    let limit = limit.and_then(|value| value.parse::<usize>().ok());
    let results = client()?.search(&SearchQuery {
        name: Some(query.to_string()),
        limit,
        ..SearchQuery::default()
    })?;
    if json {
        print_json(&serde_json::Value::Array(
            results.iter().map(content_json).collect(),
        ));
    } else if results.is_empty() {
        println!("[mxrs] no Marketplace content matched {query:?}");
    } else {
        for content in &results {
            println!(
                "{}\t{}\t{}\t{}",
                content.content_id,
                content.name().unwrap_or("(unnamed)"),
                content.content_type,
                content
                    .latest_version
                    .as_ref()
                    .map_or("-", |version| version.version_number.as_str())
            );
        }
        println!("[mxrs] {} result(s)", results.len());
    }
    Ok(())
}

fn show(identifier: &str, json: bool) -> Result<(), MarketplaceError> {
    let content = client()?.find(identifier)?;
    if json {
        print_json(&content_json(&content));
    } else {
        println!("Content ID   : {}", content.content_id);
        println!("Name         : {}", content.name().unwrap_or("(unnamed)"));
        println!("Publisher    : {}", content.publisher);
        println!("Type         : {}", content.content_type);
        println!("Private      : {}", content.is_private);
        println!("Approved     : {}", content.is_company_approved);
        if let Some(version) = &content.latest_version {
            println!("Latest       : {}", version.version_number);
            println!(
                "Min Mendix   : {}",
                version
                    .min_supported_mendix_version
                    .as_deref()
                    .unwrap_or("-")
            );
        }
    }
    Ok(())
}

fn versions(
    identifier: &str,
    mendix_version: Option<&str>,
    json: bool,
) -> Result<(), MarketplaceError> {
    let client = client()?;
    let content = client.find(identifier)?;
    let versions = client.versions(&content.content_id.to_string(), mendix_version)?;
    if json {
        print_json(&serde_json::Value::Array(
            versions
                .iter()
                .map(|version| {
                    serde_json::json!({
                        "versionId": version.version_id,
                        "versionNumber": version.version_number,
                        "minSupportedMendixVersion": version.min_supported_mendix_version,
                        "publicationDate": version.publication_date,
                        "versionType": version.version_type,
                    })
                })
                .collect(),
        ));
    } else {
        for version in &versions {
            println!(
                "{}\t{}\t{}\t{}",
                version.version_number,
                version
                    .min_supported_mendix_version
                    .as_deref()
                    .unwrap_or("-"),
                version.version_type.as_deref().unwrap_or("-"),
                version.publication_date.as_deref().unwrap_or("-")
            );
        }
        println!("[mxrs] {} version(s)", versions.len());
    }
    Ok(())
}

fn download(
    identifier: &str,
    version: Option<&str>,
    mendix_version: Option<&str>,
    output: Option<&str>,
    json: bool,
) -> Result<(), MarketplaceError> {
    let client = client()?;
    let package = client.resolve(identifier, version, mendix_version)?;
    let destination = destination_for(&package, output);
    if let Some(parent) = destination
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        std::fs::create_dir_all(parent).map_err(|source| MarketplaceError::Download {
            path: parent.display().to_string(),
            source,
        })?;
    }
    let bytes = client.download(&package, &destination)?;
    if json {
        print_json(&serde_json::json!({
            "contentId": package.content.content_id,
            "name": package.name(),
            "versionNumber": package.version.version_number,
            "versionId": package.version.version_id,
            "path": destination.display().to_string(),
            "bytes": bytes,
        }));
    } else {
        println!(
            "[mxrs] downloaded {} {} to {} ({bytes} bytes)",
            package.name(),
            package.version.version_number,
            destination.display()
        );
    }
    Ok(())
}

fn install(
    package: &Path,
    mpr: &Path,
    target_root: Option<&Path>,
    allow_model_upgrade: bool,
    apply: bool,
    json: bool,
) -> Result<(), MarketplaceError> {
    let plan = plan_install(package, mpr, target_root, allow_model_upgrade)?;
    let module_name = plan.module_name.clone();
    let units = plan.units.len();
    let files = plan.files.clone();
    let overwrites = plan.overwrites();
    let (source_version, target_version) =
        (plan.source_version.clone(), plan.target_version.clone());
    // Applying goes through the lifecycle wrapper so the install is
    // recorded in the marketplace lockfile (cached archive, owned files,
    // asset originals) and `remove` can later undo it.
    let report = if apply {
        Some(lifecycle::install_module(
            package,
            mpr,
            target_root,
            allow_model_upgrade,
            &lifecycle::OfficialProvenance::default(),
        )?)
    } else {
        None
    };

    if json {
        print_json(&serde_json::json!({
            "module": module_name,
            "applied": report.is_some(),
            "units": units,
            "sourceVersion": source_version,
            "targetVersion": target_version,
            "overwrites": overwrites,
            "files": files
                .iter()
                .map(|(path, exists)| serde_json::json!({ "path": path, "overwrites": exists }))
                .collect::<Vec<_>>(),
        }));
    } else {
        println!("Module        : {module_name}");
        println!(
            "Model version : {} -> {}",
            source_version.as_deref().unwrap_or("-"),
            target_version.as_deref().unwrap_or("-")
        );
        println!("Model units   : {units}");
        println!(
            "Files         : {} ({overwrites} would be replaced)",
            files.len()
        );
        for (path, exists) in &files {
            println!("  {}  {path}", if *exists { "replace" } else { "create " });
        }
        println!(
            "[mxrs] {}",
            if report.is_some() {
                "Installed"
            } else {
                "Install preview"
            }
        );
    }
    Ok(())
}

fn list(target_root: Option<&Path>, json: bool) -> Result<(), MarketplaceError> {
    let target = target_root.unwrap_or_else(|| Path::new("."));
    let packages = lifecycle::list(target)?;
    if json {
        print_json(&serde_json::Value::Array(
            packages
                .iter()
                .map(|(name, entry)| {
                    serde_json::json!({
                        "name": name,
                        "version": entry.version,
                        "source": entry.source,
                        "kind": entry.kind,
                    })
                })
                .collect(),
        ));
    } else {
        for (name, entry) in &packages {
            println!(
                "{name}\t{}\t{}",
                entry.version.as_deref().unwrap_or("-"),
                entry.source.as_deref().unwrap_or("-")
            );
        }
        println!("[mxrs] {} package(s)", packages.len());
    }
    Ok(())
}

fn dependencies(
    identifier: &str,
    target_root: Option<&Path>,
    mendix_version: Option<&str>,
    apply: bool,
    json: bool,
) -> Result<(), MarketplaceError> {
    let target = target_root.unwrap_or_else(|| Path::new("."));
    // Resolution needs the Content API only when something is actually
    // missing; without a credential the plan still runs and reports the
    // unresolvable names as blockers.
    let api = client().ok();
    let plan = mxrs_marketplace::resolver::plan_dependencies(
        target,
        identifier,
        api.as_ref(),
        mendix_version,
    )?;
    let safe = plan.safe();
    let state = if !safe {
        "blocked"
    } else if apply {
        "applied"
    } else {
        "preview"
    };
    if json {
        print_json(&serde_json::json!({
            "root": plan.root,
            "state": state,
            "changes": plan.changes(),
            "blockers": plan.blockers,
        }));
    } else {
        println!("[mxrs] dependencies {}: {state}", plan.root);
        for change in plan.changes() {
            println!("  change: {change}");
        }
        for blocker in &plan.blockers {
            println!("  blocker: {blocker}");
        }
    }
    if apply && safe {
        plan.apply()?;
    }
    if safe {
        Ok(())
    } else {
        Err(MarketplaceError::PlanBlocked(
            "rerun after resolving the blockers above".into(),
        ))
    }
}

/// `name[@version]` — mxrb's `identifier.split('@', 2)`.
fn split_versioned(identifier: &str) -> (&str, Option<&str>) {
    match identifier.split_once('@') {
        Some((name, version)) => (name, Some(version)),
        None => (identifier, None),
    }
}

fn update(
    identifier: &str,
    target_root: Option<&Path>,
    mendix_version: Option<&str>,
    apply: bool,
    json: bool,
) -> Result<(), MarketplaceError> {
    let (name, version) = split_versioned(identifier);
    let target = target_root.unwrap_or_else(|| Path::new("."));
    let api = client()?;
    let plan = mxrs_marketplace::resolver::plan_update_official(
        target,
        name,
        version,
        mendix_version,
        &api,
    )?;
    let safe = plan.safe();
    let plan_name = plan.name.clone();
    let installed_version = plan.installed_version.clone();
    let target_version = plan.target_version.clone();
    let changes = plan.changes.clone();
    let blockers = plan.blockers.clone();
    // Apply happens BEFORE any "applied" is printed: a failed apply must
    // surface as the error it is, never after a success line.
    if apply && safe {
        plan.apply()?;
    }
    let state = if !safe {
        "blocked"
    } else if apply {
        "applied"
    } else {
        "preview"
    };
    if json {
        print_json(&serde_json::json!({
            "action": "update",
            "name": plan_name,
            "installedVersion": installed_version,
            "targetVersion": target_version,
            "state": state,
            "changes": changes,
            "blockers": blockers,
        }));
    } else {
        println!(
            "[mxrs] update {plan_name} {} -> {target_version}: {state}",
            installed_version.as_deref().unwrap_or("-")
        );
        for change in &changes {
            println!("  change: {change}");
        }
        for blocker in &blockers {
            println!("  blocker: {blocker}");
        }
    }
    if safe {
        Ok(())
    } else {
        Err(MarketplaceError::PlanBlocked(
            "rerun after resolving the blockers above".into(),
        ))
    }
}

fn remove(
    identifier: &str,
    target_root: Option<&Path>,
    mpr: Option<&Path>,
    apply: bool,
    json: bool,
) -> Result<(), MarketplaceError> {
    let target = target_root.unwrap_or_else(|| Path::new("."));
    let plan = lifecycle::plan_remove(target, identifier, mpr)?;
    let safe = plan.safe();
    let action = plan.action;
    let name = plan.name.clone();
    let installed_version = plan.installed_version.clone();
    let changes = plan.changes.clone();
    let blockers = plan.blockers.clone();
    // Apply happens BEFORE any "applied" is printed: a failed apply must
    // surface as the error it is, never after a success line.
    if apply && safe {
        plan.apply()?;
    }
    let state = if !safe {
        "blocked"
    } else if apply {
        "applied"
    } else {
        "preview"
    };
    if json {
        print_json(&serde_json::json!({
            "action": action,
            "name": name,
            "installedVersion": installed_version,
            "state": state,
            "changes": changes,
            "blockers": blockers,
        }));
    } else {
        println!("[mxrs] {action} {name}: {state}");
        for change in &changes {
            println!("  change: {change}");
        }
        for blocker in &blockers {
            println!("  blocker: {blocker}");
        }
    }
    if safe {
        Ok(())
    } else {
        Err(MarketplaceError::PlanBlocked(
            "rerun after resolving the blockers above".into(),
        ))
    }
}

/// Names the file after the package when no `--output` was given, so a bare
/// download does not produce something called `download`.
fn destination_for(package: &Package, output: Option<&str>) -> PathBuf {
    if let Some(output) = output {
        return PathBuf::from(output);
    }
    let name: String = package
        .name()
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.') {
                character
            } else {
                '_'
            }
        })
        .collect();
    let version = package.version.version_number.replace(['/', '\\'], "_");
    PathBuf::from(format!("{name}-{version}.mpk"))
}

fn content_json(content: &mxrs_marketplace::Content) -> serde_json::Value {
    serde_json::json!({
        "contentId": content.content_id,
        "name": content.name(),
        "publisher": content.publisher,
        "type": content.content_type,
        "isPrivate": content.is_private,
        "isCompanyApproved": content.is_company_approved,
        "latestVersion": content
            .latest_version
            .as_ref()
            .map(|version| version.version_number.clone()),
    })
}

fn print_json(value: &serde_json::Value) {
    println!(
        "{}",
        serde_json::to_string_pretty(value).expect("marketplace output is serializable")
    );
}
