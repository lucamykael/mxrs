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
//! `#[page]` functions under `src/ui/pages/<module>/`, which register
//! themselves — see `page_export`'s doc comment for the widget vocabulary
//! detected and what remains snapshot-preserved.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

#[cfg(test)]
use mxrs_model::Attribute;
use mxrs_model::attribute::AttributeType;
use mxrs_model::entity::Entity;
use mxrs_model::{Association, Module, Project};

use entity_export::TypedEntityTarget;

mod entity_export;
mod flow_export;
mod flow_general;
mod names;
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

    #[error(transparent)]
    Materializer(#[from] mxrs_materializers::MaterializeError),

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
            | ExportError::Materializer(_)
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
    /// widget vocabulary, emitted as `#[page]` functions under
    /// `src/ui/pages/<module>/` — see `page_export`'s doc comment for what still stays
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
    let persistence_source = render_persistence_module(&modules);
    let infrastructure_source = render_infrastructure_module(persistence_source.is_some());
    let (converted_flows, kept_flows) = flow_export::collect_all(&project, &modules)?;
    let flow_relations = mxrs_model::relations::flow_relations(&project)?;
    let security_document = project.all_units()?.into_iter().find_map(|unit| {
        let document = project.mpr().parse_contents(&unit).ok()?;
        (document.get_str("$Type").ok() == Some("Security$ProjectSecurity")).then_some(document)
    });
    let security_source = render_project_security_module(security_document.as_ref());
    let navigation_source = render_navigation_module(&project.navigation()?);
    let packages = package_stems_for_import(&modules, mpr_path);
    let documents_export = render_documents_module(&project, &packages)?;
    let task_queues_source = render_task_queue_declarations(&project)?;
    let action_documents = collect_action_documents(&project)?;
    // The HTTP layer is generated for the axum adapter only; the other two
    // presets keep their server stub until their routers are ported.
    let published_services = match api_mode {
        ApiMode::Axum => {
            collect_published_services(&project, &packages, &http_parameters(&modules))?
        }
        _ => Vec::new(),
    };
    let export_mappings = collect_export_mappings(
        &project,
        &published_services
            .iter()
            .flat_map(|service| &service.routes)
            .flat_map(|route| &route.operations)
            .filter(|operation| !operation.export_mapping.is_empty())
            .map(|operation| operation.export_mapping.clone())
            .collect(),
    )?;
    drop(project);

    let imported = destination.join("model/imported");
    let manifest = mxrs_project::capture_imported_project(mpr_path, &imported)?;
    let imported_assets =
        mxrs_project::capture_project_assets(mpr_path, destination.join("assets"))?;
    mxrs_materializers::materialize_frontend_sources(destination.join("frontend"))?;
    let package_name = cargo_package_name(&manifest.project_name);
    let crate_name = package_name.replace('-', "_");
    let infrastructure_directory = destination.join("src/infrastructure");
    let controllers_directory = destination.join("src/controllers");
    // Only folders that receive generated content exist: an empty
    // placeholder folder is noise the reader has to rule out.
    for directory in [
        destination.join("src/domain"),
        controllers_directory.clone(),
        destination.join("src/ui"),
        infrastructure_directory.join("adapters"),
        infrastructure_directory.join("generated"),
    ] {
        std::fs::create_dir_all(&directory).map_err(|source| io_error(&directory, source))?;
    }

    let mut generated_modules: std::collections::BTreeMap<String, GeneratedModule> =
        Default::default();
    let typed_entities = entity_export::collect_entity_layer(
        &modules,
        &packages,
        &documents_export.derived_enumerations,
        &mut generated_modules,
    );
    let flow_plans = flow_export::plan_files(&converted_flows);
    let names = model_names(
        &modules,
        &packages,
        &typed_entities,
        &converted_flows,
        &flow_plans,
        &kept_flows,
        &mut generated_modules,
    );
    let microflow_files = flow_export::render_files(
        &converted_flows,
        &flow_plans,
        false,
        &names,
        &flow_relations,
    )?;
    let nanoflow_files =
        flow_export::render_files(&converted_flows, &flow_plans, true, &names, &flow_relations)?;
    let service_ports = collect_service_ports(&modules, &packages, &typed_entities);
    let action_ports = assemble_action_ports(&action_documents, &packages, &typed_entities);
    for (module_name, stem, source) in &documents_export.enumeration_files {
        generated_module(&mut generated_modules, module_name)
            .enumerations
            .push((stem.clone(), source.clone()));
    }
    for (module_name, stem, source) in &documents_export.module_files {
        generated_module(&mut generated_modules, module_name)
            .documents
            .push((stem.clone(), source.clone()));
    }
    for module in &modules {
        let Some(module_name) = module.name.as_deref() else {
            continue;
        };
        if let Some(security) = render_module_security(module_name, &module.module_roles) {
            generated_module(&mut generated_modules, module_name).security = Some(security);
        }
    }
    for flow in &microflow_files {
        generated_module(&mut generated_modules, &flow.module)
            .services
            .push((flow.file_name.clone(), flow.source.clone()));
    }
    for flow in &nanoflow_files {
        generated_module(&mut generated_modules, &flow.module)
            .nanoflows
            .push((flow.file_name.clone(), flow.source.clone()));
    }
    for module in generated_modules.values_mut() {
        module.services.sort();
        module.nanoflows.sort();
    }
    {
        let mut pages_by_module: std::collections::BTreeMap<
            String,
            Vec<&page_export::ConvertedPage>,
        > = Default::default();
        for page in &converted_pages {
            pages_by_module
                .entry(page.module_name.clone())
                .or_default()
                .push(page);
        }
        for (module_name, pages) in pages_by_module {
            let files = page_export::render_page_files(&pages, &names)?;
            if !files.is_empty() {
                generated_module(&mut generated_modules, &module_name).pages = files;
            }
        }
    }
    for port in &service_ports {
        generated_module(&mut generated_modules, &port.module_name).ports_services =
            Some(render_service_port(port));
    }
    for module in &action_ports {
        generated_module(&mut generated_modules, &module.module_name).ports_actions =
            Some(render_action_port_file(module));
    }
    let mapping_targets: HashMap<String, MappingTarget> = export_mappings
        .iter()
        .map(|mapping| {
            (
                mapping.qualified_name.clone(),
                MappingTarget {
                    root: module_root(&module_stem(&mapping.module_name), &packages),
                    module_stem: module_stem(&mapping.module_name),
                    file_stem: mapping.file_stem.clone(),
                },
            )
        })
        .collect();
    // A module's published services become its controllers; the export
    // mappings their operations apply are part of its domain, whether or not
    // the module publishes a service of its own.
    for stem in published_services
        .iter()
        .map(|service| module_stem(&service.module_name))
        .chain(
            export_mappings
                .iter()
                .map(|mapping| module_stem(&mapping.module_name)),
        )
        .collect::<std::collections::BTreeSet<_>>()
    {
        let services: Vec<&PublishedService> = published_services
            .iter()
            .filter(|service| module_stem(&service.module_name) == stem)
            .collect();
        let mappings: Vec<&ExportMappingDocument> = export_mappings
            .iter()
            .filter(|mapping| module_stem(&mapping.module_name) == stem)
            .collect();
        let Some(module_name) = services
            .first()
            .map(|service| service.module_name.clone())
            .or_else(|| mappings.first().map(|mapping| mapping.module_name.clone()))
        else {
            continue;
        };
        let module = generated_module(&mut generated_modules, &module_name);
        module.mappings = mappings
            .iter()
            .map(|mapping| (mapping.file_stem.clone(), render_export_mapping(mapping)))
            .collect();
        if !services.is_empty() {
            let mut files: Vec<(String, String)> = services
                .iter()
                .flat_map(|service| render_published_service(service, &mapping_targets))
                .collect();
            files.sort_by(|left, right| left.0.cmp(&right.0));
            module.http = Some(GeneratedHttp {
                index: render_module_http_index(&module_name, &files),
                files,
            });
        }
    }
    // Ownership combines the model's `FromAppStore` flag with any adjacent
    // MXRB authoring manifest discovered above.
    for module in &modules {
        let Some(module_name) = module.name.as_deref() else {
            continue;
        };
        let Some(entry) = generated_modules.get_mut(&module_stem(module_name)) else {
            continue;
        };
        if packages.contains(&module_stem(module_name)) {
            entry.root = ModuleRoot::Package;
            entry.version = module.app_store_version.clone().unwrap_or_default();
        }
    }
    let has_packages = generated_modules
        .values()
        .any(|module| module.root == ModuleRoot::Package);
    let authored_layers = write_modules_layer(destination, &generated_modules)?;
    // The services layer is always there, as `domain` is: a project with no
    // microflow yet still has the place its first one goes.
    if !authored_layers.contains_key("services") {
        let directory = destination.join("src/services");
        std::fs::create_dir_all(&directory).map_err(|source| io_error(&directory, source))?;
        write_text(&directory.join("mod.rs"), &render_services_index(&[]))?;
    }
    let domain_source = render_domain_module(&authored_layers, security_source.is_some());
    let ui_source = render_ui_module(&authored_layers);
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
    // The crate root is the layers and the application: every declaration
    // below registers itself, so there is nothing here to compose.
    write_text(
        &destination.join("src/lib.rs"),
        &format!(
            "pub mod controllers;\npub mod domain;\npub mod infrastructure;\n{}{}{}pub mod ui;\n\n#[mxrs::application(version = {})]\npub struct Application;\n",
            if has_packages {
                "pub mod packages;\n"
            } else {
                ""
            },
            if authored_layers.contains_key("ports") {
                "pub mod ports;\n"
            } else {
                ""
            },
            "pub mod services;\n",
            rust_string(&manifest.mendix_version),
        ),
    )?;
    write_text(&destination.join("src/domain/mod.rs"), &domain_source)?;
    write_text(&destination.join("src/ui/mod.rs"), &ui_source)?;
    if let Some(task_queues) = &task_queues_source {
        // The queues services run on are declared beside them.
        let index = destination.join("src/services/mod.rs");
        let mut source =
            std::fs::read_to_string(&index).map_err(|source| io_error(&index, source))?;
        source.push_str("pub mod task_queues;\n");
        write_text(&index, &source)?;
        write_text(
            &destination.join("src/services/task_queues.rs"),
            task_queues,
        )?;
    }
    if let Some(security) = &security_source {
        write_text(&destination.join("src/domain/security.rs"), security)?;
    }
    write_text(
        &destination.join("src/ui/navigation.rs"),
        &navigation_source,
    )?;
    match api_mode {
        ApiMode::Axum => {
            write_text(
                &controllers_directory.join("mod.rs"),
                &render_http_module(&published_services),
            )?;
            write_text(
                &controllers_directory.join("state.rs"),
                &render_http_state(&manifest.project_name),
            )?;
            write_text(
                &controllers_directory.join("error.rs"),
                &render_http_error(),
            )?;
        }
        _ => write_text(
            &controllers_directory.join("mod.rs"),
            &render_server_stub(api_mode),
        )?,
    }
    write_text(
        &destination.join("src/main.rs"),
        &build_binary_source(&crate_name, &manifest.project_name, api_mode),
    )?;
    write_text(
        &destination.join("src/infrastructure/mod.rs"),
        &infrastructure_source,
    )?;
    if let Some(persistence) = &persistence_source {
        write_text(
            &destination.join("src/infrastructure/persistence.rs"),
            persistence,
        )?;
    }
    // The axum state boots through the flow runtime, so the adapter exists
    // for every axum project, with or without typed service ports.
    let needs_flow_runtime = !service_ports.is_empty() || matches!(api_mode, ApiMode::Axum);
    // Only the axum boundary signs a request in; the other presets serve a
    // stub, so an authentication adapter there would be a promise nothing
    // keeps.
    let needs_authentication = matches!(api_mode, ApiMode::Axum);
    let mut adapters_index =
        String::from("//! Application-owned implementations of outbound ports belong here.\n");
    if needs_authentication || needs_flow_runtime {
        adapters_index
            .push_str("\n// What the generated controllers and runtime wiring reach for.\n");
    }
    if needs_authentication {
        adapters_index.push_str("pub use crate::infrastructure::generated::authentication;\n");
    }
    if needs_flow_runtime {
        adapters_index.push_str("pub use crate::infrastructure::generated::flow_runtime;\n");
    }
    write_text(
        &destination.join("src/infrastructure/adapters/mod.rs"),
        &adapters_index,
    )?;
    let mut generated_index = String::from(
        "//! MXRS-generated runtime integration. Regeneration may replace these files.\n\n",
    );
    for module in &action_ports {
        let _ = writeln!(generated_index, "pub mod {}_actions;", module.module_stem);
    }
    if needs_authentication {
        generated_index.push_str("pub mod authentication;\n");
    }
    if needs_flow_runtime {
        generated_index.push_str("pub mod flow_runtime;\n");
    }
    if !service_ports.is_empty() {
        generated_index.push_str("pub mod runtime_services;\n");
    }
    write_text(
        &destination.join("src/infrastructure/generated/mod.rs"),
        &generated_index,
    )?;
    if needs_authentication {
        write_text(
            &destination.join("src/infrastructure/generated/authentication.rs"),
            &render_authentication(),
        )?;
    }
    if needs_flow_runtime {
        write_text(
            &destination.join("src/infrastructure/generated/flow_runtime.rs"),
            &render_flow_runtime(),
        )?;
    }
    if !service_ports.is_empty() {
        write_text(
            &destination.join("src/infrastructure/generated/runtime_services.rs"),
            &render_runtime_services(&service_ports),
        )?;
    }
    for module in &action_ports {
        write_text(
            &destination.join(format!(
                "src/infrastructure/generated/{}_actions.rs",
                module.module_stem
            )),
            &render_action_registry_file(module),
        )?;
    }
    format_generated_cargo_project(destination)?;
    write_text(
        &destination.join(".gitignore"),
        "/build\n/target\n/frontend/node_modules\n/frontend/dist\n",
    )?;
    write_text(
        &destination.join("Dockerfile"),
        &dockerfile(&package_name, &manifest.project_name),
    )?;
    write_text(&destination.join("compose.yaml"), &compose_file())?;
    write_text(
        &destination.join(".dockerignore"),
        "target\nbuild\n.mxrs\nfrontend/node_modules\nfrontend/dist\n.git\n",
    )?;
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

/// The project-level domain layer: every module's model declarations, and
/// the project's own security when the model states one. It never depends
/// outward on services, infrastructure or UI.
fn render_domain_module(layers: &AuthoredLayers, security: bool) -> String {
    render_layer_module(
        "//! Business data and rules that do not depend on delivery or infrastructure.\n//! Persisted entities, value-like DTOs, enumerations and model security stay here;\n//! server-side workflows are `services`, not part of the domain.\n\n",
        "domain",
        layers,
        if security { &["security"] } else { &[] },
    )
}

/// One layer index: the concepts it holds, and nothing else — every
/// declaration below it registers itself, so a layer has nothing to compose.
///
/// `extra` names what the layer holds beyond the per-module concept folders
/// this writer tracks: project-level files like `security` or `navigation`.
fn render_layer_module(
    header: &str,
    layer: &str,
    layers: &AuthoredLayers,
    extra: &[&str],
) -> String {
    let mut declared = layers.get(layer).cloned().unwrap_or_default();
    declared.extend(extra.iter().map(|name| (*name).to_string()));
    declared.sort();
    declared.dedup();
    let mut out = String::from(header);
    for concept in &declared {
        let _ = writeln!(out, "pub mod {concept};");
    }
    out
}

/// The services layer's index: each module's folder of microflows. The
/// project's task queues, when it declares any, are added by the caller.
fn render_services_index(modules: &[&str]) -> String {
    let mut out = String::from(
        "//! What the application does: every module's microflows as services, one\n//! folder per Mendix module and one file per microflow, and the task queues\n//! they run on. A service coordinates the domain and knows no HTTP\n//! framework, database driver or other delivery mechanism.\n\n",
    );
    for module in modules {
        let _ = writeln!(out, "pub mod {module};");
    }
    out
}

/// The user interface the model declares: navigation, and every module's
/// pages, layouts and nanoflows. What the application serves over HTTP is
/// not here — that is `controllers`.
fn render_ui_module(layers: &AuthoredLayers) -> String {
    render_layer_module(
        "//! The project's user interface: navigation, and every module's pages,\n//! layouts and nanoflows, one folder per Mendix module inside each concept.\n\n",
        "ui",
        layers,
        &["navigation"],
    )
}

fn render_infrastructure_module(persistence: bool) -> String {
    format!(
        "//! Runtime integration, adapters and the model's own storage documents.\n\
         //! Hand-written repositories and database plumbing belong here too.\n\n\
         pub mod adapters;\n\
         pub mod generated;\n{}",
        if persistence {
            "pub mod persistence;\n"
        } else {
            ""
        }
    )
}

/// The non-axum presets keep their framework's server stub until their
/// published-REST routers are ported.
fn render_server_stub(api_mode: ApiMode) -> String {
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
        "//! {} server stub. Published REST services are routed for the axum\n//! preset only; this preset keeps the model routes in model/imported\n//! until its router is ported.\n\npub mod server {{\n{}\n}}\n",
        api_mode.name(),
        indent(server, 4)
    )
}

/// The OQL documents the project's views read from: one self-registering
/// function per module that declares any. `None` when the model has none.
fn render_persistence_module(modules: &[Module]) -> Option<String> {
    let mut out = String::from(
        "//! The OQL view sources the project's view entities read from.\n\n\
         use mxrs::prelude::*;\n",
    );
    let mut declared = false;
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
        declared = true;
        let _ = writeln!(
            out,
            "\n#[declaration(module = {module_name:?}, stage = ViewSource)]\npub fn {}_view_sources(module: &mut ModuleBuilder) {{",
            module_stem(module_name)
        );
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
                "    module.oql_view_source({name:?}, {query:?}, |{parameter}| {{"
            );
            if !documentation.is_empty() {
                let _ = writeln!(out, "        source.documentation({documentation:?});");
            }
            if excluded {
                out.push_str("        source.excluded(true);\n");
            }
            if export_level == "Published" {
                out.push_str("        source.export_level(ExportLevel::Published);\n");
            }
            out.push_str("    });\n");
        }
        out.push_str("}\n");
    }
    declared.then_some(out)
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
    /// `(module name, file stem, source)` per non-enumeration document.
    module_files: Vec<(String, String, String)>,
    /// `(module name, file stem, source)` per enumeration.
    enumeration_files: Vec<(String, String, String)>,
    /// Qualified `Module.Name` → the Rust enum a typed entity field can
    /// reference, for enumerations rendered as `#[derive(MxEnumeration)]`.
    derived_enumerations: HashMap<String, DerivedEnumeration>,
    counts: HashMap<&'static str, usize>,
}

/// The folder name one Mendix module's generated Rust lives under —
/// `src/<root>/<stem>/`, where the root is [`ModuleRoot`].
fn module_stem(module_name: &str) -> String {
    let mut stem = collapse_underscores(&snake_ident(module_name));
    if rust_keyword(&stem) {
        stem.push_str("_module");
    }
    stem
}

/// Which of the two module trees a module's generated Rust belongs to.
///
/// `src/modules/` is what this project created. `src/packages/` is what it
/// installed — `Projects$Module`'s own `FromAppStore` says which — and the
/// difference is ownership, not taste: an edit to an installed module is lost
/// the next time that module is upgraded. So a package contributes its *types
/// and contracts* (entity and DTO structs, enumerations, ports, markers, and
/// any REST surface the application serves) and never an `apply`: the imported
/// snapshot stays the only source of truth for what a package declares.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord)]
enum ModuleRoot {
    /// `src/modules/` — a module this project created.
    #[default]
    Authored,
    /// `src/packages/` — a module installed from the Marketplace.
    Package,
}

impl ModuleRoot {
    /// The `crate::` path one of a module's generated concepts is importable
    /// from.
    ///
    /// The authored tree is layer-first: one `domain/`, one `controllers/`,
    /// one `ui/` for the whole project, with the Mendix module a folder
    /// *inside* each concept. The module folder is not decoration — Mendix
    /// entity names are unique per module and not across a project, so a
    /// flat `domain/entities/` would not compile for a model that declares
    /// one name in two modules.
    ///
    /// A package keeps its own module shape, so its concepts are addressed
    /// through it.
    fn path(self, module_stem: &str, authored: &str, package: &str) -> String {
        match self {
            ModuleRoot::Authored => format!("crate::{authored}::{module_stem}"),
            ModuleRoot::Package => format!("crate::packages::{module_stem}::{package}"),
        }
    }
}

/// The module stems `FromAppStore` marks as installed rather than authored.
fn package_stems(modules: &[Module]) -> std::collections::BTreeSet<String> {
    modules
        .iter()
        .filter(|module| module.from_app_store)
        .filter_map(|module| module.name.as_deref())
        .map(module_stem)
        .collect()
}

/// Resolves package ownership with the model flag first and an optional MXRB
/// authoring manifest second. MXRB projects keep one
/// `modules/<Name>/module.rb` only for modules the application owns; imported
/// Marketplace modules are present in the MPR but intentionally absent there.
/// When that manifest exists it is stronger evidence than old MPRs whose
/// `FromAppStore` flag was lost during generation.
fn package_stems_for_import(
    modules: &[Module],
    mpr_path: &Path,
) -> std::collections::BTreeSet<String> {
    let mut packages = package_stems(modules);
    let modules_directory = mpr_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("modules");
    let Ok(entries) = std::fs::read_dir(&modules_directory) else {
        return packages;
    };
    let authored = entries
        .filter_map(std::result::Result::ok)
        .filter(|entry| entry.path().join("module.rb").is_file())
        .filter_map(|entry| entry.file_name().into_string().ok())
        .map(|name| module_stem(&name))
        .collect::<std::collections::BTreeSet<_>>();
    if authored.is_empty() {
        return packages;
    }
    packages.extend(
        modules
            .iter()
            .filter_map(|module| module.name.as_deref())
            .map(module_stem)
            .filter(|stem| !authored.contains(stem)),
    );
    packages
}

fn module_root(stem: &str, packages: &std::collections::BTreeSet<String>) -> ModuleRoot {
    if packages.contains(stem) {
        ModuleRoot::Package
    } else {
        ModuleRoot::Authored
    }
}

/// One enumeration that rendered as a real Rust enum: where it lives under
/// its module's `domain/enumerations/` and the type name the file declares.
struct DerivedEnumeration {
    root: ModuleRoot,
    module_stem: String,
    file_stem: String,
    type_name: String,
}

impl DerivedEnumeration {
    fn module_path(&self) -> String {
        format!(
            "{}::{}",
            self.root.path(
                &self.module_stem,
                "domain::enumerations",
                "domain::enumerations"
            ),
            self.file_stem,
        )
    }
}

fn render_documents_module(
    project: &Project,
    packages: &std::collections::BTreeSet<String>,
) -> Result<DocumentsExport> {
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
    let mut documents_by_module: Vec<(String, String, String)> = Vec::new();
    for declaration in declarations {
        if let EditableDocument::Enumeration {
            module,
            name,
            documentation,
            values,
        } = declaration
        {
            let stem = inner_file_stem(&name);
            let root = module_root(&module_stem(&module), packages);
            let (source, type_name) = render_enumeration_file(
                &module,
                &name,
                &documentation,
                &values,
                root == ModuleRoot::Package,
            );
            derived_enumerations.insert(
                format!("{module}.{name}"),
                DerivedEnumeration {
                    root,
                    module_stem: module_stem(&module),
                    file_stem: stem.clone(),
                    type_name,
                },
            );
            enumeration_files.push((module, stem, source));
            continue;
        }
        let module = editable_document_module(&declaration).to_string();
        let stem = inner_file_stem(editable_document_key(&declaration).2);
        let source = render_document_file(&module, &stem, declaration);
        documents_by_module.push((module, stem, source));
    }

    let module_files = documents_by_module;

    Ok(DocumentsExport {
        module_files,
        enumeration_files,
        derived_enumerations,
        counts,
    })
}

/// One Mendix document — a constant, regular expression, scheduled event
/// or standalone menu — as its own editable file. A constant is the function
/// that builds it; the other kinds declare into their module through its
/// builder. `stem` is the file's, and so the function's.
fn render_document_file(module_name: &str, stem: &str, document: EditableDocument) -> String {
    if let EditableDocument::Constant {
        module,
        name,
        documentation,
        value_type,
        value,
        exposed_to_client,
    } = &document
    {
        let mut source = String::from("use mxrs::prelude::*;\n\n");
        let mut lines = Vec::new();
        match entity_export::doc_comment(documentation, "") {
            Some(comment) => source.push_str(&comment),
            None => lines.push(format!(
                "constant.documentation({});",
                rust_string(documentation)
            )),
        }
        lines.push(format!("constant.value_type(ConstantType::{value_type});"));
        lines.push(match value {
            Some(value) => format!("constant.value({});", rust_string(value)),
            None => format!(
                "constant.value_from_env({});",
                rust_string(&constant_environment_variable(module, name))
            ),
        });
        if *exposed_to_client {
            lines.push("constant.exposed_to_client(true);".to_string());
        }
        let mut arguments = vec![format!("module = {}", rust_string(module_name))];
        if derive_pascal_case(stem) != *name {
            arguments.push(format!("name = {}", rust_string(name)));
        }
        let _ = writeln!(
            source,
            "#[constant({})]\npub fn {stem}(constant: &mut ConstantBuilder) {{\n{}\n}}",
            arguments.join(", "),
            lines.join("\n")
        );
        return source;
    }
    let kind = match &document {
        EditableDocument::RegularExpression { .. } => "Regular expression",
        EditableDocument::ScheduledEvent { .. } => "Scheduled event",
        EditableDocument::Menu { .. } => "Menu",
        EditableDocument::Constant { .. }
        | EditableDocument::Enumeration { .. }
        | EditableDocument::TaskQueue { .. } => "Document",
    };
    let name = editable_document_key(&document).2.to_string();
    let mut source = format!(
        "//! {kind} `{module_name}.{name}`.\n\n\
         use mxrs::prelude::*;\n\n\
         #[declaration(module = {})]\n\
         pub fn {stem}(module: &mut ModuleBuilder) {{\n",
        rust_string(module_name),
    );
    render_editable_document_body(&mut source, document);
    source.push_str("}\n");
    source
}

/// One Mendix enumeration as the Rust enum that declares it. Values are
/// variants; a value or type name Rust cannot spell is spelled differently
/// and stated with `name = "..."`. An enumeration an installed module owns
/// is `imported`: named here, declared there.
fn render_enumeration_file(
    module_name: &str,
    name: &str,
    documentation: &str,
    values: &[(String, Vec<(String, String)>)],
    imported: bool,
) -> (String, String) {
    let mut type_name = derive_pascal_case(&sanitize_ident(name));
    if type_name.starts_with(|c: char| c.is_ascii_digit()) {
        type_name.insert(0, '_');
    }
    if type_name.is_empty() || rust_keyword(&type_name) {
        type_name.push_str("Enumeration");
    }

    let mut seen = std::collections::HashSet::new();
    let mut variants = String::new();
    for (value, captions) in values {
        let mut variant = derive_pascal_case(&sanitize_ident(value));
        if variant.starts_with(|c: char| c.is_ascii_digit()) {
            variant.insert(0, '_');
        }
        if variant.is_empty() || rust_keyword(&variant) {
            variant.push_str("Value");
        }
        if !seen.insert(variant.clone()) {
            variant = (2..)
                .map(|suffix| format!("{variant}{suffix}"))
                .find(|candidate| seen.insert(candidate.clone()))
                .expect("an unbounded suffix always finds a free name");
        }
        let mut options = Vec::new();
        if variant != *value {
            options.push(format!("name = {value:?}"));
        }
        // One caption in one language is the usual value; anything else —
        // none, several, or a language code that is not an identifier —
        // lists every caption.
        match captions.as_slice() {
            [(language, caption)] if mxrs_typegen::is_rust_identifier(language) => {
                if caption != value {
                    options.push(format!("caption = {caption:?}"));
                }
                if language != "en_US" {
                    options.push(format!("language = {language:?}"));
                }
            }
            captions => options.push(format!(
                "captions({})",
                captions
                    .iter()
                    .map(|(language, caption)| {
                        if mxrs_typegen::is_rust_identifier(language) {
                            format!("{language} = {caption:?}")
                        } else {
                            format!("{language:?} = {caption:?}")
                        }
                    })
                    .collect::<Vec<_>>()
                    .join(", ")
            )),
        }
        if !options.is_empty() {
            let _ = writeln!(variants, "    #[mxrs({})]", options.join(", "));
        }
        let _ = writeln!(variants, "    {variant},");
    }

    let mut out = String::from("use mxrs::prelude::*;\n\n");
    let mut options = Vec::new();
    match entity_export::doc_comment(documentation, "") {
        Some(comment) => out.push_str(&comment),
        None => options.push(format!("documentation = {documentation:?}")),
    }
    let mut arguments = vec![format!("module = {module_name:?}")];
    if type_name != name {
        arguments.push(format!("name = {name:?}"));
    }
    if imported {
        arguments.push("imported".to_string());
    }
    let _ = writeln!(out, "#[enumeration({})]", arguments.join(", "));
    for option in options {
        let _ = writeln!(out, "#[mxrs({option})]");
    }
    if variants.is_empty() {
        let _ = writeln!(out, "pub enum {type_name} {{}}");
    } else {
        let _ = writeln!(out, "pub enum {type_name} {{\n{variants}}}");
    }
    (out, type_name)
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

/// The project's task queues for a Cargo project: one self-registering
/// function per module that declares any. `None` when the model has none.
fn render_task_queue_declarations(project: &Project) -> Result<Option<String>> {
    let mut queues = collect_editable_documents(project)?;
    queues.retain(|document| matches!(document, EditableDocument::TaskQueue { .. }));
    if queues.is_empty() {
        return Ok(None);
    }
    queues.sort_by(|left, right| editable_document_key(left).cmp(&editable_document_key(right)));
    let mut source = String::from(
        "//! The project's task queues, declared into the module that owns each.\n\n\
         use mxrs::prelude::*;\n",
    );
    let mut current_module = None::<String>;
    for declaration in queues {
        let module = editable_document_module(&declaration).to_string();
        if current_module.as_deref() != Some(module.as_str()) {
            if current_module.is_some() {
                source.push_str("}\n");
            }
            let _ = writeln!(
                source,
                "\n#[declaration(module = {}, stage = TaskQueue)]\npub fn {}_task_queues(module: &mut ModuleBuilder) {{",
                rust_string(&module),
                module_stem(&module)
            );
            current_module = Some(module);
        }
        let mut body = String::new();
        render_editable_document_body(&mut body, declaration);
        // The body is shared with the single-file export, which names the
        // implementation crate; a Cargo project has the prelude instead.
        source.push_str(&body.replace("::mxrs_ir::", ""));
    }
    source.push_str("}\n");
    Ok(Some(source))
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
        demo_users: vec![],
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

/// The variant each of a module's roles is declared as, in the order the
/// enum lists them: the role's name and the identifier that spells it.
fn role_variants(roles: &[mxrs_model::module::ModuleRole]) -> Vec<(String, String)> {
    let mut roles = roles.iter().collect::<Vec<_>>();
    roles.sort_by(|left, right| left.name.cmp(&right.name));
    let mut seen = std::collections::HashSet::new();
    roles
        .into_iter()
        .map(|role| {
            let name = role.name.as_deref().unwrap_or("Unnamed");
            let mut variant = derive_pascal_case(&sanitize_ident(name));
            if variant.starts_with(|c: char| c.is_ascii_digit()) {
                variant.insert(0, '_');
            }
            if variant.is_empty() || rust_keyword(&variant) {
                variant.push_str("Role");
            }
            if !seen.insert(variant.clone()) {
                variant = (2..)
                    .map(|suffix| format!("{variant}{suffix}"))
                    .find(|candidate| seen.insert(candidate.clone()))
                    .expect("an unbounded suffix always finds a free name");
            }
            (name.to_string(), variant)
        })
        .collect()
}

/// One module's roles as the enum that declares them: a variant per role,
/// its description as the variant's comment.
fn render_module_security(
    module_name: &str,
    roles: &[mxrs_model::module::ModuleRole],
) -> Option<String> {
    if roles.is_empty() {
        return None;
    }
    let mut source = format!(
        "use mxrs::prelude::*;\n\n#[module_roles(module = {})]\npub enum Role {{\n",
        rust_string(module_name),
    );
    let variants = role_variants(roles);
    let mut roles = roles.iter().collect::<Vec<_>>();
    roles.sort_by(|left, right| left.name.cmp(&right.name));
    for (role, (_, variant)) in roles.into_iter().zip(variants) {
        let name = role.name.as_deref().unwrap_or("Unnamed");
        let mut options = Vec::new();
        if variant != name {
            options.push(format!("name = {}", rust_string(name)));
        }
        match entity_export::doc_comment(&role.description, "    ") {
            Some(comment) => source.push_str(&comment),
            None => options.push(format!("description = {}", rust_string(&role.description))),
        }
        if !options.is_empty() {
            let _ = writeln!(source, "    #[mxrs({})]", options.join(", "));
        }
        let _ = writeln!(source, "    {variant},");
    }
    source.push_str("}\n");
    Some(source)
}

/// The project's security as the function that builds it, stating only what
/// differs from the builder's own defaults. `None` when the model carries
/// no security this renderer can restate — the imported one then stays as
/// it is.
fn render_project_security_module(document: Option<&mxrs_bson::Document>) -> Option<String> {
    let document = document?;
    let level = render_security_level(document.get_str("SecurityLevel").ok())?;
    let mut source = String::from(
        "//! The project's security. Each module's own roles are declared in\n\
         //! `crate::domain::module_security`.\n\n\
         use mxrs::prelude::*;\n\n\
         #[security]\n\
         pub fn security(security: &mut SecurityBuilder) {\n",
    );
    let _ = writeln!(source, "    security.level(SecurityLevel::{level});");
    if !document.get_bool("CheckSecurity").unwrap_or(true) {
        source.push_str("    security.check_security(false);\n");
    }
    let admin_role = document.get_str("AdminUserRole").unwrap_or("Administrator");
    if admin_role != "Administrator" {
        let _ = writeln!(
            source,
            "    security.admin_role({});",
            rust_string(admin_role)
        );
    }
    if let Some(guest_role) = document
        .get_bool("EnableGuestAccess")
        .unwrap_or(false)
        .then(|| document.get_str("GuestUserRole").unwrap_or_default())
        .filter(|name| !name.is_empty())
    {
        let _ = writeln!(
            source,
            "    security.guest_access({});",
            rust_string(guest_role)
        );
    }
    if let Some(sign_in) = document
        .get_str("SignInMicroflow")
        .ok()
        .filter(|name| !name.is_empty())
    {
        let _ = writeln!(
            source,
            "    security.sign_in_microflow({});",
            rust_string(sign_in)
        );
    }
    // The builder starts with the platform's own Administrator role; the
    // model's roles are stated in full instead.
    source.push_str("    security.clear_roles();\n");
    let mut roles = bson_documents(document, "UserRoles");
    roles.sort_by(|left, right| {
        left.get_str("Name")
            .unwrap_or_default()
            .cmp(right.get_str("Name").unwrap_or_default())
    });
    for role in roles {
        let name = role.get_str("Name").unwrap_or("Unnamed");
        let description = role.get_str("Description").unwrap_or_default();
        let mut lines = Vec::new();
        if !description.is_empty() {
            lines.push(format!("role.description({});", rust_string(description)));
        }
        if role.get_bool("ManageAllRoles").unwrap_or(false) {
            lines.push("role.administrator(true);".to_string());
        }
        if !role.get_bool("CheckSecurity").unwrap_or(true) {
            lines.push("role.check_security(false);".to_string());
        }
        if role.get_bool("ManageUsersWithoutRoles").unwrap_or(false) {
            lines.push("role.manage_users_without_roles(true);".to_string());
        }
        for manageable in bson_strings(&role, "ManageableRoles") {
            lines.push(format!(
                "role.manageable_role({});",
                rust_string(&manageable)
            ));
        }
        for module_role in bson_strings(&role, "ModuleRoles") {
            lines.push(format!("role.module_role({});", rust_string(&module_role)));
        }
        if lines.is_empty() {
            let _ = writeln!(
                source,
                "    security.role({}, |_| {{}});",
                rust_string(name)
            );
        } else {
            let _ = writeln!(
                source,
                "    security.role({}, |role| {{\n{}\n    }});",
                rust_string(name),
                lines.join("\n")
            );
        }
    }
    // Imported demo users stay losslessly preserved in the stored
    // `DemoUsers` array (the writer leaves it untouched for an empty
    // declaration list); exporting them would either embed their stored
    // passwords in source or reserialize content nobody edited.
    let policy = document.get_document("PasswordPolicySettings").ok();
    let minimum_length = policy
        .and_then(|policy| bson_integer(policy.get("MinimumLength")))
        .unwrap_or(6);
    let flag = |key: &str, default: bool| {
        policy
            .and_then(|policy| policy.get_bool(key).ok())
            .unwrap_or(default)
    };
    let mut policy_lines = Vec::new();
    if minimum_length != 6 {
        policy_lines.push(format!("policy.minimum_length = {minimum_length};"));
    }
    if !flag("RequireDigit", true) {
        policy_lines.push("policy.require_digit = false;".to_string());
    }
    if !flag("RequireMixedCase", true) {
        policy_lines.push("policy.require_mixed_case = false;".to_string());
    }
    if flag("RequireSymbol", false) {
        policy_lines.push("policy.require_symbol = true;".to_string());
    }
    if !policy_lines.is_empty() {
        let _ = writeln!(
            source,
            "    security.password_policy(|policy| {{\n{}\n    }});",
            policy_lines.join("\n")
        );
    }
    source.push_str("}\n");
    Some(source)
}

/// Whether the navigation builder can say exactly what the model holds: it
/// gives an item or a home one target, never two and never an empty role
/// home.
fn navigation_is_buildable(navigation: &mxrs_model::Navigation) -> bool {
    fn item_is_buildable(item: &mxrs_model::navigation::NavigationItem) -> bool {
        !(item.page.is_some() && item.microflow.is_some())
            && item.items.iter().all(item_is_buildable)
    }
    navigation.profiles.iter().all(|profile| {
        !(profile.home_page.is_some() && profile.home_microflow.is_some())
            && profile
                .role_homes
                .iter()
                .all(|home| home.page.is_some() != home.microflow.is_some())
            && profile.menu_items.iter().all(item_is_buildable)
    })
}

/// The project's navigation as the function that builds it. A model the
/// builder cannot restate exactly is registered as the declaration itself
/// instead, so nothing is approximated.
fn render_navigation_module(navigation: &mxrs_model::Navigation) -> String {
    if !navigation_is_buildable(navigation) {
        return render_navigation_declaration(navigation);
    }
    let mut source = String::from(
        "//! The project's navigation profiles.\n\n\
         use mxrs::prelude::*;\n\n\
         #[navigation]\n",
    );
    let mut profiles = navigation.profiles.iter().collect::<Vec<_>>();
    profiles.sort_by(|left, right| left.name.cmp(&right.name));
    let parameter = if profiles.is_empty() {
        "_navigation"
    } else {
        "navigation"
    };
    let _ = writeln!(
        source,
        "pub fn navigation({parameter}: &mut NavigationBuilder) {{"
    );
    for profile in profiles {
        let mut lines = Vec::new();
        if profile.kind != "Responsive" {
            lines.push(format!("profile.kind({});", rust_string(&profile.kind)));
        }
        for (locale, text) in &profile.app_title {
            lines.push(format!(
                "profile.title({}, {});",
                rust_string(locale),
                rust_string(text)
            ));
        }
        if let Some(page) = &profile.home_page {
            lines.push(format!("profile.home_page({});", rust_string(page)));
        }
        if let Some(microflow) = &profile.home_microflow {
            lines.push(format!(
                "profile.home_microflow({});",
                rust_string(microflow)
            ));
        }
        if let Some(page) = &profile.sign_in_page {
            lines.push(format!("profile.sign_in_page({});", rust_string(page)));
        }
        for home in &profile.role_homes {
            let role = rust_string(home.role.as_deref().unwrap_or_default());
            match (&home.page, &home.microflow) {
                (Some(page), _) => {
                    lines.push(format!(
                        "profile.home_for_page({role}, {});",
                        rust_string(page)
                    ));
                }
                (None, Some(microflow)) => lines.push(format!(
                    "profile.home_for_microflow({role}, {});",
                    rust_string(microflow)
                )),
                (None, None) => unreachable!("checked by navigation_is_buildable"),
            }
        }
        for item in &profile.menu_items {
            render_navigation_item_call(&mut lines, item, "profile");
        }
        if lines.is_empty() {
            let _ = writeln!(
                source,
                "    navigation.profile({}, |_| {{}});",
                rust_string(&profile.name)
            );
        } else {
            let _ = writeln!(
                source,
                "    navigation.profile({}, |profile| {{\n{}\n    }});",
                rust_string(&profile.name),
                lines.join("\n")
            );
        }
    }
    source.push_str("}\n");
    source
}

/// One navigation item as a call on `parent`. An item captioned in `en_US`
/// states that caption as the call's own argument; one captioned only in
/// other languages starts without a caption.
fn render_navigation_item_call(
    lines: &mut Vec<String>,
    item: &mxrs_model::navigation::NavigationItem,
    parent: &str,
) {
    let mut body = Vec::new();
    for (locale, text) in &item.caption {
        if locale != "en_US" {
            body.push(format!(
                "item.caption({}, {});",
                rust_string(locale),
                rust_string(text)
            ));
        }
    }
    if let Some(page) = &item.page {
        body.push(format!("item.page({});", rust_string(page)));
    }
    if let Some(microflow) = &item.microflow {
        body.push(format!("item.microflow({});", rust_string(microflow)));
    }
    match &item.icon {
        Some(mxrs_model::navigation::NavigationIcon::Glyph(value)) => {
            body.push(format!("item.icon({});", rust_string(value)));
        }
        Some(mxrs_model::navigation::NavigationIcon::Code(value)) => {
            body.push(format!("item.icon_code({value});"));
        }
        None => {}
    }
    for child in &item.items {
        render_navigation_item_call(&mut body, child, "item");
    }
    let opening = match item.caption.get("en_US") {
        Some(caption) => format!("{parent}.item({}, ", rust_string(caption)),
        None => format!("{parent}.localized_item("),
    };
    if body.is_empty() {
        lines.push(format!("{opening}|_| {{}});"));
    } else {
        lines.push(format!("{opening}|item| {{\n{}\n}});", body.join("\n")));
    }
}

/// The navigation as the declaration itself, for the rare model the builder
/// cannot restate exactly.
fn render_navigation_declaration(navigation: &mxrs_model::Navigation) -> String {
    let mut source = String::from(
        "//! The project's navigation profiles, stated as the declaration itself:\n\
         //! this model gives an item or a home more than one target, which the\n\
         //! navigation builder has no way to say.\n\n\
         mxrs::register!(Navigation, |project| {\n\
         project.navigation = Some(::mxrs::NavigationDecl { profiles: vec![\n",
    );
    let mut profiles = navigation.profiles.iter().collect::<Vec<_>>();
    profiles.sort_by(|left, right| left.name.cmp(&right.name));
    for profile in profiles {
        let _ = writeln!(
            source,
            "        ::mxrs::NavigationProfileDecl {{ name: {}.to_string(), kind: {}.to_string(), app_title: {}, home_page: {}, home_microflow: {}, sign_in_page: {}, role_homes: vec![",
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
                "            ::mxrs::RoleHomeDecl {{ user_role: {}.to_string(), page: {}, microflow: {} }},",
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
    source.push_str("    ] });\n});\n");
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
        "{indent}::mxrs::NavigationItemDecl {{ caption: {}, page: {}, microflow: {}, icon: {}, items: vec![",
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
            "Some(::mxrs::NavigationIconDecl::Glyph({}.to_string()))",
            rust_string(value)
        ),
        Some(mxrs_model::navigation::NavigationIcon::Code(value)) => {
            format!("Some(::mxrs::NavigationIconDecl::Code({value}))")
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

/// Builds the public marker surface directly from the imported model. Flow
/// bodies may remain opaque while their names are still compile-time checked
/// by pages and future Cargo-native flow authoring.
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
            "axum = \"0.8\"\nserde_json = \"1\"\ntokio = { version = \"1\", features = [\"macros\", \"rt-multi-thread\", \"net\"] }\n"
        }
        ApiMode::ActixWeb => "actix-web = \"4\"\n",
        ApiMode::Rocket => "rocket = \"0.5\"\n",
    };
    format!(
        "[package]\nname = {package_name:?}\nversion = \"0.1.0\"\nedition = \"2024\"\nrust-version = \"1.85\"\npublish = false\n\n[lints.rust]\nunsafe_code = \"forbid\"\n\n[lints.clippy]\nall = {{ level = \"warn\", priority = -1 }}\n# A Mendix module may hold an entity of its own name.\nmodule_inception = \"allow\"\n\n[dependencies]\nmxrs = {dependency}\n{api_dependencies}",
    )
}

fn dockerfile(package_name: &str, project_name: &str) -> String {
    format!(
        "FROM rust:1.85-bookworm AS builder\nWORKDIR /workspace\nCOPY . .\nRUN cargo build --release\nRUN ./target/release/{package_name} build/{project_name}.mpr\n\nFROM debian:bookworm-slim AS runtime\nRUN apt-get update && apt-get install -y --no-install-recommends ca-certificates && rm -rf /var/lib/apt/lists/*\nWORKDIR /app\nCOPY --from=builder /workspace/target/release/{package_name} /usr/local/bin/application\nCOPY --from=builder /workspace/build ./build\nEXPOSE 3000\nCMD [\"application\", \"serve\", \"/app/build/{project_name}.mpr\"]\n"
    )
}

fn compose_file() -> String {
    "services:\n  api:\n    build:\n      context: .\n    ports:\n      - \"3000:3000\"\n  frontend:\n    image: node:24-alpine\n    working_dir: /workspace/frontend\n    command: sh -c \"npm ci && npm run dev -- --host 0.0.0.0 --port 5173 --strictPort\"\n    environment:\n      MXRS_API_ORIGIN: http://api:3000\n    volumes:\n      - .:/workspace\n      - frontend_node_modules:/workspace/frontend/node_modules\n    ports:\n      - \"5173:5173\"\n    depends_on:\n      - api\n\nvolumes:\n  frontend_node_modules:\n"
        .to_string()
}

fn build_binary_source(crate_name: &str, project_name: &str, api_mode: ApiMode) -> String {
    let default_output = format!("build/{project_name}.mpr");
    let serve = match api_mode {
        ApiMode::Axum => format!(
            "#[tokio::main]\nasync fn main() -> Result<(), Box<dyn std::error::Error>> {{\n    if std::env::args().nth(1).as_deref() == Some(\"serve\") {{\n        return {crate_name}::controllers::serve(\n            std::env::args().nth(2).unwrap_or_else(|| {crate_name}::controllers::DEFAULT_MODEL.to_string()),\n            \"0.0.0.0:3000\",\n        )\n        .await;\n    }}\n    build()\n}}\n"
        ),
        ApiMode::ActixWeb => format!(
            "#[actix_web::main]\nasync fn main() -> Result<(), Box<dyn std::error::Error>> {{\n    if std::env::args().nth(1).as_deref() == Some(\"serve\") {{\n        {crate_name}::controllers::server::serve().await?;\n        return Ok(());\n    }}\n    build()\n}}\n"
        ),
        ApiMode::Rocket => format!(
            "#[rocket::main]\nasync fn main() -> Result<(), Box<dyn std::error::Error>> {{\n    if std::env::args().nth(1).as_deref() == Some(\"serve\") {{\n        {crate_name}::controllers::server::serve().await?;\n        return Ok(());\n    }}\n    build()\n}}\n"
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
            "\n`src/ui/pages/` holds {} page(s) this import detected as buildable from\nmxrs-dsl's native/structural widget vocabulary (out of {} page(s) total), each a\n`#[page]` function in its module's folder.\n",
            page_export.typed_candidates,
            page_export.typed_candidates + page_export.opaque,
        )
    } else {
        String::new()
    };
    format!(
        "# {project_name}\n\nCargo-native Mendix project imported by `mxrs`. The source uses a layer-first architecture with Mendix modules nested only where names need a namespace:\n\n- `src/domain/`: persisted entities, non-persistable/view data, enumerations, export mappings, model documents and security. It has no HTTP or database dependency.\n- `src/services/`: what the application does. Every microflow is a service: one folder per Mendix module, one `<name>_service.rs` per microflow with the function that declares it, and the task queues they run on.\n- `src/ports/`: the contracts between the model and hand-written code — what each module's services offer, and what its actions need an adapter in `infrastructure` to provide.\n- `src/controllers/`: what the application serves over HTTP. Each module's folder holds a route table per published REST service and a controller per resource, with one function per operation; the shared router, state and error type are at the top. Axum-specific types stay here.\n- `src/ui/`: the user interface the model declares — pages, layouts, nanoflows and navigation.\n- `src/infrastructure/`: runtime, persistence, authentication and external-action adapters.\n- `frontend/`: editable React + TypeScript + Vite client. It renders the imported page/widget tree and calls the Rust runtime through `/api`. Its `src/` is laid out the way a React project is: `api/` (calls to the runtime), `components/` (`layout/`, `widgets/`), `hooks/`, `pages/`, `types/`, `utils/` and `styles/`, with `@/` naming `src/`.\n- `model/imported/`: lossless model data and stable Mendix identities that do not yet have a typed Rust representation.\n\nMarketplace modules remain grouped under `src/packages/<module>/` because they are external, upgradeable dependencies rather than application-owned code.\n\n## Declaring the model\n\nA declaration is one annotated item in its own file, and it registers itself: adding one is the file plus its `pub mod` line.\n\n```rust\nuse mxrs::prelude::*;\n\n/// A customer order.\n#[entity(module = \"Sales\")]\n#[mxrs(index(number))]\npub struct Order {{\n    #[mxrs(length = 80, required)]\n    pub number: MxString,\n    pub total: MxDecimal,\n}}\n\n#[microflow(ACT, module = \"Sales\")]\npub fn create_order(flow: &mut FlowBuilder) {{\n    let number = flow.parameter::<MxString>(\"Number\", |_| {{}});\n    let order = flow.create_object(\n        \"Order\",\n        Ref::<Order>::new(),\n        vec![Order::number().set(number)],\n        true,\n    );\n    flow.return_value(order);\n}}\n```\n\n`#[entity]`, `#[dto]` and `#[view]` declare persistable, non-persistable and OQL-view entities; `#[enumeration]` declares an enumeration from a Rust enum. Fields are attributes and associations (`Reference<T>`, `ReferenceSet<T>`), `///` comments are the model's documentation, and every field has an accessor (`Order::number()`) wherever a flow or page names it. `#[microflow(ACT, ...)]` on `create_order` declares `ACT_CreateOrder`, nameable elsewhere as the type `ACT_CreateOrder`; `#[nanoflow]` is its client-side counterpart. The attribute also states who may run a flow and what it is related to — `roles(...)`, `calls(...)`, `uses(...)`, `used_by(...)` — each naming the Rust item that declares the role, flow or entity; a build reports where the last three no longer match the model.\n\nNothing here carries a Mendix identifier. An artifact that came from the imported model keeps the identity `model/imported/` records for it; a new one gets a stable identity derived from the project and its qualified name, the same on every build. Renaming an artifact in Rust therefore declares a new artifact; it does not rename the imported one.\n\nEvery microflow is a service under `src/services/`; client-side nanoflows stay in `ui`. `mxrs run --frontend` supervises the Rust API and Vite client together. A flow that cannot be declared yet keeps its file, which names it and says why it stayed in the imported model. Edits with the same activity structure preserve node identities and layout; structural edits rebuild the graph.\n\n```sh\n# Mendix → Rust\nmxrs convert mendix-to-rust app.mpr --output . --mode axum\n\ncargo fmt --check\ncargo check\ncargo clippy --all-targets -- -D warnings\ncargo test\n\n# Install the pinned browser client once, then run both processes\nnpm install --prefix frontend\nmxrs run . --frontend\n\n# Rust → Mendix\nmxrs convert rust-to-mendix . --output build/{project_name}.mpr\n```\n\nChoose `--mode axum`, `--mode actix-web`, or `--mode rocket` during import. `mxrs run` materializes missing web assets itself and shuts down cleanly on interrupt; it does not require Studio Pro or mxbuild.\n\nThe typed domain export omitted {gaps} association target(s) that do not resolve inside this imported project; their original model data remains preserved. Run `mxrs portability` for the complete per-family typed/partial/preserved inventory.\n{pages_note}"
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

/// `value` as a Rust string literal. Rust's own escaping, not JSON's: the
/// two disagree on control characters (`\f`, `\u000b`), and a model's
/// documentation is free to contain them.
fn rust_string(value: &str) -> String {
    format!("{value:?}")
}

fn write_text(path: &Path, contents: &str) -> Result<()> {
    std::fs::write(path, contents).map_err(|source| io_error(path, source))
}

/// Slices typegen's whole-model marker source into one `markers.rs` per
/// Mendix module folder, and routes each module's typed attribute markers
/// alongside. `src/infrastructure/markers.rs` stays the stable access path
/// (`crate::infrastructure::markers::<Module>::…`) by re-exporting each
/// module's markers under its Mendix name.
/// Decides how every flow is named in generated source, and writes the
/// lists that name the ones Rust does not declare.
///
/// A converted flow is named by the type its own `#[microflow]` declares. A
/// flow the project keeps in the imported model has a file of its own all
/// the same, where the converted ones are, that names it and says why it
/// stayed — and an installed module's flows are named in that package's
/// `markers.rs`, since a package declares nothing.
fn model_names<'a>(
    modules: &[Module],
    packages: &std::collections::BTreeSet<String>,
    entities: &'a HashMap<String, TypedEntityTarget>,
    converted: &[flow_export::ConvertedFlow],
    plans: &[flow_export::FlowFile],
    kept: &flow_export::KeptFlows,
    generated: &mut std::collections::BTreeMap<String, GeneratedModule>,
) -> names::ModelNames<'a> {
    let converted_stems: HashMap<(&str, &str, bool), &str> = converted
        .iter()
        .zip(plans)
        .map(|(flow, plan)| {
            (
                (
                    flow.module.as_str(),
                    flow.declaration.name.as_str(),
                    flow.is_nanoflow(),
                ),
                plan.file_stem.as_str(),
            )
        })
        .collect();
    let mut microflows = HashMap::new();
    let mut nanoflows = HashMap::new();
    for module in modules {
        let Some(module_name) = module.name.as_deref() else {
            continue;
        };
        let stem = module_stem(module_name);
        let root = module_root(&stem, packages);
        // One package file names both kinds, so their identifiers share a
        // namespace there; an authored module keeps them in separate files.
        let mut package_markers = std::collections::HashSet::new();
        let mut package_lines = Vec::new();
        for (nanoflow, flows, targets) in [
            (false, &module.microflows, &mut microflows),
            (true, &module.nanoflows, &mut nanoflows),
        ] {
            let (keyword, authored) = if nanoflow {
                ("nanoflow", "ui::nanoflows")
            } else {
                ("microflow", "services")
            };
            let mut flow_names = flows
                .iter()
                .filter_map(|flow| flow.name.as_deref())
                .filter(|name| !name.is_empty())
                .collect::<Vec<_>>();
            flow_names.sort_unstable();
            flow_names.dedup();
            let mut taken = std::collections::HashSet::new();
            // The files the module's converted flows already have.
            let mut files: std::collections::HashSet<String> = converted_stems
                .iter()
                .filter(|((module, _, kind), _)| *module == module_name && *kind == nanoflow)
                .map(|(_, file)| (*file).to_string())
                .collect();
            let mut imported_files = Vec::new();
            for name in flow_names {
                let qualified = format!("{module_name}.{name}");
                if root == ModuleRoot::Authored
                    && let Some(file) = converted_stems.get(&(module_name, name, nanoflow))
                {
                    targets.insert(
                        qualified,
                        names::FlowTarget {
                            module_path: format!("crate::{authored}::{stem}::{file}"),
                            marker: names::flow_marker(name),
                        },
                    );
                    continue;
                }
                let taken = match root {
                    ModuleRoot::Authored => &mut taken,
                    ModuleRoot::Package => &mut package_markers,
                };
                let mut marker = names::flow_marker(name);
                if !taken.insert(marker.clone()) {
                    marker = (2..)
                        .map(|suffix| format!("{marker}_{suffix}"))
                        .find(|candidate| taken.insert(candidate.clone()))
                        .expect("an unbounded suffix always finds a free name");
                }
                let line = if marker == name {
                    format!("    {keyword} {marker};")
                } else {
                    format!("    {keyword} {marker} = {name:?};")
                };
                let module_path = match root {
                    ModuleRoot::Authored => {
                        let base = flow_export::kept_file_stem(name, nanoflow);
                        let mut file = base.clone();
                        let mut suffix = 2;
                        while !files.insert(file.clone()) {
                            file = format!("{base}_{suffix}");
                            suffix += 1;
                        }
                        let reason = kept
                            .get(&(module_name.to_string(), name.to_string(), nanoflow))
                            .map(String::as_str)
                            .unwrap_or("nothing here reads it yet");
                        imported_files.push((
                            file.clone(),
                            format!(
                                "//! `{module_name}.{name}` stays in the imported model: {reason}.\n//! It is named here so the rest of the project can call and bind it, and\n//! becomes editable Rust when it is declared with `#[{keyword}]` instead.\n\nmxrs::imported! {{\n    module = {module_name:?};\n{line}\n}}\n"
                            ),
                        ));
                        format!("crate::{authored}::{stem}::{file}")
                    }
                    ModuleRoot::Package => {
                        package_lines.push(line);
                        format!("crate::packages::{stem}::markers")
                    }
                };
                targets.insert(
                    qualified,
                    names::FlowTarget {
                        module_path,
                        marker,
                    },
                );
            }
            if !imported_files.is_empty() {
                let module = generated_module(generated, module_name);
                let files = if nanoflow {
                    &mut module.nanoflows
                } else {
                    &mut module.services
                };
                files.extend(imported_files);
            }
        }
        if !package_lines.is_empty() {
            generated_module(generated, module_name).markers = Some(format!(
                "//! The flows the {module_name} module provides, named so this project can\n//! call and bind them.\n\nmxrs::imported! {{\n    module = {module_name:?};\n{}\n}}\n",
                package_lines.join("\n")
            ));
        }
    }
    // A role is named by the variant of the enum its module declares its
    // roles with. An installed module declares nothing, so its roles have no
    // such enum and are named as the model names them.
    let mut roles = HashMap::new();
    for module in modules {
        let Some(module_name) = module.name.as_deref() else {
            continue;
        };
        let stem = module_stem(module_name);
        if module_root(&stem, packages) != ModuleRoot::Authored {
            continue;
        }
        for (name, variant) in role_variants(&module.module_roles) {
            roles.insert(
                format!("{module_name}.{name}"),
                names::RoleTarget {
                    module_path: format!("crate::domain::module_security::{stem}"),
                    variant,
                },
            );
        }
    }
    names::ModelNames {
        entities,
        microflows,
        nanoflows,
        roles,
    }
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
        let _ = writeln!(out, "//! Also wires every page `pages` defines into its");
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

/// One generated service port: the `ports` trait for one Mendix
/// module's runnable microflows, plus everything the runtime adapter impl
/// needs to drive them through `FlowEngine::call`.
struct ServicePort {
    module_name: String,
    root: ModuleRoot,
    module_stem: String,
    trait_name: String,
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
        Ty::Binary | Ty::Enumeration(_) => return None,
        Ty::Object(entity) | Ty::List(entity) => {
            let target = typed_entities.get(entity)?;
            if target.dto {
                return None;
            }
            let path = target.type_path();
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
    packages: &std::collections::BTreeSet<String>,
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
        ports.push(ServicePort {
            module_name: module_name.to_string(),
            root: module_root(&module_stem(module_name), packages),
            module_stem: module_stem(module_name),
            trait_name: format!("{trait_base}Services"),
            methods,
        });
    }
    ports.sort_by(|left, right| left.module_stem.cmp(&right.module_stem));
    ports
}

/// Parameters, besides the receiver, at which Clippy's `too_many_arguments`
/// fires on a port method: its limit is seven arguments in all.
const TOO_MANY_PARAMETERS: usize = 7;

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
        // The port mirrors its microflow's parameters one for one, however
        // many the model declares.
        if method.parameters.len() >= TOO_MANY_PARAMETERS {
            out.push_str("    #[allow(clippy::too_many_arguments)]\n");
        }
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

/// The one place the generated crate owns a booted flow engine and store:
/// the service-port adapter and the HTTP layer both drive the model through
/// it, so there is a single definition of "call a microflow" and a single
/// definition of "turn its result into JSON".
fn render_flow_runtime() -> String {
    "//! The embedded flow runtime every adapter drives the model through.\n\n\
     use mxrs::mapping::ExportMapping;\n\
     use mxrs::ports::{\n\
     \x20   Boot, BootError, FlowEngine, FlowValue, ObjectRef, SecurityContext, ServiceError,\n\
     \x20   Variables, boot, member_to_json,\n\
     };\n\
     use mxrs::Store;\n\
     use serde_json::{Map, Value};\n\n\
     /// Owns the booted engine and store, and runs one microflow at a time.\n\
     pub struct FlowRuntime {\n\
     \x20   engine: FlowEngine,\n\
     \x20   store: Store,\n\
     }\n\n\
     impl FlowRuntime {\n\
     \x20   /// Boots from a built `.mpr` — the output of `cargo run` or\n\
     \x20   /// `cargo mxrs build`.\n\
     \x20   pub fn from_mpr(path: impl AsRef<std::path::Path>) -> Result<Self, BootError> {\n\
     \x20       Ok(Self::from_boot(&boot(path)?))\n\
     \x20   }\n\n\
     \x20   /// Assembles the engine and store a booted model describes. Take the\n\
     \x20   /// [`Boot`] by reference so the same one can also build the\n\
     \x20   /// authentication adapter.\n\
     \x20   pub fn from_boot(booted: &Boot) -> Self {\n\
     \x20       Self::new(\n\
     \x20           FlowEngine::from_modules(&booted.modules)\n\
     \x20               .with_policy(booted.security.clone()),\n\
     \x20           Store::new(booted.schema.clone()),\n\
     \x20       )\n\
     \x20   }\n\n\
     \x20   /// Wraps an engine and store the caller assembled.\n\
     \x20   pub fn new(engine: FlowEngine, store: Store) -> Self {\n\
     \x20       Self { engine, store }\n\
     \x20   }\n\n\
     \x20   /// Runs one microflow as a unit of work, on behalf of `caller`.\n\
     \x20   ///\n\
     \x20   /// `caller` is who the project's entity access rules are judged\n\
     \x20   /// against. `None` is not \"anonymous\" — it is \"nobody to judge\", and\n\
     \x20   /// the rules then do not apply at all. A request boundary always has\n\
     \x20   /// a caller, even an anonymous one.\n\
     \x20   pub fn call(\n\
     \x20       &mut self,\n\
     \x20       flow: &str,\n\
     \x20       arguments: Variables,\n\
     \x20       caller: Option<&SecurityContext>,\n\
     \x20   ) -> Result<FlowValue, ServiceError> {\n\
     \x20       let (result, _) =\n\
     \x20           self.engine\n\
     \x20               .call(&mut self.store, flow, arguments, caller.cloned())?;\n\
     \x20       Ok(result)\n\
     \x20   }\n\n\
     \x20   /// The backing store, for reading committed state.\n\
     \x20   pub fn store(&self) -> &Store {\n\
     \x20       &self.store\n\
     \x20   }\n\n\
     \x20   /// Mutable access to the backing store, for seeding data.\n\
     \x20   pub fn store_mut(&mut self) -> &mut Store {\n\
     \x20       &mut self.store\n\
     \x20   }\n\n\
     \x20   /// Shapes a flow result with the export mapping its operation\n\
     \x20   /// declares. The document carries only objects `caller` may read —\n\
     \x20   /// including the ones the mapping reaches by association, which are\n\
     \x20   /// retrieves it performs itself.\n\
     \x20   pub fn mapped(\n\
     \x20       &self,\n\
     \x20       mapping: &ExportMapping,\n\
     \x20       value: &FlowValue,\n\
     \x20       caller: Option<&SecurityContext>,\n\
     \x20   ) -> Value {\n\
     \x20       self.engine\n\
     \x20           .apply_export_mapping(&self.store, caller, mapping, value)\n\
     \x20   }\n\n\
     \x20   /// Serializes a flow result as JSON, reading object members from the\n\
     \x20   /// store. This is the entity's own shape, which is what an operation\n\
     \x20   /// without an export mapping answers; one that declares a mapping\n\
     \x20   /// goes through [`Self::mapped`] instead.\n\
     \x20   ///\n\
     \x20   /// An object `caller` may not read is not serialized: it answers\n\
     \x20   /// `null` on its own and is left out of a list, so the unmapped shape\n\
     \x20   /// carries no more than the mapped one would.\n\
     \x20   pub fn json(&self, value: &FlowValue, caller: Option<&SecurityContext>) -> Value {\n\
     \x20       match value {\n\
     \x20           FlowValue::Object(reference) => self.object_json(reference, caller),\n\
     \x20           FlowValue::List(values) => Value::Array(\n\
     \x20               values\n\
     \x20                   .iter()\n\
     \x20                   .map(|value| self.json(value, caller))\n\
     \x20                   .filter(|value| !value.is_null())\n\
     \x20                   .collect(),\n\
     \x20           ),\n\
     \x20           other => other.to_json_shallow(),\n\
     \x20       }\n\
     \x20   }\n\n\
     \x20   fn object_json(&self, reference: &ObjectRef, caller: Option<&SecurityContext>) -> Value {\n\
     \x20       let Ok(objects) = self.store.retrieve(&reference.entity) else {\n\
     \x20           return Value::Null;\n\
     \x20       };\n\
     \x20       let Some(object) = objects.iter().find(|object| object.id == reference.id) else {\n\
     \x20           return Value::Null;\n\
     \x20       };\n\
     \x20       if !self.engine.readable(caller, object) {\n\
     \x20           return Value::Null;\n\
     \x20       }\n\
     \x20       let mut fields = Map::new();\n\
     \x20       fields.insert(\"id\".to_string(), Value::String(object.id.clone()));\n\
     \x20       for (name, member) in &object.members {\n\
     \x20           fields.insert(name.clone(), member_to_json(member));\n\
     \x20       }\n\
     \x20       Value::Object(fields)\n\
     \x20   }\n\
     }\n"
        .to_string()
}

/// The runtime adapter: one struct implementing every generated service
/// port by marshalling through `mxrs::ports` and driving the shared
/// [`FlowRuntime`].
fn render_runtime_services(ports: &[ServicePort]) -> String {
    let handles = ports
        .iter()
        .flat_map(|port| &port.methods)
        .any(ServiceMethod::touches_object_handles);
    let marshals = ports
        .iter()
        .flat_map(|port| &port.methods)
        .any(|method| !method.parameters.is_empty() || method.result.is_some());
    let mut imports = vec!["BootError"];
    if handles {
        imports.push("ObjectHandle");
    }
    if marshals {
        imports.push("PortValue");
    }
    imports.extend(["SecurityContext", "ServiceError", "Variables"]);
    let mut out = format!(
        "//! Runtime-backed implementations of the domain service ports.\n\nuse mxrs::ports::{{{}}};\n\nuse super::flow_runtime::FlowRuntime;\n\n/// Drives the model's microflows on the embedded flow runtime.\npub struct RuntimeServices {{\n    runtime: FlowRuntime,\n    caller: Option<SecurityContext>,\n}}\n\nimpl RuntimeServices {{\n    /// Boots the flow runtime from a built `.mpr` — for example the\n    /// output of `cargo run` or `cargo mxrs build`.\n    pub fn from_mpr(path: impl AsRef<std::path::Path>) -> Result<Self, BootError> {{\n        Ok(Self::new(FlowRuntime::from_mpr(path)?))\n    }}\n\n    /// Wraps a runtime the caller assembled. Every port call then runs with\n    /// no caller: these are in-process domain calls from code that is already\n    /// inside the trust boundary, so the project's entity access rules do not\n    /// apply until [`Self::as_caller`] names someone for them to apply to.\n    pub fn new(runtime: FlowRuntime) -> Self {{\n        Self {{\n            runtime,\n            caller: None,\n        }}\n    }}\n\n    /// Runs every port call on behalf of `caller`, so the project's entity\n    /// access rules decide what its flows may touch.\n    pub fn as_caller(mut self, caller: SecurityContext) -> Self {{\n        self.caller = Some(caller);\n        self\n    }}\n\n    /// Who port calls run as, if anyone.\n    pub fn caller(&self) -> Option<&SecurityContext> {{\n        self.caller.as_ref()\n    }}\n\n    /// The backing runtime, for store reads and untyped calls.\n    pub fn runtime(&self) -> &FlowRuntime {{\n        &self.runtime\n    }}\n\n    /// Mutable access to the backing runtime.\n    pub fn runtime_mut(&mut self) -> &mut FlowRuntime {{\n        &mut self.runtime\n    }}\n}}\n",
        imports.join(", "),
    );
    for port in ports {
        let _ = writeln!(
            out,
            "\nimpl {}::services::{} for RuntimeServices {{",
            port.root.path(&port.module_stem, "ports", "ports"),
            port.trait_name
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
            let call = format!(
                "        {}self.runtime.call({:?}, arguments, self.caller.as_ref())?;",
                if method.result.is_some() {
                    "let result = "
                } else {
                    ""
                },
                format!("{}.{}", port.module_name, method.flow_name),
            );
            let _ = writeln!(out, "{call}");
            match &method.result {
                Some((marker, _)) => {
                    let _ = writeln!(
                        out,
                        "        Ok(<{marker} as PortValue>::from_flow_optional(result)?)"
                    );
                }
                None => out.push_str("        Ok(())\n"),
            }
            out.push_str("    }\n");
        }
        out.push_str("}\n");
    }
    out
}

/// `controllers/state.rs` — the axum state every handler extracts.
fn render_http_state(project_name: &str) -> String {
    format!(
        "//! Shared axum state: one booted flow runtime behind a mutex, so\n//! every handler drives the same store, plus the model's own answer to who\n//! is calling.\n\nuse std::sync::{{Arc, Mutex}};\n\nuse axum::Json;\nuse axum::http::header::AUTHORIZATION;\nuse axum::http::{{HeaderMap, StatusCode}};\nuse axum::response::{{IntoResponse, Response}};\nuse mxrs::mapping::ExportMapping;\nuse mxrs::ports::{{\n    BootError, FlowError, FlowValue, HttpObjects, SecurityContext, ServiceError, Variables,\n    basic_credentials, boot,\n}};\nuse serde_json::Value;\n\nuse crate::infrastructure::adapters::authentication::Authentication;\nuse crate::infrastructure::adapters::flow_runtime::FlowRuntime;\nuse crate::controllers::ApiError;\n\n/// The default model a `serve` run loads when no path is given.\npub const DEFAULT_MODEL: &str = {};\n\n#[derive(Clone)]\npub struct AppState {{\n    runtime: Arc<Mutex<FlowRuntime>>,\n    authentication: Arc<Authentication>,\n}}\n\nimpl AppState {{\n    /// Boots the runtime from a built `.mpr`, reading the model once for both\n    /// the engine and the accounts a request signs in against.\n    pub fn from_mpr(path: impl AsRef<std::path::Path>) -> Result<Self, BootError> {{\n        let booted = boot(path)?;\n        Ok(Self::new(\n            FlowRuntime::from_boot(&booted),\n            Authentication::from_boot(&booted),\n        ))\n    }}\n\n    /// Wraps a runtime and an authentication adapter the caller assembled.\n    pub fn new(runtime: FlowRuntime, authentication: Authentication) -> Self {{\n        Self {{\n            runtime: Arc::new(Mutex::new(runtime)),\n            authentication: Arc::new(authentication),\n        }}\n    }}\n\n    /// Who a request with no credentials is, for a service the model\n    /// published to anyone.\n    pub fn anonymous(&self) -> SecurityContext {{\n        self.authentication.anonymous()\n    }}\n\n    /// The caller behind an `Authorization: Basic` header, refused with `401`\n    /// when it is missing or signs nobody in, and with `403` when it signs in\n    /// a caller holding none of `allowed_roles`.\n    ///\n    /// Missing and wrong credentials are one answer on purpose: telling them\n    /// apart tells a caller which user names exist.\n    pub fn basic_caller(\n        &self,\n        headers: &HeaderMap,\n        realm: &'static str,\n        allowed_roles: &[&str],\n    ) -> Result<SecurityContext, ApiError> {{\n        let caller = headers\n            .get(AUTHORIZATION)\n            .and_then(|header| header.to_str().ok())\n            .and_then(basic_credentials)\n            .and_then(|(user, password)| self.authentication.sign_in(&user, &password))\n            .ok_or(ApiError::unauthenticated(realm))?;\n        if !self.authentication.allows(&caller, allowed_roles) {{\n            return Err(ApiError::Forbidden);\n        }}\n        Ok(caller)\n    }}\n\n    /// Runs one microflow as `caller` and serializes its result. A poisoned\n    /// mutex is recovered rather than propagated: a handler that panicked\n    /// left the store as it found it, because every call is its own unit of\n    /// work.\n    pub fn call(\n        &self,\n        flow: &str,\n        arguments: Variables,\n        caller: &SecurityContext,\n    ) -> Result<Value, ServiceError> {{\n        let mut runtime = self\n            .runtime\n            .lock()\n            .unwrap_or_else(|poisoned| poisoned.into_inner());\n        let result: FlowValue = runtime.call(flow, arguments, Some(caller))?;\n        Ok(runtime.json(&result, Some(caller)))\n    }}\n\n    /// Runs one microflow as `caller` and shapes its result with the export\n    /// mapping its operation declares, so the response is the JSON document\n    /// the model publishes instead of the entity's stored attributes. Either\n    /// way the response carries only what `caller` may read.\n    pub fn call_mapped(\n        &self,\n        flow: &str,\n        arguments: Variables,\n        mapping: &ExportMapping,\n        caller: &SecurityContext,\n    ) -> Result<Value, ServiceError> {{\n        let mut runtime = self\n            .runtime\n            .lock()\n            .unwrap_or_else(|poisoned| poisoned.into_inner());\n        let result: FlowValue = runtime.call(flow, arguments, Some(caller))?;\n        Ok(runtime.mapped(mapping, &result, Some(caller)))\n    }}\n\n    /// Runs an operation whose microflow declares the implicit\n    /// `System.HttpRequest` / `System.HttpResponse` parameters Mendix supplies\n    /// itself, and answers what the flow built.\n    ///\n    /// The objects are created and bound before the call, because a microflow\n    /// missing one of its arguments never starts. Afterwards the response\n    /// object is read back: a flow that wrote content answers that content with\n    /// the status it chose — which is how an operation documented as answering\n    /// `404` answers `404` — and a flow that wrote none answers the operation's\n    /// own document under that status. A status the flow left unusable is a\n    /// fault in the application, not in the request.\n    pub fn call_operation(\n        &self,\n        flow: &str,\n        mut arguments: Variables,\n        mapping: Option<&ExportMapping>,\n        caller: &SecurityContext,\n        objects: HttpObjects,\n    ) -> Result<Response, ServiceError> {{\n        let mut runtime = self\n            .runtime\n            .lock()\n            .unwrap_or_else(|poisoned| poisoned.into_inner());\n        let binding = objects\n            .bind(runtime.store_mut(), &mut arguments)\n            .map_err(|error| ServiceError::Flow(FlowError::Runtime(error)))?;\n        let result: FlowValue = runtime.call(flow, arguments, Some(caller))?;\n        let document = match mapping {{\n            Some(mapping) => runtime.mapped(mapping, &result, Some(caller)),\n            None => runtime.json(&result, Some(caller)),\n        }};\n        let Some(answer) = binding.answer(runtime.store()) else {{\n            return Ok(Json(document).into_response());\n        }};\n        let status = StatusCode::from_u16(answer.status)\n            .unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);\n        if answer.content.is_empty() {{\n            return Ok((status, Json(document)).into_response());\n        }}\n        // The flow wrote the body itself. Its content type would come from a\n        // header the flow set, and headers are not carried yet, so the content\n        // goes out as text.\n        Ok((status, answer.content).into_response())\n    }}\n}}\n",
        rust_string(&format!("build/{project_name}.mpr")),
    )
}

/// `infrastructure/adapters/authentication.rs` — turning a request's
/// credentials into the caller the model's rules are judged against.
fn render_authentication() -> String {
    "//! Signing a request in against the accounts the model declares.\n\
     //!\n\
     //! Mendix keeps the administrator and demo-user passwords in the model in\n\
     //! the clear. `boot` reads them from the built `.mpr` at run time, so they\n\
     //! never appear in this source tree, in a log, or in a response: this\n\
     //! adapter can answer who a credential pair is and which roles they hold,\n\
     //! and nothing else about them.\n\n\
     use mxrs::ports::{Boot, LocalAccounts, SecurityContext, SecurityPolicy};\n\n\
     /// The project's own sign-in material and role map.\n\
     pub struct Authentication {\n\
     \x20   accounts: LocalAccounts,\n\
     \x20   policy: SecurityPolicy,\n\
     }\n\n\
     impl Authentication {\n\
     \x20   /// Everything a booted model says about signing in.\n\
     \x20   pub fn from_boot(booted: &Boot) -> Self {\n\
     \x20       Self {\n\
     \x20           accounts: booted.accounts.clone(),\n\
     \x20           policy: booted.security.clone(),\n\
     \x20       }\n\
     \x20   }\n\n\
     \x20   /// The caller a user name and password sign in as, or `None` when\n\
     \x20   /// the project declares no account that matches.\n\
     \x20   pub fn sign_in(&self, user: &str, password: &str) -> Option<SecurityContext> {\n\
     \x20       self.accounts.sign_in(user, password)\n\
     \x20   }\n\n\
     \x20   /// Who a request with no credentials is: the project's guest user\n\
     \x20   /// role, or a caller holding no role at all when it declares none.\n\
     \x20   /// The role-less caller is still a caller — entity access judges it\n\
     \x20   /// and, with security on, denies it.\n\
     \x20   pub fn anonymous(&self) -> SecurityContext {\n\
     \x20       self.accounts.anonymous()\n\
     \x20   }\n\n\
     \x20   /// Whether `caller` holds any of the module roles a published\n\
     \x20   /// service allows.\n\
     \x20   pub fn allows(&self, caller: &SecurityContext, roles: &[&str]) -> bool {\n\
     \x20       self.policy.holds_any_module_role(caller, roles)\n\
     \x20   }\n\
     }\n"
    .to_string()
}

/// `controllers/error.rs` — how a flow failure becomes a response.
fn render_http_error() -> String {
    "//! Why a request did not get its answer: the model call failed, or the\n\
     //! boundary refused the request before making it.\n\n\
     use axum::Json;\n\
     use axum::http::header::WWW_AUTHENTICATE;\n\
     use axum::http::{HeaderValue, StatusCode};\n\
     use axum::response::{IntoResponse, Response};\n\
     use mxrs::ports::ServiceError;\n\
     use serde_json::json;\n\n\
     #[derive(Debug)]\n\
     pub enum ApiError {\n\
     \x20   /// The model call itself failed.\n\
     \x20   Service(ServiceError),\n\
     \x20   /// The request carried no credentials, or none that sign anyone in.\n\
     \x20   /// The two are one answer on purpose: telling them apart tells an\n\
     \x20   /// attacker which user names exist.\n\
     \x20   Unauthenticated { realm: &'static str },\n\
     \x20   /// The caller signed in but holds none of the roles the service\n\
     \x20   /// allows.\n\
     \x20   Forbidden,\n\
     \x20   /// The service declares authentication this layer does not offer, so\n\
     \x20   /// it cannot be served at all — not even unauthenticated.\n\
     \x20   UnsupportedAuthentication { declared: &'static str },\n\
     }\n\n\
     impl ApiError {\n\
     \x20   pub fn unauthenticated(realm: &'static str) -> Self {\n\
     \x20       Self::Unauthenticated { realm }\n\
     \x20   }\n\n\
     \x20   pub fn unsupported_authentication(declared: &'static str) -> Self {\n\
     \x20       Self::UnsupportedAuthentication { declared }\n\
     \x20   }\n\
     }\n\n\
     impl From<ServiceError> for ApiError {\n\
     \x20   fn from(error: ServiceError) -> Self {\n\
     \x20       Self::Service(error)\n\
     \x20   }\n\
     }\n\n\
     impl IntoResponse for ApiError {\n\
     \x20   fn into_response(self) -> Response {\n\
     \x20       match self {\n\
     \x20           // A boundary value the model rejected is the caller's\n\
     \x20           // input; anything the flow itself raised is the\n\
     \x20           // application's.\n\
     \x20           ApiError::Service(error) => {\n\
     \x20               let status = match &error {\n\
     \x20                   ServiceError::Value(_) => StatusCode::BAD_REQUEST,\n\
     \x20                   ServiceError::Flow(_) => StatusCode::INTERNAL_SERVER_ERROR,\n\
     \x20               };\n\
     \x20               (status, message(error.to_string())).into_response()\n\
     \x20           }\n\
     \x20           ApiError::Unauthenticated { realm } => {\n\
     \x20               // The challenge is what makes a 401 actionable; a realm\n\
     \x20               // that cannot be a header value is left off rather than\n\
     \x20               // guessed at.\n\
     \x20               let challenge = HeaderValue::from_str(&format!(\n\
     \x20                   \"Basic realm=\\\"{realm}\\\", charset=\\\"UTF-8\\\"\"\n\
     \x20               ));\n\
     \x20               let body = message(\"authentication required\");\n\
     \x20               match challenge {\n\
     \x20                   Ok(challenge) => (\n\
     \x20                       StatusCode::UNAUTHORIZED,\n\
     \x20                       [(WWW_AUTHENTICATE, challenge)],\n\
     \x20                       body,\n\
     \x20                   )\n\
     \x20                       .into_response(),\n\
     \x20                   Err(_) => (StatusCode::UNAUTHORIZED, body).into_response(),\n\
     \x20               }\n\
     \x20           }\n\
     \x20           ApiError::Forbidden => (\n\
     \x20               StatusCode::FORBIDDEN,\n\
     \x20               message(\"the caller holds none of the roles this service allows\"),\n\
     \x20           )\n\
     \x20               .into_response(),\n\
     \x20           ApiError::UnsupportedAuthentication { declared } => (\n\
     \x20               StatusCode::NOT_IMPLEMENTED,\n\
     \x20               message(format!(\n\
     \x20                   \"this service requires {declared}, which the generated HTTP layer does not offer yet\"\n\
     \x20               )),\n\
     \x20           )\n\
     \x20               .into_response(),\n\
     \x20       }\n\
     \x20   }\n\
     }\n\n\
     fn message(error: impl Into<String>) -> Json<serde_json::Value> {\n\
     \x20   Json(json!({ \"error\": error.into() }))\n\
     }\n"
    .to_string()
}

/// `controllers/mod.rs` — the router every published service's route table
/// merges into, plus the server that binds it.
fn render_http_module(services: &[PublishedService]) -> String {
    let mut out = String::from(
        "//! The application's HTTP surface: every published REST service's route\n\
         //! table, merged here behind the shared [`AppState`]. Controllers live\n\
         //! one folder per Mendix module.\n\n\
         pub mod error;\n\
         pub mod state;\n",
    );
    // An authored module's controllers live in a folder of this one; an
    // installed module's live in its own package, which the package tree
    // declares.
    let mut owned: Vec<String> = services
        .iter()
        .filter(|service| service.root == ModuleRoot::Authored)
        .map(|service| module_stem(&service.module_name))
        .collect();
    owned.sort();
    owned.dedup();
    for stem in &owned {
        let _ = writeln!(out, "pub mod {stem};");
    }
    out.push_str(
        "\nuse axum::routing::get;\n\
         use axum::{Json, Router};\n\
         use serde_json::{Value, json};\n\n\
         pub use error::ApiError;\n\
         pub use state::{AppState, DEFAULT_MODEL};\n\n",
    );
    let route_tables: Vec<String> = services
        .iter()
        .map(|service| {
            format!(
                "{}::{}",
                service.root.path(
                    &module_stem(&service.module_name),
                    "controllers",
                    "controllers",
                ),
                service.file_stem,
            )
        })
        .collect();
    out.push_str("/// Every route the model publishes, plus a liveness probe.\npub fn router(state: AppState) -> Router {\n    Router::new()\n        .route(\"/health\", get(health))\n");
    for path in &route_tables {
        let _ = writeln!(out, "        .merge({path}::router())");
    }
    out.push_str("        .with_state(state)\n}\n\n");
    out.push_str(
        "async fn health() -> Json<Value> {\n    Json(json!({ \"status\": \"ok\" }))\n}\n\n\
         /// Boots the model at `path` and serves it on `address`.\n\
         pub async fn serve(\n\
         \x20   path: impl AsRef<std::path::Path>,\n\
         \x20   address: &str,\n\
         ) -> Result<(), Box<dyn std::error::Error>> {\n\
         \x20   let state = AppState::from_mpr(path)?;\n\
         \x20   let listener = tokio::net::TcpListener::bind(address).await?;\n\
         \x20   axum::serve(listener, router(state)).await?;\n\
         \x20   Ok(())\n\
         }\n",
    );
    out
}

/// One module's `controllers/<module>/mod.rs`: its services' route tables and
/// their controllers. The project's own router merges each route table, so
/// this index lists files and composes nothing.
fn render_module_http_index(module_name: &str, files: &[(String, String)]) -> String {
    let mut out = format!(
        "//! The {module_name} module's published REST services: one route table per\n//! service, one controller per resource.\n\n"
    );
    for (file, _) in files {
        let _ = writeln!(out, "pub mod {file};");
    }
    out
}

/// One export mapping as an editable declaration: the element tree the model
/// declares, ready for [`mxrs::mapping::ExportMapping::apply`] at the HTTP
/// boundary.
fn render_export_mapping(mapping: &ExportMappingDocument) -> String {
    let shape = if mapping.root.multiple {
        "an array of"
    } else {
        "one"
    };
    let mut out = format!(
        "//! `{}` — the JSON document the operations\n//! declaring it answer with: {shape} `{}`.\n",
        mapping.qualified_name, mapping.root.entity,
    );
    for line in mapping
        .documentation
        .lines()
        .filter(|line| !line.is_empty())
    {
        let _ = writeln!(out, "//!\n//! {line}");
    }
    out.push_str(
        "\nuse mxrs::mapping::{ExportMapping, ObjectMapping};\n\n\
         /// The mapping, exactly as the model's element tree declares it.\n\
         pub fn mapping() -> ExportMapping {\n\
         \x20   ExportMapping::new(\n",
    );
    let mut root = String::new();
    render_mapped_object(&mut root, &mapping.root, 2);
    let _ = writeln!(out, "{},", root.trim_end());
    out.push_str("    )\n");
    if mapping.send_nils {
        // `NullValueOption: SendAsNil` — the document keeps the key.
        out.push_str("    .sending_nils()\n");
    }
    out.push_str("}\n");
    out
}

/// One level of the element tree as a builder chain. `depth` counts four-space
/// indents; `cargo fmt` normalizes the wrapping afterwards.
fn render_mapped_object(out: &mut String, object: &MappedObject, depth: usize) {
    let indent = "    ".repeat(depth);
    let constructor = if object.multiple { "array" } else { "object" };
    let _ = writeln!(
        out,
        "{indent}ObjectMapping::{constructor}({})",
        rust_string(&object.entity)
    );
    for value in &object.values {
        if value.key == value.attribute {
            let _ = writeln!(out, "{indent}    .attribute({})", rust_string(&value.key));
        } else {
            let _ = writeln!(
                out,
                "{indent}    .value({}, {})",
                rust_string(&value.key),
                rust_string(&value.attribute)
            );
        }
    }
    for child in &object.children {
        let mut nested = String::new();
        render_mapped_object(&mut nested, child, depth + 2);
        let _ = writeln!(
            out,
            "{indent}    .child(\n{indent}        {},\n{indent}        {},\n{},\n{indent}    )",
            rust_string(&child.key),
            rust_string(&child.association),
            nested.trim_end(),
        );
    }
}

/// One published REST service as the files its module's controllers folder
/// holds: the service's route table first, then one controller per resource.
fn render_published_service(
    service: &PublishedService,
    mappings: &HashMap<String, MappingTarget>,
) -> Vec<(String, String)> {
    let mut controllers: Vec<&str> = service
        .routes
        .iter()
        .flat_map(|route| &route.operations)
        .map(|operation| operation.controller.as_str())
        .collect();
    controllers.sort_unstable();
    controllers.dedup();
    let mut files = vec![(
        service.file_stem.clone(),
        render_service_routes(service, &controllers),
    )];
    for controller in controllers {
        files.push((
            controller.to_string(),
            render_controller(service, controller, mappings),
        ));
    }
    files
}

/// A service's route table: every path it publishes, each method bound to
/// the controller function that handles it. The service also states who may
/// call it, which its controllers enforce on every operation.
fn render_service_routes(service: &PublishedService, controllers: &[&str]) -> String {
    // Only the method opening each route chain is called as a free
    // function; the rest are chained onto the `MethodRouter` it returns.
    let mut methods: Vec<&str> = service
        .routes
        .iter()
        .filter_map(|route| route.operations.first().map(|operation| operation.method))
        .collect();
    methods.sort_unstable();
    methods.dedup();

    let mut out = format!(
        "//! `{}` — the REST service the {} module publishes at `/{}`.\n",
        service.name, service.module_name, service.base_path,
    );
    for line in service
        .documentation
        .lines()
        .filter(|line| !line.is_empty())
    {
        let _ = writeln!(out, "//!\n//! {line}");
    }
    match &service.authentication {
        ServiceAuthentication::Public => out.push_str(
            "//!\n//! The model publishes this service to anyone: it declares no\n//! authentication type and no allowed role. Its operations therefore run as\n//! the project's guest user role, or as a caller holding no role at all when\n//! the project declares none — which entity access then denies.\n",
        ),
        ServiceAuthentication::Basic { allowed_roles } => {
            let _ = write!(
                out,
                "//!\n//! The model requires HTTP Basic authentication. Every operation signs the\n//! request in before calling the model and runs as whoever signed in, and\n//! only these module roles may reach it:\n//!\n{}",
                allowed_roles
                    .iter()
                    .map(|role| format!("//! - `{role}`\n"))
                    .collect::<String>(),
            );
        }
        ServiceAuthentication::Unsupported { declared } => {
            let _ = write!(
                out,
                "//!\n//! Every operation here refuses with `501`. The model requires {declared},\n//! which the generated HTTP layer does not offer yet, and serving the\n//! surface without the authentication the model asks for would hand it to\n//! anyone. The routes stay in the table so the published surface stays\n//! visible, and answer the gap instead of the model.\n",
            );
        }
    }
    out.push_str(
        "//!\n//! This is the route table. Each resource's operations are handled by its\n//! controller, beside this file.\n",
    );
    let _ = write!(
        out,
        "\nuse axum::Router;\nuse axum::routing::{{{}}};\n\n",
        methods.join(", ")
    );
    match controllers {
        [controller] => {
            let _ = writeln!(out, "use super::{controller};");
        }
        controllers => {
            let _ = writeln!(out, "use super::{{{}}};", controllers.join(", "));
        }
    }
    out.push_str("use crate::controllers::AppState;\n\n");
    match &service.authentication {
        ServiceAuthentication::Basic { allowed_roles } => {
            let _ = write!(
                out,
                "/// The module roles this service allows, exactly as the model lists\n/// them. An empty list would allow nobody, not everybody.\npub(super) const ALLOWED_ROLES: &[&str] = &[{}];\n\n/// The realm a `401` challenges with, so a client knows which credentials\n/// it is being asked for.\npub(super) const REALM: &str = {};\n\n",
                allowed_roles
                    .iter()
                    .map(|role| rust_string(role))
                    .collect::<Vec<_>>()
                    .join(", "),
                rust_string(&service.name),
            );
        }
        ServiceAuthentication::Unsupported { declared } => {
            let _ = write!(
                out,
                "/// What the model requires of a caller, named in every refusal.\npub(super) const AUTHENTICATION: &str = {};\n\n",
                rust_string(declared),
            );
        }
        ServiceAuthentication::Public => {}
    }
    out.push_str("pub fn router() -> Router<AppState> {\n    Router::new()\n");
    for route in &service.routes {
        let handlers = route
            .operations
            .iter()
            .map(|operation| {
                format!(
                    "{}({}::{})",
                    operation.method, operation.controller, operation.handler
                )
            })
            .collect::<Vec<_>>()
            .join(".");
        let _ = writeln!(out, "        .route({:?}, {handlers})", route.path);
    }
    out.push_str("}\n");
    out
}

/// One controller: the operations of one resource, a function each, calling
/// the microflow the model bound to it.
fn render_controller(
    service: &PublishedService,
    controller: &str,
    mappings: &HashMap<String, MappingTarget>,
) -> String {
    // An unsupported authentication scheme routes only refusals, so the
    // controller binds no parameter, applies no mapping, and must not import
    // for either.
    let serves = service.authentication.serves_operations();
    let operations: Vec<(&str, &ServiceOperation)> = service
        .routes
        .iter()
        .flat_map(|route| {
            route
                .operations
                .iter()
                .map(move |operation| (route.path.as_str(), operation))
        })
        .filter(|(_, operation)| operation.controller == controller)
        .collect();
    let binds = |source: ParameterSource| {
        serves
            && operations.iter().any(|(_, operation)| {
                operation
                    .parameters
                    .iter()
                    .any(|parameter| parameter.source == source)
            })
    };
    let uses_path = binds(ParameterSource::Path);
    let uses_query = binds(ParameterSource::Query);
    let mut extractors = Vec::new();
    if serves {
        extractors.push("State");
    }
    if uses_path {
        extractors.push("Path");
    }
    if uses_query {
        extractors.push("Query");
    }
    extractors.sort_unstable();

    let mut resources: Vec<&str> = operations
        .iter()
        .map(|(_, operation)| operation.resource.as_str())
        .collect();
    resources.sort_unstable();
    resources.dedup();
    let served_at = |resource: &str| {
        [service.base_path.as_str(), resource.trim_matches('/')]
            .into_iter()
            .filter(|segment| !segment.is_empty())
            .fold(String::new(), |mut path, segment| {
                path.push('/');
                path.push_str(segment);
                path
            })
    };
    let mut out = match resources.as_slice() {
        [""] => format!(
            "//! The operations `{}` serves at its own path, `/{}`.\n",
            service.name, service.base_path,
        ),
        [resource] => format!(
            "//! The `{resource}` resource of `{}`, served under\n//! `{}`.\n",
            service.name,
            served_at(resource),
        ),
        resources => format!(
            "//! The {} resources of `{}`.\n",
            resources
                .iter()
                .map(|resource| format!("`{resource}`"))
                .collect::<Vec<_>>()
                .join(", "),
            service.name,
        ),
    };
    let _ = writeln!(
        out,
        "//!\n//! One function per operation, each calling the microflow the model bound\n//! to it. The paths they answer are in `{}`, the service's route table.",
        service.file_stem,
    );
    if uses_path || uses_query {
        out.push_str("\nuse std::collections::HashMap;\n");
    }
    // An operation that applies an export mapping names the declaration by
    // its file, in the domain of the module that owns it. Two modules can
    // each own a mapping of one name; those are told apart by their module.
    let mut applied_mappings = std::collections::BTreeSet::new();
    if serves {
        for (_, operation) in &operations {
            if let Some(target) = mappings.get(&operation.export_mapping) {
                applied_mappings.insert((
                    target.file_stem.clone(),
                    target.module_stem.clone(),
                    target.root,
                ));
            }
        }
    }
    let mapping_alias = |target: &MappingTarget| {
        let shared = applied_mappings
            .iter()
            .filter(|(file, _, _)| *file == target.file_stem)
            .count()
            > 1;
        if shared {
            format!("{}_{}", target.module_stem, target.file_stem)
        } else {
            target.file_stem.clone()
        }
    };
    // One `use` per owning module, the way a person would have written it.
    let mut imported: std::collections::BTreeMap<String, Vec<String>> = Default::default();
    for (file, module, root) in &applied_mappings {
        let alias = mapping_alias(&MappingTarget {
            root: *root,
            module_stem: module.clone(),
            file_stem: file.clone(),
        });
        imported
            .entry(root.path(module, "domain::mappings", "domain::mappings"))
            .or_default()
            .push(if alias == *file {
                file.clone()
            } else {
                format!("{file} as {alias}")
            });
    }
    let mut mapping_imports = String::new();
    for (path, names) in &imported {
        match names.as_slice() {
            [name] => {
                let _ = writeln!(mapping_imports, "use {path}::{name};");
            }
            names => {
                let _ = writeln!(mapping_imports, "use {path}::{{{}}};", names.join(", "));
            }
        }
    }
    // Which of the implicit HTTP parameters any operation here declares
    // decides both what the handlers extract and what they return.
    let http = |pick: fn(&HttpParameters) -> bool| {
        serves
            && operations
                .iter()
                .any(|(_, operation)| pick(&operation.http_parameters))
    };
    let binds_http_objects = http(|parameters| !parameters.is_empty());
    let binds_http_request = http(|parameters| parameters.request.is_some());
    out.push('\n');
    if !extractors.is_empty() {
        let _ = writeln!(out, "use axum::extract::{{{}}};", extractors.join(", "));
    }
    let mut http_types = Vec::new();
    if matches!(service.authentication, ServiceAuthentication::Basic { .. }) {
        http_types.push("HeaderMap");
    }
    if binds_http_request {
        http_types.push("Uri");
    }
    if !http_types.is_empty() {
        let _ = writeln!(out, "use axum::http::{{{}}};", http_types.join(", "));
    }
    if binds_http_objects {
        out.push_str("use axum::response::Response;\n");
    }
    if serves {
        // Only a bound parameter names a `FlowValue`; a controller whose
        // operations take none would carry an unused import.
        let mut ports = vec!["Variables"];
        if operations
            .iter()
            .any(|(_, operation)| !operation.parameters.is_empty())
        {
            ports.push("FlowValue");
        }
        if binds_http_objects {
            ports.push("HttpObjects");
        }
        ports.sort_unstable();
        // `Json<Value>` appears in a handler's answer and in a body extractor.
        // A controller whose every operation answers through its response
        // object and binds no body needs neither.
        let json = operations
            .iter()
            .any(|(_, operation)| operation.http_parameters.is_empty())
            || binds(ParameterSource::Body);
        if json {
            out.push_str("use axum::Json;\n");
        }
        let _ = writeln!(out, "use mxrs::ports::{{{}}};", ports.join(", "));
        if json {
            out.push_str("use serde_json::Value;\n");
        }
    }
    out.push('\n');
    match &service.authentication {
        ServiceAuthentication::Basic { .. } => {
            let _ = writeln!(
                out,
                "use super::{}::{{ALLOWED_ROLES, REALM}};",
                service.file_stem
            );
        }
        ServiceAuthentication::Unsupported { .. } => {
            let _ = writeln!(out, "use super::{}::AUTHENTICATION;", service.file_stem);
        }
        ServiceAuthentication::Public => {}
    }
    out.push_str(if serves {
        "use crate::controllers::{ApiError, AppState};\n"
    } else {
        "use crate::controllers::ApiError;\n"
    });
    out.push_str(&mapping_imports);

    for (route_path, operation) in &operations {
        out.push('\n');
        if !operation.summary.is_empty() {
            let _ = writeln!(out, "/// {}", operation.summary);
        }
        for line in operation.documentation.lines() {
            if line.is_empty() {
                out.push_str("///\n");
            } else {
                let _ = writeln!(out, "/// {line}");
            }
        }
        if !operation.summary.is_empty() || !operation.documentation.is_empty() {
            out.push_str("///\n");
        }
        let _ = writeln!(
            out,
            "#[mxrs::route(method = {:?}, path = {route_path:?}, service = {:?})]",
            operation.method.to_ascii_uppercase(),
            service.name,
        );
        if !serves {
            let _ = writeln!(
                out,
                "/// Would call `{}`; refuses instead, because the model's\n/// authentication is not available here.\npub async fn {}() -> ApiError {{\n    ApiError::unsupported_authentication(AUTHENTICATION)\n}}",
                operation.microflow, operation.handler,
            );
            continue;
        }
        let applied = mappings
            .get(&operation.export_mapping)
            .map(|target| format!("{}::mapping()", mapping_alias(target)));
        match &applied {
            Some(_) => {
                let _ = writeln!(
                    out,
                    "/// Calls `{}`, answering through the\n/// `{}` export mapping.",
                    operation.microflow, operation.export_mapping,
                );
            }
            None if !operation.export_mapping.is_empty() => {
                let _ = writeln!(
                    out,
                    "/// Calls `{}`. The model's\n/// `{}` export mapping stays preserved in `model/imported`: this\n/// operation answers the entity's stored attributes until the mapping\n/// declaration covers its element tree.",
                    operation.microflow, operation.export_mapping,
                );
            }
            // No mapping declared: Mendix answers the stored attributes too.
            None => {
                let _ = writeln!(out, "/// Calls `{}`.", operation.microflow);
            }
        }
        // axum resolves extractors in argument order, so a handler only
        // declares the ones its operation actually binds.
        let extracts = |source: ParameterSource, extractor: &'static str| {
            if operation
                .parameters
                .iter()
                .any(|parameter| parameter.source == source)
            {
                extractor
            } else {
                ""
            }
        };
        let path = extracts(
            ParameterSource::Path,
            "\n    Path(path): Path<HashMap<String, String>>,",
        );
        let query = extracts(
            ParameterSource::Query,
            "\n    Query(query): Query<HashMap<String, String>>,",
        );
        let body = extracts(ParameterSource::Body, "\n    Json(body): Json<Value>,");
        // `Json` consumes the body, so it has to come last; every other
        // extractor reads only the request parts and may precede it.
        let headers = match service.authentication {
            ServiceAuthentication::Basic { .. } => "\n    headers: HeaderMap,",
            _ => "",
        };
        // The request object carries the URI, so only an operation whose
        // microflow asks for one extracts it.
        let uri = match operation.http_parameters.request {
            Some(_) => "\n    uri: Uri,",
            None => "",
        };
        let answer = if operation.http_parameters.is_empty() {
            "Json<Value>"
        } else {
            "Response"
        };
        let _ = writeln!(
            out,
            "pub async fn {}(\n    State(state): State<AppState>,{headers}{uri}{path}{query}{body}\n) -> Result<{answer}, ApiError> {{",
            operation.handler,
        );
        match service.authentication {
            ServiceAuthentication::Basic { .. } => out.push_str(
                "    let caller = state.basic_caller(&headers, REALM, ALLOWED_ROLES)?;\n",
            ),
            _ => out.push_str("    let caller = state.anonymous();\n"),
        }
        let binding = if operation.parameters.is_empty() {
            "let arguments"
        } else {
            "let mut arguments"
        };
        let _ = writeln!(out, "    {binding} = Variables::new();");
        for parameter in &operation.parameters {
            let value = match parameter.source {
                ParameterSource::Path => format!(
                    "FlowValue::String(path.get({:?}).cloned().unwrap_or_default())",
                    parameter.name
                ),
                ParameterSource::Query => format!(
                    "query.get({:?}).map_or(FlowValue::Empty, |value| FlowValue::String(value.clone()))",
                    parameter.name
                ),
                ParameterSource::Body => format!(
                    "body.get({:?}).map_or(FlowValue::Empty, |value| FlowValue::Json(value.clone()))",
                    parameter.name
                ),
            };
            let _ = writeln!(
                out,
                "    arguments.insert({:?}.to_string(), {value});",
                parameter.microflow_parameter,
            );
        }
        // A microflow that declares the implicit HTTP parameters answers what
        // it built on its response object, so its handler returns a whole
        // response rather than a document.
        if !operation.http_parameters.is_empty() {
            let _ = writeln!(
                out,
                "    Ok(state.call_operation(\n        {:?},\n        arguments,\n        {},\n        &caller,\n        {},\n    )?)\n}}",
                operation.microflow,
                match &applied {
                    Some(mapping) => format!("Some(&{mapping})"),
                    None => "None".to_string(),
                },
                operation.http_parameters.declaration(),
            );
            continue;
        }
        match &applied {
            Some(mapping) => {
                let _ = writeln!(
                    out,
                    "    Ok(Json(state.call_mapped(\n        {:?},\n        arguments,\n        &{mapping},\n        &caller,\n    )?))\n}}",
                    operation.microflow,
                );
            }
            None => {
                let _ = writeln!(
                    out,
                    "    Ok(Json(state.call({:?}, arguments, &caller)?))\n}}",
                    operation.microflow,
                );
            }
        }
    }
    out
}

/// Decides, for every operation of every service, which controller file
/// handles it and what its function is called there.
///
/// A controller is a resource: `orders_controller` holds the operations of the
/// `orders` resource. Two services of one module that each publish a resource of
/// that name keep them apart by service (`api_service_orders_controller`), and a
/// service's operations outside any resource live in the service's own
/// controller.
///
/// A function is named by what the operation does to the resource — `index`,
/// `show`, `create`, `update`, `destroy` — when that says which operation it
/// is. Operations that would share one of those names are named after the
/// microflow each calls instead, which is what tells them apart in the model.
fn assign_controllers(services: &mut [PublishedService]) {
    let preferred = |service_stem: &str, resource: &str| {
        let resource = inner_file_stem(resource);
        if resource.is_empty() {
            format!("{service_stem}_controller")
        } else {
            format!("{resource}_controller")
        }
    };
    let mut claimed: HashMap<(String, String), std::collections::BTreeSet<usize>> = HashMap::new();
    let mut service_files = std::collections::HashSet::new();
    for (index, service) in services.iter().enumerate() {
        let module = module_stem(&service.module_name);
        service_files.insert((module.clone(), service.file_stem.clone()));
        for operation in service.routes.iter().flat_map(|route| &route.operations) {
            claimed
                .entry((
                    module.clone(),
                    preferred(&service.file_stem, &operation.resource),
                ))
                .or_default()
                .insert(index);
        }
    }
    let mut owner: HashMap<(String, String), usize> = HashMap::new();
    for (index, service) in services.iter_mut().enumerate() {
        let module = module_stem(&service.module_name);
        let service_stem = service.file_stem.clone();
        for operation in service
            .routes
            .iter_mut()
            .flat_map(|route| &mut route.operations)
        {
            let mut controller = preferred(&service_stem, &operation.resource);
            if claimed[&(module.clone(), controller.clone())].len() > 1 {
                controller = format!("{service_stem}_{controller}");
            }
            let base = controller.clone();
            let mut suffix = 2;
            // A file another service already owns, or a service's own route
            // table, is not this controller's to write.
            while service_files.contains(&(module.clone(), controller.clone()))
                || owner
                    .get(&(module.clone(), controller.clone()))
                    .is_some_and(|other| *other != index)
            {
                controller = format!("{base}_{suffix}");
                suffix += 1;
            }
            owner.insert((module.clone(), controller.clone()), index);
            operation.controller = controller;
        }
    }

    for service in services.iter_mut() {
        let conventional = |operation: &ServiceOperation| match operation.method {
            "get" if operation.addresses_one => "show",
            "get" => "index",
            "post" => "create",
            "put" | "patch" => "update",
            "delete" => "destroy",
            _ => "",
        };
        let mut wanted: HashMap<(String, &'static str), usize> = HashMap::new();
        for operation in service.routes.iter().flat_map(|route| &route.operations) {
            *wanted
                .entry((operation.controller.clone(), conventional(operation)))
                .or_default() += 1;
        }
        let mut used: std::collections::HashSet<(String, String)> =
            std::collections::HashSet::new();
        for operation in service
            .routes
            .iter_mut()
            .flat_map(|route| &mut route.operations)
        {
            let name = conventional(operation);
            if !name.is_empty() && wanted[&(operation.controller.clone(), name)] == 1 {
                operation.handler = name.to_string();
                used.insert((operation.controller.clone(), name.to_string()));
            }
        }
        for operation in service
            .routes
            .iter_mut()
            .flat_map(|route| &mut route.operations)
            .filter(|operation| operation.handler.is_empty())
        {
            let microflow = operation.microflow.rsplit('.').next().unwrap_or_default();
            let mut base = inner_file_stem(flow_export::split_prefix(microflow).1);
            if base.is_empty() {
                base = format!("{}_operation", operation.method);
            }
            let mut handler = base.clone();
            let mut suffix = 2;
            while !used.insert((operation.controller.clone(), handler.clone())) {
                handler = format!("{base}_{suffix}");
                suffix += 1;
            }
            operation.handler = handler;
        }
    }
}

/// One module's action ports: the Rust contracts its model-declared Java
/// actions expect hand-written code to fulfill, plus the registration glue
/// that adapts an implementation onto `FlowEngine::with_java_action`.
struct ActionPortModule {
    module_name: String,
    root: ModuleRoot,
    module_stem: String,
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
            let path = target.type_path();
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
            let path = target.type_path();
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
/// One `Rest$PublishedRestService` the model declares, lowered into the
/// axum router its module will expose.
struct PublishedService {
    module_name: String,
    root: ModuleRoot,
    /// Mendix service name, e.g. `API_Service`.
    name: String,
    /// The file the module's HTTP folder writes this service to.
    file_stem: String,
    /// Service base path, e.g. `api/v1`.
    base_path: String,
    documentation: String,
    /// Operations grouped by route path, in declaration order — axum wants
    /// one `route` call per path with the methods chained onto it.
    routes: Vec<ServiceRoute>,
    authentication: ServiceAuthentication,
}

/// How a published service authenticates its callers, as the model declares
/// it in `AuthenticationTypes`, `AuthenticationMicroflow` and `AllowedRoles`.
enum ServiceAuthentication {
    /// No authentication type and no allowed role: the model published this
    /// surface to anyone, so its operations run as the project's guest.
    Public,
    /// `AuthenticationTypes: ['basic']` — HTTP Basic, and the caller must hold
    /// one of these module roles.
    Basic { allowed_roles: Vec<String> },
    /// The model asks for authentication this layer does not offer. Routing is
    /// still generated, so the published surface stays visible and stays in
    /// the route table, but every operation refuses: serving it without the
    /// authentication the model requires would be a silent downgrade.
    Unsupported {
        /// What the model declared, for the doc comment and the refusal body.
        declared: String,
    },
}

impl ServiceAuthentication {
    /// Whether operations are actually served. An unsupported scheme routes
    /// only refusals, so the service binds no parameters and applies no
    /// mapping — and must not import for either.
    fn serves_operations(&self) -> bool {
        !matches!(self, ServiceAuthentication::Unsupported { .. })
    }
}

fn service_authentication(document: &mxrs_bson::Document) -> ServiceAuthentication {
    let microflow = document
        .get_str("AuthenticationMicroflow")
        .unwrap_or_default();
    if !microflow.is_empty() {
        return ServiceAuthentication::Unsupported {
            declared: format!("the custom authentication microflow {microflow}"),
        };
    }
    let allowed_roles = strings_in(document, "AllowedRoles");
    let types = strings_in(document, "AuthenticationTypes");
    match types.as_slice() {
        [] if allowed_roles.is_empty() => ServiceAuthentication::Public,
        // Roles to honour, and no way to learn who the caller is. Serving the
        // service publicly would ignore the restriction the model states.
        [] => ServiceAuthentication::Unsupported {
            declared: format!(
                "the roles {} without an authentication type to identify a caller by",
                allowed_roles.join(", ")
            ),
        },
        [only] if only == "basic" => ServiceAuthentication::Basic { allowed_roles },
        declared => ServiceAuthentication::Unsupported {
            declared: format!("the authentication type(s) {}", declared.join(", ")),
        },
    }
}

struct ServiceRoute {
    /// Full axum path, e.g. `/api/v1/orders/{id}`.
    path: String,
    operations: Vec<ServiceOperation>,
}

struct ServiceOperation {
    /// axum routing method: `get`, `post`, …
    method: &'static str,
    /// The Mendix resource the operation belongs to; empty for one the
    /// service publishes at its own path.
    resource: String,
    /// Whether the operation's own path ends in a parameter: it addresses
    /// one object of the resource, where another addresses the collection.
    addresses_one: bool,
    /// The controller file that handles it, e.g. `orders_controller`. Decided
    /// by [`assign_controllers`], across every service of the module.
    controller: String,
    /// The handler function's name within that controller.
    handler: String,
    /// Qualified microflow the operation calls.
    microflow: String,
    summary: String,
    documentation: String,
    parameters: Vec<OperationParameter>,
    /// Qualified export mapping the operation answers through, empty when the
    /// model declares none.
    export_mapping: String,
    /// The implicit `System.HttpRequest` / `System.HttpResponse` parameters the
    /// microflow declares, which the operation never binds because Mendix
    /// supplies them itself.
    http_parameters: HttpParameters,
}

/// The variable names a microflow declares its implicit HTTP parameters under.
#[derive(Default, Clone)]
struct HttpParameters {
    request: Option<String>,
    response: Option<String>,
}

impl HttpParameters {
    fn is_empty(&self) -> bool {
        self.request.is_none() && self.response.is_none()
    }

    /// The `mxrs::ports::HttpObjects` builder chain that declares them. A
    /// request object carries the request's own URI, which the handler
    /// extracts; a response-only operation never needs one.
    fn declaration(&self) -> String {
        let mut out = String::from("HttpObjects::none()");
        if let Some(parameter) = &self.request {
            let _ = write!(
                out,
                ".with_request({}, uri.to_string())",
                rust_string(parameter)
            );
        }
        if let Some(parameter) = &self.response {
            let _ = write!(out, ".with_response({})", rust_string(parameter));
        }
        out
    }
}

/// Reads every microflow's implicit HTTP parameters, by qualified name.
///
/// A published operation binds only the parameters it declares; Mendix supplies
/// `System.HttpRequest` and `System.HttpResponse` itself, so without this the
/// microflow is called an argument short and fails before it starts.
fn http_parameters(modules: &[Module]) -> HashMap<String, HttpParameters> {
    let mut found = HashMap::new();
    for module in modules {
        let Some(module_name) = module.name.as_deref() else {
            continue;
        };
        for flow in &module.microflows {
            let Some(flow_name) = flow.name.as_deref() else {
                continue;
            };
            let mut parameters = HttpParameters::default();
            for parameter in &flow.parameters {
                let Ok(name) = parameter.get_str("Name") else {
                    continue;
                };
                let entity = parameter
                    .get_document("VariableType")
                    .ok()
                    .filter(|kind| kind.get_str("$Type").ok() == Some("DataTypes$ObjectType"))
                    .and_then(|kind| kind.get_str("Entity").ok());
                match entity {
                    Some("System.HttpRequest") => parameters.request = Some(name.to_string()),
                    Some("System.HttpResponse") => parameters.response = Some(name.to_string()),
                    _ => {}
                }
            }
            if !parameters.is_empty() {
                found.insert(format!("{module_name}.{flow_name}"), parameters);
            }
        }
    }
    found
}

struct OperationParameter {
    /// REST parameter name, as it appears in the path or query string.
    name: String,
    /// Microflow parameter the value binds to.
    microflow_parameter: String,
    source: ParameterSource,
}

#[derive(PartialEq, Eq, Clone, Copy)]
enum ParameterSource {
    Path,
    Query,
    Body,
}

fn http_method(value: &str) -> Option<&'static str> {
    Some(match value {
        "Get" => "get",
        "Post" => "post",
        "Put" => "put",
        "Patch" => "patch",
        "Delete" => "delete",
        _ => return None,
    })
}

/// Reads the project's published REST services while it is still open.
/// Operations the generated router cannot express — an unsupported HTTP
/// method, a missing microflow, or a parameter with no name — are skipped
/// so the generated routes never claim more than the model declares.
fn collect_published_services(
    project: &Project,
    packages: &std::collections::BTreeSet<String>,
    http: &HashMap<String, HttpParameters>,
) -> Result<Vec<PublishedService>> {
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
    let mut services = Vec::new();
    for unit in &units {
        let document = project
            .mpr()
            .parse_contents(unit)
            .map_err(mxrs_model::ModelError::from)?;
        if document.get_str("$Type").ok() != Some("Rest$PublishedRestService") {
            continue;
        }
        let Some(module_name) = owning_module(&unit.container_id, &parent_by_id, &module_by_id)
        else {
            continue;
        };
        let Some(service) = published_service(
            &document,
            &module_name,
            |name| module_root(&module_stem(name), packages),
            http,
        ) else {
            continue;
        };
        services.push(service);
    }
    services.sort_by(|left, right| {
        (&left.module_name, &left.file_stem).cmp(&(&right.module_name, &right.file_stem))
    });
    assign_controllers(&mut services);
    Ok(services)
}

fn published_service(
    document: &mxrs_bson::Document,
    module_name: &str,
    root: impl Fn(&str) -> ModuleRoot,
    http: &HashMap<String, HttpParameters>,
) -> Option<PublishedService> {
    if document.get_bool("Excluded").unwrap_or(false) {
        return None;
    }
    let name = document
        .get_str("Name")
        .ok()
        .filter(|name| !name.is_empty())?;
    let base_path = document
        .get_str("Path")
        .unwrap_or_default()
        .trim_matches('/');
    let mut routes: Vec<ServiceRoute> = Vec::new();
    for resource in documents_in(document, "Resources") {
        let resource_name = resource.get_str("Name").unwrap_or_default().to_string();
        for operation in documents_in(&resource, "Operations") {
            let Some(method) = operation.get_str("HttpMethod").ok().and_then(http_method) else {
                continue;
            };
            let Some(microflow) = operation
                .get_str("Microflow")
                .ok()
                .filter(|name| name.contains('.'))
            else {
                continue;
            };
            let suffix = operation
                .get_str("Path")
                .unwrap_or_default()
                .trim_matches('/');
            let path = [base_path, resource_name.trim_matches('/'), suffix]
                .into_iter()
                .filter(|segment| !segment.is_empty())
                .fold(String::new(), |mut path, segment| {
                    path.push('/');
                    path.push_str(segment);
                    path
                });
            let path = if path.is_empty() {
                "/".to_string()
            } else {
                path
            };
            let mut parameters = Vec::new();
            let mut malformed = false;
            for parameter in documents_in(&operation, "Parameters") {
                let Some(parameter_name) = parameter
                    .get_str("Name")
                    .ok()
                    .filter(|name| !name.is_empty())
                else {
                    malformed = true;
                    break;
                };
                let source = match parameter.get_str("ParameterType").unwrap_or_default() {
                    "Path" => ParameterSource::Path,
                    "Query" => ParameterSource::Query,
                    "Body" => ParameterSource::Body,
                    _ => {
                        malformed = true;
                        break;
                    }
                };
                let microflow_parameter = parameter
                    .get_str("MicroflowParameter")
                    .unwrap_or_default()
                    .rsplit('.')
                    .next()
                    .unwrap_or_default()
                    .to_string();
                if microflow_parameter.is_empty() {
                    malformed = true;
                    break;
                }
                parameters.push(OperationParameter {
                    name: parameter_name.to_string(),
                    microflow_parameter,
                    source,
                });
            }
            if malformed {
                continue;
            }
            let operation = ServiceOperation {
                method,
                resource: resource_name.trim_matches('/').to_string(),
                addresses_one: suffix
                    .rsplit('/')
                    .next()
                    .is_some_and(|segment| segment.starts_with('{') && segment.ends_with('}')),
                controller: String::new(),
                handler: String::new(),
                microflow: microflow.to_string(),
                summary: operation.get_str("Summary").unwrap_or_default().to_string(),
                documentation: operation
                    .get_str("Documentation")
                    .unwrap_or_default()
                    .to_string(),
                parameters,
                export_mapping: operation
                    .get_str("ExportMapping")
                    .unwrap_or_default()
                    .to_string(),
                http_parameters: http.get(microflow).cloned().unwrap_or_default(),
            };
            match routes.iter_mut().find(|route| route.path == path) {
                Some(route)
                    if route
                        .operations
                        .iter()
                        .all(|other| other.method != operation.method) =>
                {
                    route.operations.push(operation);
                }
                // A duplicate method on the same path cannot be routed; the
                // model stays the record, so the first declaration wins.
                Some(_) => {}
                None => routes.push(ServiceRoute {
                    path,
                    operations: vec![operation],
                }),
            }
        }
    }
    if routes.is_empty() {
        return None;
    }
    Some(PublishedService {
        module_name: module_name.to_string(),
        root: root(module_name),
        name: name.to_string(),
        file_stem: inner_file_stem(name),
        base_path: base_path.to_string(),
        documentation: document
            .get_str("Documentation")
            .unwrap_or_default()
            .to_string(),
        routes,
        authentication: service_authentication(document),
    })
}

/// One `ExportMappings$ExportMapping` the model declares, lowered into the
/// declaration its module will expose.
struct ExportMappingDocument {
    module_name: String,
    /// Qualified mapping name, e.g. `API_Rest.EM_Order_List`, as a published
    /// operation names it.
    qualified_name: String,
    /// The file the module's HTTP folder writes this mapping to.
    file_stem: String,
    documentation: String,
    /// `NullValueOption: SendAsNil` — an empty member answers `null` instead
    /// of being left out.
    send_nils: bool,
    root: MappedObject,
}

/// One `ExportMappings$ObjectMappingElement`.
struct MappedObject {
    entity: String,
    /// The association traversed from the parent; empty at the root.
    association: String,
    /// The JSON key this level nests under; empty at the root.
    key: String,
    /// `true` when the level is a JSON array — `(Array)|(Object)`.
    multiple: bool,
    values: Vec<MappedValue>,
    children: Vec<MappedObject>,
}

/// One `ExportMappings$ValueMappingElement`.
struct MappedValue {
    /// The JSON key, which the model may have renamed away from the
    /// attribute's own name.
    key: String,
    /// The attribute's own name, which is also its store member name.
    attribute: String,
}

/// Where a generated export mapping declaration lives, so a service file can
/// name it.
struct MappingTarget {
    root: ModuleRoot,
    module_stem: String,
    file_stem: String,
}

/// Reads the export mappings `referenced` names, while the project is still
/// open. A mapping the declaration surface cannot express — an XML-only
/// element tree, a custom Java handler, a value converter, an element with no
/// JSON key — is left out entirely, so the operations that declare it keep
/// answering the entity's stored attributes rather than a half-applied
/// document.
fn collect_export_mappings(
    project: &Project,
    referenced: &std::collections::BTreeSet<String>,
) -> Result<Vec<ExportMappingDocument>> {
    if referenced.is_empty() {
        return Ok(Vec::new());
    }
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
    let mut mappings = Vec::new();
    for unit in &units {
        let document = project
            .mpr()
            .parse_contents(unit)
            .map_err(mxrs_model::ModelError::from)?;
        if document.get_str("$Type").ok() != Some("ExportMappings$ExportMapping") {
            continue;
        }
        let Some(module_name) = owning_module(&unit.container_id, &parent_by_id, &module_by_id)
        else {
            continue;
        };
        let Some(mapping) = export_mapping(&document, &module_name) else {
            continue;
        };
        if !referenced.contains(&mapping.qualified_name) {
            continue;
        }
        mappings.push(mapping);
    }
    mappings.sort_by(|left, right| left.qualified_name.cmp(&right.qualified_name));
    Ok(mappings)
}

fn export_mapping(
    document: &mxrs_bson::Document,
    module_name: &str,
) -> Option<ExportMappingDocument> {
    if document.get_bool("Excluded").unwrap_or(false) {
        return None;
    }
    let name = document
        .get_str("Name")
        .ok()
        .filter(|name| !name.is_empty())?;
    // Mendix allows exactly one root element per mapping; more than one is
    // not a document this declaration can describe.
    let elements = documents_in(document, "Elements");
    let [root] = elements.as_slice() else {
        return None;
    };
    let root = mapped_object(root)?;
    if !root.key.is_empty() || !root.association.is_empty() {
        return None;
    }
    Some(ExportMappingDocument {
        module_name: module_name.to_string(),
        qualified_name: format!("{module_name}.{name}"),
        file_stem: inner_file_stem(name),
        documentation: document
            .get_str("Documentation")
            .unwrap_or_default()
            .to_string(),
        send_nils: document.get_str("NullValueOption").unwrap_or_default() == "SendAsNil",
        root,
    })
}

fn mapped_object(element: &mxrs_bson::Document) -> Option<MappedObject> {
    if element.get_str("$Type").ok()? != "ExportMappings$ObjectMappingElement"
        || !element
            .get_str("CustomHandlerCall")
            .unwrap_or_default()
            .is_empty()
    {
        return None;
    }
    let children = documents_in(element, "Children");
    let entity = element.get_str("Entity").unwrap_or_default();
    // Mendix wraps a *repeated* nested object in an element of its own: the
    // wrapper carries the JSON key and no entity, its single object child
    // carries the entity, the association and `MaxOccurs: -1`. The wrapper
    // adds no level to the document — its child's own path already names the
    // same key — so it collapses into that child.
    if entity.is_empty() {
        let [wrapped] = children.as_slice() else {
            return None;
        };
        return mapped_object(wrapped);
    }
    // Multiplicity decides whether the key answers an array, and getting it
    // wrong changes the document's shape, so an element that does not state
    // it is not an element this declaration can describe.
    // Studio Pro writes the bound as a 32-bit integer; read the 64-bit
    // spelling too so a model that widened it still states its shape.
    let occurs = element
        .get_i32("MaxOccurs")
        .map(i64::from)
        .or_else(|_| element.get_i64("MaxOccurs"))
        .ok()?;
    let path = element.get_str("JsonPath").unwrap_or_default();
    let key = object_json_key(path)?;
    let association = element.get_str("Association").unwrap_or_default();
    // A nested level is reached by association; the root is supplied by the
    // flow result itself. Anything else is a level we cannot walk to.
    if key.is_some() == association.is_empty() {
        return None;
    }
    // At the root the path states the shape too, as `(Array)|(Object)`. The
    // two must agree; if they do not, the element is not read the way this
    // declaration would read it.
    if key.is_none() && json_path_segments(path).contains(&"(Array)") != (occurs != 1) {
        return None;
    }
    let mut values = Vec::new();
    let mut nested = Vec::new();
    for child in children {
        match child.get_str("$Type").ok()? {
            "ExportMappings$ValueMappingElement" => values.push(mapped_value(&child)?),
            "ExportMappings$ObjectMappingElement" => nested.push(mapped_object(&child)?),
            _ => return None,
        }
    }
    if values.is_empty() && nested.is_empty() {
        return None;
    }
    Some(MappedObject {
        entity: entity.to_string(),
        association: association.to_string(),
        key: key.unwrap_or_default(),
        multiple: occurs != 1,
        values,
        children: nested,
    })
}

fn mapped_value(element: &mxrs_bson::Document) -> Option<MappedValue> {
    // A converter rewrites the value on its way out; the declaration has no
    // surface for one yet, so the whole mapping steps aside.
    if !element.get_str("Converter").unwrap_or_default().is_empty() {
        return None;
    }
    let attribute = element
        .get_str("Attribute")
        .ok()
        .filter(|attribute| !attribute.is_empty())?
        .rsplit('.')
        .next()
        .filter(|attribute| !attribute.is_empty())?
        .to_string();
    let key = value_json_key(element.get_str("JsonPath").unwrap_or_default())?;
    Some(MappedValue { key, attribute })
}

/// `(Object)` and `(Array)` mark structure in a mapping element's `JsonPath`;
/// every other segment names a key. The model's `ExposedName` is the Studio
/// Pro caption, which can differ in case from the key the document carries —
/// so the path, not the caption, is the key.
fn json_path_segments(path: &str) -> Vec<&str> {
    path.split('|')
        .filter(|segment| !segment.is_empty())
        .collect()
}

fn json_path_marker(segment: &str) -> bool {
    matches!(segment, "(Object)" | "(Array)")
}

/// The JSON key a value element answers under, e.g. `lane` from
/// `(Object)|lane`.
fn value_json_key(path: &str) -> Option<String> {
    let segments = json_path_segments(path);
    let last = segments.last()?;
    (!json_path_marker(last)).then(|| (*last).to_string())
}

/// The JSON key an object element nests under, or `None` at the root, which
/// *is* the document: `(Object)` and `(Array)|(Object)` are the root shapes,
/// `(Object)|metadata` a single object under `metadata`, and
/// `(Object)|data|(Object)` the repeated object under `data`. Whether a key
/// answers an array comes from `MaxOccurs`, not from the path — Mendix writes
/// the `(Array)` marker only at the root.
fn object_json_key(path: &str) -> Option<Option<String>> {
    let mut segments = json_path_segments(path);
    if segments.last() == Some(&"(Object)") {
        segments.pop();
    }
    if segments.last() == Some(&"(Array)") {
        segments.pop();
    }
    match segments.pop() {
        Some(segment) if json_path_marker(segment) => None,
        Some(segment) => Some(Some(segment.to_string())),
        None => Some(None),
    }
}

/// The strings inside a BSON array field, in declaration order.
fn strings_in(document: &mxrs_bson::Document, field: &str) -> Vec<String> {
    mxrs_bson::parse_array(document.get_array(field).ok().map(Vec::as_slice))
        .items
        .into_iter()
        .filter_map(|item| match item {
            mxrs_bson::Bson::String(value) if !value.is_empty() => Some(value),
            _ => None,
        })
        .collect()
}

/// The documents inside a BSON array field, in declaration order.
fn documents_in(document: &mxrs_bson::Document, field: &str) -> Vec<mxrs_bson::Document> {
    mxrs_bson::parse_array(document.get_array(field).ok().map(Vec::as_slice))
        .items
        .into_iter()
        .filter_map(|item| match item {
            mxrs_bson::Bson::Document(document) => Some(document),
            _ => None,
        })
        .collect()
}

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
    packages: &std::collections::BTreeSet<String>,
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
            let stem = module_stem(&module_name);
            if stem.is_empty() || stem.starts_with(|c: char| c.is_ascii_digit()) {
                return None;
            }
            // Marketplace actions remain preserved with their package model,
            // but they are not application-owned adapter extension points.
            if module_root(&stem, packages) == ModuleRoot::Package {
                return None;
            }
            actions.sort_by(|left, right| left.action_name.cmp(&right.action_name));
            let mut seen = std::collections::HashSet::new();
            actions.retain(|action| seen.insert(action.trait_name.clone()));
            Some(ActionPortModule {
                module_name,
                root: module_root(&stem, packages),
                module_stem: stem,
                actions,
            })
        })
        .collect();
    modules.sort_by(|left, right| left.module_stem.cmp(&right.module_stem));
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
            "\n/// Contract of the `{}.{}` Java action.\npub trait {} {{\n{}    fn call(&self{}) -> Result<{}, ServiceError>;\n}}",
            module.module_name,
            action.action_name,
            action.trait_name,
            if action.parameters.len() >= TOO_MANY_PARAMETERS {
                "    #[allow(clippy::too_many_arguments)]\n"
            } else {
                ""
            },
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

fn render_action_registry_file(module: &ActionPortModule) -> String {
    let uses_port_value = module
        .actions
        .iter()
        .any(|action| !action.parameters.is_empty() || action.result.is_some());
    let mut out = format!(
        "//! Registers hand-written `{name}` action-port implementations on the\n//! flow engine under the names the model calls them by.\n\nuse std::collections::BTreeMap;\n\nuse mxrs::ports::{{FlowEngine, FlowError, FlowValue, JavaAction{port_value}}};\n\nuse {ports}::actions::*;\n",
        name = module.module_name,
        ports = module.root.path(&module.module_stem, "ports", "ports"),
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

/// Everything one Mendix module contributes to the generated crate: its
/// domain model, DTOs, services, ports, controllers, user interface and
/// markers. Where each lands depends on the module's root — a folder per
/// concept in the project's layers, or the module's own package folder.
#[derive(Default)]
struct GeneratedModule {
    /// Mendix module name, exactly as declared.
    name: String,
    /// Which module tree the folder belongs to, and therefore whether its
    /// declarations are applied onto the project.
    root: ModuleRoot,
    /// `AppStoreVersion` for a package, so the folder can say which release of
    /// the module it was generated from. Empty for an authored module, and for
    /// a package whose model records no version.
    version: String,
    /// `(file stem, source)` per persisted entity declaration.
    entities: Vec<(String, String)>,
    /// `(file stem, source)` per non-persistable/view entity.
    dtos: Vec<(String, String)>,
    /// `(file stem, source)` per enumeration.
    enumerations: Vec<(String, String)>,
    /// `(file stem, source)` per constant, regular expression, scheduled
    /// event and standalone menu the module declares.
    documents: Vec<(String, String)>,
    /// The module's role declarations, as an `apply` source.
    security: Option<String>,
    /// `(file stem, source)` per reconstructed microflow service.
    services: Vec<(String, String)>,
    /// `(file stem, source)` per reconstructed nanoflow.
    nanoflows: Vec<(String, String)>,
    /// `(file stem, source)` per typed page.
    pages: Vec<(String, String)>,
    /// `ports/services.rs` — the module's service port trait.
    ports_services: Option<String>,
    /// `ports/actions.rs` — the module's action port traits.
    ports_actions: Option<String>,
    /// `markers.rs` — the module's compile-time model markers.
    markers: Option<String>,
    /// `(file stem, source)` per export mapping a published REST operation
    /// answers through. A module can own mappings without publishing a
    /// service of its own.
    mappings: Vec<(String, String)>,
    /// The module's controllers: its published REST services.
    http: Option<GeneratedHttp>,
}

/// One module's controllers folder: its index, and for each published REST
/// service a route table and one controller per resource.
struct GeneratedHttp {
    index: String,
    /// `(file stem, source)` per route table and controller, in file order.
    files: Vec<(String, String)>,
}

fn generated_module<'a>(
    map: &'a mut std::collections::BTreeMap<String, GeneratedModule>,
    name: &str,
) -> &'a mut GeneratedModule {
    let entry = map.entry(module_stem(name)).or_default();
    if entry.name.is_empty() {
        entry.name = name.to_string();
    }
    entry
}

/// Writes the two trees the generated crate keeps.
///
/// The authored tree is **layer-first**: one `src/domain/`, one
/// `src/controllers/`, one `src/ui/` for the whole project, each concept
/// holding a folder per Mendix module. The module folder is not a repetition of the layers —
/// it is what keeps the names apart, because Mendix entity names are unique
/// per module and not across a project.
///
/// The package tree is **module-shaped**: `src/packages/<module>/` with the
/// module's own Clean Architecture folders inside, because a package is
/// external — the project uses its types and declares none of them.
fn write_modules_layer(
    destination: &Path,
    modules: &std::collections::BTreeMap<String, GeneratedModule>,
) -> Result<AuthoredLayers> {
    let by_root = |root: ModuleRoot| -> Vec<(&String, &GeneratedModule)> {
        modules
            .iter()
            .filter(|(_, module)| module.root == root)
            .collect()
    };

    let packages = by_root(ModuleRoot::Package);
    if !packages.is_empty() {
        let directory = destination.join("src/packages");
        std::fs::create_dir_all(&directory).map_err(|source| io_error(&directory, source))?;
        let mut index = String::from(
            "//! One folder per Mendix module this project installed from the\n//! Marketplace.\n//!\n//! These carry each module's types and contracts — entity and DTO structs,\n//! enumerations, ports, markers, and any REST surface the application serves\n//! — so the project can name them. They declare nothing: a package's model\n//! stays exactly as `model/imported` holds it, because an edit here would be\n//! lost the next time the module is upgraded. Upgrade the module in Studio\n//! Pro or with `mxrs marketplace update`, and re-import.\n\n",
        );
        for (stem, module) in &packages {
            let version = if module.version.is_empty() {
                String::new()
            } else {
                format!(" {}", module.version)
            };
            let _ = writeln!(
                index,
                "/// `{}`{version}, from the Marketplace.",
                module.name
            );
            let _ = writeln!(index, "pub mod {stem};");
        }
        write_text(&directory.join("mod.rs"), &index)?;
        for (stem, module) in packages {
            write_module_folder(&directory.join(stem), module)?;
        }
    }

    write_authored_layers(destination, &by_root(ModuleRoot::Authored))
}

/// One concept of the authored tree: the layer folder it lives under and
/// the header its index carries.
struct AuthoredConcept<'a> {
    /// Path under `src/`, e.g. `domain/entities`.
    folder: &'a str,
    /// What the concept is, for the index doc comment.
    subject: &'a str,
    /// `(module stem, files)` for every module that has any.
    modules: Vec<(&'a str, &'a Vec<(String, String)>)>,
}

/// The concept folders each authored layer holds, keyed by layer folder
/// (`domain`, `ui`). A concept is listed only when the writer
/// created its folder: declaring one that was not written is what makes the
/// generated crate fail to resolve its own modules.
type AuthoredLayers = std::collections::BTreeMap<String, Vec<String>>;

/// Records that `layer` holds `concept`.
fn record_concept(layers: &mut AuthoredLayers, layer: &str, concept: &str) {
    layers
        .entry(layer.to_string())
        .or_default()
        .push(concept.to_string());
}

/// Writes `src/domain/`, `src/services/`, `src/ports/`, `src/controllers/`
/// and `src/ui/` for the modules this project created: a module folder
/// inside each concept, and directly inside the layers that are one concept
/// — services, ports, controllers.
fn write_authored_layers(
    destination: &Path,
    modules: &[(&String, &GeneratedModule)],
) -> Result<AuthoredLayers> {
    let files = |pick: fn(&GeneratedModule) -> &Vec<(String, String)>| {
        modules
            .iter()
            .filter(|(_, module)| !pick(module).is_empty())
            .map(|(stem, module)| (stem.as_str(), pick(module)))
            .collect::<Vec<_>>()
    };
    let concepts = [
        AuthoredConcept {
            folder: "domain/entities",
            subject: "persisted entities",
            modules: files(|module| &module.entities),
        },
        AuthoredConcept {
            folder: "domain/dtos",
            subject: "non-persistable and view entities",
            modules: files(|module| &module.dtos),
        },
        AuthoredConcept {
            folder: "domain/enumerations",
            subject: "enumerations",
            modules: files(|module| &module.enumerations),
        },
        AuthoredConcept {
            folder: "domain/documents",
            subject: "constants, regular expressions, scheduled events and\n//! standalone menus",
            modules: files(|module| &module.documents),
        },
        AuthoredConcept {
            folder: "domain/mappings",
            subject: "export mappings: the JSON documents published REST\n//! operations answer with",
            modules: files(|module| &module.mappings),
        },
        AuthoredConcept {
            folder: "services",
            subject: "microflows as services",
            modules: files(|module| &module.services),
        },
        AuthoredConcept {
            folder: "ui/nanoflows",
            subject: "nanoflows",
            modules: files(|module| &module.nanoflows),
        },
        AuthoredConcept {
            folder: "ui/pages",
            subject: "editable pages",
            modules: files(|module| &module.pages),
        },
    ];
    let mut applied_layers: AuthoredLayers = Default::default();
    for concept in &concepts {
        if concept.modules.is_empty() {
            continue;
        }
        write_authored_concept(destination, concept)?;
        let (layer, name) = split_concept(concept.folder);
        record_concept(&mut applied_layers, layer, name);
    }
    write_authored_module_security(destination, modules, &mut applied_layers)?;
    write_authored_ports(destination, modules, &mut applied_layers)?;
    write_authored_http(destination, modules)?;
    Ok(applied_layers)
}

fn split_concept(folder: &str) -> (&str, &str) {
    folder.split_once('/').unwrap_or((folder, folder))
}

/// One concept folder: `src/<layer>/<concept>/<module>/<file>.rs`, with an
/// index per module and one for the concept. Each index only lists what it
/// holds: the declarations register themselves.
fn write_authored_concept(destination: &Path, concept: &AuthoredConcept<'_>) -> Result<()> {
    let directory = destination.join("src").join(concept.folder);
    std::fs::create_dir_all(&directory).map_err(|source| io_error(&directory, source))?;
    let (_, name) = split_concept(concept.folder);
    let mut index = if concept.folder == "services" {
        render_services_index(&[])
    } else {
        format!(
            "//! Every module's {}, one folder per Mendix module: names are\n//! unique per module, not across the project.\n\n",
            concept.subject
        )
    };
    for (stem, _) in &concept.modules {
        let _ = writeln!(index, "pub mod {stem};");
    }
    write_text(&directory.join("mod.rs"), &index)?;
    for (stem, files) in &concept.modules {
        let module = directory.join(stem);
        std::fs::create_dir_all(&module).map_err(|source| io_error(&module, source))?;
        let mut module_index = format!("//! The {stem} module's {name}.\n\n");
        for (file, _) in files.iter() {
            let _ = writeln!(module_index, "pub mod {file};");
        }
        write_text(&module.join("mod.rs"), &module_index)?;
        for (file, source) in files.iter() {
            write_text(&module.join(format!("{file}.rs")), source)?;
        }
    }
    Ok(())
}

/// A module's own role declarations: one file per module under
/// `domain/module_security/`, beside the project's own `domain/security.rs`.
fn write_authored_module_security(
    destination: &Path,
    modules: &[(&String, &GeneratedModule)],
    applied: &mut AuthoredLayers,
) -> Result<()> {
    let owned: Vec<_> = modules
        .iter()
        .filter_map(|(stem, module)| module.security.as_ref().map(|source| (stem, source)))
        .collect();
    if owned.is_empty() {
        return Ok(());
    }
    let directory = destination.join("src/domain/module_security");
    std::fs::create_dir_all(&directory).map_err(|source| io_error(&directory, source))?;
    let mut index = String::from(
        "//! Each Mendix module's own role declarations. The project's own\n//! security is `crate::domain::security`.\n\n",
    );
    for (stem, _) in &owned {
        let _ = writeln!(index, "pub mod {stem};");
    }
    write_text(&directory.join("mod.rs"), &index)?;
    for (stem, source) in owned {
        write_text(&directory.join(format!("{stem}.rs")), source)?;
    }
    record_concept(applied, "domain", "module_security");
    Ok(())
}

/// The contracts hand-written code implements or calls, one folder per module.
fn write_authored_ports(
    destination: &Path,
    modules: &[(&String, &GeneratedModule)],
    declared: &mut AuthoredLayers,
) -> Result<()> {
    let owned: Vec<_> = modules
        .iter()
        .filter(|(_, module)| module.ports_services.is_some() || module.ports_actions.is_some())
        .collect();
    if owned.is_empty() {
        return Ok(());
    }
    let directory = destination.join("src/ports");
    std::fs::create_dir_all(&directory).map_err(|source| io_error(&directory, source))?;
    let mut index = String::from(
        "//! The contracts between the model and hand-written code, one folder per\n//! Mendix module: what a module's services offer, and what its actions\n//! need an adapter in `infrastructure` to provide.\n\n",
    );
    for (stem, _) in &owned {
        let _ = writeln!(index, "pub mod {stem};");
    }
    write_text(&directory.join("mod.rs"), &index)?;
    for (stem, module) in owned {
        let folder = directory.join(stem.as_str());
        std::fs::create_dir_all(&folder).map_err(|source| io_error(&folder, source))?;
        let mut module_index = format!(
            "//! Contracts the {} module exposes to hand-written code.\n\n",
            module.name
        );
        if let Some(actions) = &module.ports_actions {
            module_index.push_str("pub mod actions;\n");
            write_text(&folder.join("actions.rs"), actions)?;
        }
        if let Some(services) = &module.ports_services {
            module_index.push_str("pub mod services;\n");
            write_text(&folder.join("services.rs"), services)?;
        }
        write_text(&folder.join("mod.rs"), &module_index)?;
    }
    record_concept(declared, "ports", "ports");
    Ok(())
}

/// The published REST surface, one folder per module under `controllers/`.
/// The project-level `controllers/mod.rs` merges every service's route table
/// and owns the shared state and error type.
fn write_authored_http(destination: &Path, modules: &[(&String, &GeneratedModule)]) -> Result<()> {
    for (stem, module) in modules {
        let Some(http) = &module.http else {
            continue;
        };
        write_controllers(
            &destination.join("src/controllers").join(stem.as_str()),
            http,
        )?;
    }
    Ok(())
}

/// One module's controllers folder: its index, each service's route table,
/// and their controllers.
fn write_controllers(directory: &Path, http: &GeneratedHttp) -> Result<()> {
    std::fs::create_dir_all(directory).map_err(|source| io_error(directory, source))?;
    write_text(&directory.join("mod.rs"), &http.index)?;
    for (file, source) in &http.files {
        write_text(&directory.join(format!("{file}.rs")), source)?;
    }
    Ok(())
}

/// One installed module's folder. A package contributes only what the project
/// *uses* — types, contracts and served routes — so what it declares
/// (constants, module security, microflows, nanoflows, pages) is not written:
/// the imported model stays the only record of it.
fn write_module_folder(directory: &Path, module: &GeneratedModule) -> Result<()> {
    std::fs::create_dir_all(directory).map_err(|source| io_error(directory, source))?;

    let domain_concepts = [
        (
            "entities",
            format!("//! The {} module's persisted entities.\n\n", module.name),
            &module.entities,
        ),
        (
            "enumerations",
            format!("//! The {} module's enumerations.\n\n", module.name),
            &module.enumerations,
        ),
        (
            "mappings",
            format!(
                "//! The {} module's export mappings: the JSON documents its\n//! published REST operations answer with.\n\n",
                module.name
            ),
            &module.mappings,
        ),
    ];
    let has_domain = domain_concepts
        .iter()
        .any(|(_, _, files)| !files.is_empty());
    if has_domain {
        let domain = directory.join("domain");
        std::fs::create_dir_all(&domain).map_err(|source| io_error(&domain, source))?;
        let mut domain_index = format!("//! The {} module's domain model.\n\n", module.name);
        for (concept, header, files) in &domain_concepts {
            if files.is_empty() {
                continue;
            }
            let _ = writeln!(domain_index, "pub mod {concept};");
            write_concept_files(&domain.join(concept), header, files)?;
        }
        write_text(&domain.join("mod.rs"), &domain_index)?;
    }

    if !module.dtos.is_empty() {
        write_concept_files(
            &directory.join("dto"),
            &format!(
                "//! The {} module's non-persistable and view entities.\n\n",
                module.name
            ),
            &module.dtos,
        )?;
    }

    if let Some(http) = &module.http {
        write_controllers(&directory.join("controllers"), http)?;
    }

    let has_ports = module.ports_services.is_some() || module.ports_actions.is_some();
    if has_ports {
        let ports = directory.join("ports");
        std::fs::create_dir_all(&ports).map_err(|source| io_error(&ports, source))?;
        let mut ports_index = format!(
            "//! Contracts the {} module exposes to hand-written code.\n\n",
            module.name
        );
        if let Some(actions) = &module.ports_actions {
            ports_index.push_str("pub mod actions;\n");
            write_text(&ports.join("actions.rs"), actions)?;
        }
        if let Some(services) = &module.ports_services {
            ports_index.push_str("pub mod services;\n");
            write_text(&ports.join("services.rs"), services)?;
        }
        write_text(&ports.join("mod.rs"), &ports_index)?;
    }

    if let Some(markers) = &module.markers {
        write_text(&directory.join("markers.rs"), markers)?;
    }

    let mut module_index = format!(
        "//! The {} Mendix module, installed from the Marketplace{}.\n//!\n//! Its types and contracts, so this project can name them. Editing them\n//! changes nothing: a package declares nothing, and the next upgrade of the\n//! module replaces it. `model/imported` holds what it actually declares.\n\n",
        module.name,
        if module.version.is_empty() {
            String::new()
        } else {
            format!(" at {}", module.version)
        },
    );
    for (name, present) in [
        ("controllers", module.http.is_some()),
        ("domain", has_domain),
        ("dto", !module.dtos.is_empty()),
        ("markers", module.markers.is_some()),
        ("ports", has_ports),
    ] {
        if present {
            let _ = writeln!(module_index, "pub mod {name};");
        }
    }
    write_text(&directory.join("mod.rs"), &module_index)?;
    Ok(())
}

/// Writes one concept folder: a file per item and the index that lists them.
fn write_concept_files(directory: &Path, header: &str, files: &[(String, String)]) -> Result<()> {
    std::fs::create_dir_all(directory).map_err(|source| io_error(directory, source))?;
    let mut index = String::from(header);
    for (stem, _) in files {
        let _ = writeln!(index, "pub mod {stem};");
    }
    write_text(&directory.join("mod.rs"), &index)?;
    for (stem, source) in files {
        write_text(&directory.join(format!("{stem}.rs")), source)?;
    }
    Ok(())
}

/// File stem for one concept inside its module folder: snake_case, like
/// any other Rust module file. A Mendix name that already separates words
/// with underscores (`Acme_User__EU__Location`) must not turn into a
/// module name Rust warns about, so runs collapse to one.
fn inner_file_stem(name: &str) -> String {
    let mut stem = collapse_underscores(&snake_ident(name));
    if rust_keyword(&stem) {
        stem.push('_');
    }
    stem
}

/// Collapses underscore runs and trims the edges, keeping a leading one
/// when it is all that makes the name a valid identifier.
fn collapse_underscores(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    for character in name.chars() {
        if character == '_' && out.ends_with('_') {
            continue;
        }
        out.push(character);
    }
    let trimmed = out.trim_end_matches('_');
    let leading_digit = trimmed
        .trim_start_matches('_')
        .starts_with(|c: char| c.is_ascii_digit());
    if trimmed.starts_with('_') && !leading_digit {
        return trimmed.trim_start_matches('_').to_string();
    }
    trimmed.to_string()
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
    /// stays ignorant of the ones outside it — and a layer index is only a
    /// list of what the layer holds: every declaration registers itself, so
    /// no layer composes anything and none can reach outward to do it.
    #[test]
    fn generated_layers_only_list_what_they_hold() {
        // The layers are addressed by what the module writer produced, so
        // exercise both: a project with nothing to declare, and one with every
        // concept the writer can report.
        let empty = AuthoredLayers::new();
        let mut full = AuthoredLayers::new();
        for concept in [
            "documents",
            "dtos",
            "entities",
            "enumerations",
            "module_security",
        ] {
            record_concept(&mut full, "domain", concept);
        }
        record_concept(&mut full, "services", "services");
        record_concept(&mut full, "ports", "ports");
        for concept in ["nanoflows", "pages"] {
            record_concept(&mut full, "ui", concept);
        }

        assert_eq!(
            render_domain_module(&full, true)
                .lines()
                .filter(|line| !line.starts_with("//!") && !line.is_empty())
                .collect::<Vec<_>>(),
            [
                "pub mod documents;",
                "pub mod dtos;",
                "pub mod entities;",
                "pub mod enumerations;",
                "pub mod module_security;",
                "pub mod security;",
            ]
        );
        // With nothing generated the layer declares nothing — not even a
        // security file the model gave no reason to write.
        let bare = render_domain_module(&empty, false);
        assert!(!bare.contains("pub mod"), "{bare}");

        // Services are a layer of their own: its index is the modules that
        // have any, and nothing of another layer.
        assert_eq!(
            render_services_index(&["crm", "sales"])
                .lines()
                .filter(|line| !line.starts_with("//!") && !line.is_empty())
                .collect::<Vec<_>>(),
            ["pub mod crm;", "pub mod sales;"]
        );
        // Neither services nor ports are concepts of the domain or of the
        // user interface.
        for layer in [render_domain_module(&full, true), render_ui_module(&full)] {
            assert!(!layer.contains("pub mod services;"), "{layer}");
            assert!(!layer.contains("pub mod ports;"), "{layer}");
        }

        let ui = render_ui_module(&full);
        // Nanoflows are the frontend's services, so they are a user-interface
        // concept — declared when the import produced any.
        for held in [
            "pub mod nanoflows;",
            "pub mod navigation;",
            "pub mod pages;",
        ] {
            assert!(ui.contains(held), "{held}\n{ui}");
        }
        // What the application serves over HTTP is not user interface: it
        // has a layer of its own.
        assert!(!ui.contains("pub mod http;"), "{ui}");
        assert!(!ui.contains("controllers"), "{ui}");
        // No page module is written when the import found nothing buildable,
        // so the layer must not declare one either.
        let bare_ui = render_ui_module(&empty);
        assert!(!bare_ui.contains("pub mod pages;"), "{bare_ui}");
        assert!(!bare_ui.contains("pub mod nanoflows;"), "{bare_ui}");

        assert!(render_infrastructure_module(true).contains("pub mod persistence;"));
        assert!(!render_infrastructure_module(false).contains("persistence"));

        // No layer composes, so none names another layer or the project
        // declaration at all.
        for layer in [
            render_domain_module(&full, true),
            render_services_index(&["sales"]),
            ui,
            render_infrastructure_module(true),
        ] {
            for composing in [
                "fn apply",
                "fn build",
                "ProjectDecl",
                "crate::",
                "pub mod modules;",
            ] {
                assert!(!layer.contains(composing), "{composing}\n{layer}");
            }
        }
    }

    /// The axum preset routes the model's published REST services through
    /// its own HTTP layer; the other presets keep a framework server stub
    /// and say so, rather than claiming routes they do not generate.
    #[test]
    fn api_modes_generate_their_own_server_and_dependency_surface() {
        for (mode, dependency, server_marker) in [
            (ApiMode::ActixWeb, "actix-web =", "HttpServer::new"),
            (ApiMode::Rocket, "rocket =", "rocket::build"),
        ] {
            let manifest = cargo_manifest("sample", None, mode);
            let stub = render_server_stub(mode);
            let binary = build_binary_source("sample", "Sample", mode);
            assert!(manifest.contains(dependency), "{}", mode.name());
            assert!(stub.contains(server_marker), "{}", mode.name());
            assert!(stub.contains("axum\n//! preset only"), "{}", mode.name());
            assert!(binary.contains("controllers::server::serve"));
        }

        let manifest = cargo_manifest("sample", None, ApiMode::Axum);
        assert!(manifest.contains("axum ="));
        assert!(manifest.contains("serde_json ="));
        let http = render_http_module(&[]);
        assert!(http.contains("pub fn router(state: AppState) -> Router"));
        assert!(http.contains("axum::serve(listener, router(state))"));
        let binary = build_binary_source("sample", "Sample", ApiMode::Axum);
        assert!(binary.contains("controllers::serve("));
        assert!(binary.contains("controllers::DEFAULT_MODEL"));
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

    /// A service lowered the way an import lowers it, with every file it
    /// becomes read as one text: the route table first, then its controllers.
    fn rendered_service(
        mut service: PublishedService,
        targets: &HashMap<String, MappingTarget>,
    ) -> String {
        assign_controllers(std::slice::from_mut(&mut service));
        render_published_service(&service, targets)
            .into_iter()
            .map(|(file, source)| format!("// {file}.rs\n{source}"))
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// A published REST service becomes a route table — the model's own
    /// paths and methods — and a controller per resource, with one function
    /// per operation binding the declared parameters onto the microflow it
    /// calls.
    #[test]
    fn published_rest_services_become_a_route_table_and_controllers() {
        let service = mxrs_bson::doc! {
            "$Type": "Rest$PublishedRestService",
            "Name": "API_Service",
            "Path": "api/v1",
            "Documentation": "",
            "Resources": mxrs_bson::build_array(vec![mxrs_bson::Bson::Document(mxrs_bson::doc! {
                "$Type": "Rest$PublishedRestServiceResource",
                "Name": "orders",
                "Operations": mxrs_bson::build_array(vec![
                    mxrs_bson::Bson::Document(mxrs_bson::doc! {
                        "$Type": "Rest$PublishedRestServiceOperation",
                        "HttpMethod": "Get",
                        "Path": "{id}",
                        "Summary": "Get one order",
                        "Documentation": "",
                        "Microflow": "Sales.ACT_GetOrder",
                        "ExportMapping": "Sales.EM_Order",
                        "Parameters": mxrs_bson::build_array(vec![mxrs_bson::Bson::Document(mxrs_bson::doc! {
                            "$Type": "Rest$RestOperationParameter",
                            "Name": "id",
                            "ParameterType": "Path",
                            "MicroflowParameter": "Sales.ACT_GetOrder.orderId",
                        })], 3),
                    }),
                    mxrs_bson::Bson::Document(mxrs_bson::doc! {
                        "$Type": "Rest$PublishedRestServiceOperation",
                        "HttpMethod": "Post",
                        "Path": "{id}",
                        "Summary": "",
                        "Documentation": "",
                        "Microflow": "Sales.ACT_PutOrder",
                        "Parameters": mxrs_bson::build_array(vec![mxrs_bson::Bson::Document(mxrs_bson::doc! {
                            "$Type": "Rest$RestOperationParameter",
                            "Name": "trace",
                            "ParameterType": "Query",
                            "MicroflowParameter": "Sales.ACT_PutOrder.trace",
                        })], 3),
                    }),
                    // An unroutable method is skipped rather than guessed at.
                    mxrs_bson::Bson::Document(mxrs_bson::doc! {
                        "$Type": "Rest$PublishedRestServiceOperation",
                        "HttpMethod": "Options",
                        "Path": "",
                        "Microflow": "Sales.ACT_Options",
                    }),
                ], 3),
            })], 3),
        };

        let service =
            published_service(&service, "Sales", |_| ModuleRoot::Authored, &HashMap::new())
                .expect("routable service");
        assert_eq!(service.routes.len(), 1);
        assert_eq!(service.routes[0].path, "/api/v1/orders/{id}");
        assert_eq!(service.routes[0].operations.len(), 2);

        let targets = HashMap::from([(
            "Sales.EM_Order".to_string(),
            MappingTarget {
                root: ModuleRoot::Authored,
                module_stem: "sales".to_string(),
                file_stem: "em_order".to_string(),
            },
        )]);
        let rendered = rendered_service(service, &targets);
        // The service file is the route table and nothing else: the paths,
        // each method bound to its controller's function.
        assert!(
            rendered.starts_with("// api_service.rs\n")
                && rendered.contains("\n// orders_controller.rs\n"),
            "{rendered}"
        );
        assert!(
            rendered.contains(
                ".route(\"/api/v1/orders/{id}\", get(orders_controller::show).post(orders_controller::create))"
            ),
            "{rendered}"
        );
        assert!(
            rendered.contains("use super::orders_controller;"),
            "{rendered}"
        );
        // The controller holds the operations of the `orders` resource,
        // named by what each does to it.
        assert!(
            rendered.contains("#[mxrs::route(method = \"GET\", path = \"/api/v1/orders/{id}\", service = \"API_Service\")]"),
            "{rendered}"
        );
        assert!(rendered.contains("pub async fn show("), "{rendered}");
        assert!(rendered.contains("pub async fn create("), "{rendered}");
        // Only the method that opens the chain is imported as a function.
        assert!(rendered.contains("use axum::routing::{get};"), "{rendered}");
        assert!(rendered.contains("/// Get one order"), "{rendered}");
        // The operation that declares an export mapping applies it, naming
        // the declaration through its own module's folder; the one that
        // declares none keeps answering the entity's stored attributes.
        assert!(
            rendered.contains(
                "/// Calls `Sales.ACT_GetOrder`, answering through the\n/// `Sales.EM_Order` export mapping."
            ),
            "{rendered}"
        );
        assert!(
            rendered.contains("use crate::domain::mappings::sales::em_order;"),
            "{rendered}"
        );
        assert!(
            rendered.contains("state.call_mapped(")
                && rendered.contains("&em_order::mapping(),")
                && rendered.contains("&caller,"),
            "{rendered}"
        );
        assert!(
            rendered.contains("/// Calls `Sales.ACT_PutOrder`."),
            "{rendered}"
        );
        assert!(
            rendered.contains("state.call(\"Sales.ACT_PutOrder\", arguments, &caller)"),
            "{rendered}"
        );
        // This fixture declares no authentication type and no allowed role, so
        // the model published it to anyone: every operation runs as the
        // project's guest, never as nobody.
        assert!(
            rendered.contains("let caller = state.anonymous();"),
            "{rendered}"
        );
        assert!(!rendered.contains("HeaderMap"), "{rendered}");
        assert!(
            rendered.contains(
                "arguments.insert(\n        \"orderId\".to_string(),\n        FlowValue::String(path.get(\"id\").cloned().unwrap_or_default()),\n    );"
            ) || rendered.contains(
                "arguments.insert(\"orderId\".to_string(), FlowValue::String(path.get(\"id\").cloned().unwrap_or_default()));"
            ),
            "{rendered}"
        );
        assert!(
            rendered.contains("query.get(\"trace\").map_or(FlowValue::Empty,"),
            "{rendered}"
        );
        assert!(!rendered.contains("ACT_Options"), "{rendered}");
    }

    /// `(resource, method, path, microflow)` rows as one published service.
    fn service_of(name: &str, operations: &[(&str, &str, &str, &str)]) -> PublishedService {
        let mut resources: Vec<(&str, Vec<mxrs_bson::Bson>)> = Vec::new();
        for (resource, method, path, microflow) in operations {
            let operation = mxrs_bson::Bson::Document(mxrs_bson::doc! {
                "$Type": "Rest$PublishedRestServiceOperation",
                "HttpMethod": *method,
                "Path": *path,
                "Summary": "",
                "Documentation": "",
                "Microflow": *microflow,
                "Parameters": mxrs_bson::build_array(vec![], 3),
            });
            match resources.iter_mut().find(|(known, _)| known == resource) {
                Some((_, operations)) => operations.push(operation),
                None => resources.push((resource, vec![operation])),
            }
        }
        let document = mxrs_bson::doc! {
            "$Type": "Rest$PublishedRestService",
            "Name": name,
            "Path": "api",
            "Documentation": "",
            "Resources": mxrs_bson::build_array(
                resources
                    .into_iter()
                    .map(|(resource, operations)| {
                        mxrs_bson::Bson::Document(mxrs_bson::doc! {
                            "$Type": "Rest$PublishedRestServiceResource",
                            "Name": resource,
                            "Operations": mxrs_bson::build_array(operations, 3),
                        })
                    })
                    .collect(),
                3,
            ),
        };
        published_service(
            &document,
            "Sales",
            |_| ModuleRoot::Authored,
            &HashMap::new(),
        )
        .expect("routable service")
    }

    fn handlers(service: &PublishedService) -> Vec<String> {
        service
            .routes
            .iter()
            .flat_map(|route| &route.operations)
            .map(|operation| format!("{}::{}", operation.controller, operation.handler))
            .collect()
    }

    /// A controller is a resource and a function is what its operation does
    /// to that resource — as long as that says which operation it is.
    #[test]
    fn operations_are_named_by_what_they_do_to_their_resource() {
        let mut services = [service_of(
            "OrderService",
            &[
                ("orders", "Get", "", "Sales.ACT_Order_List"),
                ("orders", "Post", "", "Sales.ACT_Order_Create"),
                ("orders", "Get", "{id}", "Sales.ACT_Order_Get"),
                ("orders", "Put", "{id}", "Sales.ACT_Order_Update"),
                ("orders", "Delete", "{id}", "Sales.ACT_Order_Delete"),
                // Two more reads of the collection: `index` no longer says
                // which, so each is what its microflow is.
                ("reports", "Get", "daily", "Sales.DS_Report_Daily"),
                ("reports", "Get", "monthly", "Sales.DS_Report_Monthly"),
                ("reports", "Post", "", "Sales.ACT_Report_Request"),
                // An operation the service publishes at its own path.
                ("", "Get", "", "Sales.ACT_Ping"),
            ],
        )];
        assign_controllers(&mut services);
        assert_eq!(
            handlers(&services[0]),
            [
                "orders_controller::index",
                "orders_controller::create",
                "orders_controller::show",
                "orders_controller::update",
                "orders_controller::destroy",
                "reports_controller::report_daily",
                "reports_controller::report_monthly",
                "reports_controller::create",
                "order_service_controller::index",
            ]
        );
    }

    /// Two services of one module that each publish a resource of the same
    /// name keep their controllers — and their callers' rules — apart.
    #[test]
    fn services_sharing_a_resource_name_get_a_controller_each() {
        let mut services = [
            service_of(
                "AdminApi",
                &[("orders", "Get", "", "Sales.ACT_Admin_Orders")],
            ),
            service_of(
                "PublicApi",
                &[
                    ("orders", "Get", "", "Sales.ACT_Public_Orders"),
                    ("status", "Get", "", "Sales.ACT_Status"),
                ],
            ),
        ];
        assign_controllers(&mut services);
        assert_eq!(
            handlers(&services[0]),
            ["admin_api_orders_controller::index"]
        );
        assert_eq!(
            handlers(&services[1]),
            [
                "public_api_orders_controller::index",
                // A resource only one of them publishes needs no telling apart.
                "status_controller::index",
            ]
        );
    }

    /// A published service is routed the way the model says it is reached:
    /// Basic authentication becomes a guard that signs the request in and
    /// checks the model's own allowed roles; an authentication scheme this
    /// layer cannot offer becomes a refusal rather than an open route.
    #[test]
    fn published_rest_authentication_follows_what_the_model_declares() {
        let service = |authentication: mxrs_bson::Document| {
            let mut document = mxrs_bson::doc! {
                "$Type": "Rest$PublishedRestService",
                "Name": "API_Service",
                "Path": "api/v1",
                "Documentation": "",
                "Resources": mxrs_bson::build_array(vec![mxrs_bson::Bson::Document(mxrs_bson::doc! {
                    "$Type": "Rest$PublishedRestServiceResource",
                    "Name": "orders",
                    "Operations": mxrs_bson::build_array(vec![mxrs_bson::Bson::Document(mxrs_bson::doc! {
                        "$Type": "Rest$PublishedRestServiceOperation",
                        "HttpMethod": "Get",
                        "Path": "",
                        "Summary": "",
                        "Documentation": "",
                        "Microflow": "Sales.ACT_GetOrder",
                        "Parameters": mxrs_bson::build_array(vec![], 3),
                    })], 3),
                })], 3),
            };
            for (key, value) in authentication {
                document.insert(key, value);
            }
            rendered_service(
                published_service(
                    &document,
                    "Sales",
                    |_| ModuleRoot::Authored,
                    &HashMap::new(),
                )
                .expect("routable service"),
                &HashMap::new(),
            )
        };
        let roles = |names: Vec<&str>| {
            mxrs_bson::build_array(
                names
                    .into_iter()
                    .map(|name| mxrs_bson::Bson::String(name.to_string()))
                    .collect(),
                1,
            )
        };

        let basic = service(mxrs_bson::doc! {
            "AuthenticationTypes": roles(vec!["basic"]),
            "AllowedRoles": roles(vec!["Sales.Admin", "Sales.User"]),
        });
        assert!(
            basic.contains("const ALLOWED_ROLES: &[&str] = &[\"Sales.Admin\", \"Sales.User\"];"),
            "{basic}"
        );
        assert!(
            basic.contains("const REALM: &str = \"API_Service\";"),
            "{basic}"
        );
        assert!(basic.contains("use axum::http::{HeaderMap};"), "{basic}");
        assert!(basic.contains("\n    headers: HeaderMap,"), "{basic}");
        assert!(
            basic.contains("let caller = state.basic_caller(&headers, REALM, ALLOWED_ROLES)?;"),
            "{basic}"
        );
        // Who may call is the service's to state and its controllers' to
        // enforce: the controller takes both from the route table's file.
        assert!(
            basic.contains("pub(super) const REALM: &str = \"API_Service\";"),
            "{basic}"
        );
        assert!(
            basic.contains("use super::api_service::{ALLOWED_ROLES, REALM};"),
            "{basic}"
        );

        // Roles to honour and no authentication type to identify a caller by:
        // serving it publicly would ignore the restriction the model states.
        let roleful = service(mxrs_bson::doc! {
            "AllowedRoles": roles(vec!["Sales.Admin"]),
        });
        assert!(
            roleful.contains("ApiError::unsupported_authentication(AUTHENTICATION)"),
            "{roleful}"
        );
        assert!(
            roleful.contains("without an authentication type to identify a caller by"),
            "{roleful}"
        );
        // A refusing service routes nothing, so it imports for nothing either.
        assert!(!roleful.contains("use mxrs::ports::"), "{roleful}");
        assert!(!roleful.contains("use axum::extract::"), "{roleful}");
        assert!(!roleful.contains("state."), "{roleful}");
        // The route table still names the surface the model publishes.
        assert!(
            roleful.contains(".route(\"/api/v1/orders\", get(orders_controller::index))"),
            "{roleful}"
        );

        let custom = service(mxrs_bson::doc! {
            "AuthenticationMicroflow": "Sales.ACT_Authenticate",
        });
        assert!(
            custom.contains("the custom authentication microflow Sales.ACT_Authenticate"),
            "{custom}"
        );
        let unknown = service(mxrs_bson::doc! {
            "AuthenticationTypes": roles(vec!["mxid"]),
        });
        assert!(
            unknown.contains("the authentication type(s) mxid"),
            "{unknown}"
        );
    }

    /// Mendix supplies a microflow's `System.HttpRequest`/`System.HttpResponse`
    /// parameters itself, so the operation never binds them and the boundary
    /// has to — and then answer what the flow built on the response object.
    #[test]
    fn implicit_http_parameters_are_bound_and_the_flow_answers_through_them() {
        let document = mxrs_bson::doc! {
            "$Type": "Rest$PublishedRestService",
            "Name": "API_Service",
            "Path": "api/v1",
            "Documentation": "",
            "Resources": mxrs_bson::build_array(vec![mxrs_bson::Bson::Document(mxrs_bson::doc! {
                "$Type": "Rest$PublishedRestServiceResource",
                "Name": "orders",
                "Operations": mxrs_bson::build_array(vec![
                    mxrs_bson::Bson::Document(mxrs_bson::doc! {
                        "$Type": "Rest$PublishedRestServiceOperation",
                        "HttpMethod": "Get",
                        "Path": "",
                        "Summary": "",
                        "Documentation": "",
                        "Microflow": "Sales.ACT_GetOrder",
                        "Parameters": mxrs_bson::build_array(vec![], 3),
                    }),
                    // The same service, one operation whose microflow declares
                    // neither: its handler keeps answering a document.
                    mxrs_bson::Bson::Document(mxrs_bson::doc! {
                        "$Type": "Rest$PublishedRestServiceOperation",
                        "HttpMethod": "Post",
                        "Path": "",
                        "Summary": "",
                        "Documentation": "",
                        "Microflow": "Sales.ACT_PutOrder",
                        "Parameters": mxrs_bson::build_array(vec![], 3),
                    }),
                ], 3),
            })], 3),
        };
        let http = HashMap::from([(
            "Sales.ACT_GetOrder".to_string(),
            HttpParameters {
                request: Some("Request".to_string()),
                response: Some("HttpResponse".to_string()),
            },
        )]);

        let service = published_service(&document, "Sales", |_| ModuleRoot::Authored, &http)
            .expect("routable service");
        let rendered = rendered_service(service, &HashMap::new());

        assert!(rendered.contains("use axum::http::{Uri};"), "{rendered}");
        assert!(
            rendered.contains("use axum::response::Response;"),
            "{rendered}"
        );
        assert!(rendered.contains("HttpObjects"), "{rendered}");
        assert!(
            rendered.contains(
                "pub async fn index(\n    State(state): State<AppState>,\n    uri: Uri,\n) -> Result<Response, ApiError> {"
            ),
            "{rendered}"
        );
        assert!(
            rendered.contains(
                "        HttpObjects::none().with_request(\"Request\", uri.to_string()).with_response(\"HttpResponse\"),"
            ),
            "{rendered}"
        );
        assert!(rendered.contains("state.call_operation("), "{rendered}");
        // The operation whose microflow declares none is untouched: no URI to
        // extract, and a document rather than a whole response.
        assert!(
            rendered.contains(
                "pub async fn create(\n    State(state): State<AppState>,\n) -> Result<Json<Value>, ApiError> {"
            ),
            "{rendered}"
        );
        assert!(
            rendered.contains("state.call(\"Sales.ACT_PutOrder\", arguments, &caller)"),
            "{rendered}"
        );
    }

    /// A module the project installed is not a module the project owns:
    /// `FromAppStore` puts it under `src/packages/`, its types stay available
    /// so authored code can name them, and it declares nothing — because an
    /// edit there is lost the next time the module is upgraded.
    #[test]
    fn installed_modules_land_in_packages_and_declare_nothing() {
        let generated = tempfile::tempdir().unwrap();
        let authored = GeneratedModule {
            name: "Sales".to_string(),
            entities: vec![("order".to_string(), "// order\n".to_string())],
            services: vec![("ping_service".to_string(), "// ping\n".to_string())],
            security: Some("// roles\n".to_string()),
            ..GeneratedModule::default()
        };
        let installed = GeneratedModule {
            name: "SweetAlert2".to_string(),
            root: ModuleRoot::Package,
            version: "2.1.0".to_string(),
            entities: vec![("alert".to_string(), "// alert\n".to_string())],
            dtos: vec![("dummy_object".to_string(), "// dummy\n".to_string())],
            documents: vec![("limit".to_string(), "// constant\n".to_string())],
            services: vec![("act_show".to_string(), "// show\n".to_string())],
            security: Some("// roles\n".to_string()),
            markers: Some("// markers\n".to_string()),
            ..GeneratedModule::default()
        };
        let modules = std::collections::BTreeMap::from([
            ("sales".to_string(), authored),
            ("sweet_alert2".to_string(), installed),
        ]);

        write_modules_layer(generated.path(), &modules).unwrap();
        let read = |path: &str| std::fs::read_to_string(generated.path().join(path)).unwrap();

        // The two trees are separate, and each module is in exactly one.
        let authored_index = read("src/domain/entities/mod.rs");
        assert!(
            authored_index.contains("pub mod sales;"),
            "{authored_index}"
        );
        assert!(!authored_index.contains("sweet_alert2"), "{authored_index}");
        // An authored concept is a list of modules and nothing else: each
        // declaration registers itself, so there is no `apply` to keep in
        // step with the files.
        assert!(!authored_index.contains("apply"), "{authored_index}");
        assert_eq!(
            read("src/domain/entities/sales/mod.rs"),
            "//! The sales module's entities.\n\npub mod order;\n"
        );
        let package_index = read("src/packages/mod.rs");
        assert!(
            package_index.contains("/// `SweetAlert2` 2.1.0, from the Marketplace."),
            "{package_index}"
        );
        assert!(
            package_index.contains("pub mod sweet_alert2;"),
            "{package_index}"
        );
        // No apply anywhere in the package tree: the snapshot declares it.
        assert!(!package_index.contains("apply"), "{package_index}");
        let package = read("src/packages/sweet_alert2/mod.rs");
        assert!(!package.contains("apply"), "{package}");
        assert!(
            package.contains("installed from the Marketplace at 2.1.0"),
            "{package}"
        );
        assert!(
            !read("src/packages/sweet_alert2/domain/mod.rs").contains("apply"),
            "no apply in a package's domain index",
        );
        assert!(
            !read("src/packages/sweet_alert2/domain/entities/mod.rs").contains("apply"),
            "no apply in a package's concept index",
        );

        // Types and contracts are kept; declarations are not written at all.
        assert!(
            generated
                .path()
                .join("src/packages/sweet_alert2/domain/entities/alert.rs")
                .is_file()
        );
        assert!(
            generated
                .path()
                .join("src/packages/sweet_alert2/dto/dummy_object.rs")
                .is_file()
        );
        assert!(
            generated
                .path()
                .join("src/packages/sweet_alert2/markers.rs")
                .is_file()
        );
        for declaration in ["domain/documents", "domain/security.rs", "services"] {
            assert!(
                !generated
                    .path()
                    .join(format!("src/packages/sweet_alert2/{declaration}"))
                    .exists(),
                "a package must not carry {declaration}",
            );
        }
        // The authored module keeps all of it.
        assert_eq!(
            read("src/services/sales/mod.rs"),
            "//! The sales module's services.\n\npub mod ping_service;\n",
            "an authored module still declares its microflows",
        );
        assert!(
            !generated.path().join("src/domain/markers").exists(),
            "an authored module's names live in the files that declare them",
        );
        assert!(
            generated
                .path()
                .join("src/domain/module_security/sales.rs")
                .is_file()
        );
    }

    fn value_element(attribute: &str, json_path: &str) -> mxrs_bson::Bson {
        mxrs_bson::Bson::Document(mxrs_bson::doc! {
            "$Type": "ExportMappings$ValueMappingElement",
            "ElementType": "Value",
            "Attribute": attribute,
            "Converter": "",
            "JsonPath": json_path,
            // Studio Pro's caption, which can differ in case from the key the
            // document carries — the JSON path is the key.
            "ExposedName": "ignored",
        })
    }

    fn object_element(
        entity: &str,
        association: &str,
        json_path: &str,
        occurs: i32,
        children: Vec<mxrs_bson::Bson>,
    ) -> mxrs_bson::Bson {
        mxrs_bson::Bson::Document(mxrs_bson::doc! {
            "$Type": "ExportMappings$ObjectMappingElement",
            "ElementType": "Object",
            "Entity": entity,
            "Association": association,
            "CustomHandlerCall": "",
            "JsonPath": json_path,
            "MinOccurs": 0i32,
            "MaxOccurs": occurs,
            "ObjectHandling": "Find",
            "Children": mxrs_bson::build_array(children, 3),
        })
    }

    /// An export mapping lowers into the element tree the model declares. The
    /// JSON path names each key; `MaxOccurs` — not the path — decides whether
    /// a key answers an array, because Mendix writes the `(Array)` marker only
    /// at the root and wraps a repeated nested object in an entity-less
    /// element that adds no level to the document.
    #[test]
    fn export_mappings_lower_into_a_declaration_the_boundary_can_apply() {
        let mapping = mxrs_bson::doc! {
            "$Type": "ExportMappings$ExportMapping",
            "Name": "EM_Order_List",
            "Documentation": "The order feed.",
            "NullValueOption": "SendAsNil",
            "Elements": mxrs_bson::build_array(vec![object_element(
                "Sales.Order",
                "",
                "(Array)|(Object)",
                -1,
                vec![
                    value_element("Sales.Order.OrderNumber", "(Array)|(Object)|OrderNumber"),
                    value_element("Sales.Order.OrderDate", "(Array)|(Object)|placed_at"),
                    // One customer per order: a single element carrying the
                    // entity, the association and `MaxOccurs: 1`.
                    object_element(
                        "Sales.Customer",
                        "Sales.Order_Customer",
                        "(Array)|(Object)|customer",
                        1,
                        vec![value_element(
                            "Sales.Customer.Name",
                            "(Array)|(Object)|customer|Name",
                        )],
                    ),
                    // Many tickets per order: the entity-less wrapper naming
                    // the key, around the element that does the work.
                    object_element(
                        "",
                        "",
                        "(Array)|(Object)|tickets",
                        1,
                        vec![object_element(
                            "Sales.Ticket",
                            "Sales.Ticket_Customer",
                            "(Array)|(Object)|tickets|(Object)",
                            -1,
                            vec![value_element(
                                "Sales.Ticket.Priority",
                                "(Array)|(Object)|tickets|(Object)|priority",
                            )],
                        )],
                    ),
                ],
            )], 3),
        };

        let lowered = export_mapping(&mapping, "Sales").expect("expressible mapping");
        assert_eq!(lowered.qualified_name, "Sales.EM_Order_List");
        assert_eq!(lowered.file_stem, "em_order_list");
        assert!(lowered.send_nils);
        assert!(lowered.root.multiple);
        assert!(lowered.root.association.is_empty() && lowered.root.key.is_empty());
        assert_eq!(lowered.root.values[1].key, "placed_at");
        assert_eq!(lowered.root.values[1].attribute, "OrderDate");
        assert_eq!(lowered.root.children[0].key, "customer");
        assert_eq!(lowered.root.children[0].association, "Sales.Order_Customer");
        assert!(!lowered.root.children[0].multiple);
        // The wrapper collapsed: one level, under the key it named.
        assert_eq!(lowered.root.children[1].key, "tickets");
        assert_eq!(lowered.root.children[1].entity, "Sales.Ticket");
        assert_eq!(
            lowered.root.children[1].association,
            "Sales.Ticket_Customer"
        );
        assert!(lowered.root.children[1].multiple);

        let rendered = render_export_mapping(&lowered);
        assert!(
            rendered.contains(
                "//! `Sales.EM_Order_List` — the JSON document the operations\n//! declaring it answer with: an array of `Sales.Order`."
            ),
            "{rendered}"
        );
        assert!(rendered.contains("//! The order feed."), "{rendered}");
        assert!(
            rendered.contains("ObjectMapping::array(\"Sales.Order\")"),
            "{rendered}"
        );
        assert!(
            rendered.contains(".attribute(\"OrderNumber\")"),
            "{rendered}"
        );
        assert!(
            rendered.contains(".value(\"placed_at\", \"OrderDate\")"),
            "{rendered}"
        );
        assert!(
            rendered.contains(
                ".child(\n                \"customer\",\n                \"Sales.Order_Customer\",\n                ObjectMapping::object(\"Sales.Customer\")\n                    .attribute(\"Name\"),\n            )"
            ),
            "{rendered}"
        );
        assert!(
            rendered.contains(
                ".child(\n                \"tickets\",\n                \"Sales.Ticket_Customer\",\n                ObjectMapping::array(\"Sales.Ticket\")"
            ),
            "{rendered}"
        );
        assert!(rendered.contains(".sending_nils()"), "{rendered}");
    }

    /// A mapping the declaration cannot express steps aside whole, so the
    /// operations that declare it keep answering the stored attributes rather
    /// than a half-applied document.
    #[test]
    fn inexpressible_export_mappings_are_left_out_entirely() {
        let mapping = |element: mxrs_bson::Document| {
            mxrs_bson::doc! {
                "$Type": "ExportMappings$ExportMapping",
                "Name": "EM_Order",
                "Documentation": "",
                "NullValueOption": "LeaveOutElement",
                "Elements": mxrs_bson::build_array(vec![mxrs_bson::Bson::Document(element)], 3),
            }
        };
        let root = |children: Vec<mxrs_bson::Bson>| {
            let mxrs_bson::Bson::Document(element) =
                object_element("Sales.Order", "", "(Object)", 1, children)
            else {
                unreachable!("object_element always builds a document")
            };
            element
        };
        let number = || value_element("Sales.Order.OrderNumber", "(Object)|OrderNumber");

        assert!(export_mapping(&mapping(root(vec![number()])), "Sales").is_some());
        // An XML-only element tree has no JSON keys to answer under.
        assert!(
            export_mapping(
                &mapping(root(vec![value_element("Sales.Order.OrderNumber", "")])),
                "Sales"
            )
            .is_none()
        );
        // A converter rewrites the value on its way out.
        let converted = mxrs_bson::Bson::Document(mxrs_bson::doc! {
            "$Type": "ExportMappings$ValueMappingElement",
            "ElementType": "Value",
            "Attribute": "Sales.Order.OrderNumber",
            "Converter": "Sales.PadNumber",
            "JsonPath": "(Object)|OrderNumber",
        });
        assert!(export_mapping(&mapping(root(vec![converted])), "Sales").is_none());
        // A custom Java handler shapes the element itself.
        let mut handled = root(vec![number()]);
        handled.insert("CustomHandlerCall", "Sales.ShapeOrder");
        assert!(export_mapping(&mapping(handled), "Sales").is_none());
        // An element with neither attributes nor children describes nothing.
        assert!(export_mapping(&mapping(root(Vec::new())), "Sales").is_none());
        // Multiplicity the element does not state is multiplicity we would
        // have to guess, and it decides the document's shape.
        let mut unstated = root(vec![number()]);
        unstated.remove("MaxOccurs");
        assert!(export_mapping(&mapping(unstated), "Sales").is_none());
        // The root path and `MaxOccurs` disagree about the shape.
        let mut disagreeing = root(vec![number()]);
        disagreeing.insert("MaxOccurs", -1i32);
        assert!(export_mapping(&mapping(disagreeing), "Sales").is_none());
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
                root: ModuleRoot::Authored,
                module_stem: "sales".to_string(),
                file_stem: "order".to_string(),
                type_name: "Order".to_string(),
                dto: false,
                attributes: HashMap::new(),
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
            &std::collections::BTreeSet::new(),
            &typed,
        );
        assert_eq!(modules.len(), 1);
        assert_eq!(modules[0].actions.len(), 1);

        let port = render_action_port_file(&modules[0]);
        assert!(port.contains("pub trait CommitInBatches"), "{port}");
        assert!(
            port.contains(
                "fn call(&self, items: Option<Vec<ObjectHandle<crate::domain::entities::sales::order::Order>>>, batch_size: Option<i64>) -> Result<Option<bool>, ServiceError>;"
            ),
            "{port}"
        );
        assert!(!port.contains("Generic"), "{port}");

        let registry = render_action_registry_file(&modules[0]);
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

    /// An enumeration is the Rust enum that declares it; the whole file is
    /// pinned like the entity render.
    #[test]
    fn an_enumeration_file_is_its_enum_pinned_whole() {
        let values = vec![
            (
                "Open".to_string(),
                vec![("en_US".to_string(), "Open".to_string())],
            ),
            (
                "_10_Minutes".to_string(),
                vec![("pt_BR".to_string(), "Dez minutos".to_string())],
            ),
            (
                "Closed".to_string(),
                vec![
                    ("en_US".to_string(), "Closed".to_string()),
                    ("pt_BR".to_string(), "Fechado".to_string()),
                ],
            ),
            ("Silent".to_string(), vec![]),
            (
                "closed".to_string(),
                vec![("zh-Hans".to_string(), "关闭".to_string())],
            ),
        ];
        let (rendered, type_name) =
            render_enumeration_file("Sales", "ENUM_Status", "Lifecycle.", &values, false);
        assert_eq!(type_name, "ENUMStatus");
        assert_eq!(
            rendered,
            r#"use mxrs::prelude::*;

/// Lifecycle.
#[enumeration(module = "Sales", name = "ENUM_Status")]
pub enum ENUMStatus {
    Open,
    #[mxrs(name = "_10_Minutes", caption = "Dez minutos", language = "pt_BR")]
    _10Minutes,
    #[mxrs(captions(en_US = "Closed", pt_BR = "Fechado"))]
    Closed,
    #[mxrs(captions())]
    Silent,
    #[mxrs(name = "closed", captions("zh-Hans" = "关闭"))]
    Closed2,
}
"#
        );

        // An installed module's enumeration is named, not declared, and
        // documentation a comment would change is stated as an option.
        let (rendered, type_name) = render_enumeration_file("Atlas", "Self", "- a list", &[], true);
        assert_eq!(type_name, "SelfEnumeration");
        assert_eq!(
            rendered,
            "use mxrs::prelude::*;\n\n#[enumeration(module = \"Atlas\", name = \"Self\", imported)]\n#[mxrs(documentation = \"- a list\")]\npub enum SelfEnumeration {}\n"
        );
    }

    #[test]
    fn file_stems_never_carry_underscore_runs_rust_warns_about() {
        assert_eq!(
            inner_file_stem("Acme_User__EU__Location"),
            "acme_user_eu_location"
        );
        assert_eq!(inner_file_stem("APIProgram"), "api_program");
        assert_eq!(inner_file_stem("_Legacy_"), "legacy");
        assert_eq!(inner_file_stem("2FA"), "_2_fa");
        assert_eq!(module_stem("API_Rest"), "api_rest");
        assert_eq!(module_stem("Acme__Commons"), "acme_commons");
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
                        view.text_box_with(
                            mxrs_ir::AttributeRef::<sales_markers::Order_Number>::new(),
                            |input| {
                                input.name("numberInput");
                                input.class("form-control");
                            },
                        );
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
            // A published REST service the same way: the model is the only
            // place routes come from, so the generated router must come
            // from a real declaration.
            mpr.insert_unit(
                &sales_id,
                "Documents",
                mxrs_bson::doc! {
                    "$ID": uuid::Uuid::new_v4().to_string(),
                    "$Type": "Rest$PublishedRestService",
                    "Name": "OrderService",
                    "Path": "api/v1",
                    "Documentation": "",
                    "Excluded": false,
                    "ExportLevel": "Hidden",
                    // The model's own answer to who may call: HTTP Basic, and
                    // only a caller holding this module role.
                    "AuthenticationTypes": mxrs_bson::build_array(
                        vec![mxrs_bson::Bson::String("basic".to_string())],
                        1,
                    ),
                    "AllowedRoles": mxrs_bson::build_array(
                        vec![mxrs_bson::Bson::String("Sales.User".to_string())],
                        1,
                    ),
                    "Resources": mxrs_bson::build_array(vec![
                        mxrs_bson::Bson::Document(mxrs_bson::doc! {
                            "$ID": uuid::Uuid::new_v4().to_string(),
                            "$Type": "Rest$PublishedRestServiceResource",
                            "Name": "orders",
                            "Documentation": "",
                            "Operations": mxrs_bson::build_array(vec![mxrs_bson::Bson::Document(mxrs_bson::doc! {
                                "$ID": uuid::Uuid::new_v4().to_string(),
                                "$Type": "Rest$PublishedRestServiceOperation",
                                "HttpMethod": "Get",
                                "Path": "",
                                "Summary": "Ping the order module",
                                "Documentation": "",
                                "Microflow": "Sales.ACT_Ping",
                                "Parameters": mxrs_bson::build_array(vec![], 3),
                            })], 3),
                        }),
                        // An operation that declares an export mapping, so the
                        // generated handler applies the model's document shape
                        // instead of the entity's stored attributes.
                        mxrs_bson::Bson::Document(mxrs_bson::doc! {
                            "$ID": uuid::Uuid::new_v4().to_string(),
                            "$Type": "Rest$PublishedRestServiceResource",
                            "Name": "order",
                            "Documentation": "",
                            "Operations": mxrs_bson::build_array(vec![mxrs_bson::Bson::Document(mxrs_bson::doc! {
                                "$ID": uuid::Uuid::new_v4().to_string(),
                                "$Type": "Rest$PublishedRestServiceOperation",
                                "HttpMethod": "Get",
                                "Path": "",
                                "Summary": "Read one order",
                                "Documentation": "",
                                "Microflow": "Sales.ACT_GetOrder",
                                "ExportMapping": "Sales.EM_Order",
                                "Parameters": mxrs_bson::build_array(vec![], 3),
                            })], 3),
                        }),
                    ], 3),
                },
                None,
            )
            .unwrap();
            // The export mapping the operation above answers through. The DSL
            // has no mapping authoring surface yet, so it arrives the way the
            // model carries it.
            mpr.insert_unit(
                &sales_id,
                "Documents",
                mxrs_bson::doc! {
                    "$ID": uuid::Uuid::new_v4().to_string(),
                    "$Type": "ExportMappings$ExportMapping",
                    "Name": "EM_Order",
                    "Documentation": "The order document.",
                    "Excluded": false,
                    "ExportLevel": "Hidden",
                    "NullValueOption": "LeaveOutElement",
                    "Elements": mxrs_bson::build_array(vec![mxrs_bson::Bson::Document(mxrs_bson::doc! {
                        "$ID": uuid::Uuid::new_v4().to_string(),
                        "$Type": "ExportMappings$ObjectMappingElement",
                        "ElementType": "Object",
                        "Entity": "Sales.Order",
                        "Association": "",
                        "CustomHandlerCall": "",
                        "JsonPath": "(Object)",
                        "MinOccurs": 1i32,
                        "MaxOccurs": 1i32,
                        "ObjectHandling": "Parameter",
                        "Children": mxrs_bson::build_array(vec![
                            mxrs_bson::Bson::Document(mxrs_bson::doc! {
                                "$ID": uuid::Uuid::new_v4().to_string(),
                                "$Type": "ExportMappings$ValueMappingElement",
                                "ElementType": "Value",
                                "Attribute": "Sales.Order.Number",
                                "Converter": "",
                                "JsonPath": "(Object)|Number",
                            }),
                            mxrs_bson::Bson::Document(mxrs_bson::doc! {
                                "$ID": uuid::Uuid::new_v4().to_string(),
                                "$Type": "ExportMappings$ValueMappingElement",
                                "ElementType": "Value",
                                "Attribute": "Sales.Order.SubmittedAt",
                                "Converter": "",
                                "JsonPath": "(Object)|submitted_at",
                            }),
                            mxrs_bson::Bson::Document(mxrs_bson::doc! {
                                "$ID": uuid::Uuid::new_v4().to_string(),
                                "$Type": "ExportMappings$ObjectMappingElement",
                                "ElementType": "Object",
                                "Entity": "Sales.Customer",
                                "Association": "Sales.Order_Customer",
                                "CustomHandlerCall": "",
                                "JsonPath": "(Object)|customer",
                                "MinOccurs": 0i32,
                                "MaxOccurs": 1i32,
                                "ObjectHandling": "Find",
                                "Children": mxrs_bson::build_array(vec![mxrs_bson::Bson::Document(mxrs_bson::doc! {
                                    "$ID": uuid::Uuid::new_v4().to_string(),
                                    "$Type": "ExportMappings$ValueMappingElement",
                                    "ElementType": "Value",
                                    "Attribute": "Sales.Customer.Name",
                                    "Converter": "",
                                    "JsonPath": "(Object)|customer|Name",
                                })], 3),
                            }),
                        ], 3),
                    })], 3),
                },
                None,
            )
            .unwrap();
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
        assert!(generated.join("src/domain/entities/sales/mod.rs").is_file());
        assert!(
            generated
                .join("src/domain/documents/sales/mod.rs")
                .is_file()
        );
        assert!(generated.join("src/domain/security.rs").is_file());
        assert!(generated.join("src/services/mod.rs").is_file());
        assert!(!generated.join("src/application").exists());
        assert!(generated.join("src/domain/dtos/sales/mod.rs").is_file());
        assert!(generated.join("src/services/sales/mod.rs").is_file());
        assert!(generated.join("src/ui/mod.rs").is_file());
        assert!(generated.join("src/ui/nanoflows/sales/mod.rs").is_file());
        assert!(generated.join("src/ui/navigation.rs").is_file());
        assert!(generated.join("src/infrastructure/mod.rs").is_file());
        assert!(!generated.join("src/generated").exists());
        assert!(generated.join("model/imported/manifest.json").is_file());
        // The pages are real, *compiled* Rust source that registers itself
        // (see `page_export`'s doc comment) — the `cargo check`/`cargo run` calls below, plus the
        // rebuilt-project assertions further down, prove it actually
        // contributes to the output `.mpr`, not just that it parses.
        let home = std::fs::read_to_string(generated.join("src/ui/pages/sales/home.rs")).unwrap();
        assert!(
            home.contains("#[page(module = \"Sales\")]\npub fn home(p: &mut PageBuilder) {"),
            "{home}"
        );
        assert!(home.contains("w.text_with(\"Welcome\""));
        assert!(home.contains("w.name("));
        assert!(home.contains("b.close_page()"));
        let detail =
            std::fs::read_to_string(generated.join("src/ui/pages/sales/order_detail.rs")).unwrap();
        assert!(detail.contains("data_view_from_microflow"));
        // The page names the model through the files that declare it: the
        // entity's own accessor, and the flows by their Mendix names.
        assert!(
            detail.contains("use crate::domain::entities::sales::order::Order;"),
            "{detail}"
        );
        assert!(
            detail.contains("w.text_box_with(Order::number(), |w| {"),
            "{detail}"
        );
        assert!(
            detail.contains("MicroflowRef::<ACT_GetOrder>::new()"),
            "{detail}"
        );
        assert!(
            detail.contains("NanoflowRef::<NF_Validate>::new()"),
            "{detail}"
        );
        assert!(detail.contains("b.call_nanoflow"));
        assert!(detail.contains("p.data_grid_2"));
        assert!(detail.contains("p.gallery"));
        assert!(detail.contains("p.combo_box"));
        // Every index, at every level, is a list of what it holds: the
        // declarations register themselves, so no file composes anything.
        assert_eq!(
            std::fs::read_to_string(generated.join("src/ui/pages/sales/mod.rs")).unwrap(),
            "//! The sales module's pages.\n\npub mod home;\npub mod order_detail;\n"
        );
        let domain_source = std::fs::read_to_string(generated.join("src/domain/mod.rs")).unwrap();
        assert!(domain_source.contains("pub mod security;"));
        let services_source =
            std::fs::read_to_string(generated.join("src/services/mod.rs")).unwrap();
        let ui_source = std::fs::read_to_string(generated.join("src/ui/mod.rs")).unwrap();
        assert!(ui_source.contains("pub mod navigation;"));
        // What the application serves over HTTP is a layer of its own.
        assert!(!ui_source.contains("pub mod http;"), "{ui_source}");
        assert!(!generated.join("src/presentation").exists());
        for layer in [&domain_source, &services_source, &ui_source] {
            assert!(!layer.contains("fn apply"), "{layer}");
            assert!(!layer.contains("crate::"), "{layer}");
        }
        assert!(!generated.join("src/composition.rs").exists());
        // The model declares no task queue, so there is no file for them.
        assert!(!generated.join("src/services/task_queues.rs").exists());
        assert!(!services_source.contains("task_queues"));
        // Project security and navigation are the functions that build them,
        // stating only what differs from the builder's own defaults.
        assert_eq!(
            std::fs::read_to_string(generated.join("src/domain/security.rs")).unwrap(),
            r#"//! The project's security. Each module's own roles are declared in
//! `crate::domain::module_security`.

use mxrs::prelude::*;

#[security]
pub fn security(security: &mut SecurityBuilder) {
    security.level(SecurityLevel::CheckNothing);
    security.check_security(false);
    security.clear_roles();
    security.role("Administrator", |role| {
        role.administrator(true);
        role.module_role("System.Administrator");
    });
    security.password_policy(|policy| {
        policy.minimum_length = 12;
    });
}
"#
        );
        assert_eq!(
            std::fs::read_to_string(generated.join("src/ui/navigation.rs")).unwrap(),
            r#"//! The project's navigation profiles.

use mxrs::prelude::*;

#[navigation]
pub fn navigation(navigation: &mut NavigationBuilder) {
    navigation.profile("Responsive", |profile| {
        profile.home_microflow("Sales.ACT_Ping");
        profile.item("Orders", |item| {
            item.microflow("Sales.ACT_Ping");
            item.icon_code(57369);
        });
    });
}
"#
        );
        assert_eq!(
            std::fs::read_to_string(generated.join("src/domain/module_security/sales.rs")).unwrap(),
            "use mxrs::prelude::*;\n\n#[module_roles(module = \"Sales\")]\npub enum Role {\n    /// Can read orders\n    User,\n}\n"
        );
        // The entity is its struct, whole: documentation as comments,
        // the image, index and event handler as options, the association as
        // a field typed by its target. Nothing is restated that the builder
        // already assumes.
        let order_source =
            std::fs::read_to_string(generated.join("src/domain/entities/sales/order.rs")).unwrap();
        assert_eq!(
            order_source,
            r#"use mxrs::prelude::*;

use crate::domain::entities::sales::customer::Customer;

#[entity(module = "Sales")]
#[mxrs(image = "Sales.OrderIcon")]
#[mxrs(stores(owner, created_date))]
#[mxrs(index(number, desc(system(CreatedDate)), include_offline))]
#[mxrs(before_commit("Sales.ACT_Ping", pass_event_object = false))]
pub struct Order {
    /// External order number
    #[mxrs(length = 80, required, unique)]
    pub number: MxString,
    #[mxrs(localize_date = false)]
    pub submitted_at: MxDateTime,
    pub customer: Reference<Customer>,
}
"#
        );
        let report_dto =
            std::fs::read_to_string(generated.join("src/domain/dtos/sales/order_report.rs"))
                .unwrap();
        assert_eq!(
            report_dto,
            r#"use mxrs::prelude::*;

#[view(module = "Sales", source = "Sales.OrderSource")]
pub struct OrderReport {
    #[mxrs(length = 200)]
    pub number: MxString,
}
"#
        );
        // A concept index lists its files and nothing else.
        assert_eq!(
            std::fs::read_to_string(generated.join("src/domain/dtos/sales/mod.rs")).unwrap(),
            "//! The sales module's dtos.\n\npub mod order_report;\n"
        );
        let persistence =
            std::fs::read_to_string(generated.join("src/infrastructure/persistence.rs")).unwrap();
        assert!(
            persistence.contains(
                "#[declaration(module = \"Sales\", stage = ViewSource)]\npub fn sales_view_sources(module: &mut ModuleBuilder) {"
            ),
            "{persistence}"
        );
        assert!(persistence.contains("oql_view_source("), "{persistence}");
        assert!(persistence.contains("\"OrderSource\""), "{persistence}");
        assert!(persistence.contains("Order projection"));
        assert!(persistence.contains("source.excluded(true)"));
        assert!(persistence.contains("ExportLevel::Published"));
        // Documents split per concept, each in its own file, indexed by
        // mod.rs.
        let documents_index =
            std::fs::read_to_string(generated.join("src/domain/documents/mod.rs")).unwrap();
        assert!(documents_index.contains("pub mod sales;"));
        assert!(!documents_index.contains("apply"), "{documents_index}");
        let enumerations_index =
            std::fs::read_to_string(generated.join("src/domain/enumerations/sales/mod.rs"))
                .unwrap();
        assert!(enumerations_index.contains("pub mod status;"));
        // Every enumeration is a Rust enum, whatever its captions, and the
        // entity referencing one uses the type itself — no embedded
        // qualified-name string.
        let status =
            std::fs::read_to_string(generated.join("src/domain/enumerations/sales/status.rs"))
                .unwrap();
        assert_eq!(
            status,
            r#"use mxrs::prelude::*;

/// Order lifecycle
#[enumeration(module = "Sales")]
pub enum Status {
    #[mxrs(captions(en_US = "Open", pt_BR = "Aberto"))]
    Open,
}
"#
        );
        let priority =
            std::fs::read_to_string(generated.join("src/domain/enumerations/sales/priority.rs"))
                .unwrap();
        assert_eq!(
            priority,
            "use mxrs::prelude::*;\n\n#[enumeration(module = \"Sales\")]\npub enum Priority {\n    Low,\n    High,\n}\n"
        );
        let ticket =
            std::fs::read_to_string(generated.join("src/domain/entities/sales/ticket.rs")).unwrap();
        assert!(
            ticket.contains("use crate::domain::enumerations::sales::priority::Priority;"),
            "{ticket}"
        );
        assert!(ticket.contains("#[mxrs(default = \"Low\")]"), "{ticket}");
        assert!(ticket.contains("pub priority: Priority,"), "{ticket}");
        assert!(!ticket.contains("enumeration = "), "{ticket}");
        // The association to the typed Customer entity is a Reference<T>
        // field importing the target struct; the default-shaped name needs
        // no #[mxrs(association = ...)] restatement.
        assert!(
            ticket.contains("use crate::domain::entities::sales::customer::Customer;"),
            "{ticket}"
        );
        assert!(
            ticket.contains("pub customer: Reference<Customer>,"),
            "{ticket}"
        );
        assert!(!ticket.contains("association = "), "{ticket}");
        // One file per document. A constant is the function that builds it.
        let maximum_orders =
            std::fs::read_to_string(generated.join("src/domain/documents/sales/maximum_orders.rs"))
                .unwrap();
        assert_eq!(
            maximum_orders,
            r#"use mxrs::prelude::*;

/// Limit
#[constant(module = "Sales")]
pub fn maximum_orders(constant: &mut ConstantBuilder) {
    constant.value_type(ConstantType::Integer);
    constant.value("25");
    constant.exposed_to_client(true);
}
"#
        );
        // A cryptographic constant never carries its value into source.
        let api_token =
            std::fs::read_to_string(generated.join("src/domain/documents/sales/api_token.rs"))
                .unwrap();
        assert!(api_token.contains("constant.value_from_env(\"MXRS_SALES_APITOKEN\")"));
        assert!(!api_token.contains("super-secret-value"));
        let documents_index =
            std::fs::read_to_string(generated.join("src/domain/documents/sales/mod.rs")).unwrap();
        assert!(
            documents_index.contains("pub mod maximum_orders;")
                && !documents_index.contains("declaration()"),
            "{documents_index}"
        );
        assert!(!generated.join("src/infrastructure/ids.rs").exists());
        assert!(!generated.join("src/infrastructure/imported.rs").exists());
        // No marker file: an entity is named by its struct, a flow by the
        // type its declaration generates, and a flow that stays in the
        // imported model by the file it would be declared in.
        assert!(!generated.join("src/domain/markers").exists());
        assert!(!generated.join("src/infrastructure/markers.rs").exists());
        let microflows =
            std::fs::read_to_string(generated.join("src/services/sales/mod.rs")).unwrap();
        assert!(!microflows.contains("apply"), "{microflows}");
        assert!(microflows.contains("pub mod ping_service;"), "{microflows}");
        assert_eq!(
            std::fs::read_to_string(generated.join("src/services/sales/ping_service.rs")).unwrap(),
            // The attribute says what refers to the flow: here the project's
            // navigation, an entity's event handler and a published service.
            "use mxrs::prelude::*;\n\n#[microflow(\n    ACT,\n    module = \"Sales\",\n    used_by(\"Navigation\", \"Sales.Order\", \"Sales.OrderService\")\n)]\npub fn ping(_flow: &mut FlowBuilder) {}\n"
        );
        let get_order =
            std::fs::read_to_string(generated.join("src/services/sales/get_order_service.rs"))
                .unwrap();
        assert_eq!(
            get_order,
            r#"use mxrs::prelude::*;

use crate::domain::entities::sales::order::Order;

#[microflow(
    ACT,
    module = "Sales",
    uses(Order),
    used_by("Sales.OrderDetail", "Sales.OrderService")
)]
pub fn get_order(flow: &mut FlowBuilder) {
    let value_order = flow.create_object("order", Ref::<Order>::new(), vec![], false);
    flow.return_value(&value_order);
}
"#
        );
        let nanoflows =
            std::fs::read_to_string(generated.join("src/ui/nanoflows/sales/mod.rs")).unwrap();
        assert_eq!(
            nanoflows,
            "//! The sales module's nanoflows.\n\npub mod validate;\n"
        );
        assert_eq!(
            std::fs::read_to_string(generated.join("src/ui/nanoflows/sales/validate.rs")).unwrap(),
            "use mxrs::prelude::*;\n\n#[nanoflow(NF, module = \"Sales\", used_by(\"Sales.OrderDetail\"))]\npub fn validate(_flow: &mut FlowBuilder) {}\n"
        );
        // Runnable microflows surface as a typed service port plus the
        // runtime adapter that implements it. Every entity is a struct now,
        // so a flow returning one is on the typed surface too.
        let ports_index =
            std::fs::read_to_string(generated.join("src/ports/sales/mod.rs")).unwrap();
        assert!(ports_index.contains("pub mod services;"), "{ports_index}");
        let sales_port =
            std::fs::read_to_string(generated.join("src/ports/sales/services.rs")).unwrap();
        assert!(
            sales_port.contains("pub trait SalesServices"),
            "{sales_port}"
        );
        assert!(
            sales_port.contains("fn act_ping(&mut self) -> Result<(), ServiceError>;"),
            "{sales_port}"
        );
        assert!(
            sales_port.contains(
                "Result<Option<ObjectHandle<crate::domain::entities::sales::order::Order>>, ServiceError>;"
            ),
            "{sales_port}"
        );
        let adapter = std::fs::read_to_string(
            generated.join("src/infrastructure/generated/runtime_services.rs"),
        )
        .unwrap();
        assert!(adapter.contains("pub struct RuntimeServices"), "{adapter}");
        assert!(
            adapter
                .contains("impl crate::ports::sales::services::SalesServices for RuntimeServices"),
            "{adapter}"
        );
        assert!(adapter.contains("\"Sales.ACT_Ping\""), "{adapter}");
        // The injected Java action surfaces as an action port with typed
        // registration glue.
        assert!(ports_index.contains("pub mod actions;"), "{ports_index}");
        let sales_actions =
            std::fs::read_to_string(generated.join("src/ports/sales/actions.rs")).unwrap();
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
        let action_registry = std::fs::read_to_string(
            generated.join("src/infrastructure/generated/sales_actions.rs"),
        )
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
        // The published REST service becomes a route table in the owning
        // module's controllers, behind the project-wide state and error
        // types, and a controller for the resource it publishes.
        let route_table =
            std::fs::read_to_string(generated.join("src/controllers/sales/order_service.rs"))
                .unwrap();
        assert!(
            route_table.contains("pub fn router() -> Router<AppState>"),
            "{route_table}"
        );
        assert!(
            route_table.contains("\"/api/v1/orders\", get(orders_controller::index)"),
            "{route_table}"
        );
        // The route table routes; it handles nothing itself.
        assert!(!route_table.contains("async fn"), "{route_table}");
        assert_eq!(
            std::fs::read_to_string(generated.join("src/controllers/sales/mod.rs")).unwrap(),
            "//! The Sales module's published REST services: one route table per\n//! service, one controller per resource.\n\npub mod order_controller;\npub mod order_service;\npub mod orders_controller;\n"
        );
        // One controller per resource the service publishes.
        let service_router =
            std::fs::read_to_string(generated.join("src/controllers/sales/orders_controller.rs"))
                .unwrap();
        assert!(
            service_router.contains("pub async fn index("),
            "{service_router}"
        );
        assert!(
            service_router.contains("/// Calls `Sales.ACT_Ping`."),
            "{service_router}"
        );
        // The operation that declares an export mapping answers through it,
        // from a declaration in the owning module's domain.
        let mapped =
            std::fs::read_to_string(generated.join("src/controllers/sales/order_controller.rs"))
                .unwrap();
        assert!(
            mapped.contains("use crate::domain::mappings::sales::em_order;"),
            "{mapped}"
        );
        assert!(
            mapped.contains("state.call_mapped(") && mapped.contains("&em_order::mapping()"),
            "{mapped}"
        );
        // A controller that applies no mapping imports none.
        assert!(!service_router.contains("mappings"), "{service_router}");
        let mapping =
            std::fs::read_to_string(generated.join("src/domain/mappings/sales/em_order.rs"))
                .unwrap();
        assert!(
            domain_source.contains("pub mod mappings;"),
            "{domain_source}"
        );
        assert!(
            mapping.contains("ObjectMapping::object(\"Sales.Order\")"),
            "{mapping}"
        );
        assert!(mapping.contains(".attribute(\"Number\")"), "{mapping}");
        assert!(
            mapping.contains(".value(\"submitted_at\", \"SubmittedAt\")"),
            "{mapping}"
        );
        assert!(mapping.contains("\"Sales.Order_Customer\""), "{mapping}");
        assert!(!mapping.contains(".sending_nils()"), "{mapping}");
        let http = std::fs::read_to_string(generated.join("src/controllers/mod.rs")).unwrap();
        assert!(
            http.contains("crate::controllers::sales::order_service::router()"),
            "{http}"
        );
        assert!(generated.join("src/controllers/error.rs").is_file());
        // The model requires Basic authentication and names an allowed role,
        // so the guard signs the request in before the model is called and
        // every operation runs as whoever signed in.
        assert!(
            route_table.contains("pub(super) const ALLOWED_ROLES: &[&str] = &[\"Sales.User\"];"),
            "{route_table}"
        );
        assert!(
            service_router
                .contains("let caller = state.basic_caller(&headers, REALM, ALLOWED_ROLES)?;"),
            "{service_router}"
        );
        // A mapped response goes through the runtime, which asks the engine
        // the caller's read rules — never straight at `ExportMapping::apply`,
        // which asks none. The unmapped path asks the same question.
        let state = std::fs::read_to_string(generated.join("src/controllers/state.rs")).unwrap();
        assert!(
            state.contains("runtime.mapped(mapping, &result, Some(caller))"),
            "{state}"
        );
        assert!(
            state.contains("runtime.json(&result, Some(caller))"),
            "{state}"
        );
        assert!(!state.contains("mapping.apply("), "{state}");
        let authentication = std::fs::read_to_string(
            generated.join("src/infrastructure/generated/authentication.rs"),
        )
        .unwrap();
        assert!(
            authentication.contains("self.policy.holds_any_module_role(caller, roles)"),
            "{authentication}"
        );
        // The model's passwords stay in the model: this adapter only forwards
        // a credential pair to the accounts `boot` read, and carries no string
        // literal at all — so no credential can be sitting in it.
        assert!(
            authentication.contains("self.accounts.sign_in(user, password)"),
            "{authentication}"
        );
        assert!(!authentication.contains('"'), "{authentication}");
        let flow_runtime =
            std::fs::read_to_string(generated.join("src/infrastructure/generated/flow_runtime.rs"))
                .unwrap();
        assert!(
            flow_runtime.contains(".apply_export_mapping(&self.store, caller, mapping, value)"),
            "{flow_runtime}"
        );
        // The flow call, the mapping and the unmapped serialization all take
        // the same caller, so they narrow together.
        assert!(
            flow_runtime.contains("arguments, caller.cloned()"),
            "{flow_runtime}"
        );
        assert!(
            flow_runtime.contains("if !self.engine.readable(caller, object) {"),
            "{flow_runtime}"
        );
        let crate_root = std::fs::read_to_string(generated.join("src/lib.rs")).unwrap();
        assert!(crate_root.contains("pub mod infrastructure;"));
        assert!(!crate_root.contains("pub mod generated"));
        // The crate root exposes layers; composition belongs to its own file.
        assert!(crate_root.contains("pub mod services;"));
        assert!(crate_root.contains("pub mod ports;"));
        assert!(!crate_root.contains("pub mod application;"));
        assert!(crate_root.contains("pub mod domain;"));
        assert!(crate_root.contains("pub mod controllers;"));
        assert!(crate_root.contains("pub mod ui;"));
        assert!(!crate_root.contains("pub mod presentation;"));
        assert!(!crate_root.contains("pub mod modules;"));
        // The crate root is the layers and the application, nothing more:
        // no composition function, no aliases for implementation crates.
        assert!(
            crate_root.ends_with(
                "pub mod ui;\n\n#[mxrs::application(version = \"11.12.1\")]\npub struct Application;\n"
            ),
            "{crate_root}"
        );
        assert!(!crate_root.contains("extern crate"), "{crate_root}");
        assert!(!crate_root.contains("fn build"), "{crate_root}");
        // Empty placeholder folders are noise; only what receives generated
        // content exists. `src/ports` and `src/services` are real layers of
        // the tree, so they are expected — the `application` folder they
        // used to share, and the module-first tree before it, must be gone.
        for absent in [
            "src/modules",
            "src/domain/modules",
            "src/application",
            "src/presentation",
            "src/ui/modules",
            "src/controllers/modules",
            "src/infrastructure/repositories",
            "src/infrastructure/database",
            "src/utils",
        ] {
            assert!(
                !generated.join(absent).exists(),
                "{absent} should not exist"
            );
        }
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
