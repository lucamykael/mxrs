//! Self-contained portable Runtime bundles.
//!
//! Ports MXRB's `compiler/portable_packager.rb`: a validated deployment plus
//! an installed Mendix Runtime tree become one `runtime.zip` that runs with
//! nothing but a JVM — `bin/start` launches `runtimelauncher.jar` against the
//! bundled `app/` and the configuration rendered from the deployment's
//! metadata. Like `pack`, this never invokes `mx`/`mxbuild` and never
//! compiles anything; it packages what a materialized deployment already is.
//!
//! The archive layout is MXRB's, byte-relevant parts pinned against its
//! output: the Runtime under `lib/runtime/`, the deployment roots under
//! `app/`, start scripts rendered from the Runtime's own `pad/bin/*.hbs`
//! templates (or a fallback `bin/start` for distributions without PAD), and
//! HOCON configuration under `etc/` derived from `model/metadata.json`.
//!
//! One deliberate improvement carries over from `pack`: MXRB stamps its
//! `FIXED_TIME` only where rubyzip does not overwrite it with the source
//! mtime, so touching one unchanged file changes its archive checksum. Here
//! every entry really is stamped with the fixed time, and packing the same
//! inputs twice produces the same bytes.

use std::io::Write as _;
use std::path::{Path, PathBuf};

use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::{
    MDA_FIXED_TIME, PackageError, Result, absolute, audit_mendix_version, io_error,
    validate_deployment, validate_deployment_freshness,
};

/// What `portable` produced; MXRB's `PortableResult`.
#[derive(Debug, Clone)]
pub struct PortableReport {
    pub path: PathBuf,
    pub mendix_version: String,
    /// Non-directory entries in the archive.
    pub files: usize,
    pub sha256: String,
    pub metadata: Value,
}

/// The deployment roots that become `app/` — MXRB's `Adapter::ROOTS + run`.
const APP_ROOTS: [&str; 6] = ["model", "web", "native", "sass", "tmp", "run"];

/// Directories the Runtime writes into at first boot, shipped empty.
const APP_STATE_DIRECTORIES: [&str; 5] = ["data", "data/database", "data/files", "data/tmp", "log"];

/// The one file that makes a Runtime tree a launchable Runtime.
const REQUIRED_RUNTIME_FILES: [&str; 1] = ["launcher/runtimelauncher.jar"];

/// Builds a self-contained portable Runtime ZIP.
///
/// Ports `PortablePackager#pack`, validations in MXRB's order: the output
/// must not exist unless `force`, the deployment directory must exist, the
/// deployment must be materialized, fresh, and agree with the model's Mendix
/// version, and the Runtime tree must at least carry the launcher.
///
/// # Errors
///
/// Every refusal is a [`PackageError`] naming what to fix; nothing is
/// written unless the whole archive lands.
pub fn pack_portable(
    mpr: impl AsRef<Path>,
    deployment: Option<&Path>,
    mendix_home: Option<&Path>,
    output: impl AsRef<Path>,
    force: bool,
) -> Result<PortableReport> {
    let mpr = absolute(mpr.as_ref())?;
    let project_root = mpr.parent().unwrap_or(Path::new("."));
    let deployment = match deployment {
        Some(path) => absolute(path)?,
        None => absolute(&project_root.join("deployment"))?,
    };
    let output = absolute(output.as_ref())?;

    if output.exists() && !force {
        return Err(PackageError::OutputExists(output.display().to_string()));
    }
    if !deployment.is_dir() {
        return Err(PackageError::DeploymentNotFound(
            deployment.display().to_string(),
        ));
    }

    let project = mxrs_model::Project::open(&mpr, true)?;
    let version = project.mendix_version()?.unwrap_or_default();
    audit_mendix_version(&version)?;
    let metadata = validate_deployment(&deployment, &version)?;
    validate_deployment_freshness(&mpr, &deployment)?;
    let runtime = runtime_root(mendix_home, &version);
    validate_runtime(&runtime)?;

    let entries = plan_entries(&deployment, &runtime, &metadata)?;
    let files = entries
        .iter()
        .filter(|entry| !matches!(entry, PlannedEntry::Directory { .. }))
        .count();
    write_portable_atomically(&output, &entries)?;

    Ok(PortableReport {
        sha256: file_sha256(&output)?,
        path: output,
        mendix_version: version,
        files,
        metadata,
    })
}

/// Ports `PortablePackager#runtime_root`: `--mendix-home` or the local
/// install for the model's version, with `runtime` appended unless the
/// caller already pointed at a `runtime` directory.
fn runtime_root(mendix_home: Option<&Path>, version: &str) -> PathBuf {
    let root = match mendix_home {
        Some(home) => home.to_path_buf(),
        None => std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_default()
            .join(".local/share/mendix")
            .join(version),
    };
    if root.file_name().is_some_and(|name| name == "runtime") {
        root
    } else {
        root.join("runtime")
    }
}

fn validate_runtime(runtime: &Path) -> Result<()> {
    let missing: Vec<&str> = REQUIRED_RUNTIME_FILES
        .iter()
        .copied()
        .filter(|relative| !runtime.join(relative).is_file())
        .collect();
    if missing.is_empty() {
        return Ok(());
    }
    Err(PackageError::RuntimeIncomplete {
        runtime: runtime.display().to_string(),
        missing: missing.join(", "),
    })
}

/// One archive entry, planned before anything is written so the whole bundle
/// is validated (and counted) first.
enum PlannedEntry {
    Directory {
        destination: String,
        mode: u32,
    },
    File {
        destination: String,
        source: PathBuf,
        mode: u32,
    },
    Rendered {
        destination: String,
        content: String,
        mode: u32,
    },
}

/// MXRB's `PortableArchiveWriter` entry order: the Runtime tree, the
/// application, the generated files.
fn plan_entries(deployment: &Path, runtime: &Path, metadata: &Value) -> Result<Vec<PlannedEntry>> {
    let mut entries = Vec::new();
    plan_tree(&mut entries, runtime, "lib/runtime")?;
    for root in APP_ROOTS {
        let source = deployment.join(root);
        if source.exists() {
            plan_tree(&mut entries, &source, &format!("app/{root}"))?;
        }
    }
    for path in APP_STATE_DIRECTORIES {
        entries.push(PlannedEntry::Directory {
            destination: format!("app/{path}/"),
            mode: 0o755,
        });
    }
    plan_start_scripts(&mut entries, runtime)?;
    for name in ["example.conf", "variables.conf"] {
        plan_runtime_configuration(&mut entries, runtime, name)?;
    }
    for (destination, content) in configuration_files(metadata) {
        entries.push(PlannedEntry::Rendered {
            destination,
            content,
            mode: 0o644,
        });
    }
    Ok(entries)
}

/// Ports `add_tree`: the root directory keeps its own mode, inner
/// directories are normalized to `0755`, files keep theirs, and everything —
/// hidden entries included — is ordered by path string exactly like MXRB's
/// sorted glob. A symlink anywhere is refused rather than followed or
/// stored, for the same reason `pack` refuses them.
fn plan_tree(entries: &mut Vec<PlannedEntry>, source_root: &Path, destination: &str) -> Result<()> {
    entries.push(PlannedEntry::Directory {
        destination: format!("{destination}/"),
        mode: source_mode(source_root)?,
    });
    let mut paths = Vec::new();
    collect_tree(source_root, source_root, &mut paths)?;
    paths.sort_by(|left, right| left.0.cmp(&right.0));
    for (relative, source, directory) in paths {
        if directory {
            entries.push(PlannedEntry::Directory {
                destination: format!("{destination}/{relative}/"),
                mode: 0o755,
            });
        } else {
            let mode = source_mode(&source)?;
            entries.push(PlannedEntry::File {
                destination: format!("{destination}/{relative}"),
                source,
                mode,
            });
        }
    }
    Ok(())
}

fn collect_tree(root: &Path, path: &Path, paths: &mut Vec<(String, PathBuf, bool)>) -> Result<()> {
    let mut children: Vec<PathBuf> = std::fs::read_dir(path)
        .map_err(|source| io_error(path, source))?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<std::result::Result<_, _>>()
        .map_err(|source| io_error(path, source))?;
    children.sort();
    for child in children {
        let metadata =
            std::fs::symlink_metadata(&child).map_err(|source| io_error(&child, source))?;
        if metadata.file_type().is_symlink() {
            return Err(PackageError::PortableSymlink(child.display().to_string()));
        }
        let relative = child
            .strip_prefix(root)
            .unwrap_or(&child)
            .to_string_lossy()
            .replace('\\', "/");
        if metadata.is_dir() {
            paths.push((relative, child.clone(), true));
            collect_tree(root, &child, paths)?;
        } else {
            paths.push((relative, child, false));
        }
    }
    Ok(())
}

#[cfg(unix)]
fn source_mode(source: &Path) -> Result<u32> {
    use std::os::unix::fs::PermissionsExt as _;
    let metadata = std::fs::metadata(source).map_err(|error| io_error(source, error))?;
    Ok(metadata.permissions().mode() & 0o777)
}

#[cfg(not(unix))]
fn source_mode(_source: &Path) -> Result<u32> {
    Ok(0o644)
}

/// Ports `add_start_scripts`: PAD distributions ship `pad/bin/*.hbs`
/// templates which are rendered into `bin/`; a distribution without them
/// gets the fallback POSIX `bin/start`. Only `start` is executable.
fn plan_start_scripts(entries: &mut Vec<PlannedEntry>, runtime: &Path) -> Result<()> {
    let directory = runtime.join("pad/bin");
    let mut templates: Vec<PathBuf> = match std::fs::read_dir(&directory) {
        Ok(children) => children
            .map(|entry| entry.map(|entry| entry.path()))
            .collect::<std::result::Result<_, _>>()
            .map_err(|source| io_error(&directory, source))?,
        Err(_) => Vec::new(),
    };
    templates.retain(|path| {
        path.extension().is_some_and(|extension| extension == "hbs") && path.is_file()
    });
    templates.sort();
    if templates.is_empty() {
        entries.push(PlannedEntry::Rendered {
            destination: "bin/start".to_string(),
            content: FALLBACK_START.to_string(),
            mode: 0o755,
        });
        return Ok(());
    }
    for source in templates {
        let name = source
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        let bytes = std::fs::read(&source).map_err(|error| io_error(&source, error))?;
        entries.push(PlannedEntry::Rendered {
            destination: format!("bin/{name}"),
            content: render_template(&bytes),
            mode: if name == "start" { 0o755 } else { 0o644 },
        });
    }
    Ok(())
}

/// Ports `add_runtime_configuration`: the Runtime's own `pad/etc` file when
/// it ships one, the fallback otherwise.
fn plan_runtime_configuration(
    entries: &mut Vec<PlannedEntry>,
    runtime: &Path,
    name: &str,
) -> Result<()> {
    let source = runtime.join("pad/etc").join(name);
    if source.is_file() {
        let mode = source_mode(&source)?;
        entries.push(PlannedEntry::File {
            destination: format!("etc/{name}"),
            source,
            mode,
        });
        return Ok(());
    }
    let fallback = if name == "variables.conf" {
        FALLBACK_VARIABLES
    } else {
        FALLBACK_EXAMPLE
    };
    entries.push(PlannedEntry::Rendered {
        destination: format!("etc/{name}"),
        content: fallback.to_string(),
        mode: 0o644,
    });
    Ok(())
}

/// Ports `render_template`: strip a UTF-8 BOM, drop `{{!-- ... --}}`
/// comments with the whitespace that follows them, and resolve the one
/// placeholder the templates use.
fn render_template(bytes: &[u8]) -> String {
    let bytes = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(bytes);
    let mut text = String::from_utf8_lossy(bytes).into_owned();
    while let Some(start) = text.find("{{!--") {
        let Some(end) = text[start..].find("--}}") else {
            break;
        };
        let mut stop = start + end + "--}}".len();
        let raw = text.as_bytes();
        // Ruby's `\s`: space, tab, newline, carriage return, vertical tab,
        // form feed.
        while stop < raw.len() && matches!(raw[stop], b' ' | b'\t' | b'\n' | b'\r' | 0x0B | 0x0C) {
            stop += 1;
        }
        text.replace_range(start..stop, "");
    }
    text.replace("{{DefaultConfig}}", "Default")
}

fn write_portable_atomically(output: &Path, entries: &[PlannedEntry]) -> Result<()> {
    if let Some(parent) = output.parent() {
        std::fs::create_dir_all(parent).map_err(|source| io_error(parent, source))?;
    }
    let temporary = output.with_extension(format!("portable-partial-{}", std::process::id()));
    let result = write_portable(&temporary, entries);
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
        return result;
    }
    std::fs::rename(&temporary, output).map_err(|source| {
        let _ = std::fs::remove_file(&temporary);
        io_error(output, source)
    })
}

fn write_portable(temporary: &Path, entries: &[PlannedEntry]) -> Result<()> {
    let (year, month, day, hour, minute, second) = MDA_FIXED_TIME;
    let fixed = zip::DateTime::from_date_and_time(year, month, day, hour, minute, second)
        .map_err(|error| PackageError::InvalidArchive(error.to_string()))?;
    let handle = std::fs::File::create(temporary).map_err(|source| io_error(temporary, source))?;
    let mut archive = zip::ZipWriter::new(std::io::BufWriter::new(handle));
    let options = |mode: u32| {
        zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated)
            .last_modified_time(fixed)
            .unix_permissions(mode)
    };
    for entry in entries {
        match entry {
            PlannedEntry::Directory { destination, mode } => archive
                .add_directory(destination, options(*mode))
                .map_err(|error| PackageError::InvalidArchive(error.to_string()))?,
            PlannedEntry::File {
                destination,
                source,
                mode,
            } => {
                archive
                    .start_file(destination, options(*mode))
                    .map_err(|error| PackageError::InvalidArchive(error.to_string()))?;
                let mut reader =
                    std::fs::File::open(source).map_err(|error| io_error(source, error))?;
                std::io::copy(&mut reader, &mut archive)
                    .map_err(|error| io_error(source, error))?;
            }
            PlannedEntry::Rendered {
                destination,
                content,
                mode,
            } => {
                archive
                    .start_file(destination, options(*mode))
                    .map_err(|error| PackageError::InvalidArchive(error.to_string()))?;
                archive
                    .write_all(content.as_bytes())
                    .map_err(|error| io_error(temporary, error))?;
            }
        }
    }
    archive
        .finish()
        .map_err(|error| PackageError::InvalidArchive(error.to_string()))?;
    Ok(())
}

fn file_sha256(path: &Path) -> Result<String> {
    let mut reader = std::fs::File::open(path).map_err(|error| io_error(path, error))?;
    let mut hasher = Sha256::new();
    std::io::copy(&mut reader, &mut hasher).map_err(|error| io_error(path, error))?;
    Ok(format!("{:x}", hasher.finalize()))
}

/// The five HOCON files `PortableConfiguration#files` renders, in MXRB's
/// order, each pinned byte-for-byte against MXRB's own rendering.
fn configuration_files(metadata: &Value) -> [(String, String); 5] {
    [
        ("etc/Default".to_string(), DEFAULT_INCLUDES.to_string()),
        ("etc/StudioPro.conf".to_string(), studio(metadata)),
        (
            "etc/configurations/Default.conf".to_string(),
            DEFAULT_CONFIGURATION.to_string(),
        ),
        (
            "etc/constants/defaults.conf".to_string(),
            constants(metadata, false),
        ),
        (
            "etc/constants/variables.conf".to_string(),
            constants(metadata, true),
        ),
    ]
}

fn studio(metadata: &Value) -> String {
    let events = metadata
        .get("ScheduledEvents")
        .and_then(Value::as_array)
        .map(|events| {
            events
                .iter()
                .filter_map(|event| match event.get("Name") {
                    None | Some(Value::Null) => None,
                    Some(name) => Some(scalar_string(name)),
                })
                .collect::<Vec<_>>()
                .join(",")
        })
        .unwrap_or_default();
    let execution = if events.is_empty() {
        "NONE"
    } else {
        "SPECIFIED"
    };
    // MXRB: `@metadata['RuntimeVersion'].to_s.to_i >= 11` — the leading
    // integer of the version decides whether the Runtime knows a debugger
    // password. The odd four-space indentation is MXRB's own; pinned.
    let debugger = if leading_integer(&scalar_string(
        metadata.get("RuntimeVersion").unwrap_or(&Value::Null),
    )) >= 11
    {
        "    debugger.password = \"\""
    } else {
        ""
    };
    format!(
        "runtime {{\n  params {{\n    DTAPMode = D\n    ScheduledEventExecution = \"{execution}\"\n    MyScheduledEvents = \"{}\"\n    CACertificates = \"\"\n    ClientCertificates = \"\"\n    ClientCertificatePasswords = \"\"\n    HashAlgorithm = \"BCRYPT:12\"\n  }}\n  adminUser.password = \"\"\n{debugger}\n}}\nlogging = [\n  {{\n    name = MySubscriber\n    type = console\n    autoSubscribe = INFO\n    levels {{}}\n  }}\n]\n",
        hocon(&events)
    )
}

fn constants(metadata: &Value, variables: bool) -> String {
    let lines = metadata
        .get("Constants")
        .and_then(Value::as_array)
        .map(|constants| {
            constants
                .iter()
                .filter_map(|constant| {
                    let name = scalar_string(constant.get("Name").unwrap_or(&Value::Null));
                    if name.is_empty() {
                        return None;
                    }
                    let value = if variables {
                        format!("${{?CONSTANTS_{}}}", environment_name(&name))
                    } else {
                        format!(
                            "\"{}\"",
                            hocon(&scalar_string(
                                constant.get("DefaultValue").unwrap_or(&Value::Null)
                            ))
                        )
                    };
                    Some(format!("  \"{}\" = {value}", hocon(&name)))
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    format!(
        "runtime.params.MicroflowConstants {{\n{}\n}}\n",
        lines.join("\n")
    )
}

/// MXRB: `name.upcase.gsub(/[^A-Z0-9]+/, '_')` — every run of anything that
/// is not an ASCII capital or digit collapses into one underscore.
fn environment_name(name: &str) -> String {
    let mut result = String::with_capacity(name.len());
    for character in name.to_uppercase().chars() {
        if character.is_ascii_uppercase() || character.is_ascii_digit() {
            result.push(character);
        } else if !result.ends_with('_') {
            result.push('_');
        }
    }
    result
}

/// Ruby's `to_s` over a parsed JSON scalar: `nil` is empty, strings pass
/// through, numbers and booleans render. Containers are not expected in the
/// metadata this reads; they render as JSON rather than Ruby `inspect`.
fn scalar_string(value: &Value) -> String {
    match value {
        Value::Null => String::new(),
        Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

/// Ruby's `String#to_i`: optional whitespace and sign, then leading digits,
/// zero when there are none.
fn leading_integer(text: &str) -> i64 {
    let trimmed = text.trim_start();
    let (sign, digits) = match trimmed.strip_prefix('-') {
        Some(rest) => (-1, rest),
        None => (1, trimmed.strip_prefix('+').unwrap_or(trimmed)),
    };
    let digits: String = digits.chars().take_while(char::is_ascii_digit).collect();
    digits.parse::<i64>().unwrap_or_default() * sign
}

/// MXRB's HOCON escaping, replacement order included.
fn hocon(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
}

const DEFAULT_INCLUDES: &str = "include file(\"etc/StudioPro.conf\")\ninclude file(\"etc/constants/defaults.conf\")\ninclude file(\"etc/configurations/Default.conf\")\ninclude file(\"etc/variables.conf\")\ninclude file(\"etc/constants/variables.conf\")\n";

const DEFAULT_CONFIGURATION: &str = "runtime.params {\n  DatabaseType = HSQLDB\n  DatabaseName = default\n  DatabaseJdbcUrl = \"jdbc:hsqldb:file:app/data/database/default\"\n}\nadmin { adminPassword = \"\", port = 8090, addresses = [ localhost ] }\nruntime {\n  http { port = 8080, addresses = [ \"*\" ] }\n  params { ApplicationRootUrl = \"http://localhost:8080/\" }\n}\n";

/// `PortableFallbackAssets::START`, byte for byte.
const FALLBACK_START: &str = r#"#!/bin/sh
set -eu

ROOT_PATH=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
if [ -n "${JAVA_HOME:-}" ] && [ -x "$JAVA_HOME/bin/java" ]; then
  JAVA="$JAVA_HOME/bin/java"
else
  JAVA=$(command -v java || true)
fi
if [ -z "$JAVA" ] || [ ! -x "$JAVA" ]; then
  echo "Cannot find java; set JAVA_HOME or add java to PATH." >&2
  exit 1
fi

if [ -z "${M2EE_ADMIN_PASS:-}" ]; then
  M2EE_ADMIN_PASS=$(od -An -N16 -tx1 /dev/urandom | tr -d ' \n')
  export M2EE_ADMIN_PASS
fi
RUNTIME_ADMINUSER_PASSWORD="${RUNTIME_ADMINUSER_PASSWORD:-$M2EE_ADMIN_PASS}"
export RUNTIME_ADMINUSER_PASSWORD

export MX_INSTALL_PATH="$ROOT_PATH/lib"
CONFIG="${CONFIG:-$ROOT_PATH/etc/Default}"
if [ "$#" -eq 0 ]; then
  set -- "$CONFIG"
fi
cd "$ROOT_PATH"
exec "$JAVA" ${JAVA_OPTS:-} \
  -Dfile.encoding=UTF-8 \
  -Djava.io.tmpdir="${TMPDIR:-/tmp}" \
  -Djava.library.path="$MX_INSTALL_PATH/runtime/lib/x64;$ROOT_PATH/app/model/lib/userlib" \
  -jar "$MX_INSTALL_PATH/runtime/launcher/runtimelauncher.jar" \
  "$ROOT_PATH/app/." "$@"
"#;

/// `PortableFallbackAssets::VARIABLES`, byte for byte.
const FALLBACK_VARIABLES: &str = "admin {\n  port = ${?ADMIN_PORT}\n  addresses = ${?ADMIN_ADDRESSES}\n  adminPassword = ${?M2EE_ADMIN_PASS}\n}\nruntime.http {\n  port = ${?RUNTIME_HTTP_PORT}\n  addresses = ${?RUNTIME_HTTP_ADDRESSES}\n}\nruntime.params {\n  DatabaseHost = ${?RUNTIME_PARAMS_DATABASEHOST}\n  DatabaseJdbcUrl = ${?RUNTIME_PARAMS_DATABASEJDBCURL}\n  DatabaseName = ${?RUNTIME_PARAMS_DATABASENAME}\n  DatabaseUserName = ${?RUNTIME_PARAMS_DATABASEUSERNAME}\n  DatabasePassword = ${?RUNTIME_PARAMS_DATABASEPASSWORD}\n  DatabaseType = ${?RUNTIME_PARAMS_DATABASETYPE}\n  DatabaseUseSsl = ${?RUNTIME_PARAMS_DATABASEUSESSL}\n  ApplicationRootUrl = ${?RUNTIME_PARAMS_APPLICATIONROOTURL}\n}\nruntime.adminUser.password = ${?RUNTIME_ADMINUSER_PASSWORD}\n";

/// `PortableFallbackAssets::EXAMPLE`, byte for byte.
const FALLBACK_EXAMPLE: &str = "# Copy this file and pass its path to bin/start to override etc/Default.\nruntime.params { DatabaseType = HSQLDB, DatabaseName = default }\nadmin { port = 8090, addresses = [ localhost ] }\nruntime.http { port = 8080, addresses = [ \"*\" ] }\n";

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    /// MXRB's own rendering of the five configuration files and the three
    /// fallback assets, dumped from `PortableConfiguration` /
    /// `PortableFallbackAssets` for the two metadata cases below.
    const ORACLE: &str = include_str!("../tests/fixtures/portable_config_oracle.json");

    fn oracle() -> Value {
        serde_json::from_str(ORACLE).expect("valid oracle fixture")
    }

    fn empty_metadata() -> Value {
        json!({ "RuntimeVersion": "11.12.1" })
    }

    fn full_metadata() -> Value {
        json!({
            "RuntimeVersion": "10.24.0.73019",
            "ScheduledEvents": [
                { "Name": "Sales.Nightly" },
                { "Name": "Sales.Weekly" },
                { "Other": 1 }
            ],
            "Constants": [
                { "Name": "Sales.ApiUrl", "DefaultValue": "https://x/?a=1&b=\"q\"" },
                { "Name": "Sales.Max-Retries", "DefaultValue": 42 },
                { "Name": "", "DefaultValue": "skipped" },
                { "Name": "Sales.Flag", "DefaultValue": true },
                { "Name": "Sales.Empty" }
            ]
        })
    }

    #[test]
    fn configuration_matches_mxrbs_rendering_byte_for_byte() {
        let oracle = oracle();
        for (label, metadata) in [("empty", empty_metadata()), ("full", full_metadata())] {
            let expected = oracle.get(label).and_then(Value::as_object).unwrap();
            let rendered = configuration_files(&metadata);
            assert_eq!(rendered.len(), expected.len(), "{label}");
            for (path, content) in &rendered {
                let want = expected
                    .get(path)
                    .and_then(Value::as_str)
                    .unwrap_or_else(|| panic!("{label}: oracle is missing {path}"));
                assert_eq!(content, want, "{label}: {path}");
            }
        }
    }

    #[test]
    fn fallback_assets_match_mxrbs_byte_for_byte() {
        let oracle = oracle();
        let fallback = oracle.get("fallback").and_then(Value::as_object).unwrap();
        for (name, ours) in [
            ("start", FALLBACK_START),
            ("variables", FALLBACK_VARIABLES),
            ("example", FALLBACK_EXAMPLE),
        ] {
            assert_eq!(ours, fallback.get(name).and_then(Value::as_str).unwrap());
        }
    }

    #[test]
    fn templates_lose_their_bom_their_comments_and_their_placeholder() {
        let template = b"\xEF\xBB\xBF{{!-- header\ncomment --}}\nline\n{{!-- inline --}}  tail {{DefaultConfig}}\n";
        assert_eq!(render_template(template), "line\ntail Default\n");
        // An unterminated comment stays, exactly like MXRB's non-matching gsub.
        assert_eq!(render_template(b"{{!-- open\nrest"), "{{!-- open\nrest");
    }

    #[test]
    fn environment_names_collapse_runs_like_mxrbs_gsub() {
        assert_eq!(environment_name("Sales.ApiUrl"), "SALES_APIURL");
        assert_eq!(environment_name("Sales.Max-Retries"), "SALES_MAX_RETRIES");
        assert_eq!(environment_name(".a--b."), "_A_B_");
    }

    #[test]
    fn leading_integer_reads_like_rubys_to_i() {
        assert_eq!(leading_integer("11.12.1"), 11);
        assert_eq!(leading_integer("  10"), 10);
        assert_eq!(leading_integer("-2x"), -2);
        assert_eq!(leading_integer(""), 0);
        assert_eq!(leading_integer("beta"), 0);
    }
}
