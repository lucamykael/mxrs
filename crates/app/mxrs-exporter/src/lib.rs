//! Reads an existing `.mpr` (via `mxrs-model`) and emits Rust
//! `project! {}` source text — the forward half of the "open a Mendix
//! project as editable Rust, change it, write it back" round trip. The
//! backward half already existed: `mxrs-writer::synchronize_project`
//! upserts entities/attributes/associations/microflows by name with `$ID`
//! preservation. Un-defers what the original phased plan called
//! `mxrs-exporter` (`decisions/mxrs-rust-rewrite-plan.md` in this
//! project's ai-memory) — that entry meant `Exporter`, model → **Ruby**
//! source, dropped for good (no Ruby anywhere in mxrs). This is a
//! different, new capability: model → **Rust** source, which the original
//! plan didn't anticipate needing.
//!
//! `export_project` is fail-closed: it refuses projects whose unsupported
//! domain features would be erased or reset by a subsequent synchronization.
//! `export_project_lossy` retains the original best-effort behavior for source
//! inspection, but its output must be reviewed before write-back.
//!
//! **Current typed projection, explicit about what's outside it**:
//!
//! - **Domain model** — entities, attributes, associations.
//! - **Documents** — enumerations (values, localized captions and
//!   documentation), constants (type, value, documentation and client
//!   exposure), regular expressions (pattern, visibility and exclusion), and
//!   scheduled events (legacy cadence, modern schedule, start instant and
//!   execution policy) are emitted into `src/domain/documents/mod.rs`. Imported
//!   fields outside that IR are retained by the writer. `portability
//!   --verify-round-trip` checks these documents by identity, containment
//!   and raw BSON bytes.
//! - **Not existing flow graphs** — reconstructing structured
//!   `create`/`change`/`if`/`call`
//!   statements from a microflow's persisted activity *graph* (arbitrary
//!   branching, not just the linear-plus-one-decision shape a hand-written
//!   `project! {}` body produces) is a real decompiler, out of scope for
//!   the current projection. Concretely safe consequence: imported source
//!   declares no microflows for any module, and
//!   `mxrs-writer::synchronize_project` never deletes anything absent from
//!   what it's given (see its own doc comment) — so writing an exported
//!   file straight back through `synchronize_project` leaves every
//!   existing microflow on disk untouched, not deleted.
//! - **All eleven attribute types are represented** by `project! {}`:
//!   string/integer/long/float/decimal/boolean/datetime/autonumber,
//!   hash-string, binary, and enumeration. Attribute documentation, string
//!   length, date localization, required and unique validation are emitted
//!   too.
//! - **Association `Owner`/`StorageFormat`/`Documentation` round-trip** via
//!   typed options in `project!`.
//! - **Entity `Image`/`indexes`/`access rules`/`lifecycle callbacks`/
//!   `generalization target`** have no `EntityDecl` DSL surface at all yet
//!   (same gap `mxrs-writer`'s own doc comment already names) — not
//!   emitted into typed Rust. `mxrs import` retains these fields in the
//!   generated snapshot and the writer preserves them during typed domain
//!   synchronization, so they are opaque rather than lossy.
//!
//! Marker types are emitted by `project!` from these same entity
//! declarations, so exported source is self-contained without a second
//! schema manifest or a generated marker module.
//!
//! `import_cargo_project` also detects pages built entirely from
//! `mxrs-dsl`'s native/structural widget vocabulary, emits them as real
//! `pub fn` builders in `src/domain/pages/mod.rs`, and wires each one into
//! `build()` — see `page_export`'s doc comment for the widget vocabulary
//! detected and what remains snapshot-preserved.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

#[cfg(test)]
use mxrs_model::Attribute;
use mxrs_model::attribute::AttributeType;
use mxrs_model::entity::Entity;
use mxrs_model::{Association, Module, Project};

#[cfg(test)]
#[path = "../../../../xtask/support/nested_cargo.rs"]
mod nested_cargo;
mod page_export;
mod portability;
pub use page_export::PageExportReport;
pub use portability::{
    DocumentRoundTripReport, PortabilityFamily, PortabilityReport, PortabilityStatus,
    PortabilitySummary, audit_portability, verify_editable_document_round_trip,
};

/// One model feature the generated `project! {}` source cannot faithfully
/// represent yet. Paths use Mendix qualified names so the user can resolve
/// every finding without having to inspect storage ids.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoundTripGap {
    pub path: String,
    pub reason: String,
}

#[derive(Debug, thiserror::Error)]
pub enum ExportError {
    #[error(transparent)]
    Model(#[from] mxrs_model::ModelError),

    #[error(transparent)]
    Project(#[from] mxrs_project::ProjectError),

    #[error(transparent)]
    Writer(#[from] mxrs_writer::WriterError),

    #[error(transparent)]
    Typegen(#[from] mxrs_typegen::TypegenError),

    #[error("cannot write {path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },

    #[error("Cargo project destination {0} already exists")]
    DestinationExists(String),

    #[error("cannot format generated Cargo project at {path}: {detail}")]
    Formatting { path: String, detail: String },

    #[error(
        "refusing a lossy Rust export; {0} unsupported model feature(s) would not round-trip (pass --allow-lossy only if this is intentional)"
    )]
    Lossy(usize, Vec<RoundTripGap>),
}

impl ExportError {
    pub fn gaps(&self) -> &[RoundTripGap] {
        match self {
            ExportError::Lossy(_, gaps) => gaps,
            ExportError::Model(_)
            | ExportError::Project(_)
            | ExportError::Writer(_)
            | ExportError::Typegen(_)
            | ExportError::Io { .. }
            | ExportError::Formatting { .. }
            | ExportError::DestinationExists(_) => &[],
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CargoProjectImport {
    pub root: PathBuf,
    pub package_name: String,
    pub project_name: String,
    pub mendix_version: String,
    pub imported_units: usize,
    pub imported_assets: usize,
    pub typed_round_trip_gaps: Vec<RoundTripGap>,
    /// Pages detected as buildable from `mxrs-dsl`'s native/structural
    /// widget vocabulary, emitted into `src/domain/pages/mod.rs` and wired into
    /// `build()` — see `page_export`'s doc comment for what still stays
    /// opaque.
    pub page_export: PageExportReport,
}

pub type Result<T> = std::result::Result<T, ExportError>;

/// Exports only when the generated source can be synchronized back without
/// erasing a feature this first-slice grammar cannot express. The old,
/// explicitly lossy behavior remains available as [`export_project_lossy`]
/// for inspection and assisted migrations.
pub fn export_project(path: impl AsRef<Path>) -> Result<String> {
    let project = Project::open(path, true)?;
    let mendix_version = project.mendix_version()?.unwrap_or_default();
    let mut modules = project.modules()?;
    modules.sort_by(|a, b| a.name.cmp(&b.name));
    let gaps = round_trip_gaps(&modules);
    if !gaps.is_empty() {
        return Err(ExportError::Lossy(gaps.len(), gaps));
    }
    Ok(render(&mendix_version, &modules, &[]))
}

/// Emits best-effort source even when unsupported features must be rendered
/// as `TODO` comments. Callers must not feed this output back to
/// `synchronize_project` without reviewing every comment.
pub fn export_project_lossy(path: impl AsRef<Path>) -> mxrs_model::Result<String> {
    let project = Project::open(path, true)?;
    let mendix_version = project.mendix_version()?.unwrap_or_default();
    let mut modules = project.modules()?;
    modules.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(render(&mendix_version, &modules, &[]))
}

/// Imports an `.mpr` into a standalone Cargo project whose source tree and
/// generated `.mxdoc` snapshot are sufficient to build a new `.mpr`.
///
/// `mxrs_workspace` points at this repository while the crates are still
/// unpublished. Omitting it emits git dependencies suitable for a normal
/// checkout with network access.
pub fn import_cargo_project(
    mpr_path: impl AsRef<Path>,
    destination: impl AsRef<Path>,
    mxrs_workspace: Option<&Path>,
) -> Result<CargoProjectImport> {
    let destination = destination.as_ref();
    if destination.exists() {
        return Err(ExportError::DestinationExists(
            destination.display().to_string(),
        ));
    }
    std::fs::create_dir_all(destination).map_err(|source| io_error(destination, source))?;
    let result = import_cargo_project_inner(mpr_path.as_ref(), destination, mxrs_workspace);
    if result.is_err() {
        let _ = std::fs::remove_dir_all(destination);
    }
    result
}

fn import_cargo_project_inner(
    mpr_path: &Path,
    destination: &Path,
    mxrs_workspace: Option<&Path>,
) -> Result<CargoProjectImport> {
    let project = Project::open(mpr_path, true)?;
    let mendix_version = project.mendix_version()?.unwrap_or_default();
    let mut modules = project.modules()?;
    modules.sort_by(|left, right| left.name.cmp(&right.name));
    let gaps = round_trip_gaps(&modules);
    let (mut converted_pages, mut page_export) =
        page_export::convert_pages_for_version(&modules, &mendix_version);
    page_export::protect_referenced_page_elements(
        &project,
        &modules,
        &mut converted_pages,
        &mut page_export,
    )?;
    let pages_module_source = page_export::render_pages_module(&converted_pages);
    let entities_source = render(&mendix_version, &modules, &[]);
    let domain_source = render_domain_module(&converted_pages);
    let flows_source = render_flows_module(&mendix_version, &modules);
    let security_document = project.all_units()?.into_iter().find_map(|unit| {
        let document = project.mpr().parse_contents(&unit).ok()?;
        (document.get_str("$Type").ok() == Some("Security$ProjectSecurity")).then_some(document)
    });
    let security_source = render_security_module(&modules, security_document.as_ref());
    let navigation_source = render_navigation_module(&project.navigation()?);
    let (documents_source, _) = render_documents_module(&project, &mendix_version)?;
    let markers_source = mxrs_typegen::generate(&marker_manifest(&modules))?;
    drop(project);

    let imported = destination.join("model/imported");
    let manifest = mxrs_project::capture_imported_project(mpr_path, &imported)?;
    let imported_assets =
        mxrs_project::capture_project_assets(mpr_path, destination.join("assets"))?;
    let package_name = cargo_package_name(&manifest.project_name);
    let crate_name = package_name.replace('-', "_");
    let infrastructure_directory = destination.join("src/infrastructure");
    std::fs::create_dir_all(&infrastructure_directory)
        .map_err(|source| io_error(&infrastructure_directory, source))?;
    let domain_directory = destination.join("src/domain");
    std::fs::create_dir_all(&domain_directory)
        .map_err(|source| io_error(&domain_directory, source))?;
    let entities_directory = domain_directory.join("entities");
    std::fs::create_dir_all(&entities_directory)
        .map_err(|source| io_error(&entities_directory, source))?;
    let flows_directory = domain_directory.join("flows");
    std::fs::create_dir_all(&flows_directory)
        .map_err(|source| io_error(&flows_directory, source))?;
    let security_directory = domain_directory.join("security");
    std::fs::create_dir_all(&security_directory)
        .map_err(|source| io_error(&security_directory, source))?;
    let navigation_directory = domain_directory.join("navigation");
    std::fs::create_dir_all(&navigation_directory)
        .map_err(|source| io_error(&navigation_directory, source))?;
    let documents_directory = domain_directory.join("documents");
    std::fs::create_dir_all(&documents_directory)
        .map_err(|source| io_error(&documents_directory, source))?;

    write_text(
        &destination.join("Cargo.toml"),
        &cargo_manifest(&package_name, mxrs_workspace),
    )?;
    write_text(
        &destination.join("mxrs.toml"),
        &format!(
            "[project]\nname = {}\nmendix_version = {}\nimported_snapshot = \"model/imported\"\n",
            toml_string(&manifest.project_name),
            toml_string(&manifest.mendix_version),
        ),
    )?;
    write_text(
        &destination.join("src/lib.rs"),
        &format!(
            "// Required by the paths emitted inside the authoring macro.\nextern crate mxrs as mxrs_dsl;\nextern crate mxrs as mxrs_expr;\nextern crate mxrs as mxrs_ir;\nextern crate mxrs as mxrs_macros;\n\npub mod domain;\npub mod infrastructure;\n\n#[mxrs::application(version = {})]\npub struct Application;\n",
            rust_string(&manifest.mendix_version),
        ),
    )?;
    write_text(&destination.join("src/domain/mod.rs"), &domain_source)?;
    write_text(
        &destination.join("src/domain/entities/mod.rs"),
        &entities_source,
    )?;
    write_text(&destination.join("src/domain/flows/mod.rs"), &flows_source)?;
    write_text(
        &destination.join("src/domain/security/mod.rs"),
        &security_source,
    )?;
    write_text(
        &destination.join("src/domain/navigation/mod.rs"),
        &navigation_source,
    )?;
    write_text(
        &destination.join("src/domain/documents/mod.rs"),
        &documents_source,
    )?;
    if let Some(pages_module_source) = &pages_module_source {
        let pages_directory = domain_directory.join("pages");
        std::fs::create_dir_all(&pages_directory)
            .map_err(|source| io_error(&pages_directory, source))?;
        write_text(
            &destination.join("src/domain/pages/mod.rs"),
            pages_module_source,
        )?;
    }
    write_text(
        &destination.join("src/main.rs"),
        &build_binary_source(&crate_name, &manifest.project_name),
    )?;
    write_text(
        &destination.join("src/infrastructure/mod.rs"),
        "//! Generated marker types used for checked model references.\n\npub mod markers;\n",
    )?;
    write_text(
        &destination.join("src/infrastructure/markers.rs"),
        &markers_source,
    )?;
    format_generated_cargo_project(destination)?;
    write_text(&destination.join(".gitignore"), "/build\n/target\n")?;
    write_text(
        &destination.join("README.md"),
        &generated_readme(&manifest.project_name, gaps.len(), &page_export),
    )?;

    Ok(CargoProjectImport {
        root: destination.to_path_buf(),
        package_name,
        project_name: manifest.project_name,
        mendix_version: manifest.mendix_version,
        imported_units: manifest.units.len(),
        imported_assets,
        typed_round_trip_gaps: gaps,
        page_export,
    })
}

/// Composes editable domain concepts without coupling the entity projection
/// to page source files. New concept families (flows, security, navigation)
/// can join this module as peers without flattening the generated project.
fn render_domain_module(pages: &[page_export::ConvertedPage]) -> String {
    let mut source = String::from(
        "//! Editable Cargo-native Mendix concepts, grouped by concept family.\n\n\
         pub mod entities;\n\
         pub mod documents;\n\
         pub mod flows;\n\
         pub mod navigation;\n\
         pub mod security;\n",
    );
    if !pages.is_empty() {
        source.push_str("pub mod pages;\n");
    }
    source.push_str(
        "\npub fn build() -> ::mxrs_ir::ProjectDecl {\n    let mut project = entities::build();\n",
    );
    for page in pages {
        let _ = writeln!(
            source,
            "    project.modules.iter_mut().find(|m| m.name == {:?}).expect(\"module {} exists\").pages.push(pages::{}());",
            page.module_name, page.module_name, page.function_name,
        );
    }
    source.push_str(
        "    documents::apply(&mut project);\n    flows::apply(&mut project);\n    security::apply(&mut project);\n    navigation::apply(&mut project);\n    project\n}\n",
    );
    source
}

#[derive(Debug)]
enum EditableDocument {
    Enumeration {
        module: String,
        name: String,
        documentation: String,
        values: Vec<(String, Vec<(String, String)>)>,
    },
    Constant {
        module: String,
        name: String,
        documentation: String,
        value_type: &'static str,
        value: Option<String>,
        exposed_to_client: bool,
    },
    RegularExpression {
        module: String,
        name: String,
        documentation: String,
        expression: String,
        excluded: bool,
        export_level: &'static str,
    },
    ScheduledEvent {
        module: String,
        declaration: mxrs_ir::ScheduledEventDecl,
    },
}

fn render_documents_module(
    project: &Project,
    mendix_version: &str,
) -> Result<(String, HashMap<&'static str, usize>)> {
    let mut declarations = collect_editable_documents(project)?;
    declarations
        .sort_by(|left, right| editable_document_key(left).cmp(&editable_document_key(right)));
    let mut editable_counts = HashMap::new();
    for declaration in &declarations {
        let native_type = match declaration {
            EditableDocument::Enumeration { .. } => "Enumerations$Enumeration",
            EditableDocument::Constant { .. } => "Constants$Constant",
            EditableDocument::RegularExpression { .. } => "RegularExpressions$RegularExpression",
            EditableDocument::ScheduledEvent { .. } => "ScheduledEvents$ScheduledEvent",
        };
        *editable_counts.entry(native_type).or_default() += 1;
    }

    let mut source = String::from(
        "//! Editable Cargo-native enumerations, constants, regular expressions, and scheduled events.\n\n\
         fn declarations() -> ::mxrs_ir::ProjectDecl {\n\
             let mut project = ::mxrs_dsl::ProjectBuilder::new(",
    );
    source.push_str(&rust_string(mendix_version));
    source.push_str(");\n");
    let mut current_module = None::<String>;
    for declaration in declarations {
        let module = editable_document_module(&declaration);
        if current_module.as_deref() != Some(module) {
            if current_module.is_some() {
                source.push_str("    });\n");
            }
            let _ = writeln!(
                source,
                "    project.module({}, |module| {{",
                rust_string(module)
            );
            current_module = Some(module.to_string());
        }
        render_editable_document_body(&mut source, declaration);
    }
    if current_module.is_some() {
        source.push_str("    });\n");
    }
    source.push_str(
        "    project.build()\n}\n\n\
         pub fn apply(project: &mut ::mxrs_ir::ProjectDecl) {\n\
             for declared in declarations().modules {\n\
                 if let Some(target) = project.modules.iter_mut().find(|module| module.name == declared.name) {\n\
                     target.enumerations.extend(declared.enumerations);\n\
                     target.constants.extend(declared.constants);\n\
                     target.regular_expressions.extend(declared.regular_expressions);\n\
                     target.scheduled_events.extend(declared.scheduled_events);\n\
                 } else {\n\
                     project.modules.push(declared);\n\
                 }\n\
             }\n\
         }\n",
    );
    Ok((source, editable_counts))
}

fn collect_editable_documents(project: &Project) -> Result<Vec<EditableDocument>> {
    let units = project.all_units()?;
    let module_by_id = project
        .modules()?
        .into_iter()
        .filter_map(|module| Some((module.id, module.name?)))
        .collect::<HashMap<_, _>>();
    let parent_by_id = units
        .iter()
        .map(|unit| (unit.unit_id.clone(), unit.container_id.clone()))
        .collect::<HashMap<_, _>>();
    let mut declarations = Vec::new();
    for unit in &units {
        let document = project
            .mpr()
            .parse_contents(unit)
            .map_err(mxrs_model::ModelError::from)?;
        let Some(module) = owning_module(&unit.container_id, &parent_by_id, &module_by_id) else {
            continue;
        };
        match document.get_str("$Type").ok() {
            Some("Enumerations$Enumeration") => {
                let Some(name) = document.get_str("Name").ok().map(str::to_string) else {
                    continue;
                };
                let Some(values) = document
                    .get_array("Values")
                    .ok()
                    .map(|values| mxrs_bson::parse_array(Some(values)))
                    .and_then(|values| {
                        values
                            .items
                            .into_iter()
                            .map(|value| value.as_document().cloned())
                            .collect::<Option<Vec<_>>>()
                    })
                    .and_then(|values| {
                        values
                            .into_iter()
                            .map(|value| {
                                let name = value.get_str("Name").ok()?.to_string();
                                let caption = value.get_document("Caption").ok()?;
                                let captions = caption
                                    .get_array("Items")
                                    .ok()
                                    .map(|items| mxrs_bson::parse_array(Some(items)))?
                                    .items
                                    .into_iter()
                                    .map(|translation| {
                                        let translation = translation.as_document()?;
                                        Some((
                                            translation.get_str("LanguageCode").ok()?.to_string(),
                                            translation.get_str("Text").ok()?.to_string(),
                                        ))
                                    })
                                    .collect::<Option<Vec<_>>>()?;
                                Some((name, captions))
                            })
                            .collect::<Option<Vec<_>>>()
                    })
                else {
                    continue;
                };
                declarations.push(EditableDocument::Enumeration {
                    module,
                    name,
                    documentation: document
                        .get_str("Documentation")
                        .unwrap_or_default()
                        .to_string(),
                    values,
                });
            }
            Some("Constants$Constant") => {
                let Some(name) = document.get_str("Name").ok().map(str::to_string) else {
                    continue;
                };
                let Some(value_type) = document
                    .get_document("Type")
                    .ok()
                    .and_then(|kind| kind.get_str("$Type").ok())
                    .and_then(constant_type_variant)
                else {
                    continue;
                };
                let value = (!sensitive_constant_name(&name)).then(|| {
                    document
                        .get_str("DefaultValue")
                        .unwrap_or_default()
                        .to_string()
                });
                declarations.push(EditableDocument::Constant {
                    module,
                    name,
                    documentation: document
                        .get_str("Documentation")
                        .unwrap_or_default()
                        .to_string(),
                    value_type,
                    value,
                    exposed_to_client: document.get_bool("ExposedToClient").unwrap_or(false),
                });
            }
            Some("RegularExpressions$RegularExpression") => {
                if !is_complete_regular_expression_document(&document) {
                    continue;
                }
                let (Some(name), Some(expression), Some(export_level)) = (
                    document.get_str("Name").ok(),
                    document.get_str("Expression").ok(),
                    document
                        .get_str("ExportLevel")
                        .ok()
                        .and_then(export_level_variant),
                ) else {
                    continue;
                };
                declarations.push(EditableDocument::RegularExpression {
                    module,
                    name: name.to_string(),
                    documentation: document
                        .get_str("Documentation")
                        .expect("shape was checked")
                        .to_string(),
                    expression: expression.to_string(),
                    excluded: document.get_bool("Excluded").expect("shape was checked"),
                    export_level,
                });
            }
            Some("ScheduledEvents$ScheduledEvent") => {
                let Some(declaration) = parse_complete_scheduled_event(&document) else {
                    continue;
                };
                declarations.push(EditableDocument::ScheduledEvent {
                    module,
                    declaration,
                });
            }
            _ => {}
        }
    }
    Ok(declarations)
}

/// Only promote a native document to the typed projection when the IR owns
/// every field and each required value has the expected representation. A
/// future Mendix field or shape therefore remains byte-preserved instead of
/// being silently normalized by a declaration that does not understand it.
fn is_complete_regular_expression_document(document: &mxrs_bson::Document) -> bool {
    const FIELDS: [&str; 7] = [
        "$ID",
        "$Type",
        "Documentation",
        "Excluded",
        "ExportLevel",
        "Expression",
        "Name",
    ];
    document.len() == FIELDS.len()
        && document
            .keys()
            .all(|field| FIELDS.contains(&field.as_str()))
        && document
            .get("$ID")
            .and_then(mxrs_bson::extract_id)
            .is_some()
        && document.get_str("$Type").is_ok()
        && document.get_str("Documentation").is_ok()
        && document.get_bool("Excluded").is_ok()
        && document.get_str("ExportLevel").is_ok()
        && document.get_str("Expression").is_ok()
        && document.get_str("Name").is_ok()
}

fn parse_complete_scheduled_event(
    document: &mxrs_bson::Document,
) -> Option<mxrs_ir::ScheduledEventDecl> {
    const FIELDS: [&str; 14] = [
        "$ID",
        "$Type",
        "Documentation",
        "Enabled",
        "Excluded",
        "ExportLevel",
        "Interval",
        "IntervalType",
        "Microflow",
        "Name",
        "OnOverlap",
        "Schedule",
        "StartDateTime",
        "TimeZone",
    ];
    if document.len() != FIELDS.len()
        || !document
            .keys()
            .all(|field| FIELDS.contains(&field.as_str()))
        || document
            .get("$ID")
            .and_then(mxrs_bson::extract_id)
            .is_none()
        || document.get_str("$Type").ok() != Some("ScheduledEvents$ScheduledEvent")
    {
        return None;
    }
    let unit = match document.get_str("IntervalType").ok()? {
        "Millisecond" => mxrs_ir::ScheduleUnit::Milliseconds,
        "Second" => mxrs_ir::ScheduleUnit::Seconds,
        "Minute" => mxrs_ir::ScheduleUnit::Minutes,
        "Hour" => mxrs_ir::ScheduleUnit::Hours,
        "Day" => mxrs_ir::ScheduleUnit::Days,
        "Week" => mxrs_ir::ScheduleUnit::Weeks,
        "Month" => mxrs_ir::ScheduleUnit::Months,
        "Year" => mxrs_ir::ScheduleUnit::Years,
        _ => return None,
    };
    let export_level = match document.get_str("ExportLevel").ok()? {
        "Hidden" => mxrs_ir::ExportLevel::Hidden,
        "Published" => mxrs_ir::ExportLevel::Published,
        _ => return None,
    };
    let on_overlap = match document.get_str("OnOverlap").ok()? {
        "SkipNext" => mxrs_ir::OnOverlap::SkipNext,
        "DelayNext" => mxrs_ir::OnOverlap::DelayNext,
        _ => return None,
    };
    let schedule = parse_complete_event_schedule(document.get("Schedule")?)?;
    let declaration = mxrs_ir::ScheduledEventDecl {
        name: document.get_str("Name").ok()?.to_string(),
        documentation: document.get_str("Documentation").ok()?.to_string(),
        excluded: document.get_bool("Excluded").ok()?,
        export_level,
        microflow: document.get_str("Microflow").ok()?.to_string(),
        unit,
        interval: document.get_i64("Interval").ok()?,
        start_at: document
            .get_datetime("StartDateTime")
            .ok()?
            .try_to_rfc3339_string()
            .ok()?,
        time_zone: document.get_str("TimeZone").ok()?.to_string(),
        schedule,
        on_overlap,
        enabled: document.get_bool("Enabled").ok()?,
    };
    scheduled_event_semantics_supported(&declaration).then_some(declaration)
}

fn scheduled_event_semantics_supported(event: &mxrs_ir::ScheduledEventDecl) -> bool {
    if event.interval < 0 || (event.enabled && event.microflow.trim().is_empty()) {
        return false;
    }
    match &event.schedule {
        mxrs_ir::ScheduledEventSchedule::None => true,
        mxrs_ir::ScheduledEventSchedule::Minute { multiplier } => *multiplier > 0,
        mxrs_ir::ScheduledEventSchedule::Hour {
            multiplier,
            minute_offset,
        } => *multiplier > 0 && (0..=59).contains(minute_offset),
        mxrs_ir::ScheduledEventSchedule::Day {
            hour_of_day,
            minute_of_hour,
        }
        | mxrs_ir::ScheduledEventSchedule::Week {
            hour_of_day,
            minute_of_hour,
            ..
        } => (0..=23).contains(hour_of_day) && (0..=59).contains(minute_of_hour),
    }
}

fn parse_complete_event_schedule(
    value: &mxrs_bson::Bson,
) -> Option<mxrs_ir::ScheduledEventSchedule> {
    let mxrs_bson::Bson::Document(schedule) = value else {
        return matches!(value, mxrs_bson::Bson::Null)
            .then_some(mxrs_ir::ScheduledEventSchedule::None);
    };
    schedule.get("$ID").and_then(mxrs_bson::extract_id)?;
    let exact = |fields: &[&str]| {
        schedule.len() == fields.len()
            && schedule
                .keys()
                .all(|field| fields.contains(&field.as_str()))
    };
    match schedule.get_str("$Type").ok()? {
        "ScheduledEvents$MinuteSchedule" if exact(&["$ID", "$Type", "Multiplier"]) => {
            Some(mxrs_ir::ScheduledEventSchedule::Minute {
                multiplier: schedule.get_i64("Multiplier").ok()?,
            })
        }
        "ScheduledEvents$HourSchedule"
            if exact(&["$ID", "$Type", "Multiplier", "MinuteOffset"]) =>
        {
            Some(mxrs_ir::ScheduledEventSchedule::Hour {
                multiplier: schedule.get_i64("Multiplier").ok()?,
                minute_offset: schedule.get_i64("MinuteOffset").ok()?,
            })
        }
        "ScheduledEvents$DaySchedule" if exact(&["$ID", "$Type", "HourOfDay", "MinuteOfHour"]) => {
            Some(mxrs_ir::ScheduledEventSchedule::Day {
                hour_of_day: schedule.get_i64("HourOfDay").ok()?,
                minute_of_hour: schedule.get_i64("MinuteOfHour").ok()?,
            })
        }
        "ScheduledEvents$WeekSchedule"
            if exact(&[
                "$ID",
                "$Type",
                "HourOfDay",
                "MinuteOfHour",
                "Monday",
                "Tuesday",
                "Wednesday",
                "Thursday",
                "Friday",
                "Saturday",
                "Sunday",
            ]) =>
        {
            Some(mxrs_ir::ScheduledEventSchedule::Week {
                hour_of_day: schedule.get_i64("HourOfDay").ok()?,
                minute_of_hour: schedule.get_i64("MinuteOfHour").ok()?,
                monday: schedule.get_bool("Monday").ok()?,
                tuesday: schedule.get_bool("Tuesday").ok()?,
                wednesday: schedule.get_bool("Wednesday").ok()?,
                thursday: schedule.get_bool("Thursday").ok()?,
                friday: schedule.get_bool("Friday").ok()?,
                saturday: schedule.get_bool("Saturday").ok()?,
                sunday: schedule.get_bool("Sunday").ok()?,
            })
        }
        _ => None,
    }
}

fn render_editable_document_body(source: &mut String, declaration: EditableDocument) {
    match declaration {
        EditableDocument::Enumeration {
            module: _,
            name,
            documentation,
            values,
        } => {
            let _ = writeln!(
                source,
                "        module.enumeration({}, |enumeration| {{",
                rust_string(&name)
            );
            if !documentation.is_empty() {
                let _ = writeln!(
                    source,
                    "            enumeration.documentation({});",
                    rust_string(&documentation)
                );
            }
            for (name, captions) in values {
                let _ = writeln!(
                    source,
                    "            enumeration.value({}).captions = vec![{}];",
                    rust_string(&name),
                    captions
                        .iter()
                        .map(|(language, text)| format!(
                            "({}.to_string(), {}.to_string())",
                            rust_string(language),
                            rust_string(text)
                        ))
                        .collect::<Vec<_>>()
                        .join(", ")
                );
            }
            source.push_str("        });\n");
        }
        EditableDocument::Constant {
            module,
            name,
            documentation,
            value_type,
            value,
            exposed_to_client,
        } => {
            let _ = writeln!(
                source,
                "        module.constant({}, |constant| {{",
                rust_string(&name)
            );
            if !documentation.is_empty() {
                let _ = writeln!(
                    source,
                    "            constant.documentation({});",
                    rust_string(&documentation)
                );
            }
            let _ = writeln!(
                source,
                "            constant.value_type(::mxrs_ir::ConstantType::{value_type});"
            );
            match value {
                Some(value) => {
                    let _ = writeln!(
                        source,
                        "            constant.value({});",
                        rust_string(&value)
                    );
                }
                None => {
                    let variable = constant_environment_variable(&module, &name);
                    let _ = writeln!(
                        source,
                        "            constant.value_from_env({});",
                        rust_string(&variable)
                    );
                }
            }
            if exposed_to_client {
                source.push_str("            constant.exposed_to_client(true);\n");
            }
            source.push_str("        });\n");
        }
        EditableDocument::RegularExpression {
            module: _,
            name,
            documentation,
            expression,
            excluded,
            export_level,
        } => {
            let has_options = !documentation.is_empty() || excluded || export_level != "Hidden";
            let parameter = if has_options {
                "regular_expression"
            } else {
                "_"
            };
            let _ = writeln!(
                source,
                "        module.regular_expression({}, {}, |{parameter}| {{",
                rust_string(&name),
                rust_string(&expression),
            );
            if !documentation.is_empty() {
                let _ = writeln!(
                    source,
                    "            regular_expression.documentation({});",
                    rust_string(&documentation)
                );
            }
            if excluded {
                source.push_str("            regular_expression.excluded(true);\n");
            }
            if export_level != "Hidden" {
                let _ = writeln!(
                    source,
                    "            regular_expression.export_level(::mxrs_ir::ExportLevel::{export_level});"
                );
            }
            source.push_str("        });\n");
        }
        EditableDocument::ScheduledEvent {
            module: _,
            declaration,
        } => render_scheduled_event_body(source, &declaration),
    }
}

fn render_scheduled_event_body(source: &mut String, event: &mxrs_ir::ScheduledEventDecl) {
    let _ = writeln!(
        source,
        "        module.scheduled_event({}, {}, ::mxrs_ir::ScheduleUnit::{:?}, |event| {{",
        rust_string(&event.name),
        rust_string(&event.microflow),
        event.unit,
    );
    if !event.documentation.is_empty() {
        let _ = writeln!(
            source,
            "            event.documentation({});",
            rust_string(&event.documentation)
        );
    }
    let _ = writeln!(source, "            event.every({});", event.interval);
    let _ = writeln!(
        source,
        "            event.start_at({});",
        rust_string(&event.start_at)
    );
    let _ = writeln!(
        source,
        "            event.time_zone({});",
        rust_string(&event.time_zone)
    );
    let _ = writeln!(
        source,
        "            event.on_overlap(::mxrs_ir::OnOverlap::{:?});",
        event.on_overlap
    );
    if !event.enabled {
        source.push_str("            event.enabled(false);\n");
    }
    if event.excluded {
        source.push_str("            event.excluded(true);\n");
    }
    if event.export_level != mxrs_ir::ExportLevel::Hidden {
        let _ = writeln!(
            source,
            "            event.export_level(::mxrs_ir::ExportLevel::{:?});",
            event.export_level
        );
    }
    let schedule = render_event_schedule(&event.schedule);
    let _ = writeln!(source, "            event.schedule({schedule});");
    source.push_str("        });\n");
}

fn render_event_schedule(schedule: &mxrs_ir::ScheduledEventSchedule) -> String {
    match schedule {
        mxrs_ir::ScheduledEventSchedule::None => {
            "::mxrs_ir::ScheduledEventSchedule::None".to_string()
        }
        mxrs_ir::ScheduledEventSchedule::Minute { multiplier } => {
            format!("::mxrs_ir::ScheduledEventSchedule::Minute {{ multiplier: {multiplier} }}")
        }
        mxrs_ir::ScheduledEventSchedule::Hour {
            multiplier,
            minute_offset,
        } => format!(
            "::mxrs_ir::ScheduledEventSchedule::Hour {{ multiplier: {multiplier}, minute_offset: {minute_offset} }}"
        ),
        mxrs_ir::ScheduledEventSchedule::Day {
            hour_of_day,
            minute_of_hour,
        } => format!(
            "::mxrs_ir::ScheduledEventSchedule::Day {{ hour_of_day: {hour_of_day}, minute_of_hour: {minute_of_hour} }}"
        ),
        mxrs_ir::ScheduledEventSchedule::Week {
            hour_of_day,
            minute_of_hour,
            monday,
            tuesday,
            wednesday,
            thursday,
            friday,
            saturday,
            sunday,
        } => format!(
            "::mxrs_ir::ScheduledEventSchedule::Week {{ hour_of_day: {hour_of_day}, minute_of_hour: {minute_of_hour}, monday: {monday}, tuesday: {tuesday}, wednesday: {wednesday}, thursday: {thursday}, friday: {friday}, saturday: {saturday}, sunday: {sunday} }}"
        ),
    }
}

fn editable_document_module(document: &EditableDocument) -> &str {
    match document {
        EditableDocument::Enumeration { module, .. }
        | EditableDocument::Constant { module, .. }
        | EditableDocument::RegularExpression { module, .. }
        | EditableDocument::ScheduledEvent { module, .. } => module,
    }
}

fn editable_document_key(document: &EditableDocument) -> (&str, u8, &str) {
    match document {
        EditableDocument::Enumeration { module, name, .. } => (module, 0, name),
        EditableDocument::Constant { module, name, .. } => (module, 1, name),
        EditableDocument::RegularExpression { module, name, .. } => (module, 2, name),
        EditableDocument::ScheduledEvent {
            module,
            declaration,
        } => (module, 3, &declaration.name),
    }
}

fn editable_project_declaration(project: &Project) -> Result<mxrs_ir::ProjectDecl> {
    let mut modules = std::collections::BTreeMap::<String, mxrs_ir::ModuleDecl>::new();
    for document in collect_editable_documents(project)? {
        let module_name = match &document {
            EditableDocument::Enumeration { module, .. }
            | EditableDocument::Constant { module, .. }
            | EditableDocument::RegularExpression { module, .. }
            | EditableDocument::ScheduledEvent { module, .. } => module.clone(),
        };
        let module = modules
            .entry(module_name.clone())
            .or_insert_with(|| mxrs_ir::ModuleDecl {
                name: module_name,
                ..mxrs_ir::ModuleDecl::default()
            });
        match document {
            EditableDocument::Enumeration {
                name,
                documentation,
                values,
                ..
            } => module.enumerations.push(mxrs_ir::EnumerationDecl {
                name,
                documentation,
                values: values
                    .into_iter()
                    .map(|(name, captions)| mxrs_ir::EnumerationValueDecl { name, captions })
                    .collect(),
            }),
            EditableDocument::Constant {
                name,
                documentation,
                value_type,
                value,
                exposed_to_client,
                ..
            } => module.constants.push(mxrs_ir::ConstantDecl {
                name,
                documentation,
                constant_type: match value_type {
                    "String" => mxrs_ir::ConstantType::String,
                    "Integer" => mxrs_ir::ConstantType::Integer,
                    "Boolean" => mxrs_ir::ConstantType::Boolean,
                    "Decimal" => mxrs_ir::ConstantType::Decimal,
                    "DateTime" => mxrs_ir::ConstantType::DateTime,
                    _ => unreachable!("constant_type_variant returns a closed set"),
                },
                value,
                exposed_to_client,
            }),
            EditableDocument::RegularExpression {
                name,
                documentation,
                expression,
                excluded,
                export_level,
                ..
            } => module
                .regular_expressions
                .push(mxrs_ir::RegularExpressionDecl {
                    name,
                    documentation,
                    expression,
                    excluded,
                    export_level: match export_level {
                        "Hidden" => mxrs_ir::ExportLevel::Hidden,
                        "Published" => mxrs_ir::ExportLevel::Published,
                        _ => unreachable!("export_level_variant returns a closed set"),
                    },
                }),
            EditableDocument::ScheduledEvent { declaration, .. } => {
                module.scheduled_events.push(declaration)
            }
        }
    }
    Ok(mxrs_ir::ProjectDecl {
        mendix_version: project.mendix_version()?.unwrap_or_default(),
        modules: modules.into_values().collect(),
        security: None,
        navigation: None,
    })
}

fn owning_module<'a>(
    container: &'a str,
    parents: &'a HashMap<String, String>,
    modules: &'a HashMap<String, String>,
) -> Option<String> {
    let mut current = container;
    for _ in 0..64 {
        if let Some(name) = modules.get(current) {
            return Some(name.clone());
        }
        let parent = parents.get(current)?;
        if parent == current {
            return None;
        }
        current = parent;
    }
    None
}

fn constant_type_variant(native_type: &str) -> Option<&'static str> {
    match native_type {
        "DataTypes$StringType" => Some("String"),
        "DataTypes$IntegerType" => Some("Integer"),
        "DataTypes$BooleanType" => Some("Boolean"),
        "DataTypes$DecimalType" => Some("Decimal"),
        "DataTypes$DateTimeType" => Some("DateTime"),
        _ => None,
    }
}

fn export_level_variant(value: &str) -> Option<&'static str> {
    match value {
        "Hidden" => Some("Hidden"),
        "Published" => Some("Published"),
        _ => None,
    }
}

fn sensitive_constant_name(name: &str) -> bool {
    let normalized = name.to_ascii_lowercase().replace(['-', '_'], "");
    [
        "token",
        "password",
        "secret",
        "credential",
        "apikey",
        "privatekey",
    ]
    .iter()
    .any(|marker| normalized.contains(marker))
}

fn constant_environment_variable(module: &str, name: &str) -> String {
    let mut result = String::from("MXRS_");
    for character in format!("{module}_{name}").chars() {
        if character.is_ascii_alphanumeric() {
            result.push(character.to_ascii_uppercase());
        } else if !result.ends_with('_') {
            result.push('_');
        }
    }
    result.trim_end_matches('_').to_string()
}

/// Creates the editable flow composition layer. New typed declarations merge
/// by module and are then upserted by the writer.
fn render_flows_module(mendix_version: &str, _modules: &[Module]) -> String {
    let mut source = String::from("//! Editable Cargo-native flow declarations.\n");
    source.push_str(
        "\nfn declarations() -> ::mxrs_ir::ProjectDecl {\n\
             #[allow(unused_mut)]\n\
             let mut project = ::mxrs_dsl::ProjectBuilder::new(",
    );
    source.push_str(&rust_string(mendix_version));
    source.push_str(
        ");\n\
             // Add `project.module(..., |module| module.microflow(...))` or\n\
             // `module.nanoflow(...)` declarations here.\n\
             project.build()\n\
         }\n\n\
         pub fn apply(project: &mut ::mxrs_ir::ProjectDecl) {\n\
             for declared in declarations().modules {\n\
                 if let Some(target) = project.modules.iter_mut().find(|module| module.name == declared.name) {\n\
                     target.microflows.extend(declared.microflows);\n\
                     target.nanoflows.extend(declared.nanoflows);\n\
                 } else {\n\
                     project.modules.push(declared);\n\
                 }\n\
             }\n\
         }\n",
    );
    source
}

fn render_security_module(modules: &[Module], document: Option<&mxrs_bson::Document>) -> String {
    let mut source = String::from(
        "//! Editable Cargo-native project and module security.\n\n\
         pub fn apply(project: &mut ::mxrs_ir::ProjectDecl) {\n",
    );
    for module in modules {
        if module.module_roles.is_empty() {
            continue;
        }
        let module_name = module.name.as_deref().unwrap_or("Unnamed");
        let _ = writeln!(
            source,
            "    if let Some(module) = project.modules.iter_mut().find(|module| module.name == {}) {{",
            rust_string(module_name)
        );
        source.push_str("        module.roles = Some(vec![\n");
        let mut roles = module.module_roles.iter().collect::<Vec<_>>();
        roles.sort_by(|left, right| left.name.cmp(&right.name));
        for role in roles {
            let name = role.name.as_deref().unwrap_or("Unnamed");
            let _ = writeln!(
                source,
                "            ::mxrs_ir::ModuleRoleDecl {{ name: {}.to_string(), description: {}.to_string() }},",
                rust_string(name),
                rust_string(&role.description)
            );
        }
        source.push_str("        ]);\n    }\n");
    }

    if let Some(document) = document
        && let Some(level) = render_security_level(document.get_str("SecurityLevel").ok())
    {
        let admin_role = document.get_str("AdminUserRole").unwrap_or("Administrator");
        let guest_role = document
            .get_bool("EnableGuestAccess")
            .unwrap_or(false)
            .then(|| document.get_str("GuestUserRole").unwrap_or_default())
            .filter(|name| !name.is_empty());
        let sign_in = document
            .get_str("SignInMicroflow")
            .ok()
            .filter(|name| !name.is_empty());
        source.push_str("    let security = ::mxrs_ir::ProjectSecurityDecl {\n");
        let _ = writeln!(source, "        level: ::mxrs_ir::SecurityLevel::{level},");
        let _ = writeln!(
            source,
            "        check_security: {},",
            document.get_bool("CheckSecurity").unwrap_or(true)
        );
        let _ = writeln!(
            source,
            "        admin_user_role: {}.to_string(),",
            rust_string(admin_role)
        );
        let _ = writeln!(
            source,
            "        guest_user_role: {},",
            rust_option_string(guest_role)
        );
        let _ = writeln!(
            source,
            "        sign_in_microflow: {},",
            rust_option_string(sign_in)
        );
        source.push_str("        user_roles: vec![\n");
        let mut roles = bson_documents(document, "UserRoles");
        roles.sort_by(|left, right| {
            left.get_str("Name")
                .unwrap_or_default()
                .cmp(right.get_str("Name").unwrap_or_default())
        });
        for role in roles {
            let name = role.get_str("Name").unwrap_or("Unnamed");
            let description = role.get_str("Description").unwrap_or_default();
            let manageable = bson_strings(&role, "ManageableRoles");
            let module_roles = bson_strings(&role, "ModuleRoles");
            let _ = writeln!(
                source,
                "        ::mxrs_ir::UserRoleDecl {{ name: {}.to_string(), description: {}.to_string(), administrator: {}, check_security: {}, manage_users_without_roles: {}, manageable_roles: {}, module_roles: {} }},",
                rust_string(name),
                rust_string(description),
                role.get_bool("ManageAllRoles").unwrap_or(false),
                role.get_bool("CheckSecurity").unwrap_or(true),
                role.get_bool("ManageUsersWithoutRoles").unwrap_or(false),
                rust_string_vec(&manageable),
                rust_string_vec(&module_roles),
            );
        }
        source.push_str("        ],\n");
        let policy = document.get_document("PasswordPolicySettings").ok();
        let _ = writeln!(
            source,
            "        password_policy: ::mxrs_ir::PasswordPolicyDecl {{ minimum_length: {}, require_digit: {}, require_mixed_case: {}, require_symbol: {} }},",
            policy
                .and_then(|policy| bson_integer(policy.get("MinimumLength")))
                .unwrap_or(6),
            policy
                .and_then(|policy| policy.get_bool("RequireDigit").ok())
                .unwrap_or(true),
            policy
                .and_then(|policy| policy.get_bool("RequireMixedCase").ok())
                .unwrap_or(true),
            policy
                .and_then(|policy| policy.get_bool("RequireSymbol").ok())
                .unwrap_or(false),
        );
        source.push_str("    };\n");
        source.push_str("    project.security = Some(security);\n");
    }
    source.push_str("}\n");
    source
}

fn render_navigation_module(navigation: &mxrs_model::Navigation) -> String {
    let mut source = String::from(
        "//! Editable Cargo-native navigation profiles.\n\n\
         pub fn apply(project: &mut ::mxrs_ir::ProjectDecl) {\n\
         project.navigation = Some(::mxrs_ir::NavigationDecl { profiles: vec![\n",
    );
    let mut profiles = navigation.profiles.iter().collect::<Vec<_>>();
    profiles.sort_by(|left, right| left.name.cmp(&right.name));
    for profile in profiles {
        let _ = writeln!(
            source,
            "        ::mxrs_ir::NavigationProfileDecl {{ name: {}.to_string(), kind: {}.to_string(), app_title: {}, home_page: {}, home_microflow: {}, sign_in_page: {}, role_homes: vec![",
            rust_string(&profile.name),
            rust_string(&profile.kind),
            rust_btree_map(&profile.app_title),
            rust_option_string(profile.home_page.as_deref()),
            rust_option_string(profile.home_microflow.as_deref()),
            rust_option_string(profile.sign_in_page.as_deref()),
        );
        for home in &profile.role_homes {
            let _ = writeln!(
                source,
                "            ::mxrs_ir::RoleHomeDecl {{ user_role: {}.to_string(), page: {}, microflow: {} }},",
                rust_string(home.role.as_deref().unwrap_or_default()),
                rust_option_string(home.page.as_deref()),
                rust_option_string(home.microflow.as_deref()),
            );
        }
        source.push_str("        ], items: vec![\n");
        for item in &profile.menu_items {
            render_navigation_item(&mut source, item, 3);
        }
        source.push_str("        ] },\n");
    }
    source.push_str("    ] });\n}\n");
    source
}

fn render_navigation_item(
    source: &mut String,
    item: &mxrs_model::navigation::NavigationItem,
    depth: usize,
) {
    let indent = "    ".repeat(depth);
    let _ = writeln!(
        source,
        "{indent}::mxrs_ir::NavigationItemDecl {{ caption: {}, page: {}, microflow: {}, icon: {}, items: vec![",
        rust_btree_map(&item.caption),
        rust_option_string(item.page.as_deref()),
        rust_option_string(item.microflow.as_deref()),
        rust_option_navigation_icon(item.icon.as_ref()),
    );
    for child in &item.items {
        render_navigation_item(source, child, depth + 1);
    }
    let _ = writeln!(source, "{indent}] }},");
}

fn rust_btree_map(values: &std::collections::BTreeMap<String, String>) -> String {
    if values.is_empty() {
        return "::std::collections::BTreeMap::new()".to_string();
    }
    let entries = values
        .iter()
        .map(|(key, value)| {
            format!(
                "({}.to_string(), {}.to_string())",
                rust_string(key),
                rust_string(value)
            )
        })
        .collect::<Vec<_>>()
        .join(", ");
    format!("::std::collections::BTreeMap::from([{entries}])")
}

fn rust_option_navigation_icon(icon: Option<&mxrs_model::navigation::NavigationIcon>) -> String {
    match icon {
        Some(mxrs_model::navigation::NavigationIcon::Glyph(value)) => format!(
            "Some(::mxrs_ir::NavigationIconDecl::Glyph({}.to_string()))",
            rust_string(value)
        ),
        Some(mxrs_model::navigation::NavigationIcon::Code(value)) => {
            format!("Some(::mxrs_ir::NavigationIconDecl::Code({value}))")
        }
        None => "None".to_string(),
    }
}

fn bson_integer(value: Option<&mxrs_bson::Bson>) -> Option<i32> {
    match value {
        Some(mxrs_bson::Bson::Int32(value)) => Some(*value),
        Some(mxrs_bson::Bson::Int64(value)) => i32::try_from(*value).ok(),
        _ => None,
    }
}

fn render_security_level(value: Option<&str>) -> Option<&'static str> {
    match value? {
        "CheckNothing" => Some("CheckNothing"),
        "CheckFormsAndMicroflows" => Some("CheckFormsAndMicroflows"),
        "CheckEverything" => Some("CheckEverything"),
        _ => None,
    }
}

fn bson_documents(document: &mxrs_bson::Document, field: &str) -> Vec<mxrs_bson::Document> {
    let Some(mxrs_bson::Bson::Array(values)) = document.get(field) else {
        return vec![];
    };
    mxrs_bson::parse_array(Some(values))
        .items
        .into_iter()
        .filter_map(|value| match value {
            mxrs_bson::Bson::Document(document) => Some(document),
            _ => None,
        })
        .collect()
}

fn bson_strings(document: &mxrs_bson::Document, field: &str) -> Vec<String> {
    let Some(mxrs_bson::Bson::Array(values)) = document.get(field) else {
        return vec![];
    };
    mxrs_bson::parse_array(Some(values))
        .items
        .into_iter()
        .filter_map(|value| match value {
            mxrs_bson::Bson::String(value) => Some(value),
            _ => None,
        })
        .collect()
}

fn rust_option_string(value: Option<&str>) -> String {
    value
        .map(|value| format!("Some({}.to_string())", rust_string(value)))
        .unwrap_or_else(|| "None".to_string())
}

fn rust_string_vec(values: &[String]) -> String {
    let values = values
        .iter()
        .map(|value| format!("{}.to_string()", rust_string(value)))
        .collect::<Vec<_>>()
        .join(", ");
    format!("vec![{values}]")
}

/// Builds the public marker surface directly from the imported model. Flow
/// bodies may remain opaque while their names are still compile-time checked
/// by pages and future Cargo-native flow authoring.
fn marker_manifest(modules: &[Module]) -> mxrs_typegen::Manifest {
    use mxrs_typegen::{AssociationManifest, EntityManifest, ModuleManifest};

    let mut qualified_by_id = HashMap::<&str, String>::new();
    let mut qualified_entities = std::collections::HashSet::new();
    for module in modules {
        let module_name = module.name.as_deref().unwrap_or("Unnamed");
        for entity in module.entities() {
            if let (Some(id), Some(name)) = (entity.id.as_deref(), entity.name.as_deref()) {
                let qualified = format!("{module_name}.{name}");
                qualified_entities.insert(qualified.clone());
                qualified_by_id.insert(id, qualified);
            }
        }
    }

    let modules = modules
        .iter()
        .map(|module| {
            let mut associations_by_entity = HashMap::<&str, Vec<AssociationManifest>>::new();
            for association in module.associations() {
                let (Some(from), Some(name), Some(target)) = (
                    association.from_entity_id.as_deref(),
                    association.name.as_deref(),
                    association.to_entity_id.as_deref(),
                ) else {
                    continue;
                };
                let target = if target.contains('.') {
                    Some(target.to_string())
                } else {
                    qualified_by_id.get(target).cloned()
                };
                let Some(target) = target.filter(|target| qualified_entities.contains(target))
                else {
                    continue;
                };
                associations_by_entity
                    .entry(from)
                    .or_default()
                    .push(AssociationManifest {
                        name: name.to_string(),
                        target,
                        association_type: match association.association_type {
                            mxrs_model::association::AssociationType::Reference => {
                                "Reference".to_string()
                            }
                            mxrs_model::association::AssociationType::ReferenceSet => {
                                "ReferenceSet".to_string()
                            }
                        },
                    });
            }

            let mut entities = module
                .entities()
                .iter()
                .filter_map(|entity| {
                    let name = entity.name.clone()?;
                    let mut attributes = entity
                        .attributes
                        .iter()
                        .filter_map(|attribute| attribute.name.clone())
                        .collect::<Vec<_>>();
                    attributes.sort();
                    let mut associations = entity
                        .id
                        .as_deref()
                        .and_then(|id| associations_by_entity.remove(id))
                        .unwrap_or_default();
                    associations.sort_by(|left, right| left.name.cmp(&right.name));
                    associations.dedup_by(|left, right| left.name == right.name);
                    associations.retain(|association| !attributes.contains(&association.name));
                    Some(EntityManifest {
                        name,
                        attributes,
                        associations,
                    })
                })
                .collect::<Vec<_>>();
            entities.sort_by(|left, right| left.name.cmp(&right.name));

            let entity_names = entities
                .iter()
                .map(|entity| entity.name.as_str())
                .collect::<std::collections::HashSet<_>>();

            let mut microflows = module
                .microflows
                .iter()
                .filter_map(|flow| flow.name.clone())
                .collect::<Vec<_>>();
            microflows.sort();
            microflows.dedup();
            microflows.retain(|name| !entity_names.contains(name.as_str()));
            let mut nanoflows = module
                .nanoflows
                .iter()
                .filter_map(|flow| flow.name.clone())
                .collect::<Vec<_>>();
            nanoflows.sort();
            nanoflows.dedup();
            nanoflows.retain(|name| {
                !entity_names.contains(name.as_str()) && microflows.binary_search(name).is_err()
            });

            ModuleManifest {
                name: module.name.clone().unwrap_or_else(|| "Unnamed".to_string()),
                entities,
                microflows,
                nanoflows,
            }
        })
        .collect();
    mxrs_typegen::Manifest { modules }
}

fn cargo_manifest(package_name: &str, mxrs_workspace: Option<&Path>) -> String {
    let dependency = match mxrs_workspace {
        Some(workspace) => format!(
            "{{ path = {} }}",
            toml_string(&workspace.join("crates/app/mxrs").display().to_string())
        ),
        None => "{ git = \"https://github.com/lucamykael/mxrs\" }".to_string(),
    };
    format!(
        "[package]\nname = {package_name:?}\nversion = \"0.1.0\"\nedition = \"2024\"\npublish = false\n\n[dependencies]\nmxrs = {dependency}\n",
    )
}

fn build_binary_source(crate_name: &str, project_name: &str) -> String {
    let default_output = format!("build/{project_name}.mpr");
    format!(
        "fn main() -> Result<(), Box<dyn std::error::Error>> {{\n    let output = std::env::args().nth(1).unwrap_or_else(|| {}.to_string());\n    let root = std::path::Path::new(env!(\"CARGO_MANIFEST_DIR\"));\n    mxrs::replace_imported_project(root.join(\"model/imported\"), &output, &{crate_name}::Application::build())?;\n    mxrs::materialize_project_assets(root.join(\"assets\"), &output)?;\n    println!(\"built {{output}}\");\n    Ok(())\n}}\n",
        serde_json::to_string(&default_output).expect("a string always serializes"),
    )
}

fn generated_readme(project_name: &str, gaps: usize, page_export: &PageExportReport) -> String {
    let pages_note = if page_export.typed_candidates > 0 {
        format!(
            "\n`src/domain/pages/mod.rs` defines {} page(s) this import detected as buildable from\nmxrs-dsl's native/structural widget vocabulary (out of {} page(s) total) — wired into\n`build()` automatically by `src/domain/mod.rs`.\n",
            page_export.typed_candidates,
            page_export.typed_candidates + page_export.opaque,
        )
    } else {
        String::new()
    };
    format!(
        "# {project_name}\n\nCargo-native Mendix project imported by `mxrs`. Editable concepts live under `src/domain/`; generated public marker types live under `src/infrastructure/`. Lossless model data and stable identities stay outside the Rust source tree under `model/imported/`.\n\n`src/domain/documents/mod.rs` contains editable enumerations, constants, regular expressions, and scheduled events. `src/domain/flows/mod.rs` is the source of truth for Cargo-native microflows and nanoflows added after import. Existing graphs remain exact in the imported model data until they are redeclared.\n\n```sh\ncargo check\ncargo test\ncargo mxrs diff\ncargo mxrs build --output build/{project_name}.mpr\nmxrs portability build/{project_name}.mpr --verify-round-trip\n# The build also materializes the embedded React shell at build/web.\n\n# Explicit frontend customization (the normal build needs no Node):\ncargo mxrs frontend-dev --output frontend\nnpm ci --prefix frontend\nnpm run build --prefix frontend\n```\n\nThe typed domain export omitted {gaps} association target(s) that do not resolve inside this imported project; their original model data remains preserved. Run `mxrs portability` for the complete per-family typed/partial/preserved inventory.\n{pages_note}"
    )
}

fn cargo_package_name(project_name: &str) -> String {
    let mut name = project_name
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>();
    while name.contains("--") {
        name = name.replace("--", "-");
    }
    name = name.trim_matches('-').to_string();
    if name.is_empty() {
        name = "mendix-app".to_string();
    }
    if name.starts_with(|character: char| character.is_ascii_digit()) {
        name.insert_str(0, "app-");
    }
    name
}

fn toml_string(value: &str) -> String {
    rust_string(value)
}

fn rust_string(value: &str) -> String {
    serde_json::to_string(value).expect("a string always serializes")
}

fn write_text(path: &Path, contents: &str) -> Result<()> {
    std::fs::write(path, contents).map_err(|source| io_error(path, source))
}

fn format_generated_cargo_project(destination: &Path) -> Result<()> {
    let manifest = destination.join("Cargo.toml");
    let output = std::process::Command::new("cargo")
        .args(["fmt", "--manifest-path"])
        .arg(&manifest)
        .current_dir(destination)
        .output()
        .map_err(|source| io_error(&manifest, source))?;
    if output.status.success() {
        return Ok(());
    }

    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let detail = if !stderr.is_empty() {
        stderr
    } else if !stdout.is_empty() {
        stdout
    } else {
        output.status.to_string()
    };
    Err(ExportError::Formatting {
        path: destination.display().to_string(),
        detail,
    })
}

fn io_error(path: &Path, source: std::io::Error) -> ExportError {
    ExportError::Io {
        path: path.display().to_string(),
        source,
    }
}

fn round_trip_gaps(modules: &[Module]) -> Vec<RoundTripGap> {
    let entity_qualified_name_by_id = index_entities_by_id(modules);
    let mut gaps = Vec::new();
    for module in modules {
        let module_name = module.name.as_deref().unwrap_or("Unnamed");
        let Some(domain_model) = &module.domain_model else {
            continue;
        };
        for entity in &domain_model.entities {
            let entity_name = entity.name.as_deref().unwrap_or("Unnamed");
            for attribute in &entity.attributes {
                let attribute_name = attribute.name.as_deref().unwrap_or("Unnamed");
                let path = format!("{module_name}.{entity_name}.{attribute_name}");
                if project_attr_keyword(attribute.attribute_type).is_none() {
                    gaps.push(RoundTripGap {
                        path: path.clone(),
                        reason: format!(
                            "attribute type {:?} has no project! grammar",
                            attribute.attribute_type
                        ),
                    });
                }
                let _ = path;
            }
        }
        for association in domain_model.all_associations() {
            let name = association.name.as_deref().unwrap_or("Unnamed");
            let path = format!("{module_name}.{name}");
            let target_resolves = association.to_entity_id.as_deref().is_some_and(|target| {
                entity_qualified_name_by_id.contains_key(target)
                    || entity_qualified_name_by_id
                        .values()
                        .any(|qualified_name| qualified_name == target)
            });
            if !target_resolves {
                gaps.push(RoundTripGap {
                    path: path.clone(),
                    reason: "association target cannot be resolved".to_string(),
                });
            }
            let _ = path;
        }
    }
    gaps
}

fn render(
    mendix_version: &str,
    modules: &[Module],
    pages: &[page_export::ConvertedPage],
) -> String {
    let entity_qualified_name_by_id = index_entities_by_id(modules);

    let mut out = String::new();
    let _ = writeln!(
        out,
        "//! Generated by `mxrs-exporter` — edit freely, then write changes"
    );
    let _ = writeln!(
        out,
        "//! back with `mxrs_writer::synchronize_project(path, &build())`."
    );
    let _ = writeln!(out, "//! Editable domain model declarations.");
    let _ = writeln!(out, "//!");
    let _ = writeln!(
        out,
        "//! Depends on `mxrs-macros`, `mxrs-expr`, `mxrs-ir`, and `mxrs-dsl`"
    );
    let _ = writeln!(
        out,
        "//! (the `project! {{}}` macro's own expansion needs the latter two"
    );
    let _ = writeln!(out, "//! in scope — see `mxrs-macros`' crate doc).");
    if !pages.is_empty() {
        let _ = writeln!(out, "//!");
        let _ = writeln!(
            out,
            "//! Also wires every page `src/domain/pages/mod.rs` defines into its"
        );
        let _ = writeln!(out, "//! module — see `pages`' own header comment and");
        let _ = writeln!(out, "//! `mxrs-exporter::page_export`'s crate doc.");
    }
    out.push('\n');

    if pages.is_empty() {
        let _ = writeln!(out, "pub fn build() -> ::mxrs_ir::ProjectDecl {{");
        let _ = writeln!(out, "    ::mxrs_macros::project! {{");
        let _ = writeln!(out, "        {:?},", mendix_version);
        for module in modules {
            out.push_str(&render_module(module, &entity_qualified_name_by_id));
        }
        let _ = writeln!(out, "    }}");
        let _ = writeln!(out, "}}");
        return out;
    }

    let _ = writeln!(out, "pub mod pages;");
    out.push('\n');
    let _ = writeln!(out, "pub fn build() -> ::mxrs_ir::ProjectDecl {{");
    let _ = writeln!(out, "    let mut project = ::mxrs_macros::project! {{");
    let _ = writeln!(out, "        {:?},", mendix_version);
    for module in modules {
        out.push_str(&render_module(module, &entity_qualified_name_by_id));
    }
    let _ = writeln!(out, "    }};");
    for page in pages {
        let _ = writeln!(
            out,
            "    project.modules.iter_mut().find(|m| m.name == {:?}).expect(\"module {} exists\").pages.push(pages::{}());",
            page.module_name, page.module_name, page.function_name,
        );
    }
    let _ = writeln!(out, "    project");
    let _ = writeln!(out, "}}");
    out
}

fn index_entities_by_id(modules: &[Module]) -> HashMap<String, String> {
    let mut by_id = HashMap::new();
    for module in modules {
        for entity in module.entities() {
            if let (Some(id), Some(qualified_name)) = (&entity.id, &entity.qualified_name) {
                by_id.insert(id.clone(), qualified_name.clone());
            }
        }
    }
    by_id
}

fn render_module(module: &Module, entity_qualified_name_by_id: &HashMap<String, String>) -> String {
    let module_name = module.name.as_deref().unwrap_or("Unnamed");
    let mut out = String::new();
    let _ = writeln!(out, "        module {} {{", sanitize_ident(module_name));

    let Some(domain_model) = &module.domain_model else {
        let _ = writeln!(out, "        }}");
        return out;
    };
    let mut associations_by_entity_id: HashMap<&str, Vec<&Association>> = HashMap::new();
    for association in domain_model.all_associations() {
        if let Some(from_id) = association.from_entity_id.as_deref() {
            associations_by_entity_id
                .entry(from_id)
                .or_default()
                .push(association);
        }
    }

    let mut entities: Vec<&Entity> = module.entities().iter().collect();
    entities.sort_by(|a, b| a.name.cmp(&b.name));
    for entity in entities {
        out.push_str(&render_entity(
            entity,
            associations_by_entity_id
                .get(entity.id.as_deref().unwrap_or(""))
                .map(Vec::as_slice)
                .unwrap_or(&[]),
            entity_qualified_name_by_id,
        ));
    }
    let _ = writeln!(out, "        }}");
    out
}

fn render_entity(
    entity: &Entity,
    associations: &[&Association],
    entity_qualified_name_by_id: &HashMap<String, String>,
) -> String {
    let entity_name = entity.name.as_deref().unwrap_or("Unnamed");
    let mut out = String::new();
    let _ = writeln!(out, "            entity {} {{", sanitize_ident(entity_name));
    if !entity.documentation.is_empty() {
        let _ = writeln!(
            out,
            "                documentation {:?};",
            entity.documentation
        );
    }
    let _ = writeln!(out, "                persistable {};", entity.persistable);

    let mut attributes = entity.attributes.clone();
    attributes.sort_by(|a, b| a.name.cmp(&b.name));
    for attribute in &attributes {
        let attribute_name = attribute.name.as_deref().unwrap_or("Unnamed");
        match project_attr_keyword(attribute.attribute_type) {
            Some(keyword) => {
                let default = attribute
                    .default_value
                    .as_deref()
                    .map(|v| format!(" = {v:?}"))
                    .unwrap_or_default();
                let name = sanitize_ident(attribute_name);
                let declaration = if attribute.attribute_type == AttributeType::Enum {
                    let enumeration = attribute.enumeration.as_deref().unwrap_or("");
                    format!("{keyword} {name}({enumeration:?}){default}")
                } else {
                    format!("{keyword} {name}{default}")
                };
                let has_options = !attribute.documentation.is_empty()
                    || attribute.length.is_some()
                    || attribute.localize_date.is_some()
                    || attribute.required
                    || attribute.unique;
                if has_options {
                    let _ = writeln!(out, "                {declaration} {{");
                    if !attribute.documentation.is_empty() {
                        let _ = writeln!(
                            out,
                            "                    documentation {:?};",
                            attribute.documentation
                        );
                    }
                    if let Some(length) = attribute.length {
                        let _ = writeln!(out, "                    length {length};");
                    }
                    if let Some(localize_date) = attribute.localize_date {
                        let _ = writeln!(out, "                    localize_date {localize_date};");
                    }
                    if attribute.required {
                        let _ = writeln!(out, "                    required true;");
                    }
                    if attribute.unique {
                        let _ = writeln!(out, "                    unique true;");
                    }
                    let _ = writeln!(out, "                }}");
                } else {
                    let _ = writeln!(out, "                {declaration};");
                }
            }
            None => {
                let _ = writeln!(
                    out,
                    "                // TODO: attribute {attribute_name:?} has type {:?}, not supported by project! {{}}'s grammar yet",
                    attribute.attribute_type
                );
            }
        }
    }

    let mut sorted_associations = associations.to_vec();
    sorted_associations.sort_by_key(|a| a.name.clone());
    for association in sorted_associations {
        let assoc_name = association.name.as_deref().unwrap_or("Unnamed");
        let target = association.to_entity_id.as_deref().and_then(|id_or_name| {
            entity_qualified_name_by_id
                .get(id_or_name)
                .cloned()
                .or_else(|| {
                    entity_qualified_name_by_id
                        .values()
                        .find(|qualified_name| qualified_name.as_str() == id_or_name)
                        .cloned()
                })
        });
        let Some(target) = target else {
            continue;
        };
        let target_path = target
            .splitn(2, '.')
            .map(sanitize_ident)
            .collect::<Vec<_>>()
            .join("::");
        let assoc_type = match association.association_type {
            mxrs_model::association::AssociationType::Reference => "Reference",
            mxrs_model::association::AssociationType::ReferenceSet => "ReferenceSet",
        };
        let declaration = format!(
            "association {} -> {target_path} as {assoc_type}",
            sanitize_ident(assoc_name)
        );
        let has_options = association.owner != mxrs_model::association::Owner::Default
            || association.storage_format != mxrs_model::association::StorageFormat::Column
            || !association.documentation.is_empty();
        if has_options {
            let _ = writeln!(out, "                {declaration} {{");
            if association.owner != mxrs_model::association::Owner::Default {
                let _ = writeln!(out, "                    owner Both;");
            }
            if association.storage_format != mxrs_model::association::StorageFormat::Column {
                let _ = writeln!(out, "                    storage Table;");
            }
            if !association.documentation.is_empty() {
                let _ = writeln!(
                    out,
                    "                    documentation {:?};",
                    association.documentation
                );
            }
            let _ = writeln!(out, "                }}");
        } else {
            let _ = writeln!(out, "                {declaration};");
        }
    }

    let _ = writeln!(out, "            }}");
    out
}

fn project_attr_keyword(attribute_type: AttributeType) -> Option<&'static str> {
    match attribute_type {
        AttributeType::String => Some("string"),
        AttributeType::Integer => Some("integer"),
        AttributeType::Long => Some("long"),
        AttributeType::Float => Some("float"),
        AttributeType::Decimal => Some("decimal"),
        AttributeType::Boolean => Some("boolean"),
        AttributeType::DateTime => Some("datetime"),
        AttributeType::AutoNumber => Some("autonumber"),
        AttributeType::HashString => Some("hash_string"),
        AttributeType::Binary => Some("binary"),
        AttributeType::Enum => Some("enumeration"),
    }
}

/// A defensive, not exhaustive, Mendix-name → Rust-identifier sanitizer:
/// non-alphanumeric/underscore characters become `_`, and a leading digit
/// (or an empty name) gets an `_` prefix. Doesn't raw-identifier-escape
/// (`r#...`) a name that collides with a Rust keyword — Mendix module/
/// entity/attribute names are conventionally PascalCase and never do in
/// practice, so this is a known, narrow simplification, not a silent gap
/// with real-world consequences.
fn sanitize_ident(name: &str) -> String {
    let mut ident: String = name
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if ident.is_empty() || ident.chars().next().is_some_and(|c| c.is_ascii_digit()) {
        ident.insert(0, '_');
    }
    ident
}

#[cfg(test)]
mod tests {
    use std::process::Command;

    use super::*;

    #[test]
    fn cargo_package_names_are_valid_and_stable() {
        assert_eq!(cargo_package_name("My Mendix App"), "my-mendix-app");
        assert_eq!(cargo_package_name("  2026 / Orders  "), "app-2026-orders");
        assert_eq!(cargo_package_name("---"), "mendix-app");
    }

    #[test]
    fn sanitize_ident_replaces_invalid_characters() {
        assert_eq!(sanitize_ident("Order Total"), "Order_Total");
        assert_eq!(sanitize_ident("2FA"), "_2FA");
        assert_eq!(sanitize_ident(""), "_");
        assert_eq!(sanitize_ident("Order"), "Order");
    }

    #[test]
    fn regular_expression_projection_is_fail_closed_on_native_shape() {
        let complete = mxrs_bson::doc! {
            "$ID": "8c3f4c59-e0a4-4a5c-8e35-8643a6dfbd09",
            "$Type": "RegularExpressions$RegularExpression",
            "Documentation": "An order code",
            "Excluded": false,
            "ExportLevel": "Hidden",
            "Expression": "[A-Z]+",
            "Name": "OrderCode",
        };
        assert!(is_complete_regular_expression_document(&complete));

        let mut future_shape = complete.clone();
        future_shape.insert("FutureField", true);
        assert!(!is_complete_regular_expression_document(&future_shape));

        let mut missing_field = complete.clone();
        missing_field.remove("Documentation");
        assert!(!is_complete_regular_expression_document(&missing_field));

        let mut wrong_type = complete;
        wrong_type.insert("Excluded", "false");
        assert!(!is_complete_regular_expression_document(&wrong_type));
    }

    #[test]
    fn optionless_regular_expression_does_not_name_an_unused_parameter() {
        let mut source = String::new();
        render_editable_document_body(
            &mut source,
            EditableDocument::RegularExpression {
                module: "Sales".to_string(),
                name: "OrderCode".to_string(),
                documentation: String::new(),
                expression: "[A-Z]+".to_string(),
                excluded: false,
                export_level: "Hidden",
            },
        );
        assert!(source.contains("|_|"), "{source}");
        assert!(!source.contains("|regular_expression|"), "{source}");
    }

    #[test]
    fn project_attr_keyword_covers_all_model_attribute_kinds() {
        assert_eq!(project_attr_keyword(AttributeType::String), Some("string"));
        assert_eq!(
            project_attr_keyword(AttributeType::AutoNumber),
            Some("autonumber")
        );
        assert_eq!(project_attr_keyword(AttributeType::Float), Some("float"));
        assert_eq!(
            project_attr_keyword(AttributeType::HashString),
            Some("hash_string")
        );
        assert_eq!(project_attr_keyword(AttributeType::Binary), Some("binary"));
        assert_eq!(
            project_attr_keyword(AttributeType::Enum),
            Some("enumeration")
        );
    }

    fn entity(name: &str, qualified_name: &str, extra: mxrs_bson::Document) -> Entity {
        let mut d = mxrs_bson::doc! {
            "$ID": uuid::Uuid::new_v4().to_string(),
            "$QualifiedName": qualified_name,
            "name": name,
        };
        d.extend(extra);
        Entity::from_bson(&d)
    }

    fn bare_module(name: &str, entities: Vec<Entity>) -> Module {
        Module {
            id: uuid::Uuid::new_v4().to_string(),
            name: Some(name.to_string()),
            sort_index: None,
            from_app_store: false,
            app_store_guid: None,
            app_store_version: None,
            export_level: "Hidden".to_string(),
            domain_model: Some(mxrs_model::DomainModel {
                id: None,
                native_type: None,
                documentation: String::new(),
                entities,
                associations: vec![],
                cross_associations: vec![],
            }),
            pages: vec![],
            microflows: vec![],
            nanoflows: vec![],
            rules: vec![],
            menus: vec![],
            module_roles: vec![],
            artifact_units: vec![],
        }
    }

    #[test]
    fn renders_the_float_attribute_type() {
        let mut order = entity("Order", "Sales.Order", mxrs_bson::doc! {});
        order
            .attributes
            .push(Attribute::from_bson(&mxrs_bson::doc! {
                "$ID": uuid::Uuid::new_v4().to_string(),
                "name": "Score",
                "type": { "$Type": "DomainModels$FloatAttributeType" },
            }));
        let module = bare_module("Sales", vec![order]);

        let source = render_module(&module, &HashMap::new());
        assert!(!source.contains("TODO"));
        assert!(source.contains("float Score"));
    }

    #[test]
    fn safe_export_includes_attribute_metadata() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("Project.mpr");
        let mut builder = mxrs_dsl::ProjectBuilder::new("11.12.1");
        builder.module("Sales", |module| {
            module.entity("Order", |entity| {
                let number = entity.string("Number");
                number.documentation = "External order number".to_string();
                number.length = Some(80);
                number.required = true;
                number.unique = true;
                entity.datetime("SubmittedAt").localize_date = Some(false);
            });
        });
        mxrs_writer::write_project(&path, &builder.build()).unwrap();

        let project = Project::open(&path, true).unwrap();
        let module = project.modules().unwrap().remove(0);
        drop(project);
        let mut mpr = mxrs_mpr::MprFile::open(&path, false).unwrap();
        let domain_unit = mpr
            .units_by_containment("DomainModel")
            .unwrap()
            .into_iter()
            .find(|unit| unit.container_id == module.id)
            .unwrap();
        let mut domain_doc = mpr.parse_contents(&domain_unit).unwrap();
        let entities = domain_doc.get_array_mut("entities").unwrap();
        let order = entities
            .iter_mut()
            .find_map(|value| value.as_document_mut())
            .unwrap();
        order
            .get_array_mut("attributes")
            .unwrap()
            .push(mxrs_bson::Bson::Document(mxrs_bson::doc! {
                "$ID": uuid::Uuid::new_v4().to_string(),
                "$Type": "DomainModels$Attribute",
                "name": "Score",
                "documentation": "hand-authored docs",
                "type": { "$Type": "DomainModels$FloatAttributeType" },
            }));
        mpr.update_unit(&domain_unit.unit_id, domain_doc).unwrap();
        drop(mpr);

        let source = export_project(&path).unwrap();
        assert!(source.contains("float Score {"));
        assert!(source.contains("documentation \"hand-authored docs\";"));
    }

    #[test]
    fn renders_an_entity_with_attributes_and_a_same_module_association() {
        let mut customer = entity("Customer", "Sales.Customer", mxrs_bson::doc! {});
        customer
            .attributes
            .push(Attribute::from_bson(&mxrs_bson::doc! {
                "$ID": uuid::Uuid::new_v4().to_string(), "name": "Name",
                "type": { "$Type": "DomainModels$StringAttributeType" },
            }));
        let mut order = entity(
            "Order",
            "Sales.Order",
            mxrs_bson::doc! { "persistable": true },
        );
        order
            .attributes
            .push(Attribute::from_bson(&mxrs_bson::doc! {
                "$ID": uuid::Uuid::new_v4().to_string(), "name": "Number",
                "type": { "$Type": "DomainModels$StringAttributeType" },
            }));
        let association = Association::from_bson(&mxrs_bson::doc! {
            "$ID": uuid::Uuid::new_v4().to_string(),
            "Name": "Order_Customer",
            "ParentID": order.id.clone().unwrap(),
            "ChildID": customer.id.clone().unwrap(),
            "Type": "Reference",
        });
        let mut module = bare_module("Sales", vec![customer, order]);
        module
            .domain_model
            .as_mut()
            .unwrap()
            .associations
            .push(association);

        let entity_ids = index_entities_by_id(std::slice::from_ref(&module));
        let source = render_module(&module, &entity_ids);

        assert!(source.contains("entity Customer"));
        assert!(source.contains("entity Order"));
        assert!(source.contains("string Name"));
        assert!(source.contains("string Number"));
        assert!(source.contains("persistable true"));
        assert!(source.contains("association Order_Customer -> Sales::Customer as Reference"));
    }

    #[test]
    fn export_project_round_trips_a_real_written_project() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("Project.mpr");

        let mut builder = mxrs_dsl::ProjectBuilder::new("11.12.1");
        builder.module("Sales", |m| {
            m.entity("Customer", |e| {
                e.string("Name");
            });
            m.entity("Order", |e| {
                e.persistable(true);
                e.string("Number").default_value = Some("A-0".to_string());
                e.association::<sales_markers::Order_Order_Customer>();
            });
        });
        mxrs_writer::write_project(&path, &builder.build()).unwrap();

        let source = export_project(&path).unwrap();
        assert!(!source.contains("pub mod markers"));
        assert!(source.contains("string Number = \"A-0\""));
        assert!(source.contains("association Order_Customer -> Sales::Customer as Reference"));
        assert!(source.contains("::mxrs_macros::project!"));
    }

    #[test]
    fn imported_cargo_project_checks_and_builds_without_the_source_mpr() {
        let source_directory = tempfile::tempdir().unwrap();
        let source_path = source_directory.path().join("OrderManagement.mpr");
        let generated_directory = tempfile::tempdir().unwrap();
        let generated = generated_directory.path().join("order-management");
        let build_directory = tempfile::tempdir().unwrap();
        let output = build_directory.path().join("OrderManagement.mpr");
        let target = crate::nested_cargo::target_dir(build_directory.path().join("cargo-target"));
        let workspace = Path::new(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .find(|path| path.join("xtask/Cargo.toml").is_file())
            .expect("workspace root contains xtask/Cargo.toml");

        std::fs::create_dir_all(source_directory.path().join("theme/web")).unwrap();
        std::fs::write(
            source_directory.path().join("theme/web/main.css"),
            b"body { color: rebeccapurple; }",
        )
        .unwrap();

        let mut builder = mxrs_dsl::ProjectBuilder::new("11.12.1");
        builder.module("Sales", |module| {
            module.entity("Order", |entity| {
                let number = entity.string("Number");
                number.documentation = "External order number".to_string();
                number.length = Some(80);
                number.required = true;
                number.unique = true;
                entity.datetime("SubmittedAt").localize_date = Some(false);
            });
            module.enumeration("Status", |enumeration| {
                enumeration.documentation("Order lifecycle");
                enumeration.value("Open").captions = vec![
                    ("en_US".to_string(), "Open".to_string()),
                    ("pt_BR".to_string(), "Aberto".to_string()),
                ];
            });
            module.constant("MaximumOrders", |constant| {
                constant
                    .documentation("Limit")
                    .value_type(mxrs_ir::ConstantType::Integer)
                    .value("25")
                    .exposed_to_client(true);
            });
            module.constant("ApiToken", |constant| {
                constant.value("super-secret-value");
            });
            module.microflow("ACT_Ping", |_flow| {});
            module.microflow("ACT_GetOrder", |flow| {
                let order = flow.create_object(
                    "order",
                    mxrs_ir::Ref::<sales_markers::Order>::new(),
                    vec![],
                    false,
                );
                flow.return_value(order);
            });
            module.nanoflow("NF_Validate", |_flow| {});
            module.page("Home", |p| {
                p.layout("Atlas_Core.ApplicationLayout", "Main");
                p.container(|c| {
                    c.text("Welcome");
                    c.button("Close", |b| {
                        b.close_page();
                    });
                });
            });
            module.page("OrderDetail", |p| {
                p.layout("Atlas_Core.ApplicationLayout", "Main");
                p.data_view_from_microflow(
                    mxrs_ir::MicroflowRef::<sales_markers::ACT_GetOrder>::new(),
                    |view| {
                        view.name("orderView");
                        view.text_box_with::<sales_markers::Order_Number>(|input| {
                            input.name("numberInput");
                            input.class("form-control");
                        });
                        view.button("Validate", |button| {
                            button.name("validateButton");
                            button.call_nanoflow(
                                mxrs_ir::NanoflowRef::<sales_markers::NF_Validate>::new(),
                            );
                        });
                    },
                );
                p.data_grid_2(|widget| {
                    widget.name("ordersGrid");
                });
                p.gallery(|widget| {
                    widget.name("ordersGallery");
                });
                p.combo_box(|widget| {
                    widget.name("orderPicker");
                });
            });
        });
        builder.security(|security| {
            security
                .check_security(false)
                .password_policy(|policy| policy.minimum_length = 12);
        });
        builder.navigation(|navigation| {
            navigation.profile("Responsive", |profile| {
                profile
                    .home_microflow("Sales.ACT_Ping")
                    .item("Orders", |item| {
                        item.icon_code(57369).microflow("Sales.ACT_Ping");
                    });
            });
        });
        mxrs_writer::write_project(&source_path, &builder.build()).unwrap();

        let imported = import_cargo_project(&source_path, &generated, Some(workspace)).unwrap();
        assert!(imported.imported_units > 1);
        assert_eq!(imported.imported_assets, 1);
        assert_eq!(imported.page_export.typed_candidates, 2);
        assert!(generated.join("Cargo.toml").is_file());
        assert!(generated.join("mxrs.toml").is_file());
        assert!(generated.join("src/lib.rs").is_file());
        assert!(generated.join("src/domain/mod.rs").is_file());
        assert!(generated.join("src/domain/entities/mod.rs").is_file());
        assert!(generated.join("src/domain/documents/mod.rs").is_file());
        assert!(generated.join("src/domain/flows/mod.rs").is_file());
        assert!(generated.join("src/domain/security/mod.rs").is_file());
        assert!(generated.join("src/domain/navigation/mod.rs").is_file());
        assert!(generated.join("src/infrastructure/mod.rs").is_file());
        assert!(!generated.join("src/generated").exists());
        assert!(generated.join("model/imported/manifest.json").is_file());
        // The pages module is real, *compiled* Rust source, wired into
        // `build()` from `src/domain/mod.rs` (see `page_export`'s doc
        // comment) — the `cargo check`/`cargo run` calls below, plus the
        // rebuilt-project assertions further down, prove it actually
        // contributes to the output `.mpr`, not just that it parses.
        let pages_source =
            std::fs::read_to_string(generated.join("src/domain/pages/mod.rs")).unwrap();
        assert!(pages_source.contains("pub fn home"));
        assert!(pages_source.contains("w.text_with(\"Welcome\""));
        assert!(pages_source.contains("w.name("));
        assert!(pages_source.contains("b.close_page()"));
        assert!(pages_source.contains("pub fn order_detail"));
        assert!(pages_source.contains("data_view_from_microflow"));
        assert!(pages_source.contains("Order_Number"));
        assert!(pages_source.contains("b.call_nanoflow"));
        assert!(pages_source.contains("p.data_grid_2"));
        assert!(pages_source.contains("p.gallery"));
        assert!(pages_source.contains("p.combo_box"));
        let domain_source = std::fs::read_to_string(generated.join("src/domain/mod.rs")).unwrap();
        assert!(domain_source.contains("pub mod entities;"));
        assert!(domain_source.contains("pub mod documents;"));
        assert!(domain_source.contains("pub mod flows;"));
        assert!(domain_source.contains("pub mod security;"));
        assert!(domain_source.contains("pub mod navigation;"));
        assert!(domain_source.contains("flows::apply(&mut project);"));
        assert!(domain_source.contains("documents::apply(&mut project);"));
        assert!(domain_source.contains("security::apply(&mut project);"));
        assert!(domain_source.contains("navigation::apply(&mut project);"));
        assert!(domain_source.contains("pub mod pages;"));
        assert!(domain_source.contains("pages::home()"));
        let entities_source =
            std::fs::read_to_string(generated.join("src/domain/entities/mod.rs")).unwrap();
        assert!(
            entities_source.contains("documentation \"External order number\";"),
            "{entities_source}"
        );
        assert!(entities_source.contains("length 80;"));
        assert!(entities_source.contains("required true;"));
        assert!(entities_source.contains("unique true;"));
        assert!(entities_source.contains("localize_date false;"));
        let documents =
            std::fs::read_to_string(generated.join("src/domain/documents/mod.rs")).unwrap();
        assert!(documents.contains("module.enumeration(\"Status\""));
        assert!(documents.contains("(\"pt_BR\".to_string(), \"Aberto\".to_string())"));
        assert!(documents.contains("module.constant(\"MaximumOrders\""));
        assert!(documents.contains("ConstantType::Integer"));
        assert!(documents.contains("constant.exposed_to_client(true)"));
        assert!(documents.contains("constant.value_from_env(\"MXRS_SALES_APITOKEN\")"));
        assert!(!documents.contains("super-secret-value"));
        assert!(!generated.join("src/infrastructure/ids.rs").exists());
        assert!(!generated.join("src/infrastructure/imported.rs").exists());
        let markers =
            std::fs::read_to_string(generated.join("src/infrastructure/markers.rs")).unwrap();
        assert!(markers.contains("pub struct Order;"));
        assert!(markers.contains("pub struct Order_Number;"));
        assert!(markers.contains("impl mxrs_ir::MicroflowMarker for ACT_Ping"));
        assert!(markers.contains("impl mxrs_ir::MicroflowMarker for ACT_GetOrder"));
        assert!(markers.contains("impl mxrs_ir::NanoflowMarker for NF_Validate"));
        let flows = std::fs::read_to_string(generated.join("src/domain/flows/mod.rs")).unwrap();
        assert!(!flows.contains("snapshot-backed"));
        assert!(flows.contains("target.nanoflows.extend(declared.nanoflows)"));
        let security =
            std::fs::read_to_string(generated.join("src/domain/security/mod.rs")).unwrap();
        assert!(security.contains("ProjectSecurityDecl {"));
        assert!(security.contains("check_security: false,"));
        assert!(security.contains("minimum_length: 12"));
        assert!(security.contains("user_roles: vec!"));
        let navigation =
            std::fs::read_to_string(generated.join("src/domain/navigation/mod.rs")).unwrap();
        assert!(navigation.contains("project.navigation = Some"));
        assert!(navigation.contains("NavigationIconDecl::Code(57369)"));
        let crate_root = std::fs::read_to_string(generated.join("src/lib.rs")).unwrap();
        assert!(crate_root.contains("pub mod infrastructure;"));
        assert!(!crate_root.contains("pub mod generated"));

        let format_check = Command::new("cargo")
            .args(["fmt", "--manifest-path"])
            .arg(generated.join("Cargo.toml"))
            .args(["--", "--check"])
            .status()
            .unwrap();
        assert!(format_check.success());

        std::fs::remove_file(&source_path).unwrap();
        std::fs::remove_dir_all(mxrs_mpr::format::contents_dir(&source_path)).unwrap();

        let check = Command::new("cargo")
            .args(["check", "--quiet", "--offline", "--manifest-path"])
            .arg(generated.join("Cargo.toml"))
            .env("CARGO_TARGET_DIR", &target)
            .status()
            .unwrap();
        assert!(check.success());

        let build = Command::new("cargo")
            .args(["run", "--quiet", "--offline", "--manifest-path"])
            .arg(generated.join("Cargo.toml"))
            .arg("--")
            .arg(&output)
            .env("CARGO_TARGET_DIR", &target)
            .status()
            .unwrap();
        assert!(build.success());

        let rebuilt = mxrs_mpr::MprFile::open(&output, true).unwrap();
        let documents = rebuilt
            .all_units()
            .unwrap()
            .into_iter()
            .map(|unit| rebuilt.parse_contents(&unit).unwrap())
            .collect::<Vec<_>>();
        assert!(documents.iter().any(|document| {
            document.get_str("$Type").ok() == Some("Microflows$Microflow")
                && document.get_str("Name").ok() == Some("ACT_Ping")
        }));
        assert!(documents.iter().any(|document| {
            document.get_str("$Type").ok() == Some("Constants$Constant")
                && document.get_str("Name").ok() == Some("ApiToken")
                && document.get_str("DefaultValue").ok() == Some("super-secret-value")
        }));
        let security = documents
            .iter()
            .find(|document| document.get_str("$Type").ok() == Some("Security$ProjectSecurity"))
            .unwrap();
        assert!(!security.get_bool("CheckSecurity").unwrap());
        assert_eq!(
            security
                .get_document("PasswordPolicySettings")
                .unwrap()
                .get_i64("MinimumLength")
                .unwrap(),
            12
        );
        assert!(documents.iter().any(|document| {
            document.get_str("$Type").ok() == Some("Enumerations$Enumeration")
                && document.get_str("Name").ok() == Some("Status")
        }));
        assert!(documents.iter().any(|document| {
            document.get_str("$Type").ok() == Some("Constants$Constant")
                && document.get_str("Name").ok() == Some("MaximumOrders")
                && document.get_str("DefaultValue").ok() == Some("25")
        }));
        drop(rebuilt);
        let rebuilt = Project::open(&output, true).unwrap();
        let modules = rebuilt.modules().unwrap();
        let order = &modules[0].entities()[0];
        let number = order
            .attributes
            .iter()
            .find(|attribute| attribute.name.as_deref() == Some("Number"))
            .unwrap();
        assert_eq!(number.documentation, "External order number");
        assert_eq!(number.length, Some(80));
        assert!(number.required);
        assert!(number.unique);
        let submitted_at = order
            .attributes
            .iter()
            .find(|attribute| attribute.name.as_deref() == Some("SubmittedAt"))
            .unwrap();
        assert_eq!(submitted_at.localize_date, Some(false));
        // Proves `pages::home()` actually got pushed into the rebuilt
        // `.mpr`, not just that the generated crate compiles and runs.
        let page = modules[0]
            .pages
            .iter()
            .find(|page| page.name.as_deref() == Some("Home"))
            .expect("the Home page detected on import round-trips through the rebuilt project");
        assert_eq!(page.widgets.len(), 1);
        let container = &page.widgets[0];
        assert_eq!(container.widget_type, "container");
        assert_eq!(container.children.len(), 2);
        assert_eq!(container.children[0].widget_type, "text");
        assert_eq!(container.children[1].widget_type, "button");
        let detail = modules[0]
            .pages
            .iter()
            .find(|page| page.name.as_deref() == Some("OrderDetail"))
            .expect("the advanced page round-trips through generated Rust");
        assert_eq!(detail.widgets.len(), 4);
        assert_eq!(detail.widgets[0].widget_type, "data_view");
        assert_eq!(detail.widgets[0].children[0].widget_type, "text_box");
        assert_eq!(detail.widgets[0].children[1].widget_type, "button");
        assert_eq!(detail.widgets[1].widget_type, "pluggable");
        assert_eq!(
            detail.widgets[1].options.get_str("widget_id").unwrap(),
            "com.mendix.widget.web.datagrid.Datagrid"
        );
        assert_eq!(
            std::fs::read(build_directory.path().join("theme/web/main.css")).unwrap(),
            b"body { color: rebeccapurple; }"
        );
    }

    #[allow(dead_code, non_snake_case, non_camel_case_types)]
    mod sales_markers {
        pub struct Customer;
        impl mxrs_ir::EntityMarker for Customer {
            const MODULE: &'static str = "Sales";
            const NAME: &'static str = "Customer";
        }
        pub struct Order;
        impl mxrs_ir::EntityMarker for Order {
            const MODULE: &'static str = "Sales";
            const NAME: &'static str = "Order";
        }
        pub struct Order_Number;
        impl mxrs_ir::AttributeMarker for Order_Number {
            type Entity = Order;
            const NAME: &'static str = "Number";
        }
        pub struct ACT_GetOrder;
        impl mxrs_ir::MicroflowMarker for ACT_GetOrder {
            const MODULE: &'static str = "Sales";
            const NAME: &'static str = "ACT_GetOrder";
        }
        pub struct NF_Validate;
        impl mxrs_ir::NanoflowMarker for NF_Validate {
            const MODULE: &'static str = "Sales";
            const NAME: &'static str = "NF_Validate";
        }
        pub struct Order_Order_Customer;
        impl mxrs_ir::AssociationMarker for Order_Order_Customer {
            type From = Order;
            type To = Customer;
            const NAME: &'static str = "Order_Customer";
            const ASSOCIATION_TYPE: mxrs_ir::AssociationType = mxrs_ir::AssociationType::Reference;
        }
    }
}
