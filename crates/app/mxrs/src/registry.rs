//! Self-registration of declarations.
//!
//! An application does not list what it contains. Each declaration —
//! `#[mxrs::entity]`, `#[mxrs::microflow]`, a page — submits itself here, and
//! `#[mxrs::application]` assembles the model from whatever its own crate
//! submitted. Adding an artifact is therefore one file and one `pub mod`
//! line; there is no aggregator to keep in step with the tree.

use mxrs_ir::ProjectDecl;

/// Where a declaration takes its place when the model is assembled.
///
/// The order is the order the layers used to be composed in — domain first,
/// then application, infrastructure and the user interface — so a project reads
/// the same to the writer whether it lists its declarations or registers
/// them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Stage {
    /// Constants, regular expressions, scheduled events and menus.
    Document,
    /// Persistable entities, data transfer objects and views.
    Entity,
    Enumeration,
    /// One module's roles.
    ModuleSecurity,
    /// Project security: user roles and the password policy.
    Security,
    /// Demo users. They join the project security declared before them,
    /// or — in a project that declares none — the one its model stores.
    DemoUser,
    TaskQueue,
    Microflow,
    /// The OQL documents views read from.
    ViewSource,
    Layout,
    Nanoflow,
    Navigation,
    /// Items declared apart from the navigation profile they join.
    NavigationItem,
    Page,
}

/// One declaration an application crate contributes to its model.
pub struct Declaration {
    stage: Stage,
    module_path: &'static str,
    file: &'static str,
    line: u32,
    apply: fn(&mut ProjectDecl),
}

impl Declaration {
    /// Called by the declaration macros; `module_path`, `file` and `line`
    /// are the expansion site's.
    pub const fn new(
        stage: Stage,
        module_path: &'static str,
        file: &'static str,
        line: u32,
        apply: fn(&mut ProjectDecl),
    ) -> Self {
        Self {
            stage,
            module_path,
            file,
            line,
            apply,
        }
    }

    pub fn stage(&self) -> Stage {
        self.stage
    }

    /// The Rust module the declaration was written in.
    pub fn module_path(&self) -> &'static str {
        self.module_path
    }

    /// The crate the declaration belongs to.
    pub fn crate_name(&self) -> &'static str {
        crate_of(self.module_path)
    }

    pub fn file(&self) -> &'static str {
        self.file
    }

    pub fn line(&self) -> u32 {
        self.line
    }

    pub fn apply(&self, project: &mut ProjectDecl) {
        (self.apply)(project);
    }
}

inventory::collect!(Declaration);

/// A nanoflow Rust names so its pages can call it, which the frontend
/// declares (`mxrs::frontend_flows!`).
pub struct FrontendMarker {
    module_path: &'static str,
    file: &'static str,
    line: u32,
    module: &'static str,
    name: &'static str,
}

impl FrontendMarker {
    /// Called by `mxrs::frontend_flows!`; `module_path`, `file` and `line`
    /// are the expansion site's.
    pub const fn new(
        module_path: &'static str,
        file: &'static str,
        line: u32,
        module: &'static str,
        name: &'static str,
    ) -> Self {
        Self {
            module_path,
            file,
            line,
            module,
            name,
        }
    }
}

inventory::collect!(FrontendMarker);

/// A flow the source keeps from the imported model rather than declaring
/// it (`mxrs::imported!`): its type, module and name.
pub struct ImportedMarker {
    native_type: &'static str,
    module: &'static str,
    name: &'static str,
}

impl ImportedMarker {
    /// Called by `mxrs::imported!`.
    pub const fn new(native_type: &'static str, module: &'static str, name: &'static str) -> Self {
        Self {
            native_type,
            module,
            name,
        }
    }
}

inventory::collect!(ImportedMarker);

/// Every flow the program's source keeps from the imported model, as
/// `(type, module, name)`.
pub fn imported_markers() -> std::collections::BTreeSet<(String, String, String)> {
    inventory::iter::<ImportedMarker>
        .into_iter()
        .map(|marker| {
            (
                marker.native_type.to_string(),
                marker.module.to_string(),
                marker.name.to_string(),
            )
        })
        .collect()
}

/// The crate a `module_path!()` value belongs to.
pub fn crate_of(module_path: &str) -> &str {
    module_path.split("::").next().unwrap_or(module_path)
}

/// Every declaration `crate_name` registered, in assembly order.
///
/// The linker decides the order declarations are discovered in, so it is
/// never relied on: declarations sort by stage, then by where they were
/// written. That makes the assembled model a function of the source tree
/// alone — the same on every machine and in every build.
pub fn declarations(crate_name: &str) -> Vec<&'static Declaration> {
    let mut declarations = inventory::iter::<Declaration>
        .into_iter()
        .filter(|declaration| declaration.crate_name() == crate_name)
        .collect::<Vec<_>>();
    // `file!()` spells a path with the separator of the machine that
    // compiled it; compared as written, `a\b.rs` and `a/b.rs` would order
    // the same tree differently on Windows.
    declarations.sort_by_cached_key(|declaration| {
        (
            declaration.stage,
            declaration.file.replace('\\', "/"),
            declaration.line,
        )
    });
    declarations
}

/// Applies every declaration `crate_name` registered to `project`.
pub fn apply(crate_name: &str, project: &mut ProjectDecl) {
    for declaration in declarations(crate_name) {
        declaration.apply(project);
    }
}

/// Applies every declaration `crate_name` registered to `project`, with
/// what the frontend declares in its place among them: its navigation at
/// the navigation stage, so the items Rust pages add for themselves join
/// it. A navigation the frontend declares is the project's only one.
pub fn apply_with_frontend(
    crate_name: &str,
    project: &mut ProjectDecl,
    frontend: mxrs_frontend::FrontendDecl,
    installed: &std::collections::BTreeSet<String>,
) -> Result<(), mxrs_frontend::FrontendError> {
    // A nanoflow Rust names as the frontend's is one the frontend declares:
    // a page would otherwise call a nanoflow that was renamed or removed.
    let mut markers: Vec<&FrontendMarker> = inventory::iter::<FrontendMarker>
        .into_iter()
        .filter(|marker| crate_of(marker.module_path) == crate_name)
        .collect();
    markers.sort_by_key(|marker| (marker.file.replace('\\', "/"), marker.line, marker.name));
    for marker in markers {
        let declared = frontend
            .nanoflows
            .iter()
            .any(|(module, nanoflow)| module == marker.module && nanoflow.name == marker.name);
        if !declared {
            return Err(mxrs_frontend::FrontendError::Shape {
                path: marker.file.to_string(),
                line: marker.line as usize,
                detail: format!(
                    "it names the nanoflow {}.{} as the frontend's, and no service in \
                     frontend/src/services declares it; declare it there, or take its name \
                     out of here and out of what calls it",
                    marker.module, marker.name
                ),
            });
        }
    }
    let origins = frontend.nanoflow_origins;
    let forms = frontend.forms;
    let form_origins = frontend.form_origins;
    let mut navigation = frontend.navigation;
    let mut nanoflows = Some(frontend.nanoflows);
    let declarations = declarations(crate_name);
    if navigation.is_some() {
        let rust = declarations
            .iter()
            .find(|declaration| declaration.stage == Stage::Navigation);
        if let Some(rust) = rust {
            return Err(mxrs_frontend::FrontendError::Duplicate(format!(
                "{}:{}",
                rust.file, rust.line
            )));
        }
        if project.navigation.is_some() {
            return Err(mxrs_frontend::FrontendError::Duplicate(
                "the project's entry point".to_string(),
            ));
        }
    }
    for declaration in declarations {
        if declaration.stage > Stage::Nanoflow
            && let Some(declared) = nanoflows.take()
        {
            add_nanoflows(project, declared, &origins, installed)?;
        }
        if declaration.stage > Stage::Navigation
            && let Some(declared) = navigation.take()
        {
            project.navigation = Some(declared);
        }
        declaration.apply(project);
    }
    if let Some(declared) = nanoflows {
        add_nanoflows(project, declared, &origins, installed)?;
    }
    if let Some(declared) = navigation {
        project.navigation = Some(declared);
    }
    add_forms(project, forms, &form_origins, installed)
}

/// The module the frontend declares something of. One the project
/// installed — the imported model says which — is written as installed:
/// what the frontend states of it, and the rest as the model holds it.
/// Any other is the project's own, whether or not Rust declared it yet.
fn module_of<'a>(
    project: &'a mut ProjectDecl,
    name: &str,
    installed: &std::collections::BTreeSet<String>,
) -> &'a mut mxrs_ir::ModuleDecl {
    let module = project.module_mut(name);
    if installed.contains(name) {
        module.installed = true;
    }
    module
}

/// Adds the pages, layouts and snippets the frontend's TSX declares to
/// their modules; one Rust declares too is declared twice.
fn add_forms(
    project: &mut ProjectDecl,
    forms: Vec<(String, mxrs_ir::FormDecl)>,
    origins: &std::collections::HashMap<String, String>,
    installed: &std::collections::BTreeSet<String>,
) -> Result<(), mxrs_frontend::FrontendError> {
    for (module, form) in forms {
        let declared = module_of(project, &module, installed);
        let twice = match form.kind() {
            "Forms$Page" => declared.pages.iter().any(|page| page.name == form.name()),
            "Forms$Layout" => declared
                .layouts
                .iter()
                .any(|layout| layout.name == form.name()),
            _ => false,
        };
        if twice {
            let folder = mxrs_frontend::forms::folder(form.kind()).unwrap_or_default();
            return Err(mxrs_frontend::FrontendError::Shape {
                path: origins
                    .get(&format!("{} {module}.{}", form.kind(), form.name()))
                    .cloned()
                    .unwrap_or_else(|| format!("frontend/src/{folder}")),
                line: 1,
                detail: format!(
                    "{module}.{} is declared in the frontend and in Rust; keep one",
                    form.name()
                ),
            });
        }
        declared.forms.push(form);
    }
    Ok(())
}

/// Adds the nanoflows the frontend declares to their modules; one Rust
/// declares too is declared twice, and one for a module the project
/// installed has no module of the project's to be written into.
fn add_nanoflows(
    project: &mut ProjectDecl,
    nanoflows: Vec<(String, mxrs_ir::flow::MicroflowDecl)>,
    origins: &std::collections::HashMap<String, String>,
    installed: &std::collections::BTreeSet<String>,
) -> Result<(), mxrs_frontend::FrontendError> {
    for (module, nanoflow) in nanoflows {
        let declared = module_of(project, &module, installed);
        if declared.installed {
            let qualified = format!("{module}.{}", nanoflow.name);
            let (path, line) = origins
                .get(&qualified)
                .and_then(|origin| origin.rsplit_once(':'))
                .and_then(|(path, line)| Some((path.to_string(), line.parse().ok()?)))
                .unwrap_or_else(|| ("frontend/src/services".to_string(), 1));
            return Err(mxrs_frontend::FrontendError::Shape {
                path,
                line,
                detail: format!(
                    "nanoflow {qualified} is declared for a module the project installed, which is written as installed; declare it in a module of the project's own"
                ),
            });
        }
        if declared
            .nanoflows
            .iter()
            .any(|existing| existing.name == nanoflow.name)
        {
            // Two services declaring one nanoflow are refused where they
            // are read, so the other declaration is Rust's.
            let qualified = format!("{module}.{}", nanoflow.name);
            let (path, line) = origins
                .get(&qualified)
                .and_then(|origin| origin.rsplit_once(':'))
                .and_then(|(path, line)| Some((path.to_string(), line.parse().ok()?)))
                .unwrap_or_else(|| ("frontend/src/services".to_string(), 1));
            return Err(mxrs_frontend::FrontendError::Shape {
                path,
                line,
                detail: format!(
                    "nanoflow {qualified} is declared here and in Rust with #[nanoflow]; keep one"
                ),
            });
        }
        declared.nanoflows.push(nanoflow);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_module_path_names_its_crate_first() {
        assert_eq!(crate_of("shop::domain::entities::sales::order"), "shop");
        assert_eq!(crate_of("shop"), "shop");
    }

    #[test]
    fn stages_follow_the_layer_composition_order() {
        assert!(Stage::Document < Stage::Entity);
        assert!(Stage::Entity < Stage::Security);
        assert!(Stage::Security < Stage::Microflow);
        assert!(Stage::Microflow < Stage::Page);
    }
}

#[cfg(test)]
mod installed_tests {
    use super::*;

    fn installed(names: &[&str]) -> std::collections::BTreeSet<String> {
        names.iter().map(|name| name.to_string()).collect()
    }

    #[test]
    fn a_module_is_installed_only_when_the_imported_model_says_so() {
        let mut project = ProjectDecl {
            mendix_version: "11.12.1".into(),
            modules: vec![],
            security: None,
            navigation: None,
            demo_users: vec![],
        };
        let atlas = installed(&["Atlas_Core"]);
        // The frontend's only module: the project's own, not installed.
        assert!(!module_of(&mut project, "Portal", &atlas).installed);
        assert!(module_of(&mut project, "Atlas_Core", &atlas).installed);
        // Asked again, the same modules.
        assert_eq!(project.modules.len(), 2);
        assert!(!module_of(&mut project, "Portal", &atlas).installed);
    }

    #[test]
    fn a_nanoflow_for_an_installed_module_is_refused_where_it_is_declared() {
        let mut project = ProjectDecl {
            mendix_version: "11.12.1".into(),
            modules: vec![],
            security: None,
            navigation: None,
            demo_users: vec![],
        };
        let mut origins = std::collections::HashMap::new();
        origins.insert(
            "Atlas_Core.NAN_X".to_string(),
            "frontend/src/services/atlas_core/atlasService.ts:12".to_string(),
        );
        let nanoflow = mxrs_ir::flow::MicroflowDecl::new("NAN_X");
        let error = add_nanoflows(
            &mut project,
            vec![("Atlas_Core".to_string(), nanoflow.clone())],
            &origins,
            &installed(&["Atlas_Core"]),
        )
        .unwrap_err()
        .to_string();
        assert!(
            error.contains("Atlas_Core.NAN_X") && error.contains("installed"),
            "{error}"
        );
        assert!(error.contains("atlasService.ts"), "{error}");
        // For the project's own module, it is declared.
        add_nanoflows(
            &mut project,
            vec![("Portal".to_string(), nanoflow)],
            &origins,
            &installed(&["Atlas_Core"]),
        )
        .unwrap();
        assert_eq!(project.module_mut("Portal").nanoflows.len(), 1);
    }
}
