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
//! **First-slice scope, loud not silent about what's outside it**:
//!
//! - **Domain model only** — entities, attributes, associations. Not
//!   microflows: reconstructing structured `create`/`change`/`if`/`call`
//!   statements from a microflow's persisted activity *graph* (arbitrary
//!   branching, not just the linear-plus-one-decision shape a hand-written
//!   `project! {}` body produces) is a real decompiler, out of scope for
//!   this pass. Concretely safe consequence: an exported source file
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
//! `import_cargo_project` additionally detects pages built entirely from
//! `mxrs-dsl`'s native/structural widget vocabulary, emits them as real
//! `pub fn` builders in `src/domain/pages/mod.rs`, and wires each one into
//! `build()` — see `page_export`'s doc comment for the widget vocabulary
//! detected and what still stays opaque.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

#[cfg(test)]
use mxrs_model::Attribute;
use mxrs_model::attribute::AttributeType;
use mxrs_model::entity::Entity;
use mxrs_model::{Association, Module, Project};

mod page_export;
pub use page_export::PageExportReport;

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
    Typegen(#[from] mxrs_typegen::TypegenError),

    #[error("cannot write {path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },

    #[error("Cargo project destination {0} already exists")]
    DestinationExists(String),

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
            | ExportError::Typegen(_)
            | ExportError::Io { .. }
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
    let identity_source = render_identity_table(&modules);
    let (converted_pages, page_export) = page_export::convert_pages(&modules);
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
            "// Compatibility aliases used by generated macro expansions.\nextern crate mxrs as mxrs_dsl;\nextern crate mxrs as mxrs_expr;\nextern crate mxrs as mxrs_ir;\nextern crate mxrs as mxrs_macros;\n\npub mod domain;\npub mod infrastructure;\n\n/// Compatibility facade for projects imported before `generated` became\n/// the explicit infrastructure layer. New code should use `infrastructure`.\n#[deprecated(note = \"use crate::infrastructure\")]\npub mod generated {{\n    pub use crate::infrastructure::ids;\n    pub use crate::infrastructure::imported;\n    pub use crate::infrastructure::markers;\n}}\n\n#[mxrs::application(version = {})]\npub struct Application;\n",
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
        "//! Generated integration boundary for imported Mendix state.\n\npub mod ids;\npub mod imported;\npub mod markers;\n",
    )?;
    write_text(
        &destination.join("src/infrastructure/ids.rs"),
        &format!(
            "//! Stable identities retained from the imported project.\n\npub const PROJECT_ROOT: &str = {};\n\n{identity_source}",
            rust_string(&manifest.root_id),
        ),
    )?;
    write_text(
        &destination.join("src/infrastructure/imported.rs"),
        &render_imported_registry(&manifest),
    )?;
    write_text(
        &destination.join("src/infrastructure/markers.rs"),
        &markers_source,
    )?;
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
        "    flows::apply(&mut project);\n    security::apply(&mut project);\n    navigation::apply(&mut project);\n    project\n}\n",
    );
    source
}

/// Creates the editable flow composition layer. Imported flow graphs remain
/// losslessly snapshot-backed until the user deliberately redeclares them;
/// new typed declarations merge by module and are then upserted by the writer.
fn render_flows_module(mendix_version: &str, modules: &[Module]) -> String {
    let mut source = String::from(
        "//! Cargo-native flow declarations.\n\
         //!\n\
         //! Imported flow graphs listed below remain losslessly backed by\n\
         //! `model/imported` until deliberately redeclared here. This avoids\n\
         //! pretending an incomplete graph decompiler is lossless.\n\n",
    );
    for module in modules {
        let module_name = module.name.as_deref().unwrap_or("Unnamed");
        for flow in &module.microflows {
            if let Some(name) = &flow.name {
                let _ = writeln!(source, "// snapshot-backed microflow: {module_name}.{name}");
            }
        }
        for flow in &module.nanoflows {
            if let Some(name) = &flow.name {
                let _ = writeln!(source, "// snapshot-backed nanoflow: {module_name}.{name}");
            }
        }
    }
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
        "//! Cargo-native project and module security. Unknown native fields\n\
         //! remain preserved by the imported snapshot and writer merge.\n\n\
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
        source.push_str("    let mut security = ::mxrs_ir::ProjectSecurityDecl::default();\n");
        let _ = writeln!(
            source,
            "    security.level = ::mxrs_ir::SecurityLevel::{level};"
        );
        let _ = writeln!(
            source,
            "    security.admin_user_role = {}.to_string();",
            rust_string(admin_role)
        );
        let _ = writeln!(
            source,
            "    security.guest_user_role = {};",
            rust_option_string(guest_role)
        );
        let _ = writeln!(
            source,
            "    security.sign_in_microflow = {};",
            rust_option_string(sign_in)
        );
        source.push_str("    security.user_roles = vec![\n");
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
        source.push_str("    ];\n");
        if let Ok(policy) = document.get_document("PasswordPolicySettings") {
            let _ = writeln!(
                source,
                "    security.password_policy = ::mxrs_ir::PasswordPolicyDecl {{ minimum_length: {}, require_digit: {}, require_mixed_case: {}, require_symbol: {} }};",
                policy.get_i32("MinimumLength").unwrap_or(6),
                policy.get_bool("RequireDigit").unwrap_or(true),
                policy.get_bool("RequireMixedCase").unwrap_or(true),
                policy.get_bool("RequireSymbol").unwrap_or(false),
            );
        }
        source.push_str("    project.security = Some(security);\n");
    } else {
        source.push_str("    // Project security remains snapshot-backed (unrecognized or absent security level).\n");
    }
    source.push_str("}\n");
    source
}

fn render_navigation_module(navigation: &mxrs_model::Navigation) -> String {
    let mut source = String::from(
        "//! Cargo-native navigation profiles. Unknown profile fields remain\n\
         //! preserved by the imported snapshot and writer merge.\n\n\
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
        rust_option_string(item.icon.as_deref()),
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
    for module in modules {
        let module_name = module.name.as_deref().unwrap_or("Unnamed");
        for entity in module.entities() {
            if let (Some(id), Some(name)) = (entity.id.as_deref(), entity.name.as_deref()) {
                qualified_by_id.insert(id, format!("{module_name}.{name}"));
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
                let Some(target) = target else {
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
                    Some(EntityManifest {
                        name,
                        attributes,
                        associations,
                    })
                })
                .collect::<Vec<_>>();
            entities.sort_by(|left, right| left.name.cmp(&right.name));

            let mut microflows = module
                .microflows
                .iter()
                .filter_map(|flow| flow.name.clone())
                .collect::<Vec<_>>();
            microflows.sort();
            let mut nanoflows = module
                .nanoflows
                .iter()
                .filter_map(|flow| flow.name.clone())
                .collect::<Vec<_>>();
            nanoflows.sort();

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
        "# {project_name}\n\nCargo-native Mendix project imported by `mxrs`. Editable concepts live under `src/domain/`; generated IDs, public marker types, and the lossless snapshot registry live under `src/infrastructure/`. `model/imported/` retains model concepts that are not typed yet.\n\n`src/domain/flows/mod.rs` is the opt-in source of truth for Cargo-native microflows and nanoflows. Imported graphs stay snapshot-backed until deliberately redeclared, so unsupported graph shapes are never silently approximated.\n\n```sh\ncargo check\ncargo test\ncargo mxrs diff\ncargo mxrs build --output build/{project_name}.mpr\n```\n\nThe initial typed domain projection reported {gaps} feature(s) still backed by the generated snapshot.\n{pages_note}"
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

fn io_error(path: &Path, source: std::io::Error) -> ExportError {
    ExportError::Io {
        path: path.display().to_string(),
        source,
    }
}

fn render_identity_table(modules: &[Module]) -> String {
    let mut identities = Vec::<(String, String)>::new();
    for module in modules {
        let module_name = module.name.as_deref().unwrap_or("Unnamed");
        identities.push((module_name.to_string(), module.id.clone()));
        if let Some(domain) = &module.domain_model {
            if let Some(id) = &domain.id {
                identities.push((format!("{module_name}.$domain"), id.clone()));
            }
            for entity in &domain.entities {
                let entity_name = entity.name.as_deref().unwrap_or("Unnamed");
                let entity_path = format!("{module_name}.{entity_name}");
                if let Some(id) = &entity.id {
                    identities.push((entity_path.clone(), id.clone()));
                }
                for attribute in &entity.attributes {
                    if let (Some(id), Some(name)) = (&attribute.id, &attribute.name) {
                        identities.push((format!("{entity_path}.{name}"), id.clone()));
                    }
                }
            }
            for association in domain.all_associations() {
                if let (Some(id), Some(name)) = (&association.id, &association.name) {
                    identities.push((format!("{module_name}.{name}"), id.clone()));
                }
            }
        }
        for page in &module.pages {
            if let (Some(id), Some(name)) = (&page.id, &page.name) {
                identities.push((format!("{module_name}.page:{name}"), id.clone()));
            }
        }
        for (kind, documents) in [
            ("microflow", &module.microflows),
            ("nanoflow", &module.nanoflows),
            ("rule", &module.rules),
        ] {
            for document in documents {
                if let (Some(id), Some(name)) = (&document.id, &document.name) {
                    identities.push((format!("{module_name}.{kind}:{name}"), id.clone()));
                }
            }
        }
        for role in &module.module_roles {
            if let (Some(id), Some(name)) = (&role.id, &role.name) {
                identities.push((format!("{module_name}.role:{name}"), id.clone()));
            }
        }
        for artifact in &module.artifact_units {
            if let (Ok(id), Ok(kind)) = (artifact.get_str("$ID"), artifact.get_str("$Type")) {
                let name = artifact.get_str("Name").unwrap_or("Unnamed");
                identities.push((format!("{module_name}.{kind}:{name}"), id.to_string()));
            }
        }
    }
    identities.sort();
    identities.dedup();

    let mut source = String::from(
        "/// Qualified model path to stable Mendix identity.\n\
         pub const MODEL_IDS: &[(&str, &str)] = &[\n",
    );
    for (path, id) in identities {
        let _ = writeln!(
            source,
            "    ({}, {}),",
            rust_string(&path),
            rust_string(&id)
        );
    }
    source.push_str("];\n");
    source
}

fn render_imported_registry(manifest: &mxrs_project::ImportedProjectManifest) -> String {
    let mut source = String::from(
        "//! Lossless model documents retained until they gain a friendly Rust representation.\n\
         //! This registry is generated; edit the typed modules under `src/` instead.\n\n\
         use mxrs::ImportedDocumentRef;\n\n\
         pub const MANIFEST: &str = include_str!(\"../../model/imported/manifest.json\");\n\n\
         pub const DOCUMENTS: &[ImportedDocumentRef] = &[\n",
    );
    for unit in &manifest.units {
        let option = |value: Option<&str>| match value {
            Some(value) => format!("Some({})", rust_string(value)),
            None => "None".to_string(),
        };
        let _ = writeln!(
            source,
            "    ImportedDocumentRef {{ unit_id: {}, container_id: {}, containment_name: {}, native_type: {}, name: {}, qualified_name: {}, file: {} }},",
            rust_string(&unit.unit_id),
            rust_string(&unit.container_id),
            rust_string(&unit.containment_name),
            rust_string(&unit.native_type),
            option(unit.name.as_deref()),
            option(unit.qualified_name.as_deref()),
            rust_string(&unit.file),
        );
    }
    source.push_str("];\n");
    source
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
                target.contains('.') || entity_qualified_name_by_id.contains_key(target)
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
    let _ = writeln!(
        out,
        "//! Domain model only (entities/attributes/associations) — see"
    );
    let _ = writeln!(
        out,
        "//! `mxrs-exporter`'s crate doc for what's not round-tripped yet"
    );
    let _ = writeln!(
        out,
        "//! (microflows, association Owner/StorageFormat, entity indexes/"
    );
    let _ = writeln!(out, "//! access rules/lifecycle).");
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
                    .filter(|v| !v.is_empty())
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
            if id_or_name.contains('.') {
                Some(id_or_name.to_string())
            } else {
                entity_qualified_name_by_id.get(id_or_name).cloned()
            }
        });
        let Some(target) = target else {
            let _ = writeln!(
                out,
                "                // TODO: association {assoc_name:?} has an unresolvable target, skipped"
            );
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
        let target = build_directory.path().join("cargo-target");
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
        assert!(domain_source.contains("pub mod flows;"));
        assert!(domain_source.contains("pub mod security;"));
        assert!(domain_source.contains("pub mod navigation;"));
        assert!(domain_source.contains("flows::apply(&mut project);"));
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
        let identities =
            std::fs::read_to_string(generated.join("src/infrastructure/ids.rs")).unwrap();
        assert!(identities.contains("Sales.Order.Number"));
        assert!(identities.contains("Sales.microflow:ACT_Ping"));
        let opaque =
            std::fs::read_to_string(generated.join("src/infrastructure/imported.rs")).unwrap();
        assert!(opaque.contains("Microflows$Microflow"));
        assert!(opaque.contains("Some(\"ACT_Ping\")"));
        let markers =
            std::fs::read_to_string(generated.join("src/infrastructure/markers.rs")).unwrap();
        assert!(markers.contains("pub struct Order;"));
        assert!(markers.contains("pub struct Order_Number;"));
        assert!(markers.contains("impl mxrs_ir::MicroflowMarker for ACT_Ping"));
        assert!(markers.contains("impl mxrs_ir::MicroflowMarker for ACT_GetOrder"));
        assert!(markers.contains("impl mxrs_ir::NanoflowMarker for NF_Validate"));
        let flows = std::fs::read_to_string(generated.join("src/domain/flows/mod.rs")).unwrap();
        assert!(flows.contains("snapshot-backed microflow: Sales.ACT_Ping"));
        assert!(flows.contains("target.nanoflows.extend(declared.nanoflows)"));
        let security =
            std::fs::read_to_string(generated.join("src/domain/security/mod.rs")).unwrap();
        assert!(security.contains("ProjectSecurityDecl::default()"));
        assert!(security.contains("security.user_roles = vec!"));
        let navigation =
            std::fs::read_to_string(generated.join("src/domain/navigation/mod.rs")).unwrap();
        assert!(navigation.contains("project.navigation = Some"));
        let crate_root = std::fs::read_to_string(generated.join("src/lib.rs")).unwrap();
        assert!(crate_root.contains("pub mod infrastructure;"));
        assert!(crate_root.contains("pub mod generated"));

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
        assert!(rebuilt.all_units().unwrap().into_iter().any(|unit| {
            let document = rebuilt.parse_contents(&unit).unwrap();
            document.get_str("$Type").ok() == Some("Microflows$Microflow")
                && document.get_str("Name").ok() == Some("ACT_Ping")
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
