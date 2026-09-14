//! Front end for `mxrs-marketplace` — the search/inspect/download half of
//! mxrb's `marketplace` command.
//!
//! Every subcommand needs a Mendix Personal Access Token. It is read from
//! `MXRS_MENDIX_PAT` or from the file named by `MXRS_MENDIX_PAT_FILE`, never
//! from a command-line argument: an argument lands in shell history and in
//! every process listing on the machine.
//!
//! Installing a downloaded package into an `.mpr` is not here — see
//! `mxrs-marketplace`'s crate doc for the boundary.

use std::path::PathBuf;
use std::process::ExitCode;

use mxrs_marketplace::{
    ContentApi, Credentials, MarketplaceError, Package, SearchQuery, ureq_transport::UreqTransport,
};

use crate::arguments::{take_flag, take_value};

pub const USAGE: &str = "usage: mxrs marketplace <search|show|versions|download> [arguments]";

pub fn run(mut args: Vec<String>) -> ExitCode {
    let json = take_flag(&mut args, "--json");
    let version = take_value(&mut args, "--version");
    let mendix_version = take_value(&mut args, "--mendix-version");
    let output = take_value(&mut args, "--output").or_else(|| take_value(&mut args, "-o"));
    let limit = take_value(&mut args, "--limit");

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
