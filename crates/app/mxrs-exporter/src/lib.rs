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
//!
//! **Current typed projection, explicit about what's outside it**:
//!
//! - **Domain model** — entities, attributes, associations.
//! - **Documents** — enumerations (values, localized captions and
//!   documentation), constants (type, value, documentation and client
//!   exposure), regular expressions (pattern, visibility and exclusion), and
//!   scheduled events (legacy cadence, modern schedule, start instant and
//!   execution policy), and standalone menus are emitted into
//!   `src/domain/documents/mod.rs`. Imported
//!   fields outside that IR are retained by the writer. `portability
//!   --verify-round-trip` checks these documents by identity, containment
//!   and raw BSON bytes.
//! - **Cargo flow bodies** — attested linear microflows and nanoflows become
//!   typed builders for parameters, microflow calls, list creation, object
//!   create/change, commit/delete and returns. Supported expressions are typed
//!   variable references, direct attribute reads and canonical scalar literals. Unedited rebuilds preserve native bytes;
//!   edits with matching linear structure retain node identities and layout.
//!   Other graphs remain in the imported model. Flow headers/layout still
//!   depend on that model, so portability reports a partial projection.
//!   Standalone `export_project` retains its domain/document scope.
//! - **All eleven attribute types are represented** by `project! {}`:
//!   string/integer/long/float/decimal/boolean/datetime/autonumber,
//!   hash-string, binary, and enumeration. Attribute documentation, string
//!   length, date localization, required and unique validation are emitted
//!   too.
//! - **Association `Owner`/`StorageFormat`/`Documentation` round-trip** via
//!   typed options in `project!`.
//! - **Entity structure and behavior** — images, stored/OQL source kind,
//!   access rules, generalization, system members, indexes, and lifecycle
//!   callbacks are emitted through typed declarations when references resolve.
//!
//! Marker types are emitted by `project!` from these same entity
//! declarations, so exported source is self-contained without a second
//! schema manifest or a generated marker module.
//!
//! `import_cargo_project` also detects pages built entirely from
//! `mxrs-dsl`'s native/structural widget vocabulary, emits them as real
//! `pub fn` builders in `src/presentation/pages/mod.rs`, and wires each one into
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

mod flow_export;
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

/// HTTP adapter emitted for an imported Cargo project.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ApiMode {
    #[default]
    Axum,
    ActixWeb,
    Rocket,
}

impl ApiMode {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "axum" => Some(Self::Axum),
            "actix-web" | "actix" => Some(Self::ActixWeb),
            "rocket" => Some(Self::Rocket),
            _ => None,
        }
    }

    pub const fn name(self) -> &'static str {
        match self {
            Self::Axum => "axum",
            Self::ActixWeb => "actix-web",
            Self::Rocket => "rocket",
        }
    }
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

    #[error("cannot split generated marker modules: {0}")]
    MarkerLayout(String),

    #[error("refusing an incomplete Rust export; {0} model feature(s) need typed support")]
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
            | ExportError::MarkerLayout(_)
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
    /// widget vocabulary, emitted into `src/presentation/pages/mod.rs` and wired into
    /// `build()` — see `page_export`'s doc comment for what still stays
    /// opaque.
    pub page_export: PageExportReport,
}

pub type Result<T> = std::result::Result<T, ExportError>;

/// Exports only when the generated source can be synchronized back without
/// erasing a feature the typed grammar cannot express.
pub fn export_project(path: impl AsRef<Path>) -> Result<String> {
    let project = Project::open(path, true)?;
    let mendix_version = project.mendix_version()?.unwrap_or_default();
    let mut modules = project.modules()?;
    modules.sort_by(|a, b| a.name.cmp(&b.name));
    let gaps = round_trip_gaps(&modules);
    if !gaps.is_empty() {
        return Err(ExportError::Lossy(gaps.len(), gaps));
    }
    let domain = render(&mendix_version, &modules, &[]);
    let queues = render_task_queues_module(&project, &mendix_version)?;
    Ok(format!(
        "mod domain {{\n{domain}\n}}\nmod task_queues {{\n{queues}\n}}\npub fn build() -> ::mxrs_ir::ProjectDecl {{\n    let mut project = domain::build();\n    task_queues::apply(&mut project);\n    project\n}}\n"
    ))
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
    import_cargo_project_with_mode(mpr_path, destination, mxrs_workspace, ApiMode::default())
}

/// Same as [`import_cargo_project`], with an explicit HTTP adapter layout.
pub fn import_cargo_project_with_mode(
    mpr_path: impl AsRef<Path>,
    destination: impl AsRef<Path>,
    mxrs_workspace: Option<&Path>,
    api_mode: ApiMode,
) -> Result<CargoProjectImport> {
    let destination = destination.as_ref();
    if destination.exists() {
        return Err(ExportError::DestinationExists(
            destination.display().to_string(),
        ));
    }
    std::fs::create_dir_all(destination).map_err(|source| io_error(destination, source))?;
    let result =
        import_cargo_project_inner(mpr_path.as_ref(), destination, mxrs_workspace, api_mode);
    if result.is_err() {
        let _ = std::fs::remove_dir_all(destination);
    }
    result
}

fn import_cargo_project_inner(
    mpr_path: &Path,
    destination: &Path,
    mxrs_workspace: Option<&Path>,
    api_mode: ApiMode,
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
    let domain_source = render_domain_module();
    let application_source = render_application_module();
    let presentation_source = render_presentation_module(&converted_pages, api_mode);
    let composition_source = render_composition_module(&mendix_version);
    let infrastructure_source = render_infrastructure_module();
    let persistence_source = render_persistence_module(&modules, &mendix_version);
    let converted_flows = flow_export::collect(&project, &modules)?;
    let microflow_files = flow_export::render_files(&converted_flows, false);
    let microflows_source = if !microflow_files.is_empty() {
        render_flow_layer_index(&microflow_files)
    } else {
        render_microflows_module(&mendix_version, &modules)
    };
    let nanoflow_files = flow_export::render_files(&converted_flows, true);
    let nanoflows_source = if !nanoflow_files.is_empty() {
        render_flow_layer_index(&nanoflow_files)
    } else {
        render_nanoflows_module(&mendix_version, &modules)
    };
    let security_document = project.all_units()?.into_iter().find_map(|unit| {
        let document = project.mpr().parse_contents(&unit).ok()?;
        (document.get_str("$Type").ok() == Some("Security$ProjectSecurity")).then_some(document)
    });
    let security_source = render_security_module(&modules, security_document.as_ref());
    let navigation_source = render_navigation_module(&project.navigation()?);
    let documents_export = render_documents_module(&project)?;
    let task_queues_source = render_task_queues_module(&project, &mendix_version)?;
    let marker_manifest = marker_manifest(&modules);
    let markers_source = mxrs_typegen::generate(&marker_manifest)?;
    let typed_markers_source = flow_export::typed_attribute_markers(&modules);
    let action_documents = collect_action_documents(&project)?;
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
    let domain_modules_directory = domain_directory.join("modules");
    std::fs::create_dir_all(&domain_modules_directory)
        .map_err(|source| io_error(&domain_modules_directory, source))?;
    let application_directory = destination.join("src/application");
    let dto_directory = application_directory.join("dto");
    std::fs::create_dir_all(&dto_directory).map_err(|source| io_error(&dto_directory, source))?;
    let services_directory = application_directory.join("services");
    std::fs::create_dir_all(&services_directory)
        .map_err(|source| io_error(&services_directory, source))?;
    let application_modules_directory = application_directory.join("modules");
    std::fs::create_dir_all(&application_modules_directory)
        .map_err(|source| io_error(&application_modules_directory, source))?;
    let presentation_directory = destination.join("src/presentation");
    let controllers_directory = presentation_directory.join("controllers");
    std::fs::create_dir_all(&controllers_directory)
        .map_err(|source| io_error(&controllers_directory, source))?;
    let routes_directory = presentation_directory.join("routes");
    std::fs::create_dir_all(&routes_directory)
        .map_err(|source| io_error(&routes_directory, source))?;
    let task_queues_directory = application_directory.join("task_queues");
    std::fs::create_dir_all(&task_queues_directory)
        .map_err(|source| io_error(&task_queues_directory, source))?;
    let nanoflows_directory = presentation_directory.join("nanoflows");
    std::fs::create_dir_all(&nanoflows_directory)
        .map_err(|source| io_error(&nanoflows_directory, source))?;
    let navigation_directory = presentation_directory.join("navigation");
    std::fs::create_dir_all(&navigation_directory)
        .map_err(|source| io_error(&navigation_directory, source))?;
    let presentation_modules_directory = presentation_directory.join("modules");
    std::fs::create_dir_all(&presentation_modules_directory)
        .map_err(|source| io_error(&presentation_modules_directory, source))?;
    let security_directory = domain_directory.join("security");
    std::fs::create_dir_all(&security_directory)
        .map_err(|source| io_error(&security_directory, source))?;
    let documents_directory = domain_directory.join("documents");
    std::fs::create_dir_all(&documents_directory)
        .map_err(|source| io_error(&documents_directory, source))?;
    let enumerations_directory = domain_directory.join("enumerations");
    std::fs::create_dir_all(&enumerations_directory)
        .map_err(|source| io_error(&enumerations_directory, source))?;
    for directory in [
        domain_directory.join("services"),
        domain_directory.join("ports"),
        infrastructure_directory.join("repositories"),
        infrastructure_directory.join("database"),
        infrastructure_directory.join("adapters"),
        infrastructure_directory.join("persistence"),
        destination.join("src/utils"),
    ] {
        std::fs::create_dir_all(&directory).map_err(|source| io_error(&directory, source))?;
    }

    let typed_entities = write_entity_layer_sources(
        destination,
        &modules,
        &documents_export.derived_enumerations,
    )?;
    let service_ports = collect_service_ports(&modules, &typed_entities);
    let action_ports = assemble_action_ports(&action_documents, &typed_entities);

    write_text(
        &destination.join("Cargo.toml"),
        &cargo_manifest(&package_name, mxrs_workspace, api_mode),
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
            "// Internal aliases keep generated authoring declarations concise.\nextern crate mxrs as mxrs_dsl;\nextern crate mxrs as mxrs_expr;\nextern crate mxrs as mxrs_ir;\nextern crate mxrs as mxrs_macros;\n\npub mod application;\npub mod composition;\npub mod domain;\npub mod infrastructure;\npub mod presentation;\npub mod utils;\n\npub fn build() -> ::mxrs_ir::ProjectDecl {{\n    composition::build()\n}}\n\n#[mxrs::application(version = {}, project = crate::build)]\npub struct Application;\n",
            rust_string(&manifest.mendix_version),
        ),
    )?;
    write_text(&destination.join("src/domain/mod.rs"), &domain_source)?;
    write_text(&destination.join("src/composition.rs"), &composition_source)?;
    write_text(
        &destination.join("src/domain/modules/mod.rs"),
        &render_scaffold_modules_module(),
    )?;
    write_text(
        &destination.join("src/domain/services/mod.rs"),
        "//! Pure domain services belong here.\n",
    )?;
    write_text(
        &destination.join("src/domain/ports/mod.rs"),
        &render_ports_index(&service_ports, &action_ports),
    )?;
    for port in &service_ports {
        write_text(
            &destination.join(format!("src/domain/ports/{}.rs", port.file_stem)),
            &render_service_port(port),
        )?;
    }
    for module in &action_ports {
        write_text(
            &destination.join(format!("src/domain/ports/{}.rs", module.file_stem)),
            &render_action_port_file(module),
        )?;
    }
    write_text(
        &destination.join("src/application/mod.rs"),
        &application_source,
    )?;
    write_text(
        &destination.join("src/presentation/mod.rs"),
        &presentation_source,
    )?;
    write_text(
        &destination.join("src/application/services/mod.rs"),
        &microflows_source,
    )?;
    for flow in &microflow_files {
        write_text(
            &destination.join(format!("src/application/services/{}.rs", flow.file_name)),
            &flow.source,
        )?;
    }
    write_text(
        &destination.join("src/application/task_queues/mod.rs"),
        &task_queues_source,
    )?;
    write_text(
        &destination.join("src/application/modules/mod.rs"),
        &render_scaffold_modules_module(),
    )?;
    write_text(
        &destination.join("src/presentation/nanoflows/mod.rs"),
        &nanoflows_source,
    )?;
    for flow in &nanoflow_files {
        write_text(
            &destination.join(format!("src/presentation/nanoflows/{}.rs", flow.file_name)),
            &flow.source,
        )?;
    }
    write_text(
        &destination.join("src/domain/security/mod.rs"),
        &security_source,
    )?;
    write_text(
        &destination.join("src/presentation/navigation/mod.rs"),
        &navigation_source,
    )?;
    write_text(
        &destination.join("src/presentation/modules/mod.rs"),
        &render_scaffold_modules_module(),
    )?;
    write_text(
        &destination.join("src/presentation/controllers/mod.rs"),
        "//! Framework-neutral presentation controllers.\n\npub fn apply(_project: &mut ::mxrs_ir::ProjectDecl) {}\n",
    )?;
    write_text(
        &destination.join("src/presentation/routes/mod.rs"),
        &render_routes_module(api_mode),
    )?;
    let mut document_entries = Vec::new();
    for (stem, source, qualified) in &documents_export.module_files {
        write_text(
            &destination.join(format!("src/domain/documents/{stem}.rs")),
            source,
        )?;
        document_entries.push((stem.clone(), qualified.clone()));
    }
    write_text(
        &destination.join("src/domain/documents/mod.rs"),
        &render_entity_layer_index(&document_entries),
    )?;
    let mut enumeration_entries = Vec::new();
    for (stem, source, qualified) in &documents_export.enumeration_files {
        write_text(
            &destination.join(format!("src/domain/enumerations/{stem}.rs")),
            source,
        )?;
        enumeration_entries.push((stem.clone(), qualified.clone()));
    }
    write_text(
        &destination.join("src/domain/enumerations/mod.rs"),
        &render_entity_layer_index(&enumeration_entries),
    )?;
    if let Some(pages_module_source) = &pages_module_source {
        let pages_directory = presentation_directory.join("pages");
        std::fs::create_dir_all(&pages_directory)
            .map_err(|source| io_error(&pages_directory, source))?;
        write_text(
            &destination.join("src/presentation/pages/mod.rs"),
            pages_module_source,
        )?;
    }
    write_text(
        &destination.join("src/main.rs"),
        &build_binary_source(&crate_name, &manifest.project_name, api_mode),
    )?;
    write_text(
        &destination.join("src/infrastructure/mod.rs"),
        &infrastructure_source,
    )?;
    write_text(
        &destination.join("src/infrastructure/persistence/mod.rs"),
        &persistence_source,
    )?;
    write_text(
        &destination.join("src/infrastructure/repositories/mod.rs"),
        "//! Implement `domain::ports` against the selected database here.\n",
    )?;
    write_text(
        &destination.join("src/infrastructure/database/mod.rs"),
        "//! Connection pools, migrations and database adapters belong here.\n",
    )?;
    let mut adapters_index =
        String::from("//! External services and Mendix-specific adapters belong here.\n");
    if !service_ports.is_empty() || !action_ports.is_empty() {
        adapters_index.push('\n');
    }
    for module in &action_ports {
        let _ = writeln!(adapters_index, "pub mod {};", module.file_stem);
    }
    if !service_ports.is_empty() {
        adapters_index.push_str("pub mod runtime_services;\n");
    }
    write_text(
        &destination.join("src/infrastructure/adapters/mod.rs"),
        &adapters_index,
    )?;
    if !service_ports.is_empty() {
        write_text(
            &destination.join("src/infrastructure/adapters/runtime_services.rs"),
            &render_runtime_services(&service_ports),
        )?;
    }
    for module in &action_ports {
        write_text(
            &destination.join(format!(
                "src/infrastructure/adapters/{}.rs",
                module.file_stem
            )),
            &render_action_registry_file(module, &module.file_stem),
        )?;
    }
    write_text(
        &destination.join("src/utils/mod.rs"),
        "//! Small framework-independent utilities.\n",
    )?;
    write_marker_layer_sources(
        destination,
        &marker_manifest,
        &markers_source,
        &typed_markers_source,
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

/// Builds the domain layer. Composition is owned by `src/composition.rs`, so
/// this layer never depends outward on application, infrastructure or UI.
fn render_domain_module() -> String {
    "//! Business entities, domain services and ports.\n\n\
     pub mod documents;\n\
     pub mod entities;\n\
     pub mod enumerations;\n\
     pub mod modules;\n\
     pub mod ports;\n\
     pub mod security;\n\n\
     pub mod services;\n\n\
     pub fn apply(project: &mut ::mxrs_ir::ProjectDecl) {\n\
         entities::apply(project);\n\
         enumerations::apply(project);\n\
         documents::apply(project);\n\
         modules::apply(project);\n\
         security::apply(project);\n\
     }\n"
    .to_string()
}

fn render_application_module() -> String {
    "//! Application use cases and boundary DTOs.\n\n\
     pub mod dto;\n\
     pub mod task_queues;\n\
     pub mod modules;\n\n\
     pub mod services;\n\n\
     pub fn apply(project: &mut ::mxrs_ir::ProjectDecl) {\n\
         dto::apply(project);\n\
         services::apply(project);\n\
         task_queues::apply(project);\n\
         modules::apply(project);\n\
     }\n"
    .to_string()
}

fn render_presentation_module(pages: &[page_export::ConvertedPage], api_mode: ApiMode) -> String {
    let mut source = String::from(
        "//! Presentation controllers, routes and client-side UI behavior.\n\n\
         pub mod controllers;\n\
         pub mod nanoflows;\n\
         pub mod navigation;\n",
    );
    source.push_str("pub mod modules;\npub mod routes;\n");
    if !pages.is_empty() {
        source.push_str("pub mod pages;\n");
    }
    source.push_str("\npub fn apply(project: &mut ::mxrs_ir::ProjectDecl) {\n");
    for page in pages {
        let _ = writeln!(
            source,
            // `module_mut` is the find-or-create accessor, so this stays one
            // total expression: open-coding it as a conditional push followed
            // by `find(..).expect(..)` puts a panic into generated product
            // code for an invariant the reader cannot check locally.
            "    project.module_mut({:?}).pages.push(pages::{}());",
            page.module_name, page.function_name,
        );
    }
    source.push_str(
        "    controllers::apply(project);\n    nanoflows::apply(project);\n    routes::apply(project);\n    modules::apply(project);\n}\n",
    );
    let _ = writeln!(
        source,
        "\n// HTTP adapter selected during import: `{}`.",
        api_mode.name()
    );
    source
}

fn render_composition_module(mendix_version: &str) -> String {
    format!(
        "//! The only layer allowed to compose Clean Architecture dependencies.\n\npub fn build() -> ::mxrs_ir::ProjectDecl {{\n    let mut project = ::mxrs::ProjectBuilder::new({}).build();\n    crate::domain::apply(&mut project);\n    crate::application::apply(&mut project);\n    crate::infrastructure::apply(&mut project);\n    crate::presentation::apply(&mut project);\n    project\n}}\n",
        rust_string(mendix_version)
    )
}

fn render_infrastructure_module() -> String {
    "//! Database, repository and Mendix adapter implementations.\n\npub mod adapters;\npub mod database;\npub mod markers;\npub mod persistence;\npub mod repositories;\n\npub fn apply(project: &mut ::mxrs_ir::ProjectDecl) {\n    persistence::apply(project);\n}\n"
        .to_string()
}

fn render_routes_module(api_mode: ApiMode) -> String {
    let server = match api_mode {
        ApiMode::Axum => {
            "use axum::{Router, routing::get};\n\npub fn router() -> Router {\n    Router::new().route(\"/health\", get(|| async { \"ok\" }))\n}\n\npub async fn serve() -> Result<(), Box<dyn std::error::Error>> {\n    let listener = tokio::net::TcpListener::bind(\"0.0.0.0:3000\").await?;\n    axum::serve(listener, router()).await?;\n    Ok(())\n}\n"
        }
        ApiMode::ActixWeb => {
            "use actix_web::{App, HttpResponse, HttpServer, web};\n\nasync fn health() -> HttpResponse {\n    HttpResponse::Ok().body(\"ok\")\n}\n\npub async fn serve() -> std::io::Result<()> {\n    HttpServer::new(|| App::new().route(\"/health\", web::get().to(health)))\n        .bind((\"0.0.0.0\", 3000))?\n        .run()\n        .await\n}\n"
        }
        ApiMode::Rocket => {
            "#[rocket::get(\"/health\")]\nfn health() -> &'static str {\n    \"ok\"\n}\n\npub async fn serve() -> Result<(), rocket::Error> {\n    rocket::build().mount(\"/\", rocket::routes![health]).launch().await?;\n    Ok(())\n}\n"
        }
    };
    format!(
        "//! {} HTTP routes.\n\npub fn apply(project: &mut ::mxrs_ir::ProjectDecl) {{\n    crate::presentation::navigation::apply(project);\n}}\n\npub mod server {{\n{}\n}}\n",
        api_mode.name(),
        indent(server, 4)
    )
}

fn render_persistence_module(modules: &[Module], mendix_version: &str) -> String {
    let has_sources = modules.iter().any(|module| {
        module.artifact_units.iter().any(|document| {
            document.get_str("$Type").ok() == Some("DomainModels$ViewEntitySourceDocument")
        })
    });
    let mutable = if has_sources { "mut " } else { "" };
    let mut out = format!(
        "//! Infrastructure adapter for Mendix module metadata.\n\npub fn apply(project: &mut ::mxrs_ir::ProjectDecl) {{\n    let {mutable}declarations = ::mxrs::ProjectBuilder::new({});\n",
        rust_string(mendix_version)
    );
    for module in modules {
        let module_name = module.name.as_deref().unwrap_or("Unnamed");
        let mut sources = module
            .artifact_units
            .iter()
            .filter(|document| {
                document.get_str("$Type").ok() == Some("DomainModels$ViewEntitySourceDocument")
            })
            .collect::<Vec<_>>();
        sources.sort_by_key(|document| document.get_str("Name").unwrap_or_default());
        if sources.is_empty() {
            continue;
        }
        let _ = writeln!(out, "    declarations.module({module_name:?}, |module| {{");
        for source in sources {
            let name = source.get_str("Name").unwrap_or("Unnamed");
            let query = source.get_str("Oql").unwrap_or_default();
            let documentation = source.get_str("Documentation").unwrap_or_default();
            let excluded = source.get_bool("Excluded").unwrap_or(false);
            let export_level = source.get_str("ExportLevel").unwrap_or("Hidden");
            let has_options = !documentation.is_empty() || excluded || export_level == "Published";
            let parameter = if has_options { "source" } else { "_source" };
            let _ = writeln!(
                out,
                "        module.oql_view_source({name:?}, {query:?}, |{parameter}| {{"
            );
            if !documentation.is_empty() {
                let _ = writeln!(out, "            source.documentation({documentation:?});");
            }
            if excluded {
                out.push_str("            source.excluded(true);\n");
            }
            if export_level == "Published" {
                out.push_str(
                    "            source.export_level(::mxrs_ir::ExportLevel::Published);\n",
                );
            }
            out.push_str("        });\n");
        }
        out.push_str("    });\n");
    }
    out.push_str(
        "    for declared in declarations.build().modules { project.merge_module(declared); }\n}\n",
    );
    out
}

fn render_scaffold_modules_module() -> String {
    "//! Scaffolded Mendix modules composed into this architectural layer.\n\n\
     pub fn apply(project: &mut ::mxrs_ir::ProjectDecl) {\n\
         for declare in MODULES {\n\
             declare(project);\n\
         }\n\
     }\n\n\
     const MODULES: &[fn(&mut ::mxrs_ir::ProjectDecl)] = &[];\n"
        .to_string()
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
    Menu {
        module: String,
        declaration: mxrs_ir::MenuDecl,
    },
    TaskQueue {
        module: String,
        declaration: mxrs_ir::TaskQueueDecl,
    },
}

/// The document layer, one file per concept: each Mendix module's constants,
/// regular expressions, scheduled events and standalone menus in its own
/// `domain/documents/<module>.rs`, and each enumeration in its own
/// `domain/enumerations/<module>_<name>.rs` — as `#[derive(MxEnumeration)]`
/// when the derive can express it.
struct DocumentsExport {
    /// `(file_stem, source, qualified name)` per Mendix module with documents.
    module_files: Vec<(String, String, String)>,
    /// `(file_stem, source, qualified name)` per enumeration.
    enumeration_files: Vec<(String, String, String)>,
    /// Qualified `Module.Name` → the Rust enum a typed entity field can
    /// reference, for enumerations rendered as `#[derive(MxEnumeration)]`.
    derived_enumerations: HashMap<String, DerivedEnumeration>,
    counts: HashMap<&'static str, usize>,
}

/// One enumeration that rendered as a real Rust enum: where it lives under
/// `domain/enumerations/` and the type name the file declares.
struct DerivedEnumeration {
    file_stem: String,
    type_name: String,
}

/// Where one entity's generated Rust lives and what the file declares, for
/// entities currently assumed to render as `#[derive(MxEntity)]` structs.
struct TypedEntityTarget {
    file_stem: String,
    type_name: String,
    dto: bool,
}

/// Project-wide context the entity layer renders against: who owns which
/// associations, how entity ids resolve to qualified names, which entities
/// the project declares, and which of them render as typed structs.
struct EntityLayerContext<'a> {
    associations_by_entity: HashMap<&'a str, Vec<&'a Association>>,
    qualified_by_id: HashMap<&'a str, String>,
    declared: std::collections::HashSet<String>,
    typed: HashMap<String, TypedEntityTarget>,
}

/// One association the entity's generated file must restate. The writer
/// preserves only associations whose target lives outside the declared
/// project; an association between declared entities that no entity file
/// re-declares would silently disappear from the rebuilt model.
struct DeclarableAssociation<'a> {
    association: &'a Association,
    name: &'a str,
    /// Qualified `Module.Entity` the association points at.
    target: String,
}

fn declarable_associations<'a>(
    entity: &Entity,
    ctx: &EntityLayerContext<'a>,
) -> Vec<DeclarableAssociation<'a>> {
    let Some(id) = entity.id.as_deref() else {
        return Vec::new();
    };
    let mut list = Vec::new();
    for association in ctx.associations_by_entity.get(id).into_iter().flatten() {
        let Some(name) = association.name.as_deref() else {
            continue;
        };
        let Some(raw_target) = association.to_entity_id.as_deref() else {
            continue;
        };
        let Some(target) = ctx.qualified_by_id.get(raw_target).cloned().or_else(|| {
            ctx.declared
                .contains(raw_target)
                .then(|| raw_target.to_string())
        }) else {
            continue;
        };
        if !ctx.declared.contains(&target) {
            // The writer's own unmodeled-external rule keeps this one.
            continue;
        }
        list.push(DeclarableAssociation {
            association,
            name,
            target,
        });
    }
    list.sort_by(|left, right| left.name.cmp(right.name));
    list
}

fn render_documents_module(project: &Project) -> Result<DocumentsExport> {
    let mut declarations = collect_editable_documents(project)?;
    let mut counts = HashMap::new();
    for declaration in &declarations {
        let native_type = match declaration {
            EditableDocument::Enumeration { .. } => "Enumerations$Enumeration",
            EditableDocument::Constant { .. } => "Constants$Constant",
            EditableDocument::RegularExpression { .. } => "RegularExpressions$RegularExpression",
            EditableDocument::ScheduledEvent { .. } => "ScheduledEvents$ScheduledEvent",
            EditableDocument::Menu { .. } => "Menus$MenuDocument",
            EditableDocument::TaskQueue { .. } => "Queues$Queue",
        };
        *counts.entry(native_type).or_default() += 1;
    }

    declarations.retain(|document| !matches!(document, EditableDocument::TaskQueue { .. }));
    declarations
        .sort_by(|left, right| editable_document_key(left).cmp(&editable_document_key(right)));

    let mut enumeration_files = Vec::new();
    let mut derived_enumerations = HashMap::new();
    let mut documents_by_module: Vec<(String, Vec<EditableDocument>)> = Vec::new();
    for declaration in declarations {
        if let EditableDocument::Enumeration {
            module,
            name,
            documentation,
            values,
        } = declaration
        {
            let stem = layer_file_stem(&module, &name, false);
            let (source, derived_type) =
                render_enumeration_file(&module, &name, &documentation, &values);
            if let Some(type_name) = derived_type {
                derived_enumerations.insert(
                    format!("{module}.{name}"),
                    DerivedEnumeration {
                        file_stem: stem.clone(),
                        type_name,
                    },
                );
            }
            enumeration_files.push((stem, source, format!("{module}.{name}")));
            continue;
        }
        let module = editable_document_module(&declaration).to_string();
        match documents_by_module.last_mut() {
            Some((current, documents)) if *current == module => documents.push(declaration),
            _ => documents_by_module.push((module, vec![declaration])),
        }
    }

    let module_files = documents_by_module
        .into_iter()
        .map(|(module, documents)| {
            let mut stem = sanitize_ident(&module);
            stem.make_ascii_lowercase();
            let source = render_module_documents_file(&module, documents);
            (stem, source, module)
        })
        .collect();

    Ok(DocumentsExport {
        module_files,
        enumeration_files,
        derived_enumerations,
        counts,
    })
}

/// One Mendix module's non-enumeration documents, on the standalone
/// [`ModuleBuilder`] the prelude exports.
fn render_module_documents_file(module_name: &str, documents: Vec<EditableDocument>) -> String {
    let mut source = format!(
        "//! Editable {module_name} documents: constants, regular expressions,\n\
         //! scheduled events, and standalone menus.\n\n\
         use mxrs::prelude::*;\n\n\
         pub fn declaration() -> ModuleDecl {{\n\
             let mut module = ModuleBuilder::new({});\n",
        rust_string(module_name),
    );
    for declaration in documents {
        render_editable_document_body(&mut source, declaration);
    }
    source.push_str("    module.into_decl()\n}\n");
    source
}

/// One Mendix enumeration as `#[derive(MxEnumeration)]` — a real Rust enum —
/// when the derive can express it: exactly one caption per value, and names
/// that survive as identifiers. Anything else keeps the builder form in the
/// same one-file-per-enumeration slot.
fn render_enumeration_file(
    module_name: &str,
    name: &str,
    documentation: &str,
    values: &[(String, Vec<(String, String)>)],
) -> (String, Option<String>) {
    if let Some((source, type_name)) =
        render_derived_enumeration(module_name, name, documentation, values)
    {
        return (source, Some(type_name));
    }
    let mut source = format!(
        "//! Editable Mendix enumeration.\n\n\
         use mxrs::prelude::*;\n\n\
         pub fn declaration() -> ModuleDecl {{\n\
             let mut module = ModuleBuilder::new({});\n",
        rust_string(module_name),
    );
    render_editable_document_body(
        &mut source,
        EditableDocument::Enumeration {
            module: module_name.to_string(),
            name: name.to_string(),
            documentation: documentation.to_string(),
            values: values.to_vec(),
        },
    );
    source.push_str("    module.into_decl()\n}\n");
    (source, None)
}

fn render_derived_enumeration(
    module_name: &str,
    name: &str,
    documentation: &str,
    values: &[(String, Vec<(String, String)>)],
) -> Option<(String, String)> {
    let mut type_name = derive_pascal_case(&sanitize_ident(name));
    if type_name.starts_with(|c: char| c.is_ascii_digit()) {
        type_name.insert(0, '_');
    }
    if type_name.is_empty() || rust_keyword(&type_name) {
        return None;
    }

    let mut seen = std::collections::HashSet::new();
    let mut variants = String::new();
    for (value, captions) in values {
        // The derive writes exactly one caption per value; zero or several
        // need the builder form.
        let [(language, caption)] = captions.as_slice() else {
            return None;
        };
        let mut variant = derive_pascal_case(&sanitize_ident(value));
        if variant.starts_with(|c: char| c.is_ascii_digit()) {
            variant.insert(0, '_');
        }
        if variant.is_empty() || rust_keyword(&variant) || !seen.insert(variant.clone()) {
            return None;
        }
        let mut options = Vec::new();
        if variant != *value {
            options.push(format!("name = {value:?}"));
        }
        if caption != value {
            options.push(format!("caption = {caption:?}"));
        }
        if language != "en_US" {
            options.push(format!("language = {language:?}"));
        }
        if !options.is_empty() {
            let _ = writeln!(variants, "    #[mxrs({})]", options.join(", "));
        }
        let _ = writeln!(variants, "    {variant},");
    }

    let mut out = String::from("//! Editable Mendix enumeration.\n\nuse mxrs::prelude::*;\n\n");
    out.push_str("#[derive(MxEnumeration)]\n");
    let mut options = vec![format!("module = {module_name:?}")];
    if type_name != name {
        options.insert(0, format!("name = {name:?}"));
    }
    if !documentation.is_empty() {
        options.push(format!("documentation = {documentation:?}"));
    }
    let _ = writeln!(out, "#[mxrs({})]", options.join(", "));
    if variants.is_empty() {
        let _ = writeln!(out, "pub enum {type_name} {{}}");
    } else {
        let _ = writeln!(out, "pub enum {type_name} {{\n{variants}}}");
    }
    let _ = writeln!(
        out,
        "\npub fn declaration() -> ModuleDecl {{\n    let mut module = ModuleBuilder::new({});\n    {type_name}::mx_register(&mut module);\n    module.into_decl()\n}}",
        rust_string(module_name),
    );
    Some((out, type_name))
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
            Some("Queues$Queue") => {
                let Some(declaration) = parse_complete_task_queue(&document) else {
                    continue;
                };
                declarations.push(EditableDocument::TaskQueue {
                    module,
                    declaration,
                });
            }
            Some("Menus$MenuDocument") => {
                let Some(declaration) = parse_complete_menu(&document) else {
                    continue;
                };
                declarations.push(EditableDocument::Menu {
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

fn parse_complete_task_queue(document: &mxrs_bson::Document) -> Option<mxrs_ir::TaskQueueDecl> {
    use mxrs_ir::{ExportLevel, TaskQueueConfig, TaskQueueDecl, TaskQueueScope};
    if !exact_document(
        document,
        &[
            "$ID",
            "$Type",
            "Name",
            "Documentation",
            "Excluded",
            "ExportLevel",
            "Config",
        ],
        "Queues$Queue",
    ) {
        return None;
    }
    let config = document.get_document("Config").ok()?;
    let config = if exact_document(
        config,
        &["$ID", "$Type", "Parallelism"],
        "Queues$BasicQueueConfig",
    ) {
        let parallelism = bson_integer(config.get("Parallelism"))?;
        if parallelism <= 0 {
            return None;
        }
        TaskQueueConfig::Fixed {
            parallelism: parallelism as u32,
        }
    } else if exact_document(
        config,
        &["$ID", "$Type", "ParallelismExpression", "ClusterWide"],
        "Queues$BasicQueueConfig",
    ) {
        let expression = config.get_str("ParallelismExpression").ok()?;
        if expression.trim().is_empty() {
            return None;
        }
        TaskQueueConfig::Dynamic {
            parallelism_expression: expression.to_string(),
            scope: if config.get_bool("ClusterWide").ok()? {
                TaskQueueScope::ClusterWide
            } else {
                TaskQueueScope::PerNode
            },
        }
    } else {
        return None;
    };
    let name = document.get_str("Name").ok()?;
    if name.trim().is_empty() {
        return None;
    }
    Some(TaskQueueDecl {
        name: name.to_string(),
        config,
        documentation: document.get_str("Documentation").ok()?.to_string(),
        excluded: document.get_bool("Excluded").ok()?,
        export_level: match document.get_str("ExportLevel").ok()? {
            "Hidden" => ExportLevel::Hidden,
            "Published" => ExportLevel::Published,
            _ => return None,
        },
    })
}

fn render_task_queue(source: &mut String, queue: &mxrs_ir::TaskQueueDecl) {
    use mxrs_ir::{TaskQueueConfig, TaskQueueScope};
    let config = match &queue.config {
        TaskQueueConfig::Fixed { parallelism } => {
            format!("::mxrs_ir::TaskQueueConfig::Fixed {{ parallelism: {parallelism} }}")
        }
        TaskQueueConfig::Dynamic {
            parallelism_expression,
            scope,
        } => format!(
            "::mxrs_ir::TaskQueueConfig::Dynamic {{ parallelism_expression: {}.to_string(), scope: ::mxrs_ir::TaskQueueScope::{} }}",
            rust_string(parallelism_expression),
            match scope {
                TaskQueueScope::PerNode => "PerNode",
                TaskQueueScope::ClusterWide => "ClusterWide",
            },
        ),
    };
    let parameter = if queue.documentation.is_empty()
        && !queue.excluded
        && queue.export_level == mxrs_ir::ExportLevel::Hidden
    {
        "_"
    } else {
        "queue"
    };
    let _ = writeln!(
        source,
        "        module.task_queue({}, {config}, |{parameter}| {{",
        rust_string(&queue.name)
    );
    if !queue.documentation.is_empty() {
        let _ = writeln!(
            source,
            "            queue.documentation({});",
            rust_string(&queue.documentation)
        );
    }
    if queue.excluded {
        source.push_str("            queue.excluded(true);\n");
    }
    if queue.export_level == mxrs_ir::ExportLevel::Published {
        source.push_str("            queue.export_level(::mxrs_ir::ExportLevel::Published);\n");
    }
    source.push_str("        });\n");
}

fn render_task_queues_module(project: &Project, mendix_version: &str) -> Result<String> {
    let mut queues = collect_editable_documents(project)?;
    queues.retain(|document| matches!(document, EditableDocument::TaskQueue { .. }));
    queues.sort_by(|left, right| editable_document_key(left).cmp(&editable_document_key(right)));
    let mutable = if queues.is_empty() { "" } else { "mut " };
    let mut source = format!(
        "//! Editable Cargo-native module documents.\n\n\
         fn declarations() -> ::mxrs_ir::ProjectDecl {{\n\
             let {mutable}project = ::mxrs_dsl::ProjectBuilder::new({});\n",
        rust_string(mendix_version),
    );
    let mut current_module = None::<String>;
    for declaration in queues {
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
                 project.merge_module(declared);\n\
             }\n\
         }\n",
    );
    Ok(source)
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

fn parse_complete_menu(document: &mxrs_bson::Document) -> Option<mxrs_ir::MenuDecl> {
    if !exact_document(
        document,
        &[
            "$ID",
            "$Type",
            "Documentation",
            "Excluded",
            "ExportLevel",
            "ItemCollection",
            "Name",
        ],
        "Menus$MenuDocument",
    ) {
        return None;
    }
    let collection = document.get_document("ItemCollection").ok()?;
    if !exact_document(
        collection,
        &["$ID", "$Type", "Items"],
        "Menus$MenuItemCollection",
    ) {
        return None;
    }
    let items = complete_document_array(collection.get("Items")?)?
        .into_iter()
        .map(parse_complete_menu_item)
        .collect::<Option<Vec<_>>>()?;
    Some(mxrs_ir::MenuDecl {
        name: document.get_str("Name").ok()?.to_string(),
        documentation: document.get_str("Documentation").ok()?.to_string(),
        excluded: document.get_bool("Excluded").ok()?,
        export_level: match document.get_str("ExportLevel").ok()? {
            "Hidden" => mxrs_ir::ExportLevel::Hidden,
            "Published" => mxrs_ir::ExportLevel::Published,
            _ => return None,
        },
        items,
    })
}

fn parse_complete_menu_item(document: &mxrs_bson::Document) -> Option<mxrs_ir::MenuItemDecl> {
    if !exact_document(
        document,
        &[
            "$ID",
            "$Type",
            "Action",
            "AlternativeText",
            "Caption",
            "Icon",
            "Items",
        ],
        "Menus$MenuItem",
    ) {
        return None;
    }
    let alternative_text = match document.get("AlternativeText")? {
        mxrs_bson::Bson::Null => None,
        mxrs_bson::Bson::Document(text) => Some(parse_complete_menu_text(text)?),
        _ => return None,
    };
    let icon = match document.get("Icon")? {
        mxrs_bson::Bson::Null => None,
        mxrs_bson::Bson::Document(icon) => Some(parse_complete_menu_icon(icon)?),
        _ => return None,
    };
    let items = complete_document_array(document.get("Items")?)?
        .into_iter()
        .map(parse_complete_menu_item)
        .collect::<Option<Vec<_>>>()?;
    Some(mxrs_ir::MenuItemDecl {
        caption: parse_complete_menu_text(document.get_document("Caption").ok()?)?,
        alternative_text,
        action: parse_complete_menu_action(document.get_document("Action").ok()?)?,
        icon,
        items,
    })
}

fn parse_complete_menu_icon(document: &mxrs_bson::Document) -> Option<mxrs_ir::MenuIconDecl> {
    match document.get_str("$Type").ok()? {
        "Forms$GlyphIcon"
            if exact_document(document, &["$ID", "$Type", "Code"], "Forms$GlyphIcon") =>
        {
            Some(mxrs_ir::MenuIconDecl::Glyph(document.get_i64("Code").ok()?))
        }
        "Forms$IconCollectionIcon"
            if exact_document(
                document,
                &["$ID", "$Type", "Image"],
                "Forms$IconCollectionIcon",
            ) =>
        {
            Some(mxrs_ir::MenuIconDecl::Image(
                document.get_str("Image").ok()?.to_string(),
            ))
        }
        _ => None,
    }
}

fn parse_complete_menu_action(document: &mxrs_bson::Document) -> Option<mxrs_ir::MenuActionDecl> {
    match document.get_str("$Type").ok()? {
        "Forms$NoAction"
            if exact_document(
                document,
                &["$ID", "$Type", "DisabledDuringExecution"],
                "Forms$NoAction",
            ) =>
        {
            Some(mxrs_ir::MenuActionDecl::None {
                disabled_during_execution: document.get_bool("DisabledDuringExecution").ok()?,
            })
        }
        "Forms$FormAction"
            if exact_document(
                document,
                &[
                    "$ID",
                    "$Type",
                    "DisabledDuringExecution",
                    "FormSettings",
                    "NumberOfPagesToClose2",
                    "PagesForSpecializations",
                ],
                "Forms$FormAction",
            ) && complete_empty_array(document.get("PagesForSpecializations")?) =>
        {
            let (page, title_override) =
                parse_complete_menu_form_settings(document.get_document("FormSettings").ok()?)?;
            Some(mxrs_ir::MenuActionDecl::OpenPage {
                page,
                disabled_during_execution: document.get_bool("DisabledDuringExecution").ok()?,
                pages_to_close: parse_pages_to_close(
                    document.get_str("NumberOfPagesToClose2").ok()?,
                )?,
                title_override,
            })
        }
        "Forms$CreateObjectClientAction"
            if exact_document(
                document,
                &[
                    "$ID",
                    "$Type",
                    "DisabledDuringExecution",
                    "EntityRef",
                    "NumberOfPagesToClose2",
                    "PageSettings",
                ],
                "Forms$CreateObjectClientAction",
            ) =>
        {
            let entity_ref = document.get_document("EntityRef").ok()?;
            if !exact_document(
                entity_ref,
                &["$ID", "$Type", "Entity"],
                "DomainModels$DirectEntityRef",
            ) {
                return None;
            }
            let (page, title_override) =
                parse_complete_menu_form_settings(document.get_document("PageSettings").ok()?)?;
            Some(mxrs_ir::MenuActionDecl::CreateObjectAndOpenPage {
                entity: entity_ref.get_str("Entity").ok()?.to_string(),
                page,
                disabled_during_execution: document.get_bool("DisabledDuringExecution").ok()?,
                pages_to_close: parse_pages_to_close(
                    document.get_str("NumberOfPagesToClose2").ok()?,
                )?,
                title_override,
            })
        }
        _ => None,
    }
}

fn parse_complete_menu_form_settings(
    document: &mxrs_bson::Document,
) -> Option<(String, Option<mxrs_ir::LocalizedText>)> {
    if !exact_document(
        document,
        &["$ID", "$Type", "Form", "ParameterMappings", "TitleOverride"],
        "Forms$FormSettings",
    ) || !complete_empty_array(document.get("ParameterMappings")?)
    {
        return None;
    }
    let title = match document.get("TitleOverride")? {
        mxrs_bson::Bson::Null => None,
        mxrs_bson::Bson::Document(template)
            if exact_document(
                template,
                &["$ID", "$Type", "Parameters", "Text"],
                "Microflows$TextTemplate",
            ) && complete_empty_array(template.get("Parameters")?) =>
        {
            Some(parse_complete_menu_text(
                template.get_document("Text").ok()?,
            )?)
        }
        _ => return None,
    };
    Some((document.get_str("Form").ok()?.to_string(), title))
}

fn parse_complete_menu_text(document: &mxrs_bson::Document) -> Option<mxrs_ir::LocalizedText> {
    if !exact_document(document, &["$ID", "$Type", "Items"], "Texts$Text") {
        return None;
    }
    let mut translations = mxrs_ir::LocalizedText::new();
    for translation in complete_document_array(document.get("Items")?)? {
        if !exact_document(
            translation,
            &["$ID", "$Type", "LanguageCode", "Text"],
            "Texts$Translation",
        ) {
            return None;
        }
        let language = translation.get_str("LanguageCode").ok()?.to_string();
        let text = translation.get_str("Text").ok()?.to_string();
        if translations.insert(language, text).is_some() {
            return None;
        }
    }
    Some(translations)
}

fn exact_document(document: &mxrs_bson::Document, fields: &[&str], native_type: &str) -> bool {
    document.len() == fields.len()
        && document
            .keys()
            .all(|field| fields.contains(&field.as_str()))
        && document
            .get("$ID")
            .and_then(mxrs_bson::extract_id)
            .is_some()
        && document.get_str("$Type").ok() == Some(native_type)
}

fn complete_document_array(value: &mxrs_bson::Bson) -> Option<Vec<&mxrs_bson::Document>> {
    let mxrs_bson::Bson::Array(values) = value else {
        return None;
    };
    let (marker, items) = values.split_first()?;
    match marker {
        mxrs_bson::Bson::Int32(_) | mxrs_bson::Bson::Int64(_) => {}
        _ => return None,
    }
    items.iter().map(mxrs_bson::Bson::as_document).collect()
}

fn complete_empty_array(value: &mxrs_bson::Bson) -> bool {
    complete_document_array(value).is_some_and(|items| items.is_empty())
}

fn parse_pages_to_close(value: &str) -> Option<Option<u32>> {
    if value.is_empty() {
        Some(None)
    } else {
        value.parse().ok().map(Some)
    }
}

fn render_editable_document_body(source: &mut String, declaration: EditableDocument) {
    match declaration {
        EditableDocument::TaskQueue { declaration, .. } => render_task_queue(source, &declaration),
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
                "            constant.value_type(ConstantType::{value_type});"
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
                    "            regular_expression.export_level(ExportLevel::{export_level});"
                );
            }
            source.push_str("        });\n");
        }
        EditableDocument::ScheduledEvent {
            module: _,
            declaration,
        } => render_scheduled_event_body(source, &declaration),
        EditableDocument::Menu {
            module: _,
            declaration,
        } => render_menu_body(source, &declaration),
    }
}

fn render_scheduled_event_body(source: &mut String, event: &mxrs_ir::ScheduledEventDecl) {
    let _ = writeln!(
        source,
        "        module.scheduled_event({}, {}, ScheduleUnit::{:?}, |event| {{",
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
        "            event.on_overlap(OnOverlap::{:?});",
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
            "            event.export_level(ExportLevel::{:?});",
            event.export_level
        );
    }
    let schedule = render_event_schedule(&event.schedule);
    let _ = writeln!(source, "            event.schedule({schedule});");
    source.push_str("        });\n");
}

fn render_event_schedule(schedule: &mxrs_ir::ScheduledEventSchedule) -> String {
    match schedule {
        mxrs_ir::ScheduledEventSchedule::None => "ScheduledEventSchedule::None".to_string(),
        mxrs_ir::ScheduledEventSchedule::Minute { multiplier } => {
            format!("ScheduledEventSchedule::Minute {{ multiplier: {multiplier} }}")
        }
        mxrs_ir::ScheduledEventSchedule::Hour {
            multiplier,
            minute_offset,
        } => format!(
            "ScheduledEventSchedule::Hour {{ multiplier: {multiplier}, minute_offset: {minute_offset} }}"
        ),
        mxrs_ir::ScheduledEventSchedule::Day {
            hour_of_day,
            minute_of_hour,
        } => format!(
            "ScheduledEventSchedule::Day {{ hour_of_day: {hour_of_day}, minute_of_hour: {minute_of_hour} }}"
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
            "ScheduledEventSchedule::Week {{ hour_of_day: {hour_of_day}, minute_of_hour: {minute_of_hour}, monday: {monday}, tuesday: {tuesday}, wednesday: {wednesday}, thursday: {thursday}, friday: {friday}, saturday: {saturday}, sunday: {sunday} }}"
        ),
    }
}

fn render_menu_body(source: &mut String, menu: &mxrs_ir::MenuDecl) {
    let _ = writeln!(
        source,
        "        module.menu({}, |menu| {{",
        rust_string(&menu.name)
    );
    if !menu.documentation.is_empty() {
        let _ = writeln!(
            source,
            "            menu.documentation({});",
            rust_string(&menu.documentation)
        );
    }
    if menu.excluded {
        source.push_str("            menu.excluded(true);\n");
    }
    if menu.export_level != mxrs_ir::ExportLevel::Hidden {
        let _ = writeln!(
            source,
            "            menu.export_level(ExportLevel::{:?});",
            menu.export_level
        );
    }
    for item in &menu.items {
        render_menu_item(source, item, 3, "menu");
    }
    source.push_str("        });\n");
}

fn render_menu_item(source: &mut String, item: &mxrs_ir::MenuItemDecl, depth: usize, parent: &str) {
    let indent = "    ".repeat(depth);
    let default_action = matches!(
        item.action,
        mxrs_ir::MenuActionDecl::None {
            disabled_during_execution: true
        }
    );
    let only_primary_caption = item.caption.len() == 1 && item.caption.contains_key("en_US");
    let has_body = !default_action
        || item.icon.is_some()
        || item.alternative_text.is_some()
        || !item.items.is_empty()
        || !only_primary_caption;
    let parameter = if has_body { "item" } else { "_" };
    if let Some(caption) = item.caption.get("en_US") {
        let _ = writeln!(
            source,
            "{indent}{parent}.item({}, |{parameter}| {{",
            rust_string(caption)
        );
    } else {
        let _ = writeln!(
            source,
            "{indent}{parent}.localized_item({}, |{parameter}| {{",
            localized_text_expression(&item.caption)
        );
    }
    if has_body {
        for (locale, caption) in &item.caption {
            if locale != "en_US" {
                let _ = writeln!(
                    source,
                    "{indent}    item.caption({}, {});",
                    rust_string(locale),
                    rust_string(caption)
                );
            }
        }
        if let Some(alternative_text) = &item.alternative_text {
            for (locale, text) in alternative_text {
                let _ = writeln!(
                    source,
                    "{indent}    item.alternative_text({}, {});",
                    rust_string(locale),
                    rust_string(text)
                );
            }
        }
        match &item.action {
            mxrs_ir::MenuActionDecl::None {
                disabled_during_execution: true,
            } => {}
            mxrs_ir::MenuActionDecl::OpenPage {
                page,
                disabled_during_execution: true,
                pages_to_close: None,
                title_override: None,
            } => {
                let _ = writeln!(source, "{indent}    item.page({});", rust_string(page));
            }
            action => {
                let _ = writeln!(
                    source,
                    "{indent}    item.action({});",
                    menu_action_expression(action)
                );
            }
        }
        match &item.icon {
            Some(mxrs_ir::MenuIconDecl::Glyph(code)) => {
                let _ = writeln!(source, "{indent}    item.glyph({code});");
            }
            Some(mxrs_ir::MenuIconDecl::Image(reference)) => {
                let _ = writeln!(
                    source,
                    "{indent}    item.image({});",
                    rust_string(reference)
                );
            }
            None => {}
        }
        for child in &item.items {
            render_menu_item(source, child, depth + 1, "item");
        }
    }
    let _ = writeln!(source, "{indent}}});");
}

fn menu_action_expression(action: &mxrs_ir::MenuActionDecl) -> String {
    match action {
        mxrs_ir::MenuActionDecl::None {
            disabled_during_execution,
        } => format!(
            "MenuActionDecl::None {{ disabled_during_execution: {disabled_during_execution} }}"
        ),
        mxrs_ir::MenuActionDecl::OpenPage {
            page,
            disabled_during_execution,
            pages_to_close,
            title_override,
        } => format!(
            "MenuActionDecl::OpenPage {{ page: {}.to_string(), disabled_during_execution: {disabled_during_execution}, pages_to_close: {}, title_override: {} }}",
            rust_string(page),
            option_u32(*pages_to_close),
            option_localized_text(title_override.as_ref()),
        ),
        mxrs_ir::MenuActionDecl::CreateObjectAndOpenPage {
            entity,
            page,
            disabled_during_execution,
            pages_to_close,
            title_override,
        } => format!(
            "MenuActionDecl::CreateObjectAndOpenPage {{ entity: {}.to_string(), page: {}.to_string(), disabled_during_execution: {disabled_during_execution}, pages_to_close: {}, title_override: {} }}",
            rust_string(entity),
            rust_string(page),
            option_u32(*pages_to_close),
            option_localized_text(title_override.as_ref()),
        ),
    }
}

fn localized_text_expression(text: &mxrs_ir::LocalizedText) -> String {
    format!(
        "[{}]",
        text.iter()
            .map(|(locale, value)| format!("({}, {})", rust_string(locale), rust_string(value)))
            .collect::<Vec<_>>()
            .join(", ")
    )
}

fn option_localized_text(text: Option<&mxrs_ir::LocalizedText>) -> String {
    text.map_or_else(
        || "None".to_string(),
        |text| {
            format!(
                "Some({}.into_iter().map(|(locale, text)| (locale.to_string(), text.to_string())).collect())",
                localized_text_expression(text)
            )
        },
    )
}

fn option_u32(value: Option<u32>) -> String {
    value.map_or_else(|| "None".to_string(), |value| format!("Some({value})"))
}

fn editable_document_module(document: &EditableDocument) -> &str {
    match document {
        EditableDocument::Enumeration { module, .. }
        | EditableDocument::Constant { module, .. }
        | EditableDocument::RegularExpression { module, .. }
        | EditableDocument::TaskQueue { module, .. }
        | EditableDocument::ScheduledEvent { module, .. } => module,
        EditableDocument::Menu { module, .. } => module,
    }
}

fn editable_document_key(document: &EditableDocument) -> (&str, u8, &str) {
    match document {
        EditableDocument::Enumeration { module, name, .. } => (module, 0, name),
        EditableDocument::Constant { module, name, .. } => (module, 1, name),
        EditableDocument::RegularExpression { module, name, .. } => (module, 2, name),
        EditableDocument::TaskQueue {
            module,
            declaration,
        } => (module, 5, &declaration.name),
        EditableDocument::ScheduledEvent {
            module,
            declaration,
        } => (module, 3, &declaration.name),
        EditableDocument::Menu {
            module,
            declaration,
        } => (module, 4, &declaration.name),
    }
}

fn editable_project_declaration(project: &Project) -> Result<mxrs_ir::ProjectDecl> {
    let mut modules = std::collections::BTreeMap::<String, mxrs_ir::ModuleDecl>::new();
    for document in collect_editable_documents(project)? {
        let module_name = match &document {
            EditableDocument::Enumeration { module, .. }
            | EditableDocument::Constant { module, .. }
            | EditableDocument::RegularExpression { module, .. }
            | EditableDocument::ScheduledEvent { module, .. }
            | EditableDocument::TaskQueue { module, .. } => module.clone(),
            EditableDocument::Menu { module, .. } => module.clone(),
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
            EditableDocument::Menu { declaration, .. } => module.menus.push(declaration),
            EditableDocument::TaskQueue { declaration, .. } => module.task_queues.push(declaration),
        }
    }
    for flow in flow_export::collect(project, &project.modules()?)? {
        let module = modules
            .entry(flow.module.clone())
            .or_insert_with(|| mxrs_ir::ModuleDecl {
                name: flow.module,
                ..Default::default()
            });
        if flow.native_type == "Microflows$Nanoflow" {
            module.nanoflows.push(flow.declaration);
        } else {
            module.microflows.push(flow.declaration);
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
        "encryptionkey",
        "signingkey",
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

/// Creates the editable server-flow composition layer. New typed declarations
/// merge by module and are then upserted by the writer.
fn render_microflows_module(mendix_version: &str, _modules: &[Module]) -> String {
    render_flow_module(
        mendix_version,
        "server-side microflow",
        "microflow_module",
        "microflow",
    )
}

/// Creates the editable client-flow composition layer independently from the
/// server path, so generated source cannot blur their runtime boundary.
fn render_nanoflows_module(mendix_version: &str, _modules: &[Module]) -> String {
    render_flow_module(
        mendix_version,
        "client-side nanoflow",
        "nanoflow_module",
        "nanoflow",
    )
}

fn render_flow_module(
    mendix_version: &str,
    description: &str,
    module_method: &str,
    builder_method: &str,
) -> String {
    let mut source = format!("//! Editable Cargo-native {description} declarations.\n");
    source.push_str(
        "\nfn declarations() -> ::mxrs_ir::ProjectDecl {\n\
             #[allow(unused_mut)]\n\
             let mut project = ::mxrs_dsl::ProjectBuilder::new(",
    );
    source.push_str(&rust_string(mendix_version));
    let _ = writeln!(
        source,
        ");\n    // Add `project.{module_method}(..., |module| module.{builder_method}(...))` declarations here."
    );
    source.push_str(
        "    project.build()\n\
         }\n\n\
         pub fn apply(project: &mut ::mxrs_ir::ProjectDecl) {\n\
             for declared in declarations().modules {\n\
                 project.merge_module(declared);\n\
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
        // Imported demo users stay losslessly preserved in the stored
        // `DemoUsers` array (the writer leaves it untouched for an empty
        // declaration list); exporting them would either embed their stored
        // passwords in source or reserialize content nobody edited.
        source.push_str("        demo_users: vec![],\n");
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

fn cargo_manifest(package_name: &str, mxrs_workspace: Option<&Path>, api_mode: ApiMode) -> String {
    let dependency = match mxrs_workspace {
        Some(workspace) => format!(
            "{{ path = {} }}",
            toml_string(&workspace.join("crates/app/mxrs").display().to_string())
        ),
        None => "{ git = \"https://github.com/lucamykael/mxrs\" }".to_string(),
    };
    let api_dependencies = match api_mode {
        ApiMode::Axum => {
            "axum = \"0.8\"\ntokio = { version = \"1\", features = [\"macros\", \"rt-multi-thread\", \"net\"] }\n"
        }
        ApiMode::ActixWeb => "actix-web = \"4\"\n",
        ApiMode::Rocket => "rocket = \"0.5\"\n",
    };
    format!(
        "[package]\nname = {package_name:?}\nversion = \"0.1.0\"\nedition = \"2024\"\npublish = false\n\n[dependencies]\nmxrs = {dependency}\n{api_dependencies}",
    )
}

fn build_binary_source(crate_name: &str, project_name: &str, api_mode: ApiMode) -> String {
    let default_output = format!("build/{project_name}.mpr");
    let serve = match api_mode {
        ApiMode::Axum => format!(
            "#[tokio::main]\nasync fn main() -> Result<(), Box<dyn std::error::Error>> {{\n    if std::env::args().nth(1).as_deref() == Some(\"serve\") {{\n        return {crate_name}::presentation::routes::server::serve().await;\n    }}\n    build()\n}}\n"
        ),
        ApiMode::ActixWeb => format!(
            "#[actix_web::main]\nasync fn main() -> Result<(), Box<dyn std::error::Error>> {{\n    if std::env::args().nth(1).as_deref() == Some(\"serve\") {{\n        {crate_name}::presentation::routes::server::serve().await?;\n        return Ok(());\n    }}\n    build()\n}}\n"
        ),
        ApiMode::Rocket => format!(
            "#[rocket::main]\nasync fn main() -> Result<(), Box<dyn std::error::Error>> {{\n    if std::env::args().nth(1).as_deref() == Some(\"serve\") {{\n        {crate_name}::presentation::routes::server::serve().await?;\n        return Ok(());\n    }}\n    build()\n}}\n"
        ),
    };
    format!(
        "fn build() -> Result<(), Box<dyn std::error::Error>> {{\n    let output = std::env::args().nth(1).unwrap_or_else(|| {}.to_string());\n    let root = std::path::Path::new(env!(\"CARGO_MANIFEST_DIR\"));\n    mxrs::replace_imported_project(root.join(\"model/imported\"), &output, &{crate_name}::Application::build())?;\n    mxrs::materialize_project_assets(root.join(\"assets\"), &output)?;\n    println!(\"built {{output}}\");\n    Ok(())\n}}\n\n{serve}",
        serde_json::to_string(&default_output).expect("a string always serializes"),
    )
}

fn indent(source: &str, spaces: usize) -> String {
    let prefix = " ".repeat(spaces);
    source
        .lines()
        .map(|line| format!("{prefix}{line}"))
        .collect::<Vec<_>>()
        .join("\n")
}

fn generated_readme(project_name: &str, gaps: usize, page_export: &PageExportReport) -> String {
    let pages_note = if page_export.typed_candidates > 0 {
        format!(
            "\n`src/presentation/pages/mod.rs` defines {} page(s) this import detected as buildable from\nmxrs-dsl's native/structural widget vocabulary (out of {} page(s) total) — wired into\n`build()` automatically by `src/presentation/mod.rs`.\n",
            page_export.typed_candidates,
            page_export.typed_candidates + page_export.opaque,
        )
    } else {
        String::new()
    };
    format!(
        "# {project_name}\n\nCargo-native Mendix project imported by `mxrs`. The source uses a flat, Clean Architecture layout: persisted entities are individual `#[derive(MxEntity)]` structs under `src/domain/entities/` (entities whose features typed authoring does not cover yet fall back to an IR declaration in the same place); view and non-persistable entities are DTO files under `src/application/dto/`; each supported server microflow is an individual service declaration under `src/application/services/`; HTTP controllers and routes live under `src/presentation/`; repositories, database adapters, and imported-model persistence live under `src/infrastructure/`. Mendix modules remain metadata, rather than becoming nested Rust folders. Lossless model data and stable identities stay outside the Rust source tree under `model/imported/`.\n\nEach enumeration is an individual `#[derive(MxEnumeration)]` Rust enum under `src/domain/enumerations/`, and each Mendix module's constants, regular expressions, scheduled events, and standalone menus live in their own file under `src/domain/documents/`. Supported server-side microflows are reconstructed as typed service declarations; `src/presentation/nanoflows/mod.rs` is the client-side counterpart. Other graphs remain exact in the imported model data. Edits with the same activity structure preserve node identities and layout; structural edits rebuild the graph.\n\n```sh\n# Mendix → Rust was performed with:\nmxrs convert mendix-to-rust app.mpr --output . --mode axum\n\ncargo check\ncargo test\n\n# Rust → Mendix, then boot it on MXRS' native runtime:\nmxrs convert rust-to-mendix . --output build/{project_name}.mpr\nmxrs run . --no-frontend\n```\n\nChoose `--mode axum`, `--mode actix-web`, or `--mode rocket` during import to generate that framework's dependencies and route server. `mxrs run` materializes missing web assets itself and shuts down cleanly on interrupt; it does not require Studio Pro or mxbuild.\n\nThe typed domain export omitted {gaps} association target(s) that do not resolve inside this imported project; their original model data remains preserved. Run `mxrs portability` for the complete per-family typed/partial/preserved inventory.\n{pages_note}"
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

fn write_marker_layer_sources(
    destination: &Path,
    manifest: &mxrs_typegen::Manifest,
    generated: &str,
    typed_markers: &str,
) -> Result<()> {
    let directory = destination.join("src/infrastructure/markers");
    std::fs::create_dir_all(&directory).map_err(|source| io_error(&directory, source))?;
    let mut index = String::from(
        "//! Compile-time model markers, split by Mendix module for concise source files.\n\n",
    );
    let mut file_names = std::collections::HashSet::new();

    for module in &manifest.modules {
        let declaration = format!("pub mod {} {{", module.name);
        let module_start = generated.find(&declaration).ok_or_else(|| {
            ExportError::MarkerLayout(format!("module {:?} was not generated", module.name))
        })?;
        let source_start = generated[..module_start].rfind("#[allow(").ok_or_else(|| {
            ExportError::MarkerLayout(format!("module {:?} has no attribute", module.name))
        })?;
        let opening_brace = module_start
            + generated[module_start..]
                .find('{')
                .expect("the module declaration contains an opening brace");
        // Counting raw braces is only sound because everything typegen emits
        // inside a module block is either a validated Rust identifier or a
        // model name that `valid_ident` already accepted — and `{:?}` on a
        // `str` does not escape braces, so a name containing one would close
        // this module early and silently truncate the marker file. If
        // `valid_ident` ever loosens, this needs a real lexer instead.
        let mut depth = 0usize;
        let mut source_end = None;
        for (offset, byte) in generated.as_bytes()[opening_brace..].iter().enumerate() {
            match byte {
                b'{' => depth += 1,
                b'}' => {
                    depth -= 1;
                    if depth == 0 {
                        source_end = Some(opening_brace + offset + 1);
                        break;
                    }
                }
                _ => {}
            }
        }
        let source_end = source_end.ok_or_else(|| {
            ExportError::MarkerLayout(format!("module {:?} is not balanced", module.name))
        })?;
        let mut source = generated[source_start..source_end].to_string();
        source.push('\n');
        let marker_prefix = format!(" for {}::", module.name);
        for line in typed_markers
            .lines()
            .filter(|line| line.contains(&marker_prefix))
        {
            source.push_str(line);
            source.push('\n');
        }

        let mut file_name = sanitize_ident(&module.name);
        file_name.make_ascii_lowercase();
        if !file_names.insert(file_name.clone()) {
            return Err(ExportError::MarkerLayout(format!(
                "marker filename collision for module {:?}",
                module.name
            )));
        }
        let _ = writeln!(index, "include!(\"markers/{file_name}.rs\");");
        write_text(&directory.join(format!("{file_name}.rs")), &source)?;
    }

    write_text(&destination.join("src/infrastructure/markers.rs"), &index)
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
    let known_microflows = known_microflows(modules);
    let known_oql_sources = known_oql_sources(modules);
    let mut gaps = Vec::new();
    for module in modules {
        let module_name = module.name.as_deref().unwrap_or("Unnamed");
        for queue in module
            .artifact_units
            .iter()
            .filter(|document| document.get_str("$Type").ok() == Some("Queues$Queue"))
        {
            if parse_complete_task_queue(queue).is_none() {
                gaps.push(RoundTripGap {
                    path: format!("{module_name}.{}", queue.get_str("Name").unwrap_or("Unnamed")),
                    reason: "task queue contains native fields or configuration outside the typed declaration".to_string(),
                });
            }
        }
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
            if let Some(target) = entity.generalization_target()
                && built_in_generalization_path(&target).is_none()
                && !entity_qualified_name_by_id
                    .values()
                    .any(|qualified_name| qualified_name == &target)
            {
                gaps.push(RoundTripGap {
                    path: format!("{module_name}.{entity_name}.generalization"),
                    reason: format!("generalization target {target:?} cannot be resolved"),
                });
            }
            if let Some(generalization) = &entity.generalization
                && generalization.target.is_none()
                && !generalization.native_type.ends_with("NoGeneralization")
            {
                gaps.push(RoundTripGap {
                    path: format!("{module_name}.{entity_name}.generalization"),
                    reason: format!(
                        "unsupported generalization shape {:?}",
                        generalization.native_type
                    ),
                });
            }
            for (position, index) in entity.indexes.iter().enumerate() {
                if index.members.iter().any(|member| {
                    matches!(
                        member.kind,
                        mxrs_model::entity::IndexMemberKind::Unresolved(_)
                    )
                }) {
                    gaps.push(RoundTripGap {
                        path: format!("{module_name}.{entity_name}.index[{position}]"),
                        reason: "index contains an unresolved native member".to_string(),
                    });
                }
            }
            for callback in &entity.lifecycle {
                if !matches!(
                    callback.event.as_str(),
                    "before_commit" | "after_commit" | "before_delete" | "after_delete"
                ) {
                    gaps.push(RoundTripGap {
                        path: format!("{module_name}.{entity_name}.{}", callback.event),
                        reason: "lifecycle event has no typed DSL variant".to_string(),
                    });
                } else if !known_microflows.contains(&callback.handler) {
                    gaps.push(RoundTripGap {
                        path: format!("{module_name}.{entity_name}.{}", callback.event),
                        reason: format!(
                            "lifecycle handler {:?} cannot be resolved",
                            callback.handler
                        ),
                    });
                }
            }
            let entity_associations = domain_model
                .all_associations()
                .filter(|association| association.from_entity_id.as_deref() == entity.id.as_deref())
                .collect::<Vec<_>>();
            if !access_rules_renderable(entity, &entity_associations, &entity_qualified_name_by_id)
            {
                gaps.push(RoundTripGap {
                    path: format!("{module_name}.{entity_name}.access_rules"),
                    reason: "access rule contains an unresolved member or rights value".to_string(),
                });
            }
            if entity.access_rules.iter().any(|rule| {
                !document_has_only(
                    &rule.raw,
                    &[
                        "$ID",
                        "$Type",
                        "Documentation",
                        "AllowedModuleRoles",
                        "ModuleRoles",
                        "AllowCreate",
                        "AllowDelete",
                        "DefaultMemberAccessRights",
                        "MemberAccesses",
                        "XPathConstraint",
                        "XPathConstraintCaption",
                    ],
                ) || rule.members.iter().any(|member| {
                    !document_has_only(
                        &member.raw,
                        &["$ID", "$Type", "Association", "Attribute", "AccessRights"],
                    )
                })
            }) {
                gaps.push(RoundTripGap {
                    path: format!("{module_name}.{entity_name}.access_rules"),
                    reason: "access rule contains native fields outside the typed declaration"
                        .to_string(),
                });
            }
            if entity.oql_view() {
                if entity.persistable {
                    gaps.push(RoundTripGap {
                        path: format!("{module_name}.{entity_name}.persistable"),
                        reason: "OQL view entity is marked persistable".to_string(),
                    });
                }
                match entity.oql_source_document() {
                    Some(source) if known_oql_sources.contains(&source) => {}
                    Some(source) => gaps.push(RoundTripGap {
                        path: format!("{module_name}.{entity_name}.source"),
                        reason: format!("OQL view source {source:?} cannot be resolved"),
                    }),
                    None => gaps.push(RoundTripGap {
                        path: format!("{module_name}.{entity_name}.source"),
                        reason: "OQL view has no source document reference".to_string(),
                    }),
                }
                if entity.source.as_ref().is_some_and(|source| {
                    !document_has_only(
                        source,
                        &["$ID", "$Type", "SourceDocument", "sourceDocument"],
                    )
                }) {
                    gaps.push(RoundTripGap {
                        path: format!("{module_name}.{entity_name}.source"),
                        reason: "view source contains native fields outside the typed declaration"
                            .to_string(),
                    });
                }
            }
        }
        for source in module.artifact_units.iter().filter(|document| {
            document.get_str("$Type").ok() == Some("DomainModels$ViewEntitySourceDocument")
        }) {
            if !document_has_only(
                source,
                &[
                    "$ID",
                    "$Type",
                    "$QualifiedName",
                    "Name",
                    "Oql",
                    "Documentation",
                    "Excluded",
                    "ExportLevel",
                ],
            ) || !matches!(
                source.get_str("ExportLevel").unwrap_or("Hidden"),
                "Hidden" | "Published"
            ) {
                gaps.push(RoundTripGap {
                    path: format!(
                        "{module_name}.{}",
                        source.get_str("Name").unwrap_or("Unnamed")
                    ),
                    reason:
                        "OQL source document contains native fields outside the typed declaration"
                            .to_string(),
                });
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

fn document_has_only(document: &mxrs_bson::Document, allowed: &[&str]) -> bool {
    document.is_empty() || document.keys().all(|key| allowed.contains(&key.as_str()))
}

fn known_oql_sources(modules: &[Module]) -> std::collections::HashSet<String> {
    modules
        .iter()
        .flat_map(|module| {
            let module_name = module.name.as_deref().unwrap_or("Unnamed");
            module.artifact_units.iter().filter_map(move |document| {
                if document.get_str("$Type").ok() != Some("DomainModels$ViewEntitySourceDocument") {
                    return None;
                }
                Some(format!("{module_name}.{}", document.get_str("Name").ok()?))
            })
        })
        .collect()
}

fn member_rights_variant(value: &str) -> Option<&'static str> {
    match value {
        "None" => Some("None"),
        "ReadOnly" => Some("ReadOnly"),
        "ReadWrite" => Some("ReadWrite"),
        _ => None,
    }
}

fn association_target_name(
    association: &Association,
    entity_qualified_name_by_id: &HashMap<String, String>,
) -> Option<String> {
    association.to_entity_id.as_deref().and_then(|id_or_name| {
        entity_qualified_name_by_id
            .get(id_or_name)
            .cloned()
            .or_else(|| {
                entity_qualified_name_by_id
                    .values()
                    .find(|qualified_name| qualified_name.as_str() == id_or_name)
                    .cloned()
            })
    })
}

fn access_rules_renderable(
    entity: &Entity,
    associations: &[&Association],
    entity_qualified_name_by_id: &HashMap<String, String>,
) -> bool {
    let qualified_entity = entity.qualified_name.as_deref().unwrap_or_default();
    let module_name = qualified_entity.split('.').next().unwrap_or_default();
    entity.access_rules.iter().all(|rule| {
        (rule.raw.is_empty() || rule.raw.get_str("$Type").ok() == Some("DomainModels$AccessRule"))
            && !rule.roles.is_empty()
            && member_rights_variant(&rule.default_rights).is_some()
            && rule.members.iter().all(|member| {
                (member.raw.is_empty()
                    || member.raw.get_str("$Type").ok() == Some("DomainModels$MemberAccess"))
                    && member_rights_variant(&member.rights).is_some()
                    && match member.kind {
                        mxrs_model::entity::AccessMemberKind::Attribute => {
                            entity.attributes.iter().any(|attribute| {
                                attribute.name.as_deref() == Some(&member.name)
                                    && member.reference
                                        == format!("{qualified_entity}.{}", member.name)
                            })
                        }
                        mxrs_model::entity::AccessMemberKind::Association => {
                            associations.iter().any(|association| {
                                association.name.as_deref() == Some(&member.name)
                                    && member.reference == format!("{module_name}.{}", member.name)
                                    // A rule refers to the marker type that the association
                                    // declaration creates.  Do not emit a typed rule for an
                                    // association whose target remains only in the lossless
                                    // native model; otherwise `project!` references a marker
                                    // which is never generated.
                                    && association_target_name(
                                        association,
                                        entity_qualified_name_by_id,
                                    )
                                    .is_some()
                            })
                        }
                    }
            })
    })
}

fn known_microflows(modules: &[Module]) -> std::collections::HashSet<String> {
    modules
        .iter()
        .flat_map(|module| {
            let module_name = module.name.as_deref().unwrap_or("Unnamed");
            module
                .microflows
                .iter()
                .filter_map(move |flow| Some(format!("{module_name}.{}", flow.name.as_deref()?)))
        })
        .collect()
}

fn built_in_generalization_path(target: &str) -> Option<&'static str> {
    match target {
        "System.User" => Some("::mxrs_ir::system::User"),
        "System.FileDocument" => Some("::mxrs_ir::system::FileDocument"),
        "System.Image" => Some("::mxrs_ir::system::Image"),
        _ => None,
    }
}

fn render(
    mendix_version: &str,
    modules: &[Module],
    pages: &[page_export::ConvertedPage],
) -> String {
    let entity_qualified_name_by_id = index_entities_by_id(modules);
    let known_microflows = known_microflows(modules);

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
            "//! Also wires every page `src/presentation/pages/mod.rs` defines into its"
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
            out.push_str(&render_module(
                module,
                &entity_qualified_name_by_id,
                &known_microflows,
            ));
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
        out.push_str(&render_module(
            module,
            &entity_qualified_name_by_id,
            &known_microflows,
        ));
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

fn render_module(
    module: &Module,
    entity_qualified_name_by_id: &HashMap<String, String>,
    known_microflows: &std::collections::HashSet<String>,
) -> String {
    let module_name = module.name.as_deref().unwrap_or("Unnamed");
    let mut out = String::new();
    let _ = writeln!(out, "        module {} {{", sanitize_ident(module_name));

    let mut roles = module.module_roles.iter().collect::<Vec<_>>();
    roles.sort_by(|left, right| left.name.cmp(&right.name));
    for role in roles {
        let _ = writeln!(
            out,
            "            role {} {:?};",
            sanitize_ident(role.name.as_deref().unwrap_or("Unnamed")),
            role.description,
        );
    }
    let mut oql_sources = module
        .artifact_units
        .iter()
        .filter(|document| {
            document.get_str("$Type").ok() == Some("DomainModels$ViewEntitySourceDocument")
        })
        .collect::<Vec<_>>();
    oql_sources.sort_by_key(|document| document.get_str("Name").unwrap_or_default());
    for source in oql_sources {
        let name = sanitize_ident(source.get_str("Name").unwrap_or("Unnamed"));
        let query = source.get_str("Oql").unwrap_or_default();
        let documentation = source.get_str("Documentation").unwrap_or_default();
        let excluded = source.get_bool("Excluded").unwrap_or(false);
        let export_level = source.get_str("ExportLevel").unwrap_or("Hidden");
        let has_options = !documentation.is_empty() || excluded || export_level != "Hidden";
        if has_options {
            let _ = writeln!(out, "            oql_view_source {name} {query:?} {{");
            if !documentation.is_empty() {
                let _ = writeln!(out, "                documentation {documentation:?};");
            }
            if excluded {
                let _ = writeln!(out, "                excluded true;");
            }
            if export_level == "Published" {
                let _ = writeln!(out, "                export_level Published;");
            }
            let _ = writeln!(out, "            }}");
        } else {
            let _ = writeln!(out, "            oql_view_source {name} {query:?};");
        }
    }

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
            module_name,
            entity,
            associations_by_entity_id
                .get(entity.id.as_deref().unwrap_or(""))
                .map(Vec::as_slice)
                .unwrap_or(&[]),
            entity_qualified_name_by_id,
            known_microflows,
        ));
    }
    let _ = writeln!(out, "        }}");
    out
}

fn render_entity(
    module_name: &str,
    entity: &Entity,
    associations: &[&Association],
    entity_qualified_name_by_id: &HashMap<String, String>,
    known_microflows: &std::collections::HashSet<String>,
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
    // `oql_view` is non-persistable by definition in the authoring DSL. Some
    // native projects still carry a contradictory legacy flag, which must not
    // make the generated Cargo project fail to compile.
    //
    // This is the one place the exporter *normalizes* rather than preserves:
    // a native view carrying `persistable true` comes back out of a
    // round trip as `persistable false`, because the pair has no consistent
    // meaning to preserve. Mendix itself never reads a view as persistable,
    // so nothing observable is lost — but the model bytes do change, which is
    // why it is stated here rather than left for a diff to surprise someone.
    let _ = writeln!(
        out,
        "                persistable {};",
        entity.persistable && !entity.oql_view()
    );
    if let Some(image) = entity.image.as_deref().filter(|image| !image.is_empty()) {
        let _ = writeln!(out, "                image {image:?};");
    } else {
        let _ = writeln!(out, "                clear_image;");
    }
    if entity.oql_view() {
        if let Some(source) = entity.oql_source_document() {
            let source_path = source
                .split('.')
                .map(sanitize_ident)
                .collect::<Vec<_>>()
                .join("::");
            let _ = writeln!(out, "                oql_view {source_path};");
        }
    } else {
        let _ = writeln!(out, "                stored;");
    }
    if let Some(generalization) = &entity.generalization {
        if let Some(target) = &generalization.target {
            if built_in_generalization_path(target).is_none()
                && !entity_qualified_name_by_id
                    .values()
                    .any(|qualified_name| qualified_name == target)
            {
                // The lossless imported unit remains authoritative until its
                // target can be represented by a marker.
            } else {
                let target_path = if let Some(path) = built_in_generalization_path(target) {
                    path.to_string()
                } else {
                    target
                        .split('.')
                        .map(sanitize_ident)
                        .collect::<Vec<_>>()
                        .join("::")
                };
                let _ = writeln!(out, "                generalizes {target_path};");
            }
        } else if generalization.native_type.ends_with("NoGeneralization") {
            let _ = writeln!(out, "                system_members {{");
            for (name, enabled) in [
                ("owner", generalization.system_members.owner),
                ("created_date", generalization.system_members.created_date),
                ("changed_date", generalization.system_members.changed_date),
                ("changed_by", generalization.system_members.changed_by),
            ] {
                if enabled {
                    let _ = writeln!(out, "                    {name} true;");
                }
            }
            let _ = writeln!(out, "                }}");
        }
    }

    let mut attributes = entity.attributes.clone();
    attributes.sort_by(|a, b| a.name.cmp(&b.name));
    for attribute in &attributes {
        let attribute_name = attribute.name.as_deref().unwrap_or("Unnamed");
        if let Some(keyword) = project_attr_keyword(attribute.attribute_type) {
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
        // A standalone export is rejected by `round_trip_gaps` before
        // rendering. Cargo imports keep the complete native unit in their
        // private preservation layer, so an unsupported attribute must not
        // leak storage detail or a placeholder into editable Rust.
    }

    let mut sorted_associations = associations.to_vec();
    sorted_associations.sort_by_key(|a| a.name.clone());
    for association in sorted_associations {
        let assoc_name = association.name.as_deref().unwrap_or("Unnamed");
        let target = association_target_name(association, entity_qualified_name_by_id);
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

    if entity.indexes.is_empty() {
        let _ = writeln!(out, "                clear_indexes;");
    } else if entity.indexes.iter().all(|index| {
        index.members.iter().all(|member| {
            !matches!(
                member.kind,
                mxrs_model::entity::IndexMemberKind::Unresolved(_)
            )
        })
    }) {
        for index in &entity.indexes {
            let _ = writeln!(out, "                index {{");
            for member in &index.members {
                let ascending = if member.ascending {
                    String::new()
                } else {
                    " ascending false".to_string()
                };
                match &member.kind {
                    mxrs_model::entity::IndexMemberKind::Attribute(name) => {
                        let name = name.rsplit('.').next().unwrap_or(name);
                        let _ = writeln!(
                            out,
                            "                    attribute {}{ascending};",
                            sanitize_ident(name)
                        );
                    }
                    mxrs_model::entity::IndexMemberKind::System(system) => {
                        let _ = writeln!(
                            out,
                            "                    system {}{ascending};",
                            system.native_name()
                        );
                    }
                    mxrs_model::entity::IndexMemberKind::Unresolved(_) => {
                        unreachable!("checked before rendering indexes")
                    }
                }
            }
            if index.include_offline {
                let _ = writeln!(out, "                    include_offline true;");
            }
            let _ = writeln!(out, "                }}");
        }
    }

    let lifecycle_is_complete = entity.lifecycle.iter().all(|callback| {
        matches!(
            callback.event.as_str(),
            "before_commit" | "after_commit" | "before_delete" | "after_delete"
        ) && known_microflows.contains(&callback.handler)
    });
    if entity.lifecycle.is_empty() {
        let _ = writeln!(out, "                clear_lifecycle;");
    } else if lifecycle_is_complete {
        for callback in &entity.lifecycle {
            let handler_path = callback
                .handler
                .split('.')
                .map(sanitize_ident)
                .collect::<Vec<_>>()
                .join("::");
            let _ = writeln!(
                out,
                "                {} crate::infrastructure::markers::{handler_path} {{",
                callback.event
            );
            if !callback.pass_event_object {
                let _ = writeln!(out, "                    pass_event_object false;");
            }
            let default_raise = callback.event.starts_with("before_");
            if callback.raise_error_on_false != default_raise {
                let _ = writeln!(
                    out,
                    "                    raise_error_on_false {};",
                    callback.raise_error_on_false
                );
            }
            let _ = writeln!(out, "                }}");
        }
    }

    if entity.access_rules.is_empty() {
        let _ = writeln!(out, "                clear_access_rules;");
    } else if access_rules_renderable(entity, associations, entity_qualified_name_by_id) {
        for rule in &entity.access_rules {
            let roles = rule
                .roles
                .iter()
                .map(|role| {
                    role.strip_prefix(&format!("{module_name}."))
                        .unwrap_or(role)
                })
                .map(|role| format!("{role:?}"))
                .collect::<Vec<_>>()
                .join(", ");
            let _ = writeln!(out, "                access_rule [{roles}] {{");
            if !rule.documentation.is_empty() {
                let _ = writeln!(
                    out,
                    "                    documentation {:?};",
                    rule.documentation
                );
            }
            if rule.create {
                let _ = writeln!(out, "                    allow_create true;");
            }
            if rule.delete {
                let _ = writeln!(out, "                    allow_delete true;");
            }
            let _ = writeln!(
                out,
                "                    default_rights {};",
                member_rights_variant(&rule.default_rights)
                    .expect("access rules checked before rendering")
            );
            if !rule.xpath.is_empty() {
                let _ = writeln!(out, "                    xpath {:?};", rule.xpath);
            }
            if let Some(caption) = &rule.xpath_caption {
                let _ = writeln!(out, "                    xpath_caption {caption:?};");
            }
            for member in &rule.members {
                let kind = match member.kind {
                    mxrs_model::entity::AccessMemberKind::Attribute => "attribute",
                    mxrs_model::entity::AccessMemberKind::Association => "association",
                };
                let _ = writeln!(
                    out,
                    "                    {kind} {} {};",
                    sanitize_ident(&member.name),
                    member_rights_variant(&member.rights)
                        .expect("access rules checked before rendering")
                );
            }
            let _ = writeln!(out, "                }}");
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

/// Emits one independently editable Rust source file for a Mendix entity.
///
/// The previous import shape placed the entire model in one `project!` macro.
/// That was compact, but it gave a Cargo project the same ergonomics as a
/// generated schema dump.  This renderer deliberately targets the public IR
/// structures instead: each declaration can live in its architectural layer
/// and composition merges its module into the project.  The IR is still the
/// exact authoring boundary consumed by `mxrs-writer`.
fn render_entity_file(
    module_name: &str,
    entity: &Entity,
    known_microflows: &std::collections::HashSet<String>,
    declarable: &[DeclarableAssociation<'_>],
) -> String {
    let entity_name = entity.name.as_deref().unwrap_or("Unnamed");
    let mut out = String::from("//! Editable Mendix entity declaration.\n\n");
    let _ = writeln!(out, "pub fn declaration() -> ::mxrs_ir::ModuleDecl {{");
    let _ = writeln!(
        out,
        "    let mut entity = ::mxrs_ir::EntityDecl::new({entity_name:?});"
    );
    if !entity.documentation.is_empty() {
        let _ = writeln!(
            out,
            "    entity.documentation = {:?}.to_string();",
            entity.documentation
        );
    }
    let _ = writeln!(
        out,
        "    entity.persistable = {};",
        entity.persistable && !entity.oql_view()
    );
    match entity.image.as_deref().filter(|image| !image.is_empty()) {
        Some(image) => {
            let _ = writeln!(
                out,
                "    entity.image = Some(::mxrs_ir::EntityImageDecl::Reference({image:?}.to_string()));"
            );
        }
        None => out.push_str("    entity.image = Some(::mxrs_ir::EntityImageDecl::None);\n"),
    }
    if entity.oql_view() {
        if let Some(source) = entity.oql_source_document() {
            let _ = writeln!(
                out,
                "    entity.source = Some(::mxrs_ir::EntitySourceDecl::OqlView {{ source_document: {source:?}.to_string() }});"
            );
        }
    } else {
        out.push_str("    entity.source = Some(::mxrs_ir::EntitySourceDecl::Stored);\n");
    }

    let mut attributes = entity.attributes.clone();
    attributes.sort_by(|left, right| left.name.cmp(&right.name));
    for attribute in attributes {
        let Some(attribute_type) = project_attr_keyword(attribute.attribute_type) else {
            continue;
        };
        let attribute_name = attribute.name.as_deref().unwrap_or("Unnamed");
        let _ = writeln!(
            out,
            "    entity.attributes.push(::mxrs_ir::AttributeDecl {{"
        );
        let _ = writeln!(out, "        name: {attribute_name:?}.to_string(),");
        let _ = writeln!(
            out,
            "        documentation: {:?}.to_string(),",
            attribute.documentation
        );
        let _ = writeln!(
            out,
            "        attribute_type: ::mxrs_ir::AttributeType::{},",
            match attribute_type {
                "string" => "String",
                "integer" => "Integer",
                "long" => "Long",
                "float" => "Float",
                "decimal" => "Decimal",
                "boolean" => "Boolean",
                "datetime" => "DateTime",
                "autonumber" => "AutoNumber",
                "hash_string" => "HashString",
                "binary" => "Binary",
                "enumeration" => "Enumeration",
                _ => unreachable!("known attribute keyword"),
            }
        );
        let _ = writeln!(
            out,
            "        default_value: {},",
            rust_option_string(attribute.default_value.as_deref())
        );
        let _ = writeln!(out, "        length: {:?},", attribute.length);
        let _ = writeln!(out, "        localize_date: {:?},", attribute.localize_date);
        let _ = writeln!(
            out,
            "        enumeration: {},",
            rust_option_string(attribute.enumeration.as_deref())
        );
        let _ = writeln!(out, "        required: {},", attribute.required);
        let _ = writeln!(out, "        unique: {},", attribute.unique);
        out.push_str("    });\n");
    }

    // Associations between declared entities must be restated — the writer
    // preserves only associations whose target lives outside the project.
    for declared in declarable {
        let association = declared.association;
        let _ = writeln!(
            out,
            "    entity.associations.push(::mxrs_ir::AssociationDecl {{"
        );
        let _ = writeln!(out, "        name: {:?}.to_string(),", declared.name);
        let _ = writeln!(out, "        target: {:?}.to_string(),", declared.target);
        let _ = writeln!(
            out,
            "        association_type: ::mxrs_ir::AssociationType::{},",
            match association.association_type {
                mxrs_model::association::AssociationType::Reference => "Reference",
                mxrs_model::association::AssociationType::ReferenceSet => "ReferenceSet",
            }
        );
        let _ = writeln!(
            out,
            "        owner: ::mxrs_ir::AssociationOwner::{},",
            match association.owner {
                mxrs_model::association::Owner::Default => "Default",
                mxrs_model::association::Owner::Both => "Both",
            }
        );
        let _ = writeln!(
            out,
            "        storage: ::mxrs_ir::AssociationStorage::{},",
            match association.storage_format {
                mxrs_model::association::StorageFormat::Column => "Column",
                mxrs_model::association::StorageFormat::Table => "Table",
            }
        );
        let _ = writeln!(
            out,
            "        documentation: {:?}.to_string(),",
            association.documentation
        );
        out.push_str("    });\n");
    }

    if entity.indexes.is_empty() {
        out.push_str("    entity.indexes = Some(vec![]);\n");
    } else if entity.indexes.iter().all(|index| {
        index.members.iter().all(|member| {
            !matches!(
                member.kind,
                mxrs_model::entity::IndexMemberKind::Unresolved(_)
            )
        })
    }) {
        out.push_str("    entity.indexes = Some(vec![\n");
        for index in &entity.indexes {
            out.push_str("        ::mxrs_ir::EntityIndexDecl { members: vec![\n");
            for member in &index.members {
                match &member.kind {
                    mxrs_model::entity::IndexMemberKind::Attribute(name) => {
                        let name = name.rsplit('.').next().unwrap_or(name);
                        let _ = writeln!(
                            out,
                            "            ::mxrs_ir::IndexMemberDecl::Attribute {{ name: {name:?}.to_string(), ascending: {} }},",
                            member.ascending
                        );
                    }
                    mxrs_model::entity::IndexMemberKind::System(system) => {
                        let system = match system.native_name() {
                            "CreatedDate" => "CreatedDate",
                            "ChangedDate" => "ChangedDate",
                            "Owner" => "Owner",
                            "ChangedBy" => "ChangedBy",
                            _ => unreachable!("known system member"),
                        };
                        let _ = writeln!(
                            out,
                            "            ::mxrs_ir::IndexMemberDecl::System {{ member: ::mxrs_ir::SystemMember::{system}, ascending: {} }},",
                            member.ascending
                        );
                    }
                    mxrs_model::entity::IndexMemberKind::Unresolved(_) => {
                        unreachable!("checked above")
                    }
                }
            }
            let _ = writeln!(
                out,
                "        ], include_offline: {} }},",
                index.include_offline
            );
        }
        out.push_str("    ]);\n");
    }

    let lifecycle_complete = entity.lifecycle.iter().all(|callback| {
        matches!(
            callback.event.as_str(),
            "before_commit" | "after_commit" | "before_delete" | "after_delete"
        ) && known_microflows.contains(&callback.handler)
    });
    if entity.lifecycle.is_empty() {
        out.push_str("    entity.lifecycle = Some(vec![]);\n");
    } else if lifecycle_complete {
        out.push_str("    entity.lifecycle = Some(vec![\n");
        for callback in &entity.lifecycle {
            let event = match callback.event.as_str() {
                "before_commit" => "BeforeCommit",
                "after_commit" => "AfterCommit",
                "before_delete" => "BeforeDelete",
                "after_delete" => "AfterDelete",
                _ => unreachable!("checked above"),
            };
            let _ = writeln!(
                out,
                "        ::mxrs_ir::LifecycleDecl {{ event: ::mxrs_ir::LifecycleEvent::{event}, handler: {:?}.to_string(), pass_event_object: {}, raise_error_on_false: {} }},",
                callback.handler, callback.pass_event_object, callback.raise_error_on_false
            );
        }
        out.push_str("    ]);\n");
    }

    let _ = writeln!(
        out,
        "    let mut module = ::mxrs_ir::ModuleDecl {{ name: {module_name:?}.to_string(), ..Default::default() }};"
    );
    out.push_str("    module.entities.push(entity);\n    module\n}\n");
    out
}

/// One generated service port: the `domain/ports` trait for one Mendix
/// module's runnable microflows, plus everything the runtime adapter impl
/// needs to drive them through `FlowEngine::call`.
struct ServicePort {
    module_name: String,
    trait_name: String,
    file_stem: String,
    methods: Vec<ServiceMethod>,
}

struct ServiceMethod {
    flow_name: String,
    method: String,
    /// `(mendix name, argument ident, marker type, runtime type)` per
    /// parameter, in declaration order.
    parameters: Vec<(String, String, String, String)>,
    /// `(marker type, runtime type)`; `None` returns unit (a void flow).
    result: Option<(String, String)>,
}

impl ServiceMethod {
    fn touches_object_handles(&self) -> bool {
        self.parameters
            .iter()
            .any(|(_, _, _, runtime)| runtime.contains("ObjectHandle"))
            || self
                .result
                .as_ref()
                .is_some_and(|(_, runtime)| runtime.contains("ObjectHandle"))
    }
}

/// The `(marker, runtime)` type pair a port boundary uses for one flow
/// type — mirroring `mxrs::ports::PortValue`'s associations. `None` for
/// types with no port representation (binary, an entity that keeps the IR
/// form, or a DTO the domain layer cannot name).
fn port_type(
    ty: &mxrs_ir::flow::FlowReturnType,
    typed_entities: &HashMap<String, TypedEntityTarget>,
) -> Option<(String, String)> {
    use mxrs_ir::flow::FlowReturnType as Ty;
    Some(match ty {
        Ty::String => ("mxrs::MxString".into(), "String".into()),
        Ty::Integer => ("mxrs::MxInteger".into(), "i32".into()),
        Ty::Long => ("mxrs::MxLong".into(), "i64".into()),
        Ty::Boolean => ("mxrs::MxBool".into(), "bool".into()),
        Ty::Float => ("mxrs::MxFloat".into(), "f64".into()),
        Ty::Decimal => ("mxrs::MxDecimal".into(), "f64".into()),
        Ty::DateTime => ("mxrs::MxDateTime".into(), "f64".into()),
        Ty::Binary => return None,
        Ty::Object(entity) | Ty::List(entity) => {
            let target = typed_entities.get(entity)?;
            if target.dto {
                return None;
            }
            let path = format!(
                "crate::domain::entities::{}::{}",
                target.file_stem, target.type_name
            );
            let handle = format!("ObjectHandle<{path}>");
            if matches!(ty, Ty::Object(_)) {
                (format!("mxrs::MxObject<{path}>"), handle)
            } else {
                (format!("mxrs::MxList<{path}>"), format!("Vec<{handle}>"))
            }
        }
    })
}

/// Collects the microflows each module can expose as a typed service port:
/// every parameter and the return type must have a port representation, and
/// every generated name must survive as a Rust identifier. Anything else
/// stays callable through the engine's string-keyed surface.
fn collect_service_ports(
    modules: &[Module],
    typed_entities: &HashMap<String, TypedEntityTarget>,
) -> Vec<ServicePort> {
    let mut ports = Vec::new();
    for module in modules {
        let Some(module_name) = module.name.as_deref().filter(|name| !name.is_empty()) else {
            continue;
        };
        let trait_base = derive_pascal_case(&sanitize_ident(module_name));
        if trait_base.is_empty() || trait_base.starts_with(|c: char| c.is_ascii_digit()) {
            continue;
        }
        let mut flows: Vec<_> = module.microflows.iter().collect();
        flows.sort_by(|left, right| left.name.cmp(&right.name));
        let mut methods = Vec::new();
        let mut seen = std::collections::HashSet::new();
        'flows: for flow in flows {
            let Some(flow_name) = flow.name.as_deref().filter(|name| !name.is_empty()) else {
                continue;
            };
            let method = snake_ident(flow_name);
            if method.is_empty() || rust_keyword(&method) || !seen.insert(method.clone()) {
                continue;
            }
            let result = match flow.return_type.as_deref() {
                None | Some("DataTypes$VoidType") => None,
                Some(_) => {
                    let Some(mapped) = flow
                        .return_type_document
                        .as_ref()
                        .and_then(flow_export::data_type)
                        .and_then(|ty| port_type(&ty, typed_entities))
                    else {
                        continue;
                    };
                    Some(mapped)
                }
            };
            let mut parameters = Vec::new();
            let mut idents = std::collections::HashSet::new();
            for parameter in &flow.parameters {
                let Some(name) = parameter
                    .get_str("Name")
                    .or_else(|_| parameter.get_str("name"))
                    .ok()
                else {
                    continue 'flows;
                };
                let ident = snake_ident(name);
                if ident.is_empty()
                    || rust_keyword(&ident)
                    || ident == "self"
                    || !idents.insert(ident.clone())
                {
                    continue 'flows;
                }
                let Some((marker, runtime)) = parameter
                    .get_document("VariableType")
                    .or_else(|_| parameter.get_document("variableType"))
                    .ok()
                    .and_then(flow_export::data_type)
                    .and_then(|ty| port_type(&ty, typed_entities))
                else {
                    continue 'flows;
                };
                parameters.push((name.to_string(), ident, marker, runtime));
            }
            methods.push(ServiceMethod {
                flow_name: flow_name.to_string(),
                method,
                parameters,
                result,
            });
        }
        if methods.is_empty() {
            continue;
        }
        let mut stem = sanitize_ident(module_name);
        stem.make_ascii_lowercase();
        ports.push(ServicePort {
            module_name: module_name.to_string(),
            trait_name: format!("{trait_base}Services"),
            file_stem: format!("{stem}_services"),
            methods,
        });
    }
    ports.sort_by(|left, right| left.file_stem.cmp(&right.file_stem));
    ports
}

fn render_service_port(port: &ServicePort) -> String {
    let handles = port
        .methods
        .iter()
        .any(ServiceMethod::touches_object_handles);
    let mut out = format!(
        "//! Service port for the {} Mendix module: each method drives one of\n//! its microflows on the embedded runtime.\n\nuse mxrs::ports::{{{}ServiceError}};\n\npub trait {} {{\n",
        port.module_name,
        if handles { "ObjectHandle, " } else { "" },
        port.trait_name,
    );
    for method in &port.methods {
        let _ = writeln!(
            out,
            "    /// Drives the `{}.{}` microflow.",
            port.module_name, method.flow_name
        );
        let _ = writeln!(
            out,
            "    fn {}(&mut self{}) -> Result<{}, ServiceError>;",
            method.method,
            method
                .parameters
                .iter()
                .map(|(_, ident, _, runtime)| format!(", {ident}: Option<{runtime}>"))
                .collect::<String>(),
            match &method.result {
                Some((_, runtime)) => format!("Option<{runtime}>"),
                None => "()".to_string(),
            },
        );
    }
    out.push_str("}\n");
    out
}

fn render_ports_index(ports: &[ServicePort], actions: &[ActionPortModule]) -> String {
    if ports.is_empty() && actions.is_empty() {
        return "//! Repository and external-service contracts belong here.\n".to_string();
    }
    let mut out = String::from(
        "//! Contracts the domain exposes: a service port per Mendix module\n//! with runnable microflows, and action ports for the Java actions the\n//! model expects hand-written Rust to fulfill. Hand-written repository\n//! and external-service contracts belong here too.\n\n",
    );
    let mut stems: Vec<&str> = ports
        .iter()
        .map(|port| port.file_stem.as_str())
        .chain(actions.iter().map(|module| module.file_stem.as_str()))
        .collect();
    stems.sort_unstable();
    for stem in stems {
        let _ = writeln!(out, "pub mod {stem};");
    }
    out
}

/// The runtime adapter: one struct implementing every generated service
/// port by marshalling through `mxrs::ports` and driving `FlowEngine::call`.
fn render_runtime_services(ports: &[ServicePort]) -> String {
    let handles = ports
        .iter()
        .flat_map(|port| &port.methods)
        .any(ServiceMethod::touches_object_handles);
    let marshals = ports
        .iter()
        .flat_map(|port| &port.methods)
        .any(|method| !method.parameters.is_empty() || method.result.is_some());
    let mut imports = vec!["BootError", "FlowEngine"];
    if handles {
        imports.push("ObjectHandle");
    }
    if marshals {
        imports.push("PortValue");
    }
    imports.extend(["ServiceError", "Variables", "boot"]);
    let mut out = format!(
        "//! Runtime-backed implementations of the domain service ports.\n\nuse mxrs::Store;\nuse mxrs::ports::{{{}}};\n\n/// Drives the model's microflows on the embedded flow engine.\npub struct RuntimeServices {{\n    engine: FlowEngine,\n    store: Store,\n}}\n\nimpl RuntimeServices {{\n    /// Boots the flow engine and store from a built `.mpr` — for example\n    /// the output of `cargo run` or `cargo mxrs build`.\n    pub fn from_mpr(path: impl AsRef<std::path::Path>) -> Result<Self, BootError> {{\n        let booted = boot(path)?;\n        Ok(Self::new(\n            FlowEngine::from_modules(&booted.modules).with_policy(booted.security),\n            Store::new(booted.schema),\n        ))\n    }}\n\n    /// Wraps an engine and store the caller assembled.\n    pub fn new(engine: FlowEngine, store: Store) -> Self {{\n        Self {{ engine, store }}\n    }}\n\n    /// The backing store, for reading committed state.\n    pub fn store(&self) -> &Store {{\n        &self.store\n    }}\n\n    /// Mutable access to the backing store, for seeding data.\n    pub fn store_mut(&mut self) -> &mut Store {{\n        &mut self.store\n    }}\n}}\n",
        imports.join(", "),
    );
    for port in ports {
        let _ = writeln!(
            out,
            "\nimpl crate::domain::ports::{}::{} for RuntimeServices {{",
            port.file_stem, port.trait_name
        );
        for (index, method) in port.methods.iter().enumerate() {
            if index > 0 {
                out.push('\n');
            }
            let _ = writeln!(
                out,
                "    fn {}(&mut self{}) -> Result<{}, ServiceError> {{",
                method.method,
                method
                    .parameters
                    .iter()
                    .map(|(_, ident, _, runtime)| format!(", {ident}: Option<{runtime}>"))
                    .collect::<String>(),
                match &method.result {
                    Some((_, runtime)) => format!("Option<{runtime}>"),
                    None => "()".to_string(),
                },
            );
            let binding = if method.parameters.is_empty() {
                "let arguments"
            } else {
                "let mut arguments"
            };
            let _ = writeln!(out, "        {binding} = Variables::new();");
            for (name, ident, marker, _) in &method.parameters {
                let _ = writeln!(
                    out,
                    "        arguments.insert({name:?}.to_string(), <{marker} as PortValue>::to_flow_optional({ident}));"
                );
            }
            match &method.result {
                Some((marker, _)) => {
                    let _ = writeln!(
                        out,
                        "        let (result, _) = self.engine.call(&mut self.store, {:?}, arguments, None)?;",
                        format!("{}.{}", port.module_name, method.flow_name),
                    );
                    let _ = writeln!(
                        out,
                        "        Ok(<{marker} as PortValue>::from_flow_optional(result)?)"
                    );
                }
                None => {
                    let _ = writeln!(
                        out,
                        "        self.engine.call(&mut self.store, {:?}, arguments, None)?;",
                        format!("{}.{}", port.module_name, method.flow_name),
                    );
                    out.push_str("        Ok(())\n");
                }
            }
            out.push_str("    }\n");
        }
        out.push_str("}\n");
    }
    out
}

/// One module's action ports: the Rust contracts its model-declared Java
/// actions expect hand-written code to fulfill, plus the registration glue
/// that adapts an implementation onto `FlowEngine::with_java_action`.
struct ActionPortModule {
    module_name: String,
    file_stem: String,
    actions: Vec<ActionPort>,
}

struct ActionPort {
    action_name: String,
    trait_name: String,
    register_fn: String,
    /// `(mendix name, argument ident, marker type, runtime type)` per
    /// parameter, in declaration order.
    parameters: Vec<(String, String, String, String)>,
    /// `(marker type, runtime type)`; `None` returns unit (a void action).
    result: Option<(String, String)>,
}

impl ActionPort {
    fn touches_object_handles(&self) -> bool {
        self.parameters
            .iter()
            .any(|(_, _, _, runtime)| runtime.contains("ObjectHandle"))
            || self
                .result
                .as_ref()
                .is_some_and(|(_, runtime)| runtime.contains("ObjectHandle"))
    }
}

/// The `(marker, runtime)` port pair for one `CodeActions$*` type document.
/// `None` when the type has no port representation (enumerations, generic
/// entity types, or an entity that keeps the IR form).
fn code_action_port_type(
    doc: &mxrs_bson::Document,
    typed_entities: &HashMap<String, TypedEntityTarget>,
) -> Option<(String, String)> {
    let raw = doc.get_str("$Type").ok()?;
    let kind = raw
        .strip_prefix("JavaActions$")
        .map(|rest| format!("CodeActions${rest}"))
        .unwrap_or_else(|| raw.to_string());
    Some(match kind.as_str() {
        "CodeActions$StringType" => ("mxrs::MxString".into(), "String".into()),
        "CodeActions$BooleanType" => ("mxrs::MxBool".into(), "bool".into()),
        "CodeActions$IntegerType" | "CodeActions$LongType" => ("mxrs::MxLong".into(), "i64".into()),
        "CodeActions$DecimalType" | "CodeActions$FloatType" => {
            ("mxrs::MxDecimal".into(), "f64".into())
        }
        "CodeActions$DateTimeType" => ("mxrs::MxDateTime".into(), "f64".into()),
        "CodeActions$ConcreteEntityType" => {
            let target = typed_entities.get(doc.get_str("Entity").ok()?)?;
            if target.dto {
                return None;
            }
            let path = format!(
                "crate::domain::entities::{}::{}",
                target.file_stem, target.type_name
            );
            (
                format!("mxrs::MxObject<{path}>"),
                format!("ObjectHandle<{path}>"),
            )
        }
        "CodeActions$ListType" => {
            let parameter = doc.get_document("Parameter").ok()?;
            let raw_element = parameter.get_str("$Type").ok()?;
            let element = raw_element
                .strip_prefix("JavaActions$")
                .map(|rest| format!("CodeActions${rest}"))
                .unwrap_or_else(|| raw_element.to_string());
            if element != "CodeActions$ConcreteEntityType" {
                return None;
            }
            let target = typed_entities.get(parameter.get_str("Entity").ok()?)?;
            if target.dto {
                return None;
            }
            let path = format!(
                "crate::domain::entities::{}::{}",
                target.file_stem, target.type_name
            );
            (
                format!("mxrs::MxList<{path}>"),
                format!("Vec<ObjectHandle<{path}>>"),
            )
        }
        _ => return None,
    })
}

/// Every `JavaActions$JavaAction` declaration and the module that owns it,
/// read while the project is still open.
fn collect_action_documents(project: &Project) -> Result<Vec<(String, mxrs_bson::Document)>> {
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
    let mut documents = Vec::new();
    for unit in &units {
        let document = project
            .mpr()
            .parse_contents(unit)
            .map_err(mxrs_model::ModelError::from)?;
        if document.get_str("$Type").ok() != Some("JavaActions$JavaAction") {
            continue;
        }
        let Some(module) = owning_module(&unit.container_id, &parent_by_id, &module_by_id) else {
            continue;
        };
        documents.push((module, document));
    }
    Ok(documents)
}

/// Assembles the Java actions each module declares with a fully port-typed
/// signature: basic parameters and a concrete return type only. Actions
/// with string templates, generics, microflow parameters, enumerations or
/// unresolvable entities stay on the engine's string-keyed registration.
fn assemble_action_ports(
    documents: &[(String, mxrs_bson::Document)],
    typed_entities: &HashMap<String, TypedEntityTarget>,
) -> Vec<ActionPortModule> {
    let mut by_module: HashMap<String, Vec<ActionPort>> = HashMap::new();
    for (module, document) in documents {
        let Some(action) = action_port(document, typed_entities) else {
            continue;
        };
        by_module.entry(module.clone()).or_default().push(action);
    }
    let mut modules: Vec<ActionPortModule> = by_module
        .into_iter()
        .filter_map(|(module_name, mut actions)| {
            let mut stem = sanitize_ident(&module_name);
            stem.make_ascii_lowercase();
            if stem.is_empty() || stem.starts_with(|c: char| c.is_ascii_digit()) {
                return None;
            }
            actions.sort_by(|left, right| left.action_name.cmp(&right.action_name));
            let mut seen = std::collections::HashSet::new();
            actions.retain(|action| seen.insert(action.trait_name.clone()));
            Some(ActionPortModule {
                module_name,
                file_stem: format!("{stem}_actions"),
                actions,
            })
        })
        .collect();
    modules.sort_by(|left, right| left.file_stem.cmp(&right.file_stem));
    modules
}

fn action_port(
    document: &mxrs_bson::Document,
    typed_entities: &HashMap<String, TypedEntityTarget>,
) -> Option<ActionPort> {
    let action_name = document
        .get_str("Name")
        .ok()
        .filter(|name| !name.is_empty())?;
    let trait_name = derive_pascal_case(&sanitize_ident(action_name));
    if trait_name.is_empty()
        || trait_name.starts_with(|c: char| c.is_ascii_digit())
        || rust_keyword(&trait_name)
    {
        return None;
    }
    let register_fn = format!("register_{}", snake_ident(action_name));
    // Generic actions need type arguments the port cannot express.
    if document
        .get_array("TypeParameters")
        .ok()
        .is_some_and(|parameters| {
            !mxrs_bson::parse_array(Some(parameters.as_slice()))
                .items
                .is_empty()
        })
    {
        return None;
    }
    let result = match document.get_document("JavaReturnType").ok() {
        None => None,
        Some(return_doc) => {
            let raw = return_doc.get_str("$Type").ok()?;
            if raw == "CodeActions$VoidType" || raw == "JavaActions$VoidType" {
                None
            } else {
                Some(code_action_port_type(return_doc, typed_entities)?)
            }
        }
    };
    let mut parameters = Vec::new();
    let mut idents = std::collections::HashSet::new();
    for parameter in
        mxrs_bson::parse_array(document.get_array("Parameters").ok().map(Vec::as_slice)).items
    {
        let mxrs_bson::Bson::Document(parameter) = parameter else {
            continue;
        };
        let name = parameter
            .get_str("Name")
            .ok()
            .filter(|name| !name.is_empty())?;
        let ident = snake_ident(name);
        if ident.is_empty()
            || rust_keyword(&ident)
            || ident == "self"
            || ident == "engine"
            || !idents.insert(ident.clone())
        {
            return None;
        }
        let parameter_type = parameter.get_document("ParameterType").ok()?;
        if parameter_type.get_str("$Type").ok() != Some("CodeActions$BasicParameterType") {
            return None;
        }
        let (marker, runtime) =
            code_action_port_type(parameter_type.get_document("Type").ok()?, typed_entities)?;
        parameters.push((name.to_string(), ident, marker, runtime));
    }
    Some(ActionPort {
        action_name: action_name.to_string(),
        trait_name,
        register_fn,
        parameters,
        result,
    })
}

fn render_action_port_file(module: &ActionPortModule) -> String {
    let handles = module
        .actions
        .iter()
        .any(ActionPort::touches_object_handles);
    let mut out = format!(
        "//! Action ports for the {} Mendix module: the contracts its declared\n//! Java actions expect hand-written Rust to fulfill.\n\nuse mxrs::ports::{{{}ServiceError}};\n",
        module.module_name,
        if handles { "ObjectHandle, " } else { "" },
    );
    for action in &module.actions {
        let _ = writeln!(
            out,
            "\n/// Contract of the `{}.{}` Java action.\npub trait {} {{\n    fn call(&self{}) -> Result<{}, ServiceError>;\n}}",
            module.module_name,
            action.action_name,
            action.trait_name,
            action
                .parameters
                .iter()
                .map(|(_, ident, _, runtime)| format!(", {ident}: Option<{runtime}>"))
                .collect::<String>(),
            match &action.result {
                Some((_, runtime)) => format!("Option<{runtime}>"),
                None => "()".to_string(),
            },
        );
    }
    out
}

fn render_action_registry_file(module: &ActionPortModule, ports_stem: &str) -> String {
    let uses_port_value = module
        .actions
        .iter()
        .any(|action| !action.parameters.is_empty() || action.result.is_some());
    let mut out = format!(
        "//! Registers hand-written `{ports}` implementations on the flow\n//! engine under the names the model calls them by.\n\nuse std::collections::BTreeMap;\n\nuse mxrs::ports::{{FlowEngine, FlowError, FlowValue, JavaAction{port_value}}};\n\nuse crate::domain::ports::{ports}::*;\n",
        ports = ports_stem,
        port_value = if uses_port_value { ", PortValue" } else { "" },
    );
    for action in &module.actions {
        let qualified = format!("{}.{}", module.module_name, action.action_name);
        let arguments_ident = if action.parameters.is_empty() {
            "_arguments"
        } else {
            "arguments"
        };
        let _ = write!(
            out,
            "\n/// Registers a [`{trait_name}`] implementation for `{qualified}`.\npub fn {register}(\n    engine: FlowEngine,\n    action: impl {trait_name} + Send + Sync + 'static,\n) -> FlowEngine {{\n    struct Adapter<T>(T);\n    impl<T: {trait_name} + Send + Sync> JavaAction for Adapter<T> {{\n        fn call(&self, {arguments_ident}: &BTreeMap<String, FlowValue>) -> Result<FlowValue, FlowError> {{\n",
            trait_name = action.trait_name,
            register = action.register_fn,
        );
        for (name, ident, marker, _) in &action.parameters {
            let _ = writeln!(
                out,
                "            let {ident} = <{marker} as PortValue>::from_flow_optional(\n                arguments.get({name:?}).cloned().unwrap_or(FlowValue::Empty),\n            )\n            .map_err(|error| FlowError::native(error.to_string()))?;"
            );
        }
        let call_arguments = action
            .parameters
            .iter()
            .map(|(_, ident, _, _)| ident.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        match &action.result {
            Some((marker, _)) => {
                let _ = writeln!(
                    out,
                    "            let result = self\n                .0\n                .call({call_arguments})\n                .map_err(|error| FlowError::native(error.to_string()))?;\n            Ok(<{marker} as PortValue>::to_flow_optional(result))"
                );
            }
            None => {
                let _ = writeln!(
                    out,
                    "            self.0\n                .call({call_arguments})\n                .map_err(|error| FlowError::native(error.to_string()))?;\n            Ok(FlowValue::Empty)"
                );
            }
        }
        let _ = writeln!(
            out,
            "        }}\n    }}\n    engine.with_java_action({qualified:?}, Adapter(action))\n}}"
        );
    }
    out
}

/// Renders a Mendix entity as a typed `#[derive(MxEntity)]` struct — the
/// same authoring surface a hand author uses — instead of an imperative IR
/// dump. Returns `None` when the entity carries something the derive cannot
/// yet express (an image, an OQL view source, indexes, lifecycle callbacks,
/// or names that do not survive as Rust identifiers); those fall back to
/// [`render_entity_file`], so nothing is lost, only less eloquent.
///
/// Equivalence with the IR renderer is deliberate, not incidental: the
/// derive lowers to the same `mxrs-dsl` builder calls, attribute defaults
/// (`None`, `false`) match [`::mxrs_ir::AttributeDecl::new`], and the
/// eligibility gate excludes exactly the fields where "preserve imported"
/// and "explicitly empty" could diverge.
fn render_typed_entity_file(
    module_name: &str,
    entity: &Entity,
    derived_enums: &HashMap<String, DerivedEnumeration>,
    ctx: &EntityLayerContext<'_>,
) -> Option<String> {
    let entity_name = entity.name.as_deref().filter(|name| !name.is_empty())?;
    if entity.oql_view()
        || entity
            .image
            .as_deref()
            .is_some_and(|image| !image.is_empty())
        || !entity.indexes.is_empty()
        || !entity.lifecycle.is_empty()
    {
        return None;
    }
    // The struct is UpperCamelCase like any Rust type; when Mendix's own
    // name differs (`Custom_FormData`), `name = "..."` keeps the model's.
    let mut type_name = derive_pascal_case(&sanitize_ident(entity_name));
    if type_name.starts_with(|c: char| c.is_ascii_digit()) {
        type_name.insert(0, '_');
    }
    if type_name.is_empty() || rust_keyword(&type_name) {
        return None;
    }

    let mut attributes = entity.attributes.clone();
    attributes.sort_by(|left, right| left.name.cmp(&right.name));

    // Associations between declared entities become Reference<T> fields on
    // this struct; every target must itself be a typed struct this file can
    // name, and a domain entity cannot reach into the application layer.
    let own_qualified = format!("{module_name}.{entity_name}");
    let own_dto = !entity.persistable;
    let declarable = declarable_associations(entity, ctx);
    for declared in &declarable {
        if declared.target == own_qualified {
            continue;
        }
        let target = ctx.typed.get(&declared.target)?;
        if !own_dto && target.dto {
            return None;
        }
    }

    // A referenced Rust type — a derived enum, or the target struct of an
    // association — is imported once by its short name; a name that
    // collides with another import, this entity, or an item `declaration()`
    // needs from the prelude is spelled by its full path instead.
    let mut requests: Vec<(&str, String)> = Vec::new();
    for attribute in &attributes {
        let Some(derived) = attribute
            .enumeration
            .as_deref()
            .and_then(|qualified| derived_enums.get(qualified))
        else {
            continue;
        };
        if enum_type_shadows_scalar(&derived.type_name) {
            continue;
        }
        requests.push((
            &derived.type_name,
            format!("crate::domain::enumerations::{}", derived.file_stem),
        ));
    }
    for declared in &declarable {
        if declared.target == own_qualified {
            continue;
        }
        let target = ctx.typed.get(&declared.target)?;
        if enum_type_shadows_scalar(&target.type_name) {
            // The full path is unambiguous for an association target; only
            // the bare import would shadow a prelude scalar.
            continue;
        }
        requests.push((&target.type_name, typed_entity_module_path(target)));
    }
    let mut imported: std::collections::BTreeMap<&str, &str> = std::collections::BTreeMap::new();
    let mut colliding = std::collections::HashSet::new();
    for (name, path) in &requests {
        if let Some(existing) = imported.insert(name, path.as_str())
            && existing != path.as_str()
        {
            colliding.insert(*name);
        }
    }
    imported.retain(|name, _| {
        !colliding.contains(name)
            && *name != type_name
            && !matches!(
                *name,
                "ModuleBuilder" | "ModuleDecl" | "MxEntity" | "Reference" | "ReferenceSet"
            )
    });
    let spell = |name: &str, path: &str| -> String {
        if imported.get(name).is_some_and(|found| *found == path) {
            name.to_string()
        } else {
            format!("{path}::{name}")
        }
    };
    let enum_spellings: HashMap<&str, String> = attributes
        .iter()
        .filter_map(|attribute| {
            let qualified = attribute.enumeration.as_deref()?;
            let derived = derived_enums.get(qualified)?;
            if enum_type_shadows_scalar(&derived.type_name) {
                return None;
            }
            let path = format!("crate::domain::enumerations::{}", derived.file_stem);
            Some((qualified, spell(&derived.type_name, &path)))
        })
        .collect();

    let mut seen = std::collections::HashSet::new();
    let mut fields = String::new();
    for attribute in &attributes {
        let kind = project_attr_keyword(attribute.attribute_type)?;
        let mendix_name = attribute.name.as_deref().filter(|name| !name.is_empty())?;
        let field = snake_ident(mendix_name);
        if rust_keyword(&field) || !seen.insert(field.clone()) {
            return None;
        }
        let mut options = Vec::new();
        if derive_pascal_case(&field) != mendix_name {
            options.push(format!("name = {mendix_name:?}"));
        }
        // The kinds the derive infers from the field type stay implicit.
        let (scalar_type, explicit_kind) = match kind {
            "string" => ("MxString", false),
            "integer" => ("MxInteger", false),
            "long" => ("MxLong", false),
            "float" => ("MxFloat", false),
            "decimal" => ("MxDecimal", false),
            "boolean" => ("MxBool", false),
            "datetime" => ("MxDateTime", false),
            "autonumber" => ("MxLong", true),
            "hash_string" => ("MxString", true),
            "binary" => ("Vec<u8>", true),
            "enumeration" => ("MxString", true),
            _ => return None,
        };
        let mut field_type = scalar_type.to_string();
        if kind == "enumeration" {
            let enumeration = attribute
                .enumeration
                .as_deref()
                .filter(|name| !name.is_empty())?;
            match enum_spellings.get(enumeration) {
                Some(spelling) => field_type = spelling.clone(),
                None => {
                    options.push(format!("kind = {kind:?}"));
                    options.push(format!("enumeration = {enumeration:?}"));
                }
            }
        } else if explicit_kind {
            options.push(format!("kind = {kind:?}"));
        }
        if let Some(default) = attribute.default_value.as_deref() {
            options.push(format!("default = {default:?}"));
        }
        if !attribute.documentation.is_empty() {
            options.push(format!("documentation = {:?}", attribute.documentation));
        }
        if let Some(length) = attribute.length {
            options.push(format!("length = {length}"));
        }
        if let Some(localize_date) = attribute.localize_date {
            options.push(format!("localize_date = {localize_date}"));
        }
        if attribute.required {
            options.push("required".to_string());
        }
        if attribute.unique {
            options.push("unique".to_string());
        }
        if !options.is_empty() {
            let _ = writeln!(fields, "    #[mxrs({})]", options.join(", "));
        }
        let _ = writeln!(fields, "    pub {field}: {field_type},");
    }

    for declared in &declarable {
        // The derive names an association `{Entity}_{PascalField}` by
        // default; a name of that shape round-trips from the field alone.
        let (field, explicit_name) = match declared
            .name
            .strip_prefix(&format!("{entity_name}_"))
            .filter(|rest| !rest.is_empty())
        {
            Some(rest) => {
                let field = snake_ident(rest);
                let explicit = derive_pascal_case(&field) != rest;
                (field, explicit)
            }
            None => (snake_ident(declared.name), true),
        };
        if field.is_empty() || rust_keyword(&field) || !seen.insert(field.clone()) {
            return None;
        }
        let spelling = if declared.target == own_qualified {
            type_name.clone()
        } else {
            let target = ctx.typed.get(&declared.target)?;
            spell(&target.type_name, &typed_entity_module_path(target))
        };
        let container = match declared.association.association_type {
            mxrs_model::association::AssociationType::Reference => "Reference",
            mxrs_model::association::AssociationType::ReferenceSet => "ReferenceSet",
        };
        let mut options = Vec::new();
        if explicit_name {
            options.push(format!("association = {:?}", declared.name));
        }
        if !declared.association.documentation.is_empty() {
            options.push(format!(
                "documentation = {:?}",
                declared.association.documentation
            ));
        }
        if matches!(
            declared.association.owner,
            mxrs_model::association::Owner::Both
        ) {
            options.push("owner = \"Both\"".to_string());
        }
        if matches!(
            declared.association.storage_format,
            mxrs_model::association::StorageFormat::Table
        ) {
            options.push("storage = \"Table\"".to_string());
        }
        if !options.is_empty() {
            let _ = writeln!(fields, "    #[mxrs({})]", options.join(", "));
        }
        let _ = writeln!(fields, "    pub {field}: {container}<{spelling}>,");
    }

    let mut out =
        String::from("//! Editable Mendix entity declaration.\n\nuse mxrs::prelude::*;\n");
    if !imported.is_empty() {
        out.push('\n');
        for (name, path) in &imported {
            let _ = writeln!(out, "use {path}::{name};");
        }
    }
    out.push('\n');
    out.push_str("#[derive(MxEntity)]\n");
    let mut entity_options = vec![format!("module = {module_name:?}")];
    if type_name != entity_name {
        entity_options.insert(0, format!("name = {entity_name:?}"));
    }
    if !entity.documentation.is_empty() {
        entity_options.push(format!("documentation = {:?}", entity.documentation));
    }
    if !entity.persistable {
        entity_options.push("persistable = false".to_string());
    }
    let _ = writeln!(out, "#[mxrs({})]", entity_options.join(", "));
    if fields.is_empty() {
        let _ = writeln!(out, "pub struct {type_name} {{}}");
    } else {
        let _ = writeln!(out, "pub struct {type_name} {{\n{fields}}}");
    }
    let _ = writeln!(
        out,
        "\npub fn declaration() -> ModuleDecl {{\n    let mut module = ModuleBuilder::new({module_name:?});\n    {type_name}::mx_register(&mut module);\n    module.into_decl()\n}}"
    );
    Some(out)
}

/// The module path a typed entity's struct is importable from.
fn typed_entity_module_path(target: &TypedEntityTarget) -> String {
    if target.dto {
        format!("crate::application::dto::{}", target.file_stem)
    } else {
        format!("crate::domain::entities::{}", target.file_stem)
    }
}

/// Enum type names the entity derive would mistake for a scalar kind when
/// inferring the attribute from the field type; entities referencing an
/// enumeration spelled like one of these keep the string form.
fn enum_type_shadows_scalar(name: &str) -> bool {
    matches!(
        name,
        "String"
            | "MxString"
            | "MxInteger"
            | "MxLong"
            | "MxFloat"
            | "MxDecimal"
            | "MxBool"
            | "MxDateTime"
            | "MxBinary"
            | "Vec"
            | "Option"
            | "MxEnumeration"
    )
}

/// The strict and reserved Rust keywords a generated identifier must avoid.
fn rust_keyword(ident: &str) -> bool {
    matches!(
        ident,
        "as" | "async"
            | "await"
            | "break"
            | "const"
            | "continue"
            | "crate"
            | "dyn"
            | "else"
            | "enum"
            | "extern"
            | "false"
            | "fn"
            | "for"
            | "gen"
            | "if"
            | "impl"
            | "in"
            | "let"
            | "loop"
            | "match"
            | "mod"
            | "move"
            | "mut"
            | "pub"
            | "ref"
            | "return"
            | "self"
            | "Self"
            | "static"
            | "struct"
            | "super"
            | "trait"
            | "true"
            | "try"
            | "type"
            | "union"
            | "unsafe"
            | "use"
            | "where"
            | "while"
            | "abstract"
            | "become"
            | "box"
            | "do"
            | "final"
            | "macro"
            | "override"
            | "priv"
            | "typeof"
            | "unsized"
            | "virtual"
            | "yield"
    )
}

/// A Mendix attribute name as a snake_case Rust field: sanitized first, then
/// an underscore before each case boundary (`ParameterToMeasure` →
/// `parameter_to_measure`, `APIKey` → `api_key`).
fn snake_ident(name: &str) -> String {
    let sanitized = sanitize_ident(name);
    let characters: Vec<char> = sanitized.chars().collect();
    let mut out = String::with_capacity(sanitized.len() + 4);
    for (index, &character) in characters.iter().enumerate() {
        if character.is_uppercase() {
            let after_lower = index > 0
                && (characters[index - 1].is_lowercase() || characters[index - 1].is_ascii_digit());
            let before_lower = index > 0
                && characters[index - 1].is_uppercase()
                && characters.get(index + 1).is_some_and(|c| c.is_lowercase());
            if (after_lower || before_lower) && !out.ends_with('_') {
                out.push('_');
            }
            out.extend(character.to_lowercase());
        } else {
            out.push(character);
        }
    }
    out
}

/// `#[derive(MxEntity)]`'s own field-name fallback, copied so the exporter
/// can omit `name = "..."` exactly when the derive would reconstruct it.
fn derive_pascal_case(field_name: &str) -> String {
    field_name
        .split('_')
        .filter(|segment| !segment.is_empty())
        .map(|segment| {
            let mut chars = segment.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                None => String::new(),
            }
        })
        .collect()
}

fn layer_file_stem(module_name: &str, entity_name: &str, dto: bool) -> String {
    let mut stem = format!(
        "{}_{}",
        sanitize_ident(module_name),
        sanitize_ident(entity_name)
    );
    stem.make_ascii_lowercase();
    if dto && !stem.ends_with("_dto") {
        stem.push_str("_dto");
    }
    stem
}

fn render_entity_layer_index(entries: &[(String, String)]) -> String {
    let mut out = String::from("//! Individually editable model declarations for this layer.\n\n");
    for (module, _) in entries {
        let _ = writeln!(out, "pub mod {module};");
    }
    let project = if entries.is_empty() {
        "_project"
    } else {
        "project"
    };
    let _ = writeln!(
        out,
        "\npub fn apply({project}: &mut ::mxrs_ir::ProjectDecl) {{"
    );
    for (module, _) in entries {
        let _ = writeln!(out, "    project.merge_module({module}::declaration());");
    }
    out.push_str("}\n");
    out
}

fn render_flow_layer_index(entries: &[flow_export::RenderedFlowSource]) -> String {
    let mut out = String::from("//! Individually editable flow declarations for this layer.\n\n");
    for entry in entries {
        let _ = writeln!(out, "pub mod {};", entry.file_name);
    }
    out.push_str("\npub fn apply(project: &mut mxrs::ProjectDecl) {\n");
    for entry in entries {
        let _ = writeln!(
            out,
            "    project.merge_module({}::declaration());",
            entry.file_name
        );
    }
    out.push_str("}\n");
    out
}

/// Writes each persisted entity to `domain/entities` and every non-persisted
/// Mendix object to `application/dto`.  File names are deliberately flat:
/// Mendix modules remain model metadata, not a second Rust package hierarchy.
fn write_entity_layer_sources(
    destination: &Path,
    modules: &[Module],
    derived_enums: &HashMap<String, DerivedEnumeration>,
) -> Result<HashMap<String, TypedEntityTarget>> {
    let microflows = known_microflows(modules);
    let mut entities = Vec::new();
    let mut dtos = Vec::new();

    let mut ctx = EntityLayerContext {
        associations_by_entity: HashMap::new(),
        qualified_by_id: HashMap::new(),
        declared: std::collections::HashSet::new(),
        typed: HashMap::new(),
    };
    for module in modules {
        let module_name = module.name.as_deref().unwrap_or("Unnamed");
        for entity in module.entities() {
            let Some(entity_name) = entity.name.as_deref().filter(|name| !name.is_empty()) else {
                continue;
            };
            let qualified = format!("{module_name}.{entity_name}");
            ctx.declared.insert(qualified.clone());
            if let Some(id) = entity.id.as_deref() {
                ctx.qualified_by_id.insert(id, qualified.clone());
            }
            let mut type_name = derive_pascal_case(&sanitize_ident(entity_name));
            if type_name.starts_with(|c: char| c.is_ascii_digit()) {
                type_name.insert(0, '_');
            }
            if type_name.is_empty() || rust_keyword(&type_name) {
                continue;
            }
            let dto = !entity.persistable || entity.oql_view();
            ctx.typed.insert(
                qualified,
                TypedEntityTarget {
                    file_stem: layer_file_stem(module_name, entity_name, dto),
                    type_name,
                    dto,
                },
            );
        }
        for association in module.associations() {
            if let Some(from) = association.from_entity_id.as_deref() {
                ctx.associations_by_entity
                    .entry(from)
                    .or_default()
                    .push(association);
            }
        }
    }
    // Typed eligibility is mutual: a Reference<T> field needs its target to
    // render as a struct too, so entities that fall back to the IR form
    // demote their referrers until the set is stable.
    loop {
        let mut demoted = Vec::new();
        for module in modules {
            let module_name = module.name.as_deref().unwrap_or("Unnamed");
            for entity in module.entities() {
                let Some(entity_name) = entity.name.as_deref() else {
                    continue;
                };
                let qualified = format!("{module_name}.{entity_name}");
                if !ctx.typed.contains_key(&qualified) {
                    continue;
                }
                if render_typed_entity_file(module_name, entity, derived_enums, &ctx).is_none() {
                    demoted.push(qualified);
                }
            }
        }
        if demoted.is_empty() {
            break;
        }
        for qualified in demoted {
            ctx.typed.remove(&qualified);
        }
    }

    for module in modules {
        let module_name = module.name.as_deref().unwrap_or("Unnamed");
        let mut module_entities = module.entities().iter().collect::<Vec<_>>();
        module_entities.sort_by(|left, right| left.name.cmp(&right.name));
        for entity in module_entities {
            let entity_name = entity.name.as_deref().unwrap_or("Unnamed");
            let dto = !entity.persistable || entity.oql_view();
            let stem = layer_file_stem(module_name, entity_name, dto);
            let qualified = format!("{module_name}.{entity_name}");
            let source = if ctx.typed.contains_key(&qualified) {
                render_typed_entity_file(module_name, entity, derived_enums, &ctx)
                    .expect("the fixed point above only keeps renderable entities")
            } else {
                render_entity_file(
                    module_name,
                    entity,
                    &microflows,
                    &declarable_associations(entity, &ctx),
                )
            };
            let directory = if dto {
                destination.join("src/application/dto")
            } else {
                destination.join("src/domain/entities")
            };
            write_text(&directory.join(format!("{stem}.rs")), &source)?;
            let entry = (stem, format!("{module_name}.{entity_name}"));
            if dto {
                dtos.push(entry);
            } else {
                entities.push(entry);
            }
        }
    }
    entities.sort();
    dtos.sort();
    write_text(
        &destination.join("src/domain/entities/mod.rs"),
        &render_entity_layer_index(&entities),
    )?;
    write_text(
        &destination.join("src/application/dto/mod.rs"),
        &render_entity_layer_index(&dtos),
    )?;
    Ok(ctx.typed)
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

    /// The layered import is only worth its extra directories if every layer
    /// stays ignorant of the ones outside it, so assert the dependency
    /// direction directly on the rendered source rather than only through the
    /// end-to-end import test.
    #[test]
    fn generated_layers_point_dependencies_inward() {
        let domain = render_domain_module();
        assert!(!domain.contains("crate::application"));
        assert!(!domain.contains("crate::presentation"));
        for outer in [
            "pub mod microflows;",
            "pub mod nanoflows;",
            "pub mod navigation;",
            "pub mod pages;",
        ] {
            assert!(!domain.contains(outer), "domain must not declare {outer}");
        }

        // Application owns use cases and DTOs.  The composition root is the
        // sole place that can depend on every architectural layer.
        let application = render_application_module();
        assert!(application.contains("pub mod dto;"));
        assert!(application.contains("pub mod services;"));
        assert!(application.contains("services::apply(project);"));
        assert!(!application.contains("crate::domain"));
        assert!(!application.contains("crate::presentation"));
        assert!(!application.contains("pub mod nanoflows;"));
        assert!(!application.contains("pub mod pages;"));

        let presentation = render_presentation_module(&[], ApiMode::Axum);
        assert!(presentation.contains("pub mod nanoflows;"));
        assert!(presentation.contains("pub mod navigation;"));
        // Presentation contributes through `apply`, never through its own
        // `build`, so the crate root stays the single composition root.
        assert!(presentation.contains("pub fn apply(project: &mut ::mxrs_ir::ProjectDecl)"));
        assert!(!presentation.contains("pub fn build()"));
        assert!(!presentation.contains("crate::application"));
        // No page module is written when the import found nothing buildable,
        // so the layer must not declare one either.
        assert!(!presentation.contains("pub mod pages;"));
        assert!(!presentation.contains("pub mod microflows;"));

        // Every layer carries the aggregator `mxrs add` wires scaffolded
        // modules into, so a scaffold never has to invent one.
        for layer in [&domain, &application, &presentation] {
            assert!(layer.contains("pub mod modules;"));
        }

        let composition = render_composition_module("11.12.1");
        assert!(composition.contains("crate::domain::apply(&mut project);"));
        assert!(composition.contains("crate::application::apply(&mut project);"));
        assert!(composition.contains("crate::infrastructure::apply(&mut project);"));
        assert!(composition.contains("crate::presentation::apply(&mut project);"));
    }

    #[test]
    fn api_modes_generate_their_own_server_and_dependency_surface() {
        for (mode, dependency, server_marker) in [
            (ApiMode::Axum, "axum =", "axum::serve"),
            (ApiMode::ActixWeb, "actix-web =", "HttpServer::new"),
            (ApiMode::Rocket, "rocket =", "rocket::build"),
        ] {
            let manifest = cargo_manifest("sample", None, mode);
            let routes = render_routes_module(mode);
            let binary = build_binary_source("sample", "Sample", mode);
            assert!(manifest.contains(dependency), "{}", mode.name());
            assert!(routes.contains(server_marker), "{}", mode.name());
            assert!(binary.contains("presentation::routes::server::serve"));
        }
    }

    /// Each flow module may only reach the builder facet of its own runtime
    /// side, and merges through `merge_module` so nothing a declaration
    /// carries is dropped on the way into an existing module.
    #[test]
    fn generated_flow_modules_use_the_typed_facet_of_their_own_runtime_side() {
        let microflows = render_microflows_module("11.12.1", &[]);
        assert!(
            microflows.contains("project.microflow_module(..., |module| module.microflow(...))")
        );
        assert!(microflows.contains("project.merge_module(declared);"));
        assert!(!microflows.contains("nanoflow"));

        let nanoflows = render_nanoflows_module("11.12.1", &[]);
        assert!(nanoflows.contains("project.nanoflow_module(..., |module| module.nanoflow(...))"));
        assert!(nanoflows.contains("project.merge_module(declared);"));
        assert!(!nanoflows.contains("microflow"));

        // The hand-rolled upsert this replaced extended one family and pushed
        // the whole module in the other branch, so a flow declared on the
        // wrong side was kept or dropped depending on whether the module
        // already existed.
        for source in [&microflows, &nanoflows] {
            assert!(!source.contains("project.modules.push(declared)"));
            assert!(!source.contains(".extend(declared."));
        }
    }

    #[test]
    fn cargo_package_names_are_valid_and_stable() {
        assert_eq!(cargo_package_name("My Mendix App"), "my-mendix-app");
        assert_eq!(cargo_package_name("  2026 / Orders  "), "app-2026-orders");
        assert_eq!(cargo_package_name("---"), "mendix-app");
    }

    #[test]
    fn cryptographic_constant_values_never_enter_generated_rust() {
        assert!(sensitive_constant_name("EncryptionKey"));
        assert!(sensitive_constant_name("JWT_Signing_Key"));
        assert!(!sensitive_constant_name("PublicKeyAlgorithm"));
    }

    #[test]
    fn sanitize_ident_replaces_invalid_characters() {
        assert_eq!(sanitize_ident("Order Total"), "Order_Total");
        assert_eq!(sanitize_ident("2FA"), "_2FA");
        assert_eq!(sanitize_ident(""), "_");
        assert_eq!(sanitize_ident("Order"), "Order");
    }

    fn bare_entity(name: &str) -> Entity {
        use mxrs_model::entity::{Location, SystemMembers};
        Entity {
            id: None,
            name: Some(name.to_string()),
            qualified_name: None,
            documentation: String::new(),
            persistable: true,
            location: Location { x: 0, y: 0 },
            data_storage_guid: None,
            image: None,
            export_level: String::new(),
            generalization: None,
            access_rules: Vec::new(),
            indexes: Vec::new(),
            system_members: SystemMembers::default(),
            lifecycle: Vec::new(),
            validation_rules: Vec::new(),
            source: None,
            oql_query: None,
            native_type: None,
            attributes: Vec::new(),
        }
    }

    fn attribute(name: &str, attribute_type: AttributeType) -> mxrs_model::attribute::Attribute {
        mxrs_model::attribute::Attribute {
            id: None,
            name: Some(name.to_string()),
            documentation: String::new(),
            attribute_type,
            default_value: None,
            data_storage_guid: None,
            export_level: String::new(),
            raw_type_doc: None,
            raw_value_doc: None,
            length: None,
            localize_date: None,
            enumeration: None,
            required: false,
            unique: false,
        }
    }

    fn empty_layer_context() -> EntityLayerContext<'static> {
        EntityLayerContext {
            associations_by_entity: HashMap::new(),
            qualified_by_id: HashMap::new(),
            declared: std::collections::HashSet::new(),
            typed: HashMap::new(),
        }
    }

    /// The typed renderer is the authoring surface a hand author uses: the
    /// whole file is pinned so a regression in eloquence — a stray absolute
    /// path, a lost option, a broken field name — fails loudly.
    #[test]
    fn typed_entity_files_are_derive_structs_pinned_whole() {
        let mut entity = bare_entity("Parameter");
        entity.documentation = "Catalog parameter.".to_string();
        let mut limit = attribute("Limit", AttributeType::Enum);
        limit.enumeration = Some("Catalogs.ENUM_Limit".to_string());
        limit.default_value = Some(String::new());
        entity.attributes.push(limit);
        let mut measure = attribute("ParameterToMeasure", AttributeType::String);
        measure.length = Some(50);
        measure.required = true;
        entity.attributes.push(measure);
        let mut sequence = attribute("APIKey", AttributeType::AutoNumber);
        sequence.unique = true;
        entity.attributes.push(sequence);
        let mut updated = attribute("UpdatedAt", AttributeType::DateTime);
        updated.localize_date = Some(false);
        entity.attributes.push(updated);

        let rendered =
            render_typed_entity_file("Catalogs", &entity, &HashMap::new(), &empty_layer_context())
                .expect("typed-eligible");
        assert_eq!(
            rendered,
            r#"//! Editable Mendix entity declaration.

use mxrs::prelude::*;

#[derive(MxEntity)]
#[mxrs(module = "Catalogs", documentation = "Catalog parameter.")]
pub struct Parameter {
    #[mxrs(name = "APIKey", kind = "autonumber", unique)]
    pub api_key: MxLong,
    #[mxrs(kind = "enumeration", enumeration = "Catalogs.ENUM_Limit", default = "")]
    pub limit: MxString,
    #[mxrs(length = 50, required)]
    pub parameter_to_measure: MxString,
    #[mxrs(localize_date = false)]
    pub updated_at: MxDateTime,
}

pub fn declaration() -> ModuleDecl {
    let mut module = ModuleBuilder::new("Catalogs");
    Parameter::mx_register(&mut module);
    module.into_decl()
}
"#
        );

        // A non-persistable Mendix object says so once, at the entity level.
        let mut dto = bare_entity("AccountPasswordData");
        dto.persistable = false;
        let rendered = render_typed_entity_file(
            "Administration",
            &dto,
            &HashMap::new(),
            &empty_layer_context(),
        )
        .expect("typed-eligible");
        assert!(
            rendered.contains("#[mxrs(module = \"Administration\", persistable = false)]"),
            "{rendered}"
        );
        assert!(
            rendered.contains("pub struct AccountPasswordData {}"),
            "{rendered}"
        );
    }

    /// Associations between declared entities become Reference<T> fields: a
    /// self-reference names the entity's own struct, a default-shaped name
    /// needs no restatement, and non-default name/owner/storage/
    /// documentation are restated as field options.
    #[test]
    fn entity_association_fields_reference_the_typed_targets() {
        use mxrs_model::association::{Association, AssociationType, Owner, StorageFormat};

        let association =
            |name: &str, to: &str, set: bool, owner, storage, docs: &str| Association {
                id: None,
                name: Some(name.to_string()),
                documentation: docs.to_string(),
                from_entity_id: Some("t1".to_string()),
                to_entity_id: Some(to.to_string()),
                association_type: if set {
                    AssociationType::ReferenceSet
                } else {
                    AssociationType::Reference
                },
                owner,
                storage_format: storage,
                source: None,
                guid: None,
                delete_behavior: None,
                export_level: "Hidden".to_string(),
            };
        let owned = [
            association(
                "Ticket_Parent",
                "t1",
                false,
                Owner::Default,
                StorageFormat::Column,
                "",
            ),
            association(
                "Assigned",
                "o1",
                true,
                Owner::Both,
                StorageFormat::Table,
                "Who.",
            ),
        ];

        let mut ctx = empty_layer_context();
        ctx.associations_by_entity
            .insert("t1", owned.iter().collect());
        ctx.qualified_by_id.insert("t1", "Sales.Ticket".to_string());
        ctx.qualified_by_id.insert("o1", "Sales.Order".to_string());
        ctx.declared.insert("Sales.Ticket".to_string());
        ctx.declared.insert("Sales.Order".to_string());
        ctx.typed.insert(
            "Sales.Ticket".to_string(),
            TypedEntityTarget {
                file_stem: "sales_ticket".to_string(),
                type_name: "Ticket".to_string(),
                dto: false,
            },
        );
        ctx.typed.insert(
            "Sales.Order".to_string(),
            TypedEntityTarget {
                file_stem: "sales_order".to_string(),
                type_name: "Order".to_string(),
                dto: false,
            },
        );

        let mut entity = bare_entity("Ticket");
        entity.id = Some("t1".to_string());
        let rendered = render_typed_entity_file("Sales", &entity, &HashMap::new(), &ctx)
            .expect("typed-eligible");
        assert!(
            rendered.contains("use crate::domain::entities::sales_order::Order;"),
            "{rendered}"
        );
        assert!(
            rendered.contains("pub parent: Reference<Ticket>,"),
            "{rendered}"
        );
        assert!(
            rendered.contains(
                "#[mxrs(association = \"Assigned\", documentation = \"Who.\", owner = \"Both\", storage = \"Table\")]"
            ),
            "{rendered}"
        );
        assert!(
            rendered.contains("pub assigned: ReferenceSet<Order>,"),
            "{rendered}"
        );

        // A target that keeps the IR form demotes the referrer too — the
        // struct cannot name a type that does not exist.
        ctx.typed.remove("Sales.Order");
        assert!(render_typed_entity_file("Sales", &entity, &HashMap::new(), &ctx).is_none());
    }

    /// A declared Java action with a fully port-typed signature becomes an
    /// action port trait plus registration glue; generics and untypable
    /// shapes stay on the engine's string-keyed registration.
    #[test]
    fn action_ports_type_declared_java_actions_and_skip_the_rest() {
        let mut typed = HashMap::new();
        typed.insert(
            "Sales.Order".to_string(),
            TypedEntityTarget {
                file_stem: "sales_order".to_string(),
                type_name: "Order".to_string(),
                dto: false,
            },
        );
        let action = mxrs_bson::doc! {
            "$Type": "JavaActions$JavaAction",
            "Name": "CommitInBatches",
            "JavaReturnType": { "$Type": "CodeActions$BooleanType" },
            "Parameters": mxrs_bson::build_array(vec![
                mxrs_bson::Bson::Document(mxrs_bson::doc! {
                    "$Type": "JavaActions$JavaActionParameter",
                    "Name": "Items",
                    "ParameterType": {
                        "$Type": "CodeActions$BasicParameterType",
                        "Type": {
                            "$Type": "CodeActions$ListType",
                            "Parameter": { "$Type": "CodeActions$ConcreteEntityType", "Entity": "Sales.Order" },
                        },
                    },
                }),
                mxrs_bson::Bson::Document(mxrs_bson::doc! {
                    "$Type": "JavaActions$JavaActionParameter",
                    "Name": "BatchSize",
                    "ParameterType": {
                        "$Type": "CodeActions$BasicParameterType",
                        "Type": { "$Type": "CodeActions$IntegerType" },
                    },
                }),
            ], 3),
        };
        let generic = mxrs_bson::doc! {
            "$Type": "JavaActions$JavaAction",
            "Name": "Generic",
            "JavaReturnType": { "$Type": "CodeActions$VoidType" },
            "TypeParameters": mxrs_bson::build_array(vec![mxrs_bson::Bson::Document(mxrs_bson::doc! {
                "$Type": "CodeActions$TypeParameter", "Name": "T",
            })], 2),
        };
        let modules = assemble_action_ports(
            &[
                ("Sales".to_string(), action),
                ("Sales".to_string(), generic),
            ],
            &typed,
        );
        assert_eq!(modules.len(), 1);
        assert_eq!(modules[0].actions.len(), 1);

        let port = render_action_port_file(&modules[0]);
        assert!(port.contains("pub trait CommitInBatches"), "{port}");
        assert!(
            port.contains(
                "fn call(&self, items: Option<Vec<ObjectHandle<crate::domain::entities::sales_order::Order>>>, batch_size: Option<i64>) -> Result<Option<bool>, ServiceError>;"
            ),
            "{port}"
        );
        assert!(!port.contains("Generic"), "{port}");

        let registry = render_action_registry_file(&modules[0], &modules[0].file_stem);
        assert!(
            registry.contains("pub fn register_commit_in_batches"),
            "{registry}"
        );
        assert!(
            registry
                .contains("engine.with_java_action(\"Sales.CommitInBatches\", Adapter(action))"),
            "{registry}"
        );
        assert!(
            registry.contains("arguments.get(\"BatchSize\")"),
            "{registry}"
        );
    }

    /// An enumeration that rendered as a real Rust enum becomes the field's
    /// type — imported once when the short name is free, spelled by its full
    /// path on a collision, and left as the string form when the enum never
    /// derived.
    #[test]
    fn entity_enumeration_fields_reference_the_derived_enum_types() {
        let mut derived = HashMap::new();
        derived.insert(
            "Catalogs.ENUM_Limit".to_string(),
            DerivedEnumeration {
                file_stem: "catalogs_enum_limit".into(),
                type_name: "ENUMLimit".into(),
            },
        );
        derived.insert(
            "Sales.Status".to_string(),
            DerivedEnumeration {
                file_stem: "sales_status".into(),
                type_name: "Status".into(),
            },
        );
        derived.insert(
            "Support.Status".to_string(),
            DerivedEnumeration {
                file_stem: "support_status".into(),
                type_name: "Status".into(),
            },
        );

        let mut entity = bare_entity("Parameter");
        let mut limit = attribute("Limit", AttributeType::Enum);
        limit.enumeration = Some("Catalogs.ENUM_Limit".to_string());
        limit.default_value = Some("Ten".to_string());
        entity.attributes.push(limit);
        let mut sales = attribute("SalesStatus", AttributeType::Enum);
        sales.enumeration = Some("Sales.Status".to_string());
        entity.attributes.push(sales);
        let mut support = attribute("SupportStatus", AttributeType::Enum);
        support.enumeration = Some("Support.Status".to_string());
        entity.attributes.push(support);
        let mut opaque = attribute("Opaque", AttributeType::Enum);
        opaque.enumeration = Some("Catalogs.Unknown".to_string());
        entity.attributes.push(opaque);

        let rendered =
            render_typed_entity_file("Catalogs", &entity, &derived, &empty_layer_context())
                .expect("typed-eligible");
        assert_eq!(
            rendered,
            r#"//! Editable Mendix entity declaration.

use mxrs::prelude::*;

use crate::domain::enumerations::catalogs_enum_limit::ENUMLimit;

#[derive(MxEntity)]
#[mxrs(module = "Catalogs")]
pub struct Parameter {
    #[mxrs(default = "Ten")]
    pub limit: ENUMLimit,
    #[mxrs(kind = "enumeration", enumeration = "Catalogs.Unknown")]
    pub opaque: MxString,
    pub sales_status: crate::domain::enumerations::sales_status::Status,
    pub support_status: crate::domain::enumerations::support_status::Status,
}

pub fn declaration() -> ModuleDecl {
    let mut module = ModuleBuilder::new("Catalogs");
    Parameter::mx_register(&mut module);
    module.into_decl()
}
"#
        );

        // An enum sharing the entity's own type name keeps its full path.
        let mut status = bare_entity("Status");
        let mut current = attribute("Current", AttributeType::Enum);
        current.enumeration = Some("Sales.Status".to_string());
        status.attributes.push(current);
        let rendered = render_typed_entity_file("Sales", &status, &derived, &empty_layer_context())
            .expect("typed-eligible");
        assert!(
            rendered.contains("pub current: crate::domain::enumerations::sales_status::Status,"),
            "{rendered}"
        );
        assert!(!rendered.contains("use crate::"), "{rendered}");

        // An enum whose Rust name the derive would read as a scalar kind
        // keeps the explicit string form.
        let mut shadowing = HashMap::new();
        shadowing.insert(
            "Sales.MxString".to_string(),
            DerivedEnumeration {
                file_stem: "sales_mx_string".into(),
                type_name: "MxString".into(),
            },
        );
        let mut wrapper = bare_entity("Wrapper");
        let mut value = attribute("Value", AttributeType::Enum);
        value.enumeration = Some("Sales.MxString".to_string());
        wrapper.attributes.push(value);
        let rendered =
            render_typed_entity_file("Sales", &wrapper, &shadowing, &empty_layer_context())
                .expect("typed-eligible");
        assert!(
            rendered.contains(r#"#[mxrs(kind = "enumeration", enumeration = "Sales.MxString")]"#),
            "{rendered}"
        );
    }

    /// Enumerations with one caption per value become real Rust enums; the
    /// whole file is pinned like the entity render.
    #[test]
    fn single_caption_enumerations_are_derived_rust_enums_pinned_whole() {
        let values = vec![
            (
                "Open".to_string(),
                vec![("en_US".to_string(), "Open".to_string())],
            ),
            (
                "_10_Minutes".to_string(),
                vec![("pt_BR".to_string(), "Dez minutos".to_string())],
            ),
        ];
        let (rendered, derived_type) =
            render_enumeration_file("Sales", "ENUM_Status", "Lifecycle.", &values);
        assert_eq!(derived_type.as_deref(), Some("ENUMStatus"));
        assert_eq!(
            rendered,
            r#"//! Editable Mendix enumeration.

use mxrs::prelude::*;

#[derive(MxEnumeration)]
#[mxrs(name = "ENUM_Status", module = "Sales", documentation = "Lifecycle.")]
pub enum ENUMStatus {
    Open,
    #[mxrs(name = "_10_Minutes", caption = "Dez minutos", language = "pt_BR")]
    _10Minutes,
}

pub fn declaration() -> ModuleDecl {
    let mut module = ModuleBuilder::new("Sales");
    ENUMStatus::mx_register(&mut module);
    module.into_decl()
}
"#
        );

        // Multi-language captions keep the builder form, still one file.
        let multi = vec![(
            "Open".to_string(),
            vec![
                ("en_US".to_string(), "Open".to_string()),
                ("pt_BR".to_string(), "Aberto".to_string()),
            ],
        )];
        let (rendered, derived_type) = render_enumeration_file("Sales", "Status", "", &multi);
        assert_eq!(derived_type, None);
        assert!(
            rendered.contains("module.enumeration(\"Status\""),
            "{rendered}"
        );
        assert!(rendered.contains("use mxrs::prelude::*;"));
        assert!(!rendered.contains("derive(MxEnumeration)"));
    }

    /// What the derive cannot yet express falls back to the IR renderer —
    /// less eloquent, never lost.
    #[test]
    fn entities_beyond_the_derive_surface_fall_back_to_the_ir_renderer() {
        use mxrs_model::entity::EntityIndex;

        let mut indexed = bare_entity("Order");
        indexed
            .attributes
            .push(attribute("Number", AttributeType::String));
        indexed.indexes.push(EntityIndex {
            id: None,
            guid: None,
            include_offline: false,
            members: Vec::new(),
            raw: mxrs_bson::Document::new(),
        });
        assert!(
            render_typed_entity_file("Sales", &indexed, &HashMap::new(), &empty_layer_context())
                .is_none()
        );

        let mut pictured = bare_entity("Asset");
        pictured.image = Some("Assets.Image".to_string());
        assert!(
            render_typed_entity_file("Assets", &pictured, &HashMap::new(), &empty_layer_context())
                .is_none()
        );

        // Attribute names that collide once snake_cased cannot become two
        // struct fields.
        let mut colliding = bare_entity("Pair");
        colliding
            .attributes
            .push(attribute("FooBar", AttributeType::String));
        colliding
            .attributes
            .push(attribute("Foo_Bar", AttributeType::String));
        assert!(
            render_typed_entity_file("Sales", &colliding, &HashMap::new(), &empty_layer_context())
                .is_none()
        );

        // A keyword survives as neither a field nor a struct name.
        let mut keyword = bare_entity("Order");
        keyword
            .attributes
            .push(attribute("Type", AttributeType::String));
        assert!(
            render_typed_entity_file("Sales", &keyword, &HashMap::new(), &empty_layer_context())
                .is_none()
        );
    }

    #[test]
    fn snake_idents_break_on_case_boundaries_like_readers_expect() {
        assert_eq!(snake_ident("ParameterToMeasure"), "parameter_to_measure");
        assert_eq!(snake_ident("APIKey"), "api_key");
        assert_eq!(snake_ident("WikiUrl"), "wiki_url");
        assert_eq!(snake_ident("Max_Retries"), "max_retries");
        assert_eq!(snake_ident("2FA"), "_2_fa");
        assert_eq!(
            derive_pascal_case("parameter_to_measure"),
            "ParameterToMeasure"
        );
        assert_eq!(derive_pascal_case("api_key"), "ApiKey");
    }

    #[test]
    fn access_rule_with_unrenderable_association_is_not_typed() {
        use mxrs_model::association::{AssociationType, Owner, StorageFormat};
        use mxrs_model::entity::{
            AccessMember, AccessMemberKind, AccessRule, Location, SystemMembers,
        };

        let entity = Entity {
            id: Some("order-id".to_string()),
            name: Some("Order".to_string()),
            qualified_name: Some("Sales.Order".to_string()),
            documentation: String::new(),
            persistable: true,
            location: Location { x: 0, y: 0 },
            data_storage_guid: None,
            image: None,
            export_level: String::new(),
            generalization: None,
            access_rules: vec![AccessRule {
                id: None,
                roles: vec!["Sales.User".to_string()],
                create: true,
                delete: true,
                documentation: String::new(),
                default_rights: "ReadWrite".to_string(),
                members: vec![AccessMember {
                    id: None,
                    name: "Order_Customer".to_string(),
                    reference: "Sales.Order_Customer".to_string(),
                    rights: "ReadWrite".to_string(),
                    kind: AccessMemberKind::Association,
                    raw: mxrs_bson::Document::new(),
                }],
                xpath: String::new(),
                xpath_caption: None,
                raw: mxrs_bson::Document::new(),
            }],
            indexes: Vec::new(),
            system_members: SystemMembers::default(),
            lifecycle: Vec::new(),
            validation_rules: Vec::new(),
            source: None,
            oql_query: None,
            native_type: None,
            attributes: Vec::new(),
        };
        let association = Association {
            id: None,
            name: Some("Order_Customer".to_string()),
            documentation: String::new(),
            from_entity_id: Some("order-id".to_string()),
            to_entity_id: Some("missing-customer-id".to_string()),
            association_type: AssociationType::Reference,
            owner: Owner::Default,
            storage_format: StorageFormat::Column,
            source: None,
            guid: None,
            delete_behavior: None,
            export_level: String::new(),
        };
        let associations = [&association];
        let known_entities = HashMap::new();

        assert!(!access_rules_renderable(
            &entity,
            &associations,
            &known_entities
        ));

        let known_entities = HashMap::from([(
            "missing-customer-id".to_string(),
            "Sales.Customer".to_string(),
        )]);
        assert!(access_rules_renderable(
            &entity,
            &associations,
            &known_entities
        ));
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

    fn identified(native_type: &str) -> mxrs_bson::Document {
        mxrs_bson::doc! {
            "$ID": uuid::Uuid::new_v4().to_string(),
            "$Type": native_type,
        }
    }

    fn translated(text: &str) -> mxrs_bson::Document {
        let mut translation = identified("Texts$Translation");
        translation.insert("LanguageCode", "en_US");
        translation.insert("Text", text);
        let mut result = identified("Texts$Text");
        result.insert(
            "Items",
            mxrs_bson::build_array(vec![mxrs_bson::Bson::Document(translation)], 3),
        );
        result
    }

    #[test]
    fn complete_menu_projection_types_actions_icons_and_nested_items() {
        let mut settings = identified("Forms$FormSettings");
        settings.insert("Form", "Sales.Home");
        settings.insert("ParameterMappings", mxrs_bson::build_array(vec![], 2));
        settings.insert("TitleOverride", mxrs_bson::Bson::Null);
        let mut action = identified("Forms$FormAction");
        action.insert("DisabledDuringExecution", true);
        action.insert("FormSettings", settings);
        action.insert("NumberOfPagesToClose2", "1");
        action.insert("PagesForSpecializations", mxrs_bson::build_array(vec![], 2));
        let mut icon = identified("Forms$IconCollectionIcon");
        icon.insert("Image", "Atlas_Core.Atlas_Filled.home");
        let mut item = identified("Menus$MenuItem");
        item.insert("Action", action);
        item.insert("AlternativeText", mxrs_bson::Bson::Null);
        item.insert("Caption", translated("Home"));
        item.insert("Icon", icon);
        item.insert("Items", mxrs_bson::build_array(vec![], 3));
        let mut collection = identified("Menus$MenuItemCollection");
        collection.insert(
            "Items",
            mxrs_bson::build_array(vec![mxrs_bson::Bson::Document(item)], 3),
        );
        let mut document = identified("Menus$MenuDocument");
        document.insert("Documentation", "Main navigation");
        document.insert("Excluded", false);
        document.insert("ExportLevel", "Hidden");
        document.insert("ItemCollection", collection);
        document.insert("Name", "Main");

        let menu = parse_complete_menu(&document).unwrap();
        assert_eq!(menu.items[0].caption["en_US"], "Home");
        assert_eq!(
            menu.items[0].icon,
            Some(mxrs_ir::MenuIconDecl::Image(
                "Atlas_Core.Atlas_Filled.home".into()
            ))
        );
        assert!(matches!(
            menu.items[0].action,
            mxrs_ir::MenuActionDecl::OpenPage {
                pages_to_close: Some(1),
                ..
            }
        ));

        document.insert("FutureField", true);
        assert!(parse_complete_menu(&document).is_none());
    }

    #[test]
    fn leaf_menu_item_uses_an_unnamed_builder_parameter() {
        let mut caption = mxrs_ir::LocalizedText::new();
        caption.insert("en_US".into(), "Heading".into());
        let menu = mxrs_ir::MenuDecl {
            name: "Main".into(),
            documentation: String::new(),
            excluded: false,
            export_level: mxrs_ir::ExportLevel::Hidden,
            items: vec![mxrs_ir::MenuItemDecl {
                caption,
                ..Default::default()
            }],
        };
        let mut source = String::new();
        render_menu_body(&mut source, &menu);
        assert!(source.contains("menu.item(\"Heading\", |_|"), "{source}");
        assert!(!source.contains("$ID"), "{source}");
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
    fn contradictory_native_oql_view_is_rendered_as_non_persistable_rust() {
        let report = entity(
            "Report",
            "Sales.Report",
            mxrs_bson::doc! {
                "persistable": true,
                "source": {
                    "$Type": "DomainModels$OqlViewEntitySource",
                    "SourceDocument": "Sales.ReportSource",
                },
            },
        );
        let rendered = render_entity(
            "Sales",
            &report,
            &[],
            &HashMap::new(),
            &std::collections::HashSet::new(),
        );
        assert!(rendered.contains("persistable false;"), "{rendered}");
        assert!(
            rendered.contains("oql_view Sales::ReportSource;"),
            "{rendered}"
        );
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

        let source = render_module(&module, &HashMap::new(), &Default::default());
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
        let source = render_module(&module, &entity_ids, &Default::default());

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
            module.role("User", "Can read orders");
            module.oql_view_source("OrderSource", "SELECT Number FROM Sales.Order", |source| {
                source
                    .documentation("Order projection")
                    .excluded(true)
                    .export_level(mxrs_ir::ExportLevel::Published);
            });
            module.entity("Order", |entity| {
                let number = entity.string("Number");
                number.documentation = "External order number".to_string();
                number.length = Some(80);
                number.required = true;
                number.unique = true;
                entity.datetime("SubmittedAt").localize_date = Some(false);
                entity.image("Sales.OrderIcon");
                entity.system_members(|members| {
                    members.owner(true).created_date(true);
                });
                entity.index(|index| {
                    index
                        .attribute::<sales_markers::Order_Number>()
                        .system_descending(mxrs_ir::SystemMember::CreatedDate)
                        .include_offline(true);
                });
                entity.before_commit::<sales_markers::ACT_Ping>(|callback| {
                    callback.pass_event_object(false);
                });
                entity.access_rule(["User"], |rule| {
                    rule.documentation("Visible orders")
                        .attribute::<sales_markers::Order_Number>(mxrs_ir::MemberRights::ReadOnly)
                        .xpath("[Number != empty]")
                        .xpath_caption("Orders with a number");
                });
                entity.association::<sales_markers::Order_Order_Customer>();
            });
            module.entity("OrderReport", |entity| {
                entity.oql_view("Sales.OrderSource");
                entity.string("Number");
            });
            module.enumeration("Status", |enumeration| {
                enumeration.documentation("Order lifecycle");
                enumeration.value("Open").captions = vec![
                    ("en_US".to_string(), "Open".to_string()),
                    ("pt_BR".to_string(), "Aberto".to_string()),
                ];
            });
            module.enumeration("Priority", |enumeration| {
                enumeration.value("Low");
                enumeration.value("High");
            });
            module.entity("Customer", |entity| {
                entity.string("Name");
            });
            module.entity("Ticket", |entity| {
                entity
                    .enumeration("Priority", "Sales.Priority")
                    .default_value = Some("Low".to_string());
                entity.association::<sales_markers::Ticket_Ticket_Customer>();
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

        // The DSL cannot author Java actions yet; inject one the way a real
        // model carries it, so the action-port surface is exercised
        // end-to-end.
        {
            let mut mpr = mxrs_mpr::MprFile::open(&source_path, false).unwrap();
            let sales_id = mpr
                .all_units()
                .unwrap()
                .into_iter()
                .find(|unit| {
                    let Ok(document) = mpr.parse_contents(unit) else {
                        return false;
                    };
                    document.get_str("$Type").ok() == Some("Projects$Module")
                        && document.get_str("Name").ok() == Some("Sales")
                })
                .map(|unit| unit.unit_id)
                .expect("the written project has a Sales module unit");
            mpr.insert_unit(
                &sales_id,
                "Documents",
                mxrs_bson::doc! {
                    "$ID": uuid::Uuid::new_v4().to_string(),
                    "$Type": "JavaActions$JavaAction",
                    "Name": "ReverseText",
                    "Documentation": "",
                    "Excluded": false,
                    "ExportLevel": "Hidden",
                    "JavaReturnType": {
                        "$ID": uuid::Uuid::new_v4().to_string(),
                        "$Type": "CodeActions$StringType",
                    },
                    "Parameters": mxrs_bson::build_array(vec![mxrs_bson::Bson::Document(mxrs_bson::doc! {
                        "$ID": uuid::Uuid::new_v4().to_string(),
                        "$Type": "JavaActions$JavaActionParameter",
                        "Name": "Input",
                        "ParameterType": {
                            "$ID": uuid::Uuid::new_v4().to_string(),
                            "$Type": "CodeActions$BasicParameterType",
                            "Type": {
                                "$ID": uuid::Uuid::new_v4().to_string(),
                                "$Type": "CodeActions$StringType",
                            },
                        },
                    })], 3),
                },
                None,
            )
            .unwrap();
        }

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
        assert!(generated.join("src/domain/security/mod.rs").is_file());
        assert!(generated.join("src/application/mod.rs").is_file());
        assert!(generated.join("src/application/dto/mod.rs").is_file());
        assert!(generated.join("src/application/services/mod.rs").is_file());
        assert!(generated.join("src/presentation/mod.rs").is_file());
        assert!(
            generated
                .join("src/presentation/nanoflows/mod.rs")
                .is_file()
        );
        assert!(
            generated
                .join("src/presentation/navigation/mod.rs")
                .is_file()
        );
        assert!(generated.join("src/infrastructure/mod.rs").is_file());
        assert!(!generated.join("src/generated").exists());
        assert!(generated.join("model/imported/manifest.json").is_file());
        // The pages module is real, *compiled* Rust source, wired into
        // `build()` from `src/presentation/mod.rs` (see `page_export`'s doc
        // comment) — the `cargo check`/`cargo run` calls below, plus the
        // rebuilt-project assertions further down, prove it actually
        // contributes to the output `.mpr`, not just that it parses.
        let pages_source =
            std::fs::read_to_string(generated.join("src/presentation/pages/mod.rs")).unwrap();
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
        assert!(domain_source.contains("pub mod security;"));
        assert!(domain_source.contains("documents::apply(project);"));
        assert!(domain_source.contains("security::apply(project);"));
        let application_source =
            std::fs::read_to_string(generated.join("src/application/mod.rs")).unwrap();
        assert!(application_source.contains("pub mod dto;"));
        assert!(application_source.contains("services::apply(project);"));
        assert!(!application_source.contains("crate::presentation"));
        let presentation_source =
            std::fs::read_to_string(generated.join("src/presentation/mod.rs")).unwrap();
        assert!(presentation_source.contains("nanoflows::apply(project);"));
        assert!(presentation_source.contains("routes::apply(project);"));
        assert!(presentation_source.contains("pub mod pages;"));
        assert!(presentation_source.contains("pages::home()"));
        let entities_source =
            std::fs::read_to_string(generated.join("src/domain/entities/sales_order.rs")).unwrap();
        assert!(
            entities_source.contains("External order number"),
            "{entities_source}"
        );
        assert!(entities_source.contains("length: Some(80)"));
        assert!(entities_source.contains("required: true"));
        assert!(entities_source.contains("unique: true"));
        assert!(entities_source.contains("localize_date: Some(false)"));
        assert!(entities_source.contains("IndexMemberDecl::Attribute"));
        assert!(entities_source.contains("SystemMember::CreatedDate"));
        assert!(entities_source.contains("include_offline: true"));
        assert!(entities_source.contains("LifecycleEvent::BeforeCommit"));
        assert!(entities_source.contains("pass_event_object: false"));
        assert!(entities_source.contains("Sales.OrderIcon"));
        let report_dto =
            std::fs::read_to_string(generated.join("src/application/dto/sales_orderreport_dto.rs"))
                .unwrap();
        assert!(report_dto.contains("OqlView"));
        assert!(report_dto.contains("Sales.OrderSource"));
        let persistence =
            std::fs::read_to_string(generated.join("src/infrastructure/persistence/mod.rs"))
                .unwrap();
        assert!(persistence.contains("oql_view_source(\"OrderSource\""));
        assert!(persistence.contains("Order projection"));
        assert!(persistence.contains("source.excluded(true)"));
        assert!(persistence.contains("ExportLevel::Published"));
        // Documents split per concept: the multi-language enumeration keeps
        // the builder form in its own file, the constants land in the
        // module's documents file, and both are indexed by mod.rs.
        let documents_index =
            std::fs::read_to_string(generated.join("src/domain/documents/mod.rs")).unwrap();
        assert!(documents_index.contains("pub mod sales;"));
        assert!(documents_index.contains("project.merge_module(sales::declaration());"));
        let enumerations_index =
            std::fs::read_to_string(generated.join("src/domain/enumerations/mod.rs")).unwrap();
        assert!(enumerations_index.contains("pub mod sales_status;"));
        let status =
            std::fs::read_to_string(generated.join("src/domain/enumerations/sales_status.rs"))
                .unwrap();
        assert!(status.contains("module.enumeration(\"Status\""), "{status}");
        assert!(status.contains("(\"pt_BR\".to_string(), \"Aberto\".to_string())"));
        assert!(!status.contains("derive(MxEnumeration)"), "{status}");
        // The single-caption enumeration derives, and the entity referencing
        // it uses the Rust type itself — no embedded qualified-name string.
        let priority =
            std::fs::read_to_string(generated.join("src/domain/enumerations/sales_priority.rs"))
                .unwrap();
        assert!(priority.contains("derive(MxEnumeration)"), "{priority}");
        let ticket =
            std::fs::read_to_string(generated.join("src/domain/entities/sales_ticket.rs")).unwrap();
        assert!(
            ticket.contains("use crate::domain::enumerations::sales_priority::Priority;"),
            "{ticket}"
        );
        assert!(ticket.contains("#[mxrs(default = \"Low\")]"), "{ticket}");
        assert!(ticket.contains("pub priority: Priority,"), "{ticket}");
        assert!(!ticket.contains("enumeration = "), "{ticket}");
        // The association to the typed Customer entity is a Reference<T>
        // field importing the target struct; the default-shaped name needs
        // no #[mxrs(association = ...)] restatement.
        assert!(
            ticket.contains("use crate::domain::entities::sales_customer::Customer;"),
            "{ticket}"
        );
        assert!(
            ticket.contains("pub customer: Reference<Customer>,"),
            "{ticket}"
        );
        assert!(!ticket.contains("association = "), "{ticket}");
        // Order keeps the IR form (image + index), so its association is
        // restated as an AssociationDecl rather than silently dropped.
        let order_source =
            std::fs::read_to_string(generated.join("src/domain/entities/sales_order.rs")).unwrap();
        assert!(
            order_source.contains("entity.associations.push(::mxrs_ir::AssociationDecl {"),
            "{order_source}"
        );
        assert!(
            order_source.contains("name: \"Order_Customer\".to_string(),"),
            "{order_source}"
        );
        assert!(
            order_source.contains("target: \"Sales.Customer\".to_string(),"),
            "{order_source}"
        );
        let documents =
            std::fs::read_to_string(generated.join("src/domain/documents/sales.rs")).unwrap();
        assert!(documents.contains("use mxrs::prelude::*;"));
        assert!(documents.contains("module.constant(\"MaximumOrders\""));
        assert!(documents.contains("ConstantType::Integer"));
        assert!(!documents.contains("::mxrs_ir::"), "{documents}");
        assert!(documents.contains("constant.exposed_to_client(true)"));
        assert!(documents.contains("constant.value_from_env(\"MXRS_SALES_APITOKEN\")"));
        assert!(!documents.contains("super-secret-value"));
        assert!(!generated.join("src/infrastructure/ids.rs").exists());
        assert!(!generated.join("src/infrastructure/imported.rs").exists());
        let markers =
            std::fs::read_to_string(generated.join("src/infrastructure/markers/sales.rs")).unwrap();
        assert!(markers.contains("pub struct Order;"));
        assert!(markers.contains("pub struct Order_Number;"));
        assert!(markers.contains("impl mxrs_ir::MicroflowMarker for ACT_Ping"));
        assert!(markers.contains("impl mxrs_ir::MicroflowMarker for ACT_GetOrder"));
        assert!(markers.contains("impl mxrs_ir::NanoflowMarker for NF_Validate"));
        let microflows =
            std::fs::read_to_string(generated.join("src/application/services/mod.rs")).unwrap();
        assert!(!microflows.contains("snapshot-backed"));
        assert!(microflows.contains("pub mod sales_act_ping;"));
        assert!(microflows.contains("project.merge_module(sales_act_ping::declaration());"));
        assert!(!microflows.contains("nanoflow"));
        let nanoflows =
            std::fs::read_to_string(generated.join("src/presentation/nanoflows/mod.rs")).unwrap();
        assert!(nanoflows.contains("pub mod sales_nf_validate;"));
        assert!(nanoflows.contains("project.merge_module(sales_nf_validate::declaration());"));
        assert!(!nanoflows.contains("microflow"));
        // Runnable microflows surface as a typed service port plus the
        // runtime adapter that implements it. ACT_GetOrder returns the
        // IR-form Order entity, so it stays off the typed surface.
        let ports_index =
            std::fs::read_to_string(generated.join("src/domain/ports/mod.rs")).unwrap();
        assert!(
            ports_index.contains("pub mod sales_services;"),
            "{ports_index}"
        );
        let sales_port =
            std::fs::read_to_string(generated.join("src/domain/ports/sales_services.rs")).unwrap();
        assert!(
            sales_port.contains("pub trait SalesServices"),
            "{sales_port}"
        );
        assert!(
            sales_port.contains("fn act_ping(&mut self) -> Result<(), ServiceError>;"),
            "{sales_port}"
        );
        assert!(!sales_port.contains("act_get_order"), "{sales_port}");
        let adapter = std::fs::read_to_string(
            generated.join("src/infrastructure/adapters/runtime_services.rs"),
        )
        .unwrap();
        assert!(adapter.contains("pub struct RuntimeServices"), "{adapter}");
        assert!(
            adapter.contains(
                "impl crate::domain::ports::sales_services::SalesServices for RuntimeServices"
            ),
            "{adapter}"
        );
        assert!(adapter.contains("\"Sales.ACT_Ping\""), "{adapter}");
        // The injected Java action surfaces as an action port with typed
        // registration glue.
        assert!(
            ports_index.contains("pub mod sales_actions;"),
            "{ports_index}"
        );
        let sales_actions =
            std::fs::read_to_string(generated.join("src/domain/ports/sales_actions.rs")).unwrap();
        assert!(
            sales_actions.contains("pub trait ReverseText"),
            "{sales_actions}"
        );
        assert!(
            sales_actions.contains(
                "fn call(&self, input: Option<String>) -> Result<Option<String>, ServiceError>;"
            ),
            "{sales_actions}"
        );
        let action_registry =
            std::fs::read_to_string(generated.join("src/infrastructure/adapters/sales_actions.rs"))
                .unwrap();
        assert!(
            action_registry.contains("pub fn register_reverse_text"),
            "{action_registry}"
        );
        assert!(
            action_registry
                .contains("engine.with_java_action(\"Sales.ReverseText\", Adapter(action))"),
            "{action_registry}"
        );
        let security =
            std::fs::read_to_string(generated.join("src/domain/security/mod.rs")).unwrap();
        assert!(security.contains("ProjectSecurityDecl {"));
        assert!(security.contains("check_security: false,"));
        assert!(security.contains("minimum_length: 12"));
        assert!(security.contains("user_roles: vec!"));
        let navigation =
            std::fs::read_to_string(generated.join("src/presentation/navigation/mod.rs")).unwrap();
        assert!(navigation.contains("project.navigation = Some"));
        assert!(navigation.contains("NavigationIconDecl::Code(57369)"));
        let crate_root = std::fs::read_to_string(generated.join("src/lib.rs")).unwrap();
        assert!(crate_root.contains("pub mod infrastructure;"));
        assert!(!crate_root.contains("pub mod generated"));
        // The crate root exposes layers; composition belongs to its own file.
        assert!(crate_root.contains("pub mod application;"));
        assert!(crate_root.contains("pub mod domain;"));
        assert!(crate_root.contains("pub mod presentation;"));
        assert!(crate_root.contains("pub fn build() -> ::mxrs_ir::ProjectDecl"));
        assert!(crate_root.contains("composition::build()"));
        assert!(crate_root.contains("project = crate::build"));
        // Every layer ships the aggregator `mxrs add` wires scaffolded modules
        // into, so a later scaffold never has to invent one.
        for relative in [
            "src/domain/modules/mod.rs",
            "src/application/modules/mod.rs",
            "src/presentation/modules/mod.rs",
        ] {
            assert!(generated.join(relative).is_file(), "{relative} is missing");
        }
        assert!(domain_source.contains("pub mod modules;"));
        assert!(application_source.contains("modules::apply(project);"));
        assert!(presentation_source.contains("modules::apply(project);"));
        // The pre-split layout must not survive alongside the layered one:
        // two homes for the same concept is exactly the ambiguity the split
        // exists to remove.
        assert!(!generated.join("src/domain/flows").exists());
        assert!(!generated.join("src/domain/navigation").exists());
        assert!(!generated.join("src/domain/pages").exists());
        assert!(!domain_source.contains("pub mod navigation;"));
        assert!(!domain_source.contains("pub mod pages;"));
        assert!(!domain_source.contains("pub mod flows;"));

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
        let rebuilt_project = Project::open(&output, true).unwrap();
        let rebuilt_sales = rebuilt_project
            .modules()
            .unwrap()
            .into_iter()
            .find(|module| module.name.as_deref() == Some("Sales"))
            .unwrap();
        let rebuilt_order = rebuilt_sales
            .entities()
            .iter()
            .find(|entity| entity.name.as_deref() == Some("Order"))
            .unwrap();
        assert!(rebuilt_order.system_members.owner);
        assert!(rebuilt_order.system_members.created_date);
        assert_eq!(rebuilt_order.indexes.len(), 1);
        assert_eq!(rebuilt_order.indexes[0].members.len(), 2);
        assert!(rebuilt_order.indexes[0].include_offline);
        assert_eq!(rebuilt_order.lifecycle.len(), 1);
        assert_eq!(rebuilt_order.lifecycle[0].event, "before_commit");
        assert_eq!(rebuilt_order.lifecycle[0].handler, "Sales.ACT_Ping");
        assert_eq!(rebuilt_order.image.as_deref(), Some("Sales.OrderIcon"));
        assert_eq!(rebuilt_order.access_rules.len(), 1);
        assert_eq!(rebuilt_order.access_rules[0].members[0].rights, "ReadOnly");
        let rebuilt_report = rebuilt_sales
            .entities()
            .iter()
            .find(|entity| entity.name.as_deref() == Some("OrderReport"))
            .unwrap();
        assert!(rebuilt_report.oql_view());
        assert!(!rebuilt_report.persistable);
        assert_eq!(
            rebuilt_report.oql_source_document().as_deref(),
            Some("Sales.OrderSource")
        );
        assert_eq!(
            rebuilt_report.attributes[0]
                .raw_value_doc
                .as_ref()
                .unwrap()
                .get_str("$Type")
                .unwrap(),
            "DomainModels$OqlViewValue"
        );
        assert!(documents.iter().any(|document| {
            document.get_str("$Type").ok() == Some("DomainModels$ViewEntitySourceDocument")
                && document.get_str("Name").ok() == Some("OrderSource")
                && document.get_str("Oql").ok() == Some("SELECT Number FROM Sales.Order")
                && document.get_str("Documentation").ok() == Some("Order projection")
                && document.get_bool("Excluded").ok() == Some(true)
                && document.get_str("ExportLevel").ok() == Some("Published")
        }));
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
        let order = modules[0]
            .entities()
            .iter()
            .find(|entity| entity.name.as_deref() == Some("Order"))
            .unwrap();
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
        // Associations between declared entities must survive the round
        // trip through both renderers: the typed Ticket restates its as a
        // Reference<T> field, the IR-form Order as an AssociationDecl.
        let rebuilt_association_names = modules[0]
            .associations()
            .iter()
            .map(|association| association.name.clone())
            .collect::<Vec<_>>();
        let ticket_id = modules[0]
            .entities()
            .iter()
            .find(|entity| entity.name.as_deref() == Some("Ticket"))
            .and_then(|entity| entity.id.clone())
            .expect("rebuilt Ticket entity has an id");
        assert!(
            modules[0].associations().iter().any(|association| {
                association.name.as_deref() == Some("Ticket_Customer")
                    && association.from_entity_id.as_deref() == Some(ticket_id.as_str())
            }),
            "Ticket_Customer association was lost in the rebuild; remaining: {rebuilt_association_names:?}"
        );
        assert!(
            modules[0]
                .associations()
                .iter()
                .any(|association| association.name.as_deref() == Some("Order_Customer")),
            "Order_Customer association was lost in the rebuild; remaining: {rebuilt_association_names:?}"
        );
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
        pub struct ACT_Ping;
        impl mxrs_ir::MicroflowMarker for ACT_Ping {
            const MODULE: &'static str = "Sales";
            const NAME: &'static str = "ACT_Ping";
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
        pub struct Ticket;
        impl mxrs_ir::EntityMarker for Ticket {
            const MODULE: &'static str = "Sales";
            const NAME: &'static str = "Ticket";
        }
        pub struct Ticket_Ticket_Customer;
        impl mxrs_ir::AssociationMarker for Ticket_Ticket_Customer {
            type From = Ticket;
            type To = Customer;
            const NAME: &'static str = "Ticket_Customer";
            const ASSOCIATION_TYPE: mxrs_ir::AssociationType = mxrs_ir::AssociationType::Reference;
        }
    }
}
