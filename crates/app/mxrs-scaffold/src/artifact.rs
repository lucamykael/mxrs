//! Artifact scaffolds for an existing Cargo-native project — the mxrs
//! equivalent of mxrb's `Scaffold::Generator`/`Scaffold::Recipes`
//! (`lib/mxrb/scaffold/{generator,recipes}.rb`).
//!
//! **What is the same as mxrb**: one file per artifact, never overwriting an
//! existing file, aggregators connected automatically, the whole set published
//! atomically, every created file recorded so `scaffold destroy` removes
//! exactly what was generated, and `--dry-run` reporting the same paths
//! without writing.
//!
//! **What is necessarily different**: mxrb scaffolds Ruby into
//! `modules/<Module>/<layer>/<family>/`, evaluated at runtime by `project.rb`.
//! mxrs scaffolds Rust into `src/<layer>/<concept>/<module>/`, compiled by
//! `cargo` — the same folders the importer writes. Two consequences follow
//! from that and are not stylistic:
//!
//! 1. Rust module paths must be identifiers, so directories are snake_cased
//!    (`src/domain/entities/sales`, not `.../Sales`). The Mendix name stays
//!    verbatim inside the generated declaration and in the registry key.
//! 2. There is no aggregator to keep in step. A scaffolded declaration
//!    registers itself with the application, so connecting it is one
//!    `pub mod` line per level — and the scaffold never has to recognize how
//!    a project composes its model, or refuse one it does not recognize.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use crate::templates::snake_case;
use crate::transaction::Transaction;
use crate::{
    Result, ScaffoldError, atlas_crud, installed_templates, io_error, page_templates, registry,
    templates,
};

/// Placeholder name on the scaffolded `ApplicationLayout` that scaffolded
/// pages attach their widgets to. Matches the `mxrs new` project scaffold.
const LAYOUT_PARAMETER: &str = "Main";

const RESERVED_ENTITY_NAMES: &[&str] = &["Owner", "ChangedBy", "CreatedDate", "ChangedDate"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArtifactKind {
    Entity,
    Dto,
    Enumeration,
    Constant,
    ScheduledEvent,
    UseCase,
    Microflow,
    Page,
    Nanoflow,
    PublishedRest,
    ConsumedRest,
    JavaAction,
    FunctionalTest,
    Evaluation,
    Validation,
    Integration,
    Ci,
    Repository,
    Security,
    DemoUser,
    Design,
    Module,
    Presentation,
}

/// One scaffold command as `mxrs scaffold list` prints it. `destination` is
/// the directory the command writes into, relative to the project root.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScaffoldCommand {
    pub name: &'static str,
    pub action: &'static str,
    pub argument: &'static str,
    pub summary: &'static str,
    pub destination: &'static str,
    pub kind: ArtifactKind,
}

/// Only commands that actually generate something are listed: advertising a
/// generator mxrs does not have would be a promise `scaffold list` cannot keep.
pub const SCAFFOLD_COMMANDS: &[ScaffoldCommand] = &[
    ScaffoldCommand {
        name: "presentation",
        action: "init",
        argument: "<Module>",
        summary: "Initialize presentation and the application layout",
        destination: "frontend/src/components/layout/<module>",
        kind: ArtifactKind::Presentation,
    },
    ScaffoldCommand {
        name: "ci",
        action: "init",
        argument: "github",
        summary: "Create a GitHub Actions workflow",
        destination: ".github/workflows",
        kind: ArtifactKind::Ci,
    },
    ScaffoldCommand {
        name: "consumed-rest",
        action: "new",
        argument: "<Module.Client>",
        summary: "Create a consumed REST adapter microflow",
        destination: "src/domain/integrations/<module>",
        kind: ArtifactKind::ConsumedRest,
    },
    ScaffoldCommand {
        name: "evaluation",
        action: "new",
        argument: "<Name>",
        summary: "Create declarative static model checks",
        destination: "evaluations",
        kind: ArtifactKind::Evaluation,
    },
    ScaffoldCommand {
        name: "constant",
        action: "new",
        argument: "<Module.Constant>",
        summary: "Create a string constant declaration",
        destination: "src/domain/documents/<module>",
        kind: ArtifactKind::Constant,
    },
    ScaffoldCommand {
        name: "integration",
        action: "new",
        argument: "<Module.Adapter>",
        summary: "Create an integration adapter microflow",
        destination: "src/domain/integrations/<module>",
        kind: ArtifactKind::Integration,
    },
    ScaffoldCommand {
        name: "entity",
        action: "new",
        argument: "<Module.Entity>",
        summary: "Create a domain entity declaration",
        destination: "src/domain/entities/<module>",
        kind: ArtifactKind::Entity,
    },
    ScaffoldCommand {
        name: "dto",
        action: "new",
        argument: "<Module.Dto>",
        summary: "Create a non-persistable entity (DTO) declaration",
        destination: "src/domain/dtos/<module>",
        kind: ArtifactKind::Dto,
    },
    ScaffoldCommand {
        name: "enumeration",
        action: "new",
        argument: "<Module.Enumeration>",
        summary: "Create an enumeration declaration",
        destination: "src/domain/enumerations/<module>",
        kind: ArtifactKind::Enumeration,
    },
    ScaffoldCommand {
        name: "functional-test",
        action: "new",
        argument: "<Module.Flow>",
        summary: "Create a declarative runtime test suite",
        destination: "functional_tests",
        kind: ArtifactKind::FunctionalTest,
    },
    ScaffoldCommand {
        name: "java-action",
        action: "new",
        argument: "<Module.Adapter>",
        summary: "Create a Java Action adapter microflow",
        destination: "src/domain/actions/<module>",
        kind: ArtifactKind::JavaAction,
    },
    ScaffoldCommand {
        name: "module",
        action: "new",
        argument: "<Module>",
        summary: "Create an editable module declaration layer",
        destination: "src/domain/modules",
        kind: ArtifactKind::Module,
    },
    ScaffoldCommand {
        name: "nanoflow",
        action: "new",
        argument: "<Module.Flow>",
        summary: "Create a client nanoflow declaration",
        destination: "frontend/src/services/<module>",
        kind: ArtifactKind::Nanoflow,
    },
    ScaffoldCommand {
        name: "page",
        action: "new",
        argument: "<Module.Page>",
        summary: "Create a page declaration and its module layout",
        destination: "frontend/src/pages/<module>",
        kind: ArtifactKind::Page,
    },
    ScaffoldCommand {
        name: "published-rest",
        action: "new",
        argument: "<Module.Handler>",
        summary: "Create a published REST handler microflow",
        destination: "src/services/<module>",
        kind: ArtifactKind::PublishedRest,
    },
    ScaffoldCommand {
        name: "repository",
        action: "new",
        argument: "<Module.Name>",
        summary: "Create a repository port and infrastructure adapter",
        destination: "src/{ports,infrastructure}/repositories",
        kind: ArtifactKind::Repository,
    },
    ScaffoldCommand {
        name: "scheduled-event",
        action: "new",
        argument: "<Module.Event>",
        summary: "Create a scheduled event and its handler microflow",
        destination: "src/domain/documents/<module>",
        kind: ArtifactKind::ScheduledEvent,
    },
    ScaffoldCommand {
        name: "security",
        action: "init",
        argument: "<Module>",
        summary: "Create module roles and project security",
        destination: "src/domain/module_security",
        kind: ArtifactKind::Security,
    },
    // Listed after `security`: a demo user requires initialized project
    // security, and catalog order is also the order the scaffold audit
    // generates every public scaffold in.
    ScaffoldCommand {
        name: "demo-user",
        action: "new",
        argument: "<Name>",
        summary: "Create a local Mendix demo user backed by an ignored .env secret",
        destination: "src/domain/demo_users",
        kind: ArtifactKind::DemoUser,
    },
    ScaffoldCommand {
        name: "design",
        action: "init",
        argument: "",
        summary: "Initialize the project theme and design assets",
        destination: "theme",
        kind: ArtifactKind::Design,
    },
    ScaffoldCommand {
        name: "validation",
        action: "new",
        argument: "<Module.Flow>",
        summary: "Create an application validation microflow",
        destination: "src/services/<module>",
        kind: ArtifactKind::Validation,
    },
    ScaffoldCommand {
        name: "use-case",
        action: "new",
        argument: "<Module.Flow>",
        summary: "Create an application service microflow",
        destination: "src/services/<module>",
        kind: ArtifactKind::UseCase,
    },
    ScaffoldCommand {
        name: "microflow",
        action: "new",
        argument: "<Module.Flow>",
        summary: "Create a microflow in the service of what it is about",
        destination: "src/services/<module>",
        kind: ArtifactKind::Microflow,
    },
];

impl ArtifactKind {
    /// Registry key prefix, matching mxrb's `"#{@kind}:#{@name}"` where the
    /// kind is the command name with dashes turned into underscores.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Entity => "entity",
            Self::Dto => "dto",
            Self::Enumeration => "enumeration",
            Self::Constant => "constant",
            Self::ScheduledEvent => "scheduled_event",
            Self::UseCase => "use_case",
            Self::Microflow => "microflow",
            Self::Page => "page",
            Self::Nanoflow => "nanoflow",
            Self::PublishedRest => "published_rest",
            Self::ConsumedRest => "consumed_rest",
            Self::JavaAction => "java_action",
            Self::FunctionalTest => "functional_test",
            Self::Evaluation => "evaluation",
            Self::Validation => "validation",
            Self::Integration => "integration",
            Self::Ci => "ci",
            Self::Repository => "repository",
            Self::Security => "security",
            Self::DemoUser => "demo-user",
            Self::Design => "design",
            Self::Module => "module",
            Self::Presentation => "presentation",
        }
    }

    /// `Security`, `Module` and `Presentation` name a module; the other kinds name an
    /// artifact inside one — the same split mxrb draws between its `init`
    /// commands and its `new` commands.
    fn names_a_module(self) -> bool {
        matches!(self, Self::Security | Self::Module | Self::Presentation)
    }
}

/// The page-led vertical slices `mxrs page new --chain` generates, mirroring
/// mxrb's `Generator::PAGE_CHAINS`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PageChain {
    /// `page:microflow` — the page's Refresh button calls a microflow.
    Microflow,
    /// `page:nanoflow` — it calls a client nanoflow instead.
    Nanoflow,
    /// `page:nanoflow:microflow` — it calls a nanoflow that calls a microflow.
    NanoflowMicroflow,
}

impl PageChain {
    pub fn parse(value: &str) -> Result<Self> {
        match value {
            "page:microflow" => Ok(Self::Microflow),
            "page:nanoflow" => Ok(Self::Nanoflow),
            "page:nanoflow:microflow" => Ok(Self::NanoflowMicroflow),
            other => Err(ScaffoldError::UnknownPageChain(other.to_string())),
        }
    }

    fn has_microflow(self) -> bool {
        matches!(self, Self::Microflow | Self::NanoflowMicroflow)
    }

    fn has_nanoflow(self) -> bool {
        matches!(self, Self::Nanoflow | Self::NanoflowMicroflow)
    }
}

#[derive(Debug, Clone)]
pub struct ArtifactScaffold {
    pub kind: ArtifactKind,
    /// `Module.Artifact`, or `Module` for [`ArtifactKind::Security`] and
    /// [`ArtifactKind::Module`] or [`ArtifactKind::Presentation`].
    pub name: String,
    pub target: PathBuf,
    pub dry_run: bool,
    /// Module roles a scaffolded page is restricted to (`--role`, repeatable).
    pub page_roles: Vec<String>,
    /// Named page pattern (`--template`). `None` with no chain scaffolds the
    /// minimal page; `None` with a chain uses
    /// [`page_templates::DEFAULT_CHAIN_TEMPLATE`], as mxrb does.
    pub page_template: Option<String>,
    /// Page-led vertical slice to generate (`--chain`).
    pub page_chain: Option<PageChain>,
    /// The `crud` template from Atlas's templates (`--atlas`): the
    /// overview from `Grid`, the edit page from `Form_Vertical_Edit`.
    pub atlas: bool,
    /// Demo-user backing entity (`--entity`, `System.User` by default). The
    /// repeatable `--role` values arrive through [`Self::page_roles`], which
    /// doubles as the generic role list for kinds that grant roles.
    pub demo_entity: Option<String>,
}

impl ArtifactScaffold {
    pub fn new(kind: ArtifactKind, name: impl Into<String>, target: impl Into<PathBuf>) -> Self {
        Self {
            kind,
            name: name.into(),
            target: target.into(),
            dry_run: false,
            page_roles: vec![],
            page_template: None,
            page_chain: None,
            atlas: false,
            demo_entity: None,
        }
    }

    pub fn atlas(mut self, atlas: bool) -> Self {
        self.atlas = atlas;
        self
    }

    pub fn dry_run(mut self, dry_run: bool) -> Self {
        self.dry_run = dry_run;
        self
    }

    pub fn page_roles(mut self, roles: Vec<String>) -> Self {
        self.page_roles = roles;
        self
    }

    pub fn page_template(mut self, template: Option<String>) -> Self {
        self.page_template = template;
        self
    }

    pub fn page_chain(mut self, chain: Option<PageChain>) -> Self {
        self.page_chain = chain;
        self
    }

    pub fn demo_entity(mut self, entity: Option<String>) -> Self {
        self.demo_entity = entity;
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScaffoldOutcome {
    pub kind: &'static str,
    pub name: String,
    pub dry_run: bool,
    pub files: Vec<PathBuf>,
    pub updated: Vec<PathBuf>,
    /// What the scaffold left for its user to do, and why.
    pub notes: Vec<String>,
}

pub fn scaffold_artifact(options: &ArtifactScaffold) -> Result<ScaffoldOutcome> {
    let root =
        std::path::absolute(&options.target).map_err(|error| io_error(&options.target, error))?;
    require_project(&root)?;
    // An earlier tree would get the new layers beside its old ones.
    if let layout @ (crate::lifecycle::ProjectLayout::ModuleFirst
    | crate::lifecycle::ProjectLayout::ApplicationLayer) =
        crate::lifecycle::project_layout(&root)?
    {
        return Err(ScaffoldError::OutdatedLayout {
            root: root.display().to_string(),
            layout: layout.as_str(),
        });
    }
    let mut transaction = Transaction::default();
    if options.kind == ArtifactKind::FunctionalTest {
        let (module_name, artifact_name) = qualified_name(options.kind, &options.name)?;
        let path = root
            .join("functional_tests")
            .join(format!("{}.json", snake_case(artifact_name)));
        transaction.create(path, templates::functional_test(module_name, artifact_name))?;
    } else if options.kind == ArtifactKind::Evaluation {
        let name = identifier(&options.name, "evaluation")?;
        transaction.create(
            root.join("evaluations")
                .join(format!("{}.json", snake_case(name))),
            templates::evaluation(name),
        )?;
    } else if options.kind == ArtifactKind::DemoUser {
        create_demo_user(&mut transaction, &root, options)?;
    } else if options.kind == ArtifactKind::Design {
        create_design(&mut transaction, &root)?;
    } else if options.kind == ArtifactKind::Ci {
        if options.name != "github" {
            return Err(ScaffoldError::InvalidIdentifier {
                label: "CI provider (expected github)",
                value: options.name.clone(),
            });
        }
        transaction.create(
            root.join(".github/workflows/mxrs.yml"),
            templates::github_workflow(),
        )?;
    } else if options.kind.names_a_module() {
        let module_name = identifier(&options.name, "module")?;
        match options.kind {
            ArtifactKind::Module => create_module_layer(&mut transaction, &root, module_name)?,
            ArtifactKind::Presentation => {
                initialize_presentation(&mut transaction, &root, module_name)?
            }
            _ => create_module_security(&mut transaction, &root, module_name)?,
        }
    } else {
        let (module_name, artifact_name) = qualified_name(options.kind, &options.name)?;
        if options.kind == ArtifactKind::Page {
            refuse_undeclared_roles(&transaction, &root, &options.page_roles)?;
        }
        create_artifact(&mut transaction, &root, options, module_name, artifact_name)?;
    }

    let mut files = transaction.created().to_vec();
    let updated = transaction.updated().to_vec();
    let notes = transaction.notes().to_vec();
    if !options.dry_run {
        let key = format!("{}:{}", options.kind.as_str(), options.name);
        // What the scaffold created for the project to keep — the
        // elements its pages are written with — or for another scaffold to
        // own — the layout a module's pages share — is not removed with it.
        let elsewhere = transaction.elsewhere().to_vec();
        let recorded: Vec<PathBuf> = files
            .iter()
            .filter(|file| elsewhere.iter().all(|(_, other)| other != *file))
            .cloned()
            .collect();
        let registry_path = registry::stage(&mut transaction, &root, &key, &recorded)?;
        let mut owners: Vec<&String> = elsewhere
            .iter()
            .filter_map(|(owner, _)| owner.as_ref())
            .collect();
        owners.sort();
        owners.dedup();
        for owner in owners {
            let owned: Vec<PathBuf> = elsewhere
                .iter()
                .filter(|(other, _)| other.as_ref() == Some(owner))
                .map(|(_, file)| file.clone())
                .collect();
            registry::stage(&mut transaction, &root, owner, &owned)?;
        }
        files.retain(|file| file != &registry_path);
        transaction.commit()?;
    }
    Ok(ScaffoldOutcome {
        kind: options.kind.as_str(),
        name: options.name.clone(),
        dry_run: options.dry_run,
        files,
        updated,
        notes,
    })
}

/// Facts about a Cargo-native project workspace, for `mxrs project inspect`.
/// `modules` lists the snake-cased Rust directories under
/// `src/modules`; the verbatim Mendix names appear in
/// `registered_scaffolds` as `module:<Name>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectInspection {
    pub root: PathBuf,
    pub manifest: bool,
    pub domain_module: bool,
    pub layout: crate::lifecycle::ProjectLayout,
    pub declared_version: Option<String>,
    pub modules: Vec<String>,
    pub mprs: Vec<PathBuf>,
    pub registered_scaffolds: Vec<String>,
}

pub fn inspect_project(target: impl AsRef<Path>) -> Result<ProjectInspection> {
    let target = target.as_ref();
    let root = std::path::absolute(target).map_err(|error| io_error(target, error))?;
    let root = lexical_root(&root);
    let mut mprs = Vec::new();
    for directory in [root.clone(), root.join("build")] {
        let Some(entries) = optional_directory(&directory)? else {
            continue;
        };
        for entry in entries {
            let path = entry.map_err(|error| io_error(&directory, error))?.path();
            if !is_hidden(&path) && path.extension().is_some_and(|extension| extension == "mpr") {
                mprs.push(path);
            }
        }
    }
    mprs.sort();
    let mut modules = Vec::new();
    // A layer-first tree has no module folders, so the modules a project
    // declares are the files in the registry.
    let registry = root.join("src/domain/modules");
    if let Some(entries) = optional_directory(&registry)? {
        for entry in entries {
            let path = entry.map_err(|error| io_error(&registry, error))?.path();
            if is_hidden(&path) || path.extension().is_none_or(|kind| kind != "rs") {
                continue;
            }
            if let Some(name) = path.file_stem().and_then(|name| name.to_str())
                && name != "mod"
            {
                modules.push(name.to_string());
            }
        }
    }
    modules.sort();
    Ok(ProjectInspection {
        manifest: root.join("Cargo.toml").is_file(),
        domain_module: root.join("src/domain/mod.rs").is_file(),
        layout: crate::lifecycle::project_layout(&root)?,
        declared_version: declared_version(&root)?,
        modules,
        mprs,
        registered_scaffolds: registry::entries(&root)?
            .into_iter()
            .map(|entry| entry.key)
            .collect(),
        root,
    })
}

fn is_hidden(path: &Path) -> bool {
    path.file_name()
        .is_some_and(|name| name.as_encoded_bytes().starts_with(b"."))
}

// Match a lexical absolute workspace path, without resolving symlinks.
fn lexical_root(path: &Path) -> PathBuf {
    let mut result = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::ParentDir => {
                result.pop();
            }
            std::path::Component::CurDir => {}
            component => result.push(component.as_os_str()),
        }
    }
    result
}

fn optional_directory(path: &Path) -> Result<Option<std::fs::ReadDir>> {
    match std::fs::read_dir(path) {
        Ok(entries) => Ok(Some(entries)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(io_error(path, error)),
    }
}

/// Reads the Mendix version an imported project records in `mxrs.toml`, then
/// falls back to the `#[mxrs::application(version = "…")]` attribute the
/// `mxrs new` scaffold emits. Returns `None` rather than a guess when neither
/// is present.
fn declared_version(root: &Path) -> Result<Option<String>> {
    let manifest = root.join("mxrs.toml");
    if let Some(text) = read_optional(&manifest)?
        && let Some(version) = crate::manifest::project_setting(&text, "mendix_version")
    {
        return Ok(Some(version));
    }
    let Some(text) = read_optional(&root.join("src/lib.rs"))? else {
        return Ok(None);
    };
    rust_application_metadata(&text)
        .map(|(version, _)| version)
        .map_err(|error| ScaffoldError::InvalidProjectSource {
            path: root.join("src/lib.rs").display().to_string(),
            reason: error.to_string(),
        })
}

pub(crate) fn rust_application_metadata(source: &str) -> syn::Result<(Option<String>, bool)> {
    use syn::visit::Visit;
    #[derive(Default)]
    struct Versions {
        values: Vec<String>,
        has_project: bool,
        error: Option<syn::Error>,
    }
    impl<'ast> Visit<'ast> for Versions {
        fn visit_attribute(&mut self, attribute: &'ast syn::Attribute) {
            let path = attribute.path();
            if path.segments.len() != 2
                || path.segments[0].ident != "mxrs"
                || path.segments[1].ident != "application"
            {
                return;
            }
            let parsed = attribute.parse_args_with(
                syn::punctuated::Punctuated::<syn::Meta, syn::Token![,]>::parse_terminated,
            );
            match parsed {
                Ok(items) => {
                    for item in items {
                        if let syn::Meta::NameValue(value) = &item
                            && value.path.is_ident("project")
                        {
                            self.has_project = true;
                        }
                        if let syn::Meta::NameValue(value) = item
                            && value.path.is_ident("version")
                        {
                            if let syn::Expr::Lit(syn::ExprLit {
                                lit: syn::Lit::Str(text),
                                ..
                            }) = &value.value
                            {
                                self.values.push(text.value());
                            } else {
                                self.error = Some(syn::Error::new_spanned(
                                    value,
                                    "application version must be a string literal",
                                ));
                            }
                        }
                    }
                }
                Err(error) => self.error = Some(error),
            }
        }
    }
    let file = syn::parse_file(source)?;
    let mut versions = Versions::default();
    versions.visit_file(&file);
    if let Some(error) = versions.error {
        return Err(error);
    }
    if versions.values.len() > 1 {
        return Err(syn::Error::new_spanned(
            file,
            "multiple application versions are ambiguous",
        ));
    }
    Ok((versions.values.pop(), versions.has_project))
}

fn read_optional(path: &Path) -> Result<Option<String>> {
    match std::fs::read_to_string(path) {
        Ok(text) => Ok(Some(text)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(io_error(path, error)),
    }
}

fn require_project(root: &Path) -> Result<()> {
    for relative in ["Cargo.toml", "src/domain/mod.rs"] {
        if !root.join(relative).is_file() {
            return Err(ScaffoldError::ProjectNotFound(
                root.join(relative).display().to_string(),
            ));
        }
    }
    Ok(())
}

/// Declares a Mendix module in the registry — `src/domain/modules/<module>.rs`.
///
/// A layer-first tree has no single folder a module owns, so a module is not a
/// directory to create: it is a named thing that exists before it holds
/// anything, and every concept grows a folder for it on demand. That is also
/// why no other scaffold needs this to have run first.
fn create_module_layer(
    transaction: &mut Transaction,
    root: &Path,
    module_name: &str,
) -> Result<()> {
    let stem = snake_case(module_name);
    let declaration = root.join(format!("src/domain/modules/{stem}.rs"));
    if declaration.is_file() {
        return Err(ScaffoldError::ModuleExists(
            declaration.display().to_string(),
        ));
    }
    create_concept_file(
        transaction,
        root,
        module_name,
        module_folder(ArtifactKind::Module),
        &stem,
        templates::module_declaration(module_name),
    )
}

/// Ports mxrb's `scaffold_demo_user` to the Cargo-native layout: the
/// `#[demo_user]` declaration lands in `src/domain/demo_users/` and
/// registers itself, and the generated password lands in a `0o600` `.env`
/// at the project root (with an empty `.env.example` key for sharing).
/// Role and entity references are
/// validated structurally against the generated layout — the same
/// source-scanning contract mxrb applies to its Ruby projects.
fn create_demo_user(
    transaction: &mut Transaction,
    root: &Path,
    options: &ArtifactScaffold,
) -> Result<()> {
    let name = identifier(&options.name, "demo user")?;
    let entity = options
        .demo_entity
        .clone()
        .unwrap_or_else(|| "System.User".to_string());
    validate_demo_user_entity(root, &entity)?;
    let mut roles: Vec<String> = Vec::new();
    for role in &options.page_roles {
        let role = identifier(role, "user role")?;
        if !roles.iter().any(|known| known == role) {
            roles.push(role.to_string());
        }
    }
    if roles.is_empty() {
        roles.push("User".to_string());
    }
    // The importer and `security init` both write `src/domain/security.rs`;
    // a project scaffolded before they agreed keeps its `security/mod.rs`.
    let mut security_source = None;
    for relative in ["src/domain/security.rs", "src/domain/security/mod.rs"] {
        if let Some(source) = transaction.content(&root.join(relative))? {
            security_source = Some(source);
            break;
        }
    }
    let security_source = security_source
        .ok_or_else(|| ScaffoldError::SecurityNotInitialized(root.display().to_string()))?;
    let known_roles = declared_user_roles(&security_source);
    for role in &roles {
        if !known_roles.contains(role) {
            return Err(ScaffoldError::UnknownDemoUserRole(role.clone()));
        }
    }

    let password_env = format!(
        "MXRS_DEMO_USER_{}_PASSWORD",
        name.chars()
            .map(|character| if character.is_ascii_alphanumeric() {
                character.to_ascii_uppercase()
            } else {
                '_'
            })
            .collect::<String>()
    );

    // A demo user registers itself and joins the project security declared
    // before it, so its folder is a list of files and nothing else.
    let index = root.join("src/domain/demo_users/mod.rs");
    if transaction.content(&index)?.is_none() {
        transaction.create(&index, "//! Local demo users, one file each.\n".to_string())?;
        declare_child_module(transaction, &root.join("src/domain/mod.rs"), "demo_users")?;
    }
    let stem = snake_case(name);
    transaction.create(
        index.with_file_name(format!("{stem}.rs")),
        templates::demo_user(name, &entity, &roles, &password_env),
    )?;
    declare_child_module(transaction, &index, &stem)?;

    if !options.dry_run {
        ensure_demo_user_secret(transaction, root, &password_env)?;
    }
    Ok(())
}

/// User roles the project's security declaration names: every
/// `security.role("Name", …)` — the form both `security init` and the
/// importer write — and the struct-literal forms earlier imports used.
/// A page is allowed to module roles, each `Module.Role`: one of a module
/// the project declares must be among the roles its `#[module_roles]` enum
/// declares. A module the project installed or imported keeps its roles in
/// its model, which the build checks.
fn refuse_undeclared_roles(transaction: &Transaction, root: &Path, roles: &[String]) -> Result<()> {
    if roles.is_empty() {
        return Ok(());
    }
    let declared = declared_module_roles(transaction, root)?;
    for role in roles {
        let refused = |reason: String| ScaffoldError::UnknownModuleRole {
            role: role.clone(),
            reason,
        };
        let Some((module, name)) = role.rsplit_once('.') else {
            return Err(refused(
                "a page is allowed to module roles, named `Module.Role`".to_string(),
            ));
        };
        match declared.get(module) {
            Some(names) if names.contains(name) => {}
            Some(names) => {
                return Err(refused(format!(
                    "{module} declares {} in src/domain/module_security",
                    names.iter().cloned().collect::<Vec<_>>().join(", ")
                )));
            }
            None => {
                let own = root.join(format!("src/domain/modules/{}.rs", snake_case(module)));
                if transaction.content(&own)?.is_some() {
                    return Err(refused(format!(
                        "{module} declares no module roles yet: run `mxrs security init {module}` first"
                    )));
                }
            }
        }
    }
    Ok(())
}

/// The module roles the project declares, by module: the variants of each
/// `#[module_roles(module = "...")]` enum under `src/domain/module_security`
/// (a variant's `#[mxrs(name = "...")]` is its name).
fn declared_module_roles(
    transaction: &Transaction,
    root: &Path,
) -> Result<BTreeMap<String, BTreeSet<String>>> {
    let mut declared: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut folders = vec![root.join("src/domain/module_security")];
    while let Some(folder) = folders.pop() {
        let Ok(entries) = std::fs::read_dir(&folder) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                folders.push(path);
                continue;
            }
            if path.extension().and_then(|extension| extension.to_str()) != Some("rs") {
                continue;
            }
            let Some(source) = transaction.content(&path)? else {
                continue;
            };
            let Ok(file) = syn::parse_file(&source) else {
                continue;
            };
            for item in file.items {
                let syn::Item::Enum(roles) = item else {
                    continue;
                };
                let Some(module) = roles.attrs.iter().find_map(|attribute| {
                    if attribute
                        .path()
                        .segments
                        .last()
                        .is_none_or(|segment| segment.ident != "module_roles")
                    {
                        return None;
                    }
                    let mut module = None;
                    attribute
                        .parse_nested_meta(|meta| {
                            if meta.path.is_ident("module") {
                                module = Some(meta.value()?.parse::<syn::LitStr>()?.value());
                            } else if meta.input.peek(syn::Token![=]) {
                                meta.value()?.parse::<syn::Expr>()?;
                            }
                            Ok(())
                        })
                        .ok()?;
                    module
                }) else {
                    continue;
                };
                let names = declared.entry(module).or_default();
                for variant in &roles.variants {
                    let mut name = variant.ident.to_string();
                    for attribute in &variant.attrs {
                        if attribute.path().is_ident("mxrs") {
                            let _ = attribute.parse_nested_meta(|meta| {
                                if meta.path.is_ident("name") {
                                    name = meta.value()?.parse::<syn::LitStr>()?.value();
                                }
                                Ok(())
                            });
                        }
                    }
                    names.insert(name);
                }
            }
        }
    }
    Ok(declared)
}

fn declared_user_roles(source: &str) -> Vec<String> {
    let mut roles = Vec::new();
    for pattern in [
        ".role(\"",
        "UserRoleDecl { name: \"",
        "UserRoleDecl::new(\"",
    ] {
        let mut rest = source;
        while let Some(start) = rest.find(pattern) {
            rest = &rest[start + pattern.len()..];
            if let Some(end) = rest.find('"') {
                let role = &rest[..end];
                if !role.is_empty() && !roles.iter().any(|known| known == role) {
                    roles.push(role.to_string());
                }
            }
        }
    }
    roles
}

fn validate_demo_user_entity(root: &Path, entity: &str) -> Result<()> {
    if entity == "System.User" {
        return Ok(());
    }
    let invalid = || ScaffoldError::UnknownDemoUserEntity(entity.to_string());
    let (module_name, entity_name) = entity.split_once('.').ok_or_else(invalid)?;
    if identifier(module_name, "module").is_err() || identifier(entity_name, "entity").is_err() {
        return Err(invalid());
    }
    let declaration = root.join(format!(
        "src/domain/entities/{}/{}.rs",
        snake_case(module_name),
        snake_case(entity_name)
    ));
    if declaration.is_file() {
        Ok(())
    } else {
        Err(invalid())
    }
}

/// Mirrors mxrb's `ensure_demo_user_secret`: the generated password goes
/// into a private `.env` (created `0o600`), and `.env.example` records the
/// key with an empty value so the requirement is shareable without the
/// secret. An already-present key is left untouched.
fn ensure_demo_user_secret(
    transaction: &mut Transaction,
    root: &Path,
    password_env: &str,
) -> Result<()> {
    let env = root.join(".env");
    let password = format!("Mxrs{}7", uuid::Uuid::new_v4().simple());
    match transaction.content(&env)? {
        Some(source) => {
            if !env_key_present(&source, password_env) {
                transaction.write(&env, append_env(&source, password_env, &password))?;
            }
        }
        None => {
            let header = "# Local secrets; never commit this file.\n";
            transaction.create_private(&env, append_env(header, password_env, &password))?;
        }
    }
    let example = root.join(".env.example");
    let source = transaction
        .content(&example)?
        .unwrap_or_else(|| "# Copy to .env and keep real values local.\n".to_string());
    if !env_key_present(&source, password_env) {
        transaction.write(&example, append_env(&source, password_env, ""))?;
    }
    Ok(())
}

fn append_env(source: &str, key: &str, value: &str) -> String {
    format!("{}\n{key}={value}\n", source.trim_end())
}

fn env_key_present(source: &str, key: &str) -> bool {
    source.lines().any(|line| {
        line.strip_prefix(key)
            .is_some_and(|rest| rest.starts_with('='))
    })
}

/// Ports mxrb's `design init` asset kit to the Cargo-native layout. Files
/// already present are left untouched (mxrb's `ensure_file`), so re-running
/// never clobbers an edited theme. The `design_system` Ruby DSL policy file
/// mxrb also writes has no MXRS equivalent and is deliberately not faked.
fn create_design(transaction: &mut Transaction, root: &Path) -> Result<()> {
    for (relative, content) in [
        (
            "theme/web/custom-variables.scss",
            templates::theme_custom_variables(),
        ),
        ("theme/web/main.scss", templates::theme_main()),
        (
            "theme/web/exclusion-variables.scss",
            templates::theme_exclusion_variables(),
        ),
        (
            "theme/web/settings.json",
            templates::THEME_SETTINGS.to_string(),
        ),
        (
            "theme-cache/web/theme.compiled.css",
            templates::THEME_COMPILED.to_string(),
        ),
    ] {
        let target = root.join(relative);
        if transaction.content(&target)?.is_none() {
            transaction.create(&target, content)?;
        }
    }
    Ok(())
}

fn create_module_security(
    transaction: &mut Transaction,
    root: &Path,
    module_name: &str,
) -> Result<()> {
    require_module(root, module_name)?;
    // One file per module, where the importer writes it too.
    create_concept_file(
        transaction,
        root,
        module_name,
        module_folder(ArtifactKind::Security),
        &snake_case(module_name),
        templates::module_roles(module_name),
    )?;
    let security = root.join("src/domain/security.rs");
    if transaction.content(&security)?.is_none()
        && transaction
            .content(&root.join("src/domain/security/mod.rs"))?
            .is_none()
    {
        transaction.create(&security, templates::project_security(module_name))?;
        declare_child_module(transaction, &root.join("src/domain/mod.rs"), "security")?;
    }
    Ok(())
}

/// A `--template`/`--chain` page is not one file but a slice: optionally a
/// backing entity and loader, the refresh flow(s) the chain names, and the
/// page itself. Mirrors mxrb's `scaffold_templated_page`/`page_support_specs`.
/// The page is the frontend's, and joins the items of the Responsive
/// profile its navigation declares, after the ones already there.
fn create_page_slice(
    transaction: &mut Transaction,
    root: &Path,
    options: &ArtifactScaffold,
    module_name: &str,
    artifact_name: &str,
) -> Result<()> {
    let chain = options.page_chain;
    let template_name = options
        .page_template
        .clone()
        .unwrap_or_else(|| page_templates::DEFAULT_CHAIN_TEMPLATE.to_string());
    let template = match page_templates::fetch(&template_name) {
        Ok(template) => template,
        // Not one of mxrs's own: one the project installed, as Studio Pro
        // offers them — named, so never applied without asking.
        Err(ScaffoldError::UnknownPageTemplate(_)) => {
            return match installed_templates::find(root, &template_name)? {
                Some(installed) => create_installed_template_page(
                    transaction,
                    root,
                    options,
                    module_name,
                    artifact_name,
                    &installed,
                ),
                None => Err(ScaffoldError::UnknownPageTemplate(template_name)),
            };
        }
        Err(error) => return Err(error),
    };
    if template.name == page_templates::CRUD {
        return create_crud(transaction, root, options, module_name, artifact_name);
    }
    let stem = snake_case(artifact_name);

    if template.data_backed {
        create_concept_file(
            transaction,
            root,
            module_name,
            module_folder(ArtifactKind::Entity),
            &stem,
            templates::page_chain_entity(module_name, artifact_name),
        )?;
        add_microflow(
            transaction,
            root,
            module_name,
            &templates::page_chain_loader(module_name, artifact_name),
            &[artifact_name.to_string()],
        )?;
    }
    // The slice's entity, when it has one, is what its flows are about.
    let slice_entities = if template.data_backed {
        vec![artifact_name.to_string()]
    } else {
        Vec::new()
    };
    let refresh_service = if chain.is_some_and(PageChain::has_microflow) {
        Some(
            add_microflow(
                transaction,
                root,
                module_name,
                &templates::page_chain_action(module_name, artifact_name),
                &slice_entities,
            )?
            .module_path,
        )
    } else {
        None
    };
    if let Some(chain) = chain.filter(|chain| chain.has_nanoflow()) {
        let calls = chain
            .has_microflow()
            .then_some(())
            .and(refresh_service.as_deref());
        add_frontend_nanoflow(
            transaction,
            root,
            module_name,
            &templates::page_chain_nanoflow(module_name, artifact_name, calls.is_some()),
            &slice_entities,
        )?;
    }

    let refresh = chain.map(|chain| {
        if chain.has_nanoflow() {
            templates::RefreshAction::Nanoflow
        } else {
            templates::RefreshAction::Microflow
        }
    });
    let page = crate::forms::templated_page(
        &page_layout(transaction, root, module_name)?,
        module_name,
        artifact_name,
        template.name,
        refresh,
        &options.page_roles,
    );
    add_page(transaction, root, module_name, &page)?;
    join_navigation(transaction, root, module_name, artifact_name)
}

/// The page `module_name.artifact_name` joins the navigation the frontend
/// declares, when it declares one the way an import or `mxrs new` writes it.
fn join_navigation(
    transaction: &mut Transaction,
    root: &Path,
    module_name: &str,
    artifact_name: &str,
) -> Result<()> {
    let navigation = root.join("frontend/src/navigation/index.ts");
    let qualified = format!("{module_name}.{artifact_name}");
    match transaction.content(&navigation)? {
        Some(source) => {
            match crate::forms::add_navigation_item(
                &source,
                &templates::humanize(artifact_name),
                &qualified,
            ) {
                Some(source) => transaction.write(&navigation, source)?,
                None => transaction.note(format!(
                    "{} has no Responsive profile laid out the way mxrs writes one: add {qualified} to its items yourself",
                    navigation.display()
                )),
            }
        }
        None => transaction.note(format!(
            "the frontend declares no navigation ({} is absent): add {qualified} to the project's navigation yourself",
            navigation.display()
        )),
    }
    Ok(())
}

/// A page from a template the project installed: the template's widgets in
/// the placeholder of the layout the page is shown in, stated as TSX like
/// any scaffolded page, and in the navigation.
fn create_installed_template_page(
    transaction: &mut Transaction,
    root: &Path,
    options: &ArtifactScaffold,
    module_name: &str,
    artifact_name: &str,
    template: &installed_templates::InstalledTemplate,
) -> Result<()> {
    if options.page_chain.is_some() {
        return Err(ScaffoldError::InvalidProjectSource {
            path: template.qualified_name(),
            reason: "an installed page template takes no --chain: the page holds what the template holds"
                .to_string(),
        });
    }
    refuse_declared_page(transaction, root, module_name, artifact_name)?;
    let shown_in = page_layout(transaction, root, module_name)?;
    let layout =
        mxrs_ir::page::LayoutRef::new(shown_in.layout.clone(), shown_in.parameter.as_str());
    let version = forms_version(transaction, root)?;
    let (document, notes) = installed_templates::page_document(
        template,
        &version,
        &layout,
        artifact_name,
        &options.page_roles,
    )?;
    for note in notes {
        transaction.note(note);
    }
    crate::forms::add_forms(transaction, root, &[(module_name, document)])?;
    join_navigation(transaction, root, module_name, artifact_name)
}

/// The overview and edit pages of an entity the project declares, written
/// from its attributes, with the overview in the navigation.
fn create_crud(
    transaction: &mut Transaction,
    root: &Path,
    options: &ArtifactScaffold,
    module_name: &str,
    entity: &str,
) -> Result<()> {
    if options.page_chain.is_some() {
        return Err(ScaffoldError::InvalidProjectSource {
            path: format!("{module_name}.{entity}"),
            reason: "the crud template takes no --chain: its pages work on the entity itself"
                .to_string(),
        });
    }
    let module_stem = snake_case(module_name);
    // The struct that declares the entity, by the name the model gives it:
    // in the file named for it, or whichever of the module's declares it.
    let folder = root.join("src/domain/entities").join(&module_stem);
    let mut candidates = vec![folder.join(format!("{}.rs", snake_case(entity)))];
    if let Ok(entries) = std::fs::read_dir(&folder) {
        let mut others: Vec<_> = entries.flatten().map(|entry| entry.path()).collect();
        others.sort();
        candidates.extend(others);
    }
    let mut found = None;
    for path in candidates {
        let Some(source) = transaction.content(&path)? else {
            continue;
        };
        match declared_entity(&source, module_name, entity) {
            Ok(Some(declared)) => {
                found = Some((path, declared));
                break;
            }
            Ok(None) => {}
            Err(reason) => {
                return Err(ScaffoldError::InvalidProjectSource {
                    path: path.display().to_string(),
                    reason,
                });
            }
        }
    }
    let Some((path, declared)) = found else {
        return Err(ScaffoldError::InvalidProjectSource {
            path: format!(
                "src/domain/entities/{module_stem}/{}.rs",
                snake_case(entity)
            ),
            reason: format!(
                "the crud template is named by a persistable entity the project declares with `#[entity]`, and {module_name}.{entity} is not one: declare it (`mxrs entity new {module_name}.{entity}`) and give it its attributes first"
            ),
        });
    };
    let attributes = declared.attributes;
    if attributes.is_empty() {
        return Err(ScaffoldError::InvalidProjectSource {
            path: path.display().to_string(),
            reason: format!("{module_name}.{entity} declares no attribute to show or edit"),
        });
    }
    // What the pages leave out is said, not dropped.
    for (field, why) in declared.left_out {
        transaction.note(format!(
            "{module_name}.{entity}.{field} is not on the pages: {why}"
        ));
    }
    let (overview, _) = crate::forms::crud_page_names(entity);
    let shown_in = page_layout(transaction, root, module_name)?;
    let pages = [
        crate::forms::crud_edit_page(
            &shown_in,
            module_name,
            entity,
            &attributes,
            &options.page_roles,
        ),
        crate::forms::crud_overview_page(
            &shown_in,
            module_name,
            entity,
            &attributes,
            &options.page_roles,
        ),
    ];
    if options.atlas {
        // The same pages, from Atlas's templates: what mxrs's own compile
        // to is what binds the templates' widgets to the entity.
        let find = |name: &str| {
            installed_templates::find(root, name)?.ok_or_else(|| {
                ScaffoldError::InvalidProjectSource {
                    path: format!("{module_name}.{entity}"),
                    reason: format!(
                        "--atlas needs the page template {name} of Atlas Web Content installed (run `mxrs page templates`)"
                    ),
                }
            })
        };
        let grid = find(atlas_crud::OVERVIEW_TEMPLATE)?;
        let form = find(atlas_crud::EDIT_TEMPLATE)?;
        for page in &pages {
            refuse_declared_page(transaction, root, module_name, &page.name)?;
        }
        let version = forms_version(transaction, root)?;
        let own_edit = crate::forms::page_document(&version, &pages[0])?;
        let own_overview = crate::forms::page_document(&version, &pages[1])?;
        let edit_shown_in = popup_layout(transaction, root)?.unwrap_or_else(|| shown_in.clone());
        let (list, edit, notes) = atlas_crud::pages(&atlas_crud::Crud {
            overview: &grid,
            edit: &form,
            version: &version,
            shown_in: &shown_in,
            edit_shown_in: &edit_shown_in,
            entity: &format!("{module_name}.{entity}"),
            attributes: &attributes,
            roles: &options.page_roles,
            own_overview: &own_overview,
            own_edit: &own_edit,
        })?;
        for note in notes {
            transaction.note(note);
        }
        crate::forms::add_forms(
            transaction,
            root,
            &[(module_name, edit), (module_name, list)],
        )?;
    } else {
        for page in &pages {
            add_page(transaction, root, module_name, page)?;
        }
    }
    add_to_navigation(
        transaction,
        root,
        &templates::humanize(&templates::plural(entity)),
        &format!("{module_name}.{overview}"),
    )
}

/// What the struct declaring an entity says of it, for its CRUD.
struct DeclaredEntity {
    attributes: Vec<crate::forms::CrudAttribute>,
    /// The fields its pages do not show, and why.
    left_out: Vec<(String, String)>,
}

/// The `key = "text"` an attribute's arguments state, e.g. `name` in
/// `#[entity(module = "Sales", name = "Order_Line")]`.
fn stated(attribute: &syn::Attribute, key: &str) -> Option<String> {
    let mut found = None;
    let _ = attribute.parse_nested_meta(|meta| {
        if meta.path.is_ident(key) {
            let value: syn::LitStr = meta.value()?.parse()?;
            found = Some(value.value());
        } else if meta.input.peek(syn::Token![=]) {
            let _: syn::Expr = meta.value()?.parse()?;
        } else if meta.input.peek(syn::token::Paren) {
            let content;
            syn::parenthesized!(content in meta.input);
            syn::punctuated::Punctuated::<syn::Expr, syn::Token![,]>::parse_terminated(&content)?;
        }
        Ok(())
    });
    found
}

/// The entity `module_name.entity` as `source` declares it with `#[entity]`,
/// when it does: its attributes by the names the model gives them, and what
/// its pages cannot show. An error when the file is not Rust.
fn declared_entity(
    source: &str,
    module_name: &str,
    entity: &str,
) -> std::result::Result<Option<DeclaredEntity>, String> {
    let file =
        syn::parse_file(source).map_err(|error| format!("it does not read as Rust: {error}"))?;
    for item in &file.items {
        let syn::Item::Struct(declaration) = item else {
            continue;
        };
        let Some(attribute) = declaration
            .attrs
            .iter()
            .find(|attribute| attribute.path().is_ident("entity"))
        else {
            continue;
        };
        let name = stated(attribute, "name").unwrap_or_else(|| declaration.ident.to_string());
        if name != entity || stated(attribute, "module").as_deref() != Some(module_name) {
            continue;
        }
        let mut attributes = Vec::new();
        let mut left_out = Vec::new();
        for field in &declaration.fields {
            let Some(ident) = &field.ident else {
                continue;
            };
            let ident = ident.to_string();
            let field_name = field
                .attrs
                .iter()
                .filter(|attribute| attribute.path().is_ident("mxrs"))
                .find_map(|attribute| stated(attribute, "name"))
                .unwrap_or_else(|| templates::pascal_case(ident.trim_start_matches("r#")));
            let last = match &field.ty {
                syn::Type::Path(path) => path
                    .path
                    .segments
                    .last()
                    .map(|segment| segment.ident.to_string()),
                _ => None,
            };
            let holds = match last.as_deref() {
                Some("Reference" | "ReferenceSet") => {
                    left_out.push((field_name, "it is an association".to_string()));
                    continue;
                }
                Some("MxBinary" | "MxHashedString" | "MxAutoNumber") | None => {
                    left_out.push((field_name, "a form does not edit what it holds".to_string()));
                    continue;
                }
                Some("MxBool") => crate::forms::CrudKind::Boolean,
                Some("MxDateTime") => crate::forms::CrudKind::DateTime,
                Some(other) if other.starts_with("Mx") => crate::forms::CrudKind::Text,
                // A type of the project's own is an enumeration.
                Some(_) => crate::forms::CrudKind::Choice,
            };
            attributes.push(crate::forms::CrudAttribute {
                name: field_name,
                holds,
            });
        }
        return Ok(Some(DeclaredEntity {
            attributes,
            left_out,
        }));
    }
    Ok(None)
}

/// Adds a page to the navigation the frontend declares, when it declares
/// one the way an import or `mxrs new` writes it.
fn add_to_navigation(
    transaction: &mut Transaction,
    root: &Path,
    caption: &str,
    qualified: &str,
) -> Result<()> {
    let navigation = root.join("frontend/src/navigation/index.ts");
    match transaction.content(&navigation)? {
        Some(source) => match crate::forms::add_navigation_item(&source, caption, qualified) {
            Some(source) => transaction.write(&navigation, source)?,
            None => transaction.note(format!(
                "{} has no Responsive profile laid out the way mxrs writes one: add {qualified} to its items yourself",
                navigation.display()
            )),
        },
        None => transaction.note(format!(
            "the frontend declares no navigation ({} is absent): add {qualified} to the project's navigation yourself",
            navigation.display()
        )),
    }
    Ok(())
}

/// The Mendix version the project's forms are written for: its own where
/// mxrs knows how that version stores a page, and the one it knows
/// otherwise — said, because the page may then need Studio Pro's eye.
fn forms_version(transaction: &mut Transaction, root: &Path) -> Result<String> {
    let version = declared_version(root)?.ok_or(ScaffoldError::MissingVersionDeclaration)?;
    if crate::forms::knows(&version) {
        return Ok(version);
    }
    let note = format!(
        "pages are written the way Mendix {} stores them; this project is {version}",
        crate::forms::KNOWN_VERSION
    );
    if !transaction.notes().contains(&note) {
        transaction.note(note);
    }
    Ok(crate::forms::KNOWN_VERSION.to_string())
}

/// Declares `page` in the frontend: `frontend/src/pages/<module>/`.
/// A page the project already declares — in Rust, or in the frontend
/// under a name a file system may not tell from this one — is not
/// declared again.
fn refuse_declared_page(
    transaction: &mut Transaction,
    root: &Path,
    module_name: &str,
    name: &str,
) -> Result<()> {
    let stem = snake_case(module_name);
    let rust = root.join(format!("src/ui/pages/{stem}/{}.rs", snake_case(name)));
    if transaction.content(&rust)?.is_some() {
        return Err(ScaffoldError::FileExists(rust.display().to_string()));
    }
    let folder = root.join("frontend/src/pages").join(&stem);
    if let Ok(entries) = std::fs::read_dir(&folder) {
        for entry in entries.flatten() {
            let existing = entry.path();
            let same = existing
                .file_stem()
                .is_some_and(|stem| stem.to_string_lossy().eq_ignore_ascii_case(name));
            if same {
                return Err(ScaffoldError::FileExists(existing.display().to_string()));
            }
        }
    }
    Ok(())
}

fn add_page(
    transaction: &mut Transaction,
    root: &Path,
    module_name: &str,
    page: &mxrs_ir::page::PageDecl,
) -> Result<()> {
    refuse_declared_page(transaction, root, module_name, &page.name)?;
    let version = forms_version(transaction, root)?;
    let document = crate::forms::page_document(&version, page)?;
    crate::forms::add_forms(transaction, root, &[(module_name, document)])
}

fn create_artifact(
    transaction: &mut Transaction,
    root: &Path,
    options: &ArtifactScaffold,
    module_name: &str,
    artifact_name: &str,
) -> Result<()> {
    require_module(root, module_name)?;
    // What is scaffolded joins a project: without its crate root there is
    // none, whichever side of it the artifact lands on.
    let library = root.join("src/lib.rs");
    if transaction.content(&library)?.is_none() {
        return Err(ScaffoldError::ProjectNotFound(
            library.display().to_string(),
        ));
    }
    if options.kind == ArtifactKind::Page
        && options.atlas
        && options.page_template.as_deref() != Some(page_templates::CRUD)
    {
        return Err(ScaffoldError::InvalidProjectSource {
            path: format!("{module_name}.{artifact_name}"),
            reason: "--atlas makes the crud template's pages from Atlas's templates: it goes with `--template crud`"
                .to_string(),
        });
    }
    if options.kind == ArtifactKind::Page
        && (options.page_template.is_some() || options.page_chain.is_some())
    {
        return create_page_slice(transaction, root, options, module_name, artifact_name);
    }
    if options.kind == ArtifactKind::Page {
        // A page is the frontend's: the TSX that states its document.
        let page = crate::forms::plain_page(
            &page_layout(transaction, root, module_name)?,
            artifact_name,
            &options.page_roles,
        );
        return add_page(transaction, root, module_name, &page);
    }
    if options.kind == ArtifactKind::Repository {
        return create_repository(transaction, root, module_name, artifact_name);
    }
    // A nanoflow is the frontend's: a method of a TypeScript service.
    if options.kind == ArtifactKind::Nanoflow {
        return add_frontend_nanoflow(
            transaction,
            root,
            module_name,
            &crate::nanoflow::NanoflowMethod {
                name: artifact_name.to_string(),
                docs: vec!["Client nanoflow.".to_string()],
                body: Vec::new(),
                vocabulary: Vec::new(),
            },
            &[],
        );
    }
    // A microflow is a method of the service of what it is about — the
    // place the importer would have put it.
    let service_docs = match options.kind {
        ArtifactKind::UseCase => Some(vec![format!(
            "Application service `{module_name}.{artifact_name}`."
        )]),
        ArtifactKind::Microflow => {
            Some(vec![format!("Microflow `{module_name}.{artifact_name}`.")])
        }
        ArtifactKind::Validation => Some(vec![format!(
            "Application validation `{module_name}.{artifact_name}`."
        )]),
        ArtifactKind::PublishedRest => Some(vec![
            format!("Published REST handler `{module_name}.{artifact_name}`."),
            String::new(),
            "Publishing the REST service document itself stays a native Studio Pro".to_string(),
            "operation: mxrs has no published-REST declaration surface, so this".to_string(),
            "scaffold creates only the handler microflow the service calls.".to_string(),
        ]),
        _ => None,
    };
    if let Some(docs) = service_docs {
        add_microflow(
            transaction,
            root,
            module_name,
            &crate::service::ServiceMethod {
                name: artifact_name.to_string(),
                docs,
                imports: Vec::new(),
                body: Vec::new(),
            },
            &[],
        )?;
        return Ok(());
    }
    let source = match options.kind {
        ArtifactKind::Entity => templates::entity(module_name, artifact_name),
        ArtifactKind::Dto => templates::dto(module_name, artifact_name),
        ArtifactKind::Microflow => unreachable!("a microflow is a method of its service"),
        ArtifactKind::Enumeration => templates::enumeration(module_name, artifact_name),
        ArtifactKind::Constant => templates::constant(module_name, artifact_name),
        ArtifactKind::ScheduledEvent => templates::scheduled_event(module_name, artifact_name),
        ArtifactKind::UseCase => templates::use_case(module_name, artifact_name),
        ArtifactKind::PublishedRest => templates::published_rest(module_name, artifact_name),
        ArtifactKind::ConsumedRest => templates::consumed_rest(module_name, artifact_name),
        ArtifactKind::JavaAction => templates::java_action(module_name, artifact_name),
        ArtifactKind::FunctionalTest => unreachable!("handled by the caller"),
        ArtifactKind::Evaluation | ArtifactKind::Ci => unreachable!("handled by the caller"),
        ArtifactKind::Validation => templates::validation(module_name, artifact_name),
        ArtifactKind::Integration => templates::integration(module_name, artifact_name),
        ArtifactKind::Repository | ArtifactKind::Nanoflow => unreachable!("handled above"),
        ArtifactKind::Page => unreachable!("handled above"),
        ArtifactKind::Security
        | ArtifactKind::Module
        | ArtifactKind::Presentation
        | ArtifactKind::DemoUser
        | ArtifactKind::Design => {
            unreachable!("handled by the caller")
        }
    };
    // A service is named by what it does, so its file is the declaring
    // function's — the same file the importer would have written.
    let stem = match options.kind {
        ArtifactKind::UseCase | ArtifactKind::Validation | ArtifactKind::PublishedRest => {
            templates::service_stem(artifact_name)
        }
        _ => snake_case(artifact_name),
    };
    create_concept_file(
        transaction,
        root,
        module_name,
        module_folder(options.kind),
        &stem,
        source,
    )
}

fn create_repository(
    transaction: &mut Transaction,
    root: &Path,
    module_name: &str,
    artifact_name: &str,
) -> Result<()> {
    let stem = snake_case(artifact_name);
    let port_root = root.join("src/ports");
    let port_family = port_root.join("repositories");
    connect_plain_family(transaction, &root.join("src"), "ports", "repositories")?;
    let port = port_family.join(format!("{stem}.rs"));
    transaction.create(
        &port,
        templates::repository_port(module_name, artifact_name),
    )?;
    declare_child_module(transaction, &port_family.join("mod.rs"), &stem)?;

    let adapter_root = root.join("src/infrastructure");
    let adapter_family = adapter_root.join("repositories");
    connect_plain_family(
        transaction,
        &root.join("src"),
        "infrastructure",
        "repositories",
    )?;
    let implementation_stem = format!("{stem}_implementation");
    let adapter = adapter_family.join(format!("{implementation_stem}.rs"));
    transaction.create(
        &adapter,
        templates::repository_adapter(module_name, artifact_name, &stem),
    )?;
    declare_child_module(
        transaction,
        &adapter_family.join("mod.rs"),
        &implementation_stem,
    )
}

fn connect_plain_family(
    transaction: &mut Transaction,
    src: &Path,
    layer: &str,
    family: &str,
) -> Result<()> {
    let layer_module = src.join(layer).join("mod.rs");
    if transaction.content(&layer_module)?.is_none() {
        transaction.create(&layer_module, format!("//! `{layer}` layer.\n"))?;
        declare_child_module(transaction, &src.join("lib.rs"), layer)?;
    }
    let family_module = src.join(layer).join(family).join("mod.rs");
    if transaction.content(&family_module)?.is_none() {
        transaction.create(&family_module, format!("//! `{family}` modules.\n"))?;
        declare_child_module(transaction, &layer_module, family)?;
    }
    Ok(())
}

/// The layout a page scaffolded into `module_name` is shown in: Atlas's
/// default when the project has Atlas, the module's own otherwise — which
/// is declared for it when it has none yet.
fn page_layout(
    transaction: &mut Transaction,
    root: &Path,
    module_name: &str,
) -> Result<crate::forms::PageLayout> {
    let atlas = root.join("frontend/src/components/layout/atlas_core/Atlas_Default.tsx");
    if transaction.content(&atlas)?.is_some() {
        return Ok(crate::forms::PageLayout {
            layout: "Atlas_Core.Atlas_Default".to_string(),
            parameter: LAYOUT_PARAMETER.to_string(),
        });
    }
    ensure_module_layout(transaction, root, module_name)?;
    Ok(crate::forms::PageLayout::of_module(
        module_name,
        LAYOUT_PARAMETER,
    ))
}

/// Atlas's popup layout, when the project has it: where Studio Pro shows
/// an entity's edit page, over the list it was opened from.
fn popup_layout(
    transaction: &mut Transaction,
    root: &Path,
) -> Result<Option<crate::forms::PageLayout>> {
    let popup = root.join("frontend/src/components/layout/atlas_core/PopupLayout.tsx");
    Ok(transaction
        .content(&popup)?
        .is_some()
        .then(|| crate::forms::PageLayout {
            layout: "Atlas_Core.PopupLayout".to_string(),
            parameter: LAYOUT_PARAMETER.to_string(),
        }))
}

/// mxrb's `ensure_presentation` creates the module's `ApplicationLayout`
/// before a page that references it. Ported for pages only: the mxrs family
/// directories are flat, so a nanoflow has no layout dependency to satisfy.
fn ensure_module_layout(
    transaction: &mut Transaction,
    root: &Path,
    module_name: &str,
) -> Result<()> {
    // The layout a project already declares, in Rust or in the frontend,
    // is the one its pages are shown in.
    let family = root.join(format!("src/ui/layouts/{}/mod.rs", snake_case(module_name)));
    let declared = root.join(format!(
        "frontend/src/components/layout/{}/ApplicationLayout.tsx",
        snake_case(module_name)
    ));
    if transaction.content(&family)?.is_some() || transaction.content(&declared)?.is_some() {
        return Ok(());
    }
    let layout = crate::forms::application_layout(LAYOUT_PARAMETER);
    let version = forms_version(transaction, root)?;
    let document = crate::forms::layout_document(&version, &layout)?;
    crate::forms::add_forms(transaction, root, &[(module_name, document)])?;
    // The layout is the module's, shared by every page scaffolded into it:
    // it is its own scaffold, not a part of the page that needed it first.
    transaction.owned_elsewhere(
        Some(format!("layout:{module_name}.ApplicationLayout")),
        declared,
    );
    Ok(())
}

/// Which `<layer>/<concept>` folder a scaffolded artifact lives in. These are
/// the importer's own folders, so a scaffolded concept and an imported one are
/// the same file in the same place — the Mendix module is a folder *inside*
/// the concept, which `connect_concept_folder` appends.
fn module_folder(kind: ArtifactKind) -> &'static str {
    match kind {
        ArtifactKind::Entity => "domain/entities",
        ArtifactKind::Dto => "domain/dtos",
        ArtifactKind::Enumeration => "domain/enumerations",
        ArtifactKind::Constant | ArtifactKind::ScheduledEvent => "domain/documents",
        // The microflow a published operation calls is a service like any
        // other; `controllers/` is the route tables and handlers an import
        // generates from the published service itself.
        ArtifactKind::UseCase
        | ArtifactKind::Microflow
        | ArtifactKind::Validation
        | ArtifactKind::PublishedRest => "services",
        ArtifactKind::Page => "ui/pages",
        ArtifactKind::Nanoflow => "ui/nanoflows",
        ArtifactKind::ConsumedRest | ArtifactKind::Integration => "domain/integrations",
        ArtifactKind::JavaAction => "domain/actions",
        ArtifactKind::Security => "domain/module_security",
        ArtifactKind::Presentation => "ui/layouts",
        // A module is declared by a file of its own in the registry, not by a
        // folder: under a layer-first tree there is no single module home.
        ArtifactKind::Module => "domain/modules",
        ArtifactKind::FunctionalTest
        | ArtifactKind::Evaluation
        | ArtifactKind::Ci
        | ArtifactKind::Repository
        | ArtifactKind::DemoUser
        | ArtifactKind::Design => {
            unreachable!("artifact is handled outside module folders")
        }
    }
}

/// Writes one artifact file into its module folder and makes it a module of
/// the crate. That is the whole connection: the declaration registers
/// itself, so there is no aggregator to edit and no composition shape the
/// scaffold has to recognize.
/// Adds a microflow to its subject's service in `module_name`, creating
/// the service's file — and wiring it into the module's services — when
/// the service is new.
fn add_microflow(
    transaction: &mut Transaction,
    root: &Path,
    module_name: &str,
    method: &crate::service::ServiceMethod,
    entities: &[String],
) -> Result<crate::service::Placed> {
    crate::service::add_service_method(
        transaction,
        root,
        module_name,
        method,
        entities,
        |transaction, stem, source| {
            create_concept_file(
                transaction,
                root,
                module_name,
                module_folder(ArtifactKind::UseCase),
                stem,
                source,
            )
        },
    )
}

/// Adds a nanoflow to the frontend's services. Nothing names it in Rust:
/// a page of the frontend calls it by its own name, and a Rust page that
/// should call it says so in its module's `in_frontend.rs`.
fn add_frontend_nanoflow(
    transaction: &mut Transaction,
    root: &Path,
    module_name: &str,
    method: &crate::nanoflow::NanoflowMethod,
    entities: &[String],
) -> Result<()> {
    crate::nanoflow::add_nanoflow(transaction, root, module_name, method, entities)
}

fn create_concept_file(
    transaction: &mut Transaction,
    root: &Path,
    module_name: &str,
    folder: &str,
    stem: &str,
    source: String,
) -> Result<()> {
    let index = connect_concept_folder(transaction, root, module_name, folder)?;
    let path = index.with_file_name(format!("{stem}.rs"));
    transaction.create(&path, source)?;
    declare_child_module(transaction, &index, stem)
}

/// Concepts that hold one file per Mendix module instead of a folder: a
/// module's declaration, and its roles.
const REGISTRY_CONCEPTS: &[&str] = &["domain/modules", "domain/module_security"];

/// Creates the `mod.rs` chain from the crate root down to `folder`, and
/// answers with the folder's own index. Every level is the shape the
/// importer writes: a header and `pub mod` declarations.
fn connect_concept_folder(
    transaction: &mut Transaction,
    root: &Path,
    module_name: &str,
    concept: &str,
) -> Result<PathBuf> {
    let src = root.join("src");
    let library = src.join("lib.rs");
    // A project whose crate root cannot be read is a project this scaffold
    // cannot wire into, whatever else happens to be present. Checked up front
    // so the guarantee does not depend on which layer already exists.
    if transaction.content(&library)?.is_none() {
        return Err(ScaffoldError::ProjectNotFound(
            library.display().to_string(),
        ));
    }
    let mut parent = library;
    let mut current = src.clone();
    // `<layer>/<concept>`, then the Mendix module inside it — except for a
    // registry concept, which stops one level short.
    let module_directory = snake_case(module_name);
    let registry = REGISTRY_CONCEPTS.contains(&concept);
    let mut segments: Vec<&str> = concept.split('/').collect();
    if !registry {
        segments.push(&module_directory);
    }
    for (depth, segment) in segments.iter().enumerate() {
        current = current.join(segment);
        let index = current.join("mod.rs");
        if transaction.content(&index)?.is_none() {
            let last = depth + 1 == segments.len();
            let header = match (last, registry) {
                (true, true) => templates::registry_index(segment),
                (true, false) => {
                    templates::empty_folder_index(module_name, segments[depth.saturating_sub(1)])
                }
                (false, _) => templates::empty_concept_index(segment),
            };
            transaction.create(&index, header)?;
        }
        // Declared at every level, not only where an index was just created:
        // a `pub mod` line removed by hand would otherwise leave everything
        // below it out of the crate, and its declarations out of the model
        // without a word.
        declare_child_module(transaction, &parent, segment)?;
        parent = index;
    }
    Ok(parent)
}

fn declare_child_module(
    transaction: &mut Transaction,
    aggregator: &Path,
    stem: &str,
) -> Result<()> {
    let source = transaction
        .content(aggregator)?
        .ok_or_else(|| ScaffoldError::AggregatorNotFound(aggregator.display().to_string()))?;
    let declaration = format!("pub mod {};", rust_module_path(stem)?);
    if source.lines().any(|line| line.trim() == declaration) {
        return Ok(());
    }
    let mut lines = source.lines().map(str::to_string).collect::<Vec<_>>();
    match lines.iter().rposition(|line| line.starts_with("pub mod ")) {
        Some(last) => {
            // Into the block of declarations the file already has, at the
            // place rustfmt would move it to: a scaffolded project stays
            // formatted without anyone running the formatter.
            let first = lines[..last]
                .iter()
                .rposition(|line| !line.starts_with("pub mod "))
                .map_or(0, |position| position + 1);
            let position = lines[first..=last]
                .iter()
                .position(|line| {
                    version_order(module_order(line), module_order(&declaration)).is_gt()
                })
                .map_or(last + 1, |offset| first + offset);
            lines.insert(position, declaration);
        }
        None => {
            let position = header_end(&lines);
            if position >= lines.len() {
                // An index that is its header alone: the declaration is the
                // file's last line, one blank line below the header.
                if lines.last().is_some_and(|line| !line.is_empty()) {
                    lines.push(String::new());
                }
                lines.push(declaration);
            } else {
                lines.splice(position..position, [declaration, String::new()]);
            }
        }
    }
    transaction.write(aggregator, join(&lines))
}

/// What rustfmt orders a `pub mod` line by: the module's name, with a raw
/// identifier sorted as the name it spells.
fn module_order(declaration: &str) -> &str {
    let name = declaration
        .trim_start_matches("pub mod ")
        .trim_end_matches(';');
    name.strip_prefix("r#").unwrap_or(name)
}

/// rustfmt's version sort, for the names a module can have: runs of digits
/// compare as numbers (`page2` before `page10`), everything else as text.
fn version_order(left: &str, right: &str) -> std::cmp::Ordering {
    fn chunk(text: &str) -> (&str, &str) {
        let numeric = text.starts_with(|character: char| character.is_ascii_digit());
        let end = text
            .find(|character: char| character.is_ascii_digit() != numeric)
            .unwrap_or(text.len());
        text.split_at(end)
    }
    let (mut left, mut right) = (left, right);
    loop {
        if left.is_empty() || right.is_empty() {
            return left.len().cmp(&right.len());
        }
        let (left_chunk, left_rest) = chunk(left);
        let (right_chunk, right_rest) = chunk(right);
        let numeric = |chunk: &str| chunk.starts_with(|character: char| character.is_ascii_digit());
        let order = if numeric(left_chunk) && numeric(right_chunk) {
            let (left_digits, right_digits) = (
                left_chunk.trim_start_matches('0'),
                right_chunk.trim_start_matches('0'),
            );
            left_digits
                .len()
                .cmp(&right_digits.len())
                .then_with(|| left_digits.cmp(right_digits))
        } else {
            left_chunk.cmp(right_chunk)
        };
        if order.is_ne() {
            return order;
        }
        (left, right) = (left_rest, right_rest);
    }
}

/// The first line after the `//!` header and the blank line following it —
/// where a `pub mod` declaration goes in a file that has none yet.
fn header_end(lines: &[String]) -> usize {
    let mut position = 0;
    while lines
        .get(position)
        .is_some_and(|line| line.starts_with("//!"))
    {
        position += 1;
    }
    if position > 0 && lines.get(position).is_some_and(|line| line.is_empty()) {
        position += 1;
    }
    position
}

fn join(lines: &[String]) -> String {
    let mut source = lines.join("\n");
    source.push('\n');
    source
}

/// A module must be declared before artifacts attach to it. Under a
/// layer-first tree the declaration is a file in the registry, not a folder —
/// and a module the importer produced counts too, which is why any concept
/// folder carrying its name answers as well.
fn require_module(root: &Path, module_name: &str) -> Result<()> {
    let stem = snake_case(module_name);
    let declaration = root.join(format!("src/domain/modules/{stem}.rs"));
    if declaration.is_file() {
        return Ok(());
    }
    let imported = [
        "src/domain/entities",
        "src/domain/dtos",
        "src/domain/enumerations",
        "src/domain/documents",
        "src/services",
        "src/ui/pages",
        "src/ui/nanoflows",
        // A module may have nothing but what the frontend declares of it.
        "frontend/src/pages",
        "frontend/src/services",
        "frontend/src/components/layout",
        "frontend/src/components/snippets",
    ]
    .iter()
    .any(|concept| root.join(concept).join(&stem).is_dir());
    if imported {
        return Ok(());
    }
    Err(ScaffoldError::ModuleNotFound(
        declaration.display().to_string(),
    ))
}

fn qualified_name(kind: ArtifactKind, name: &str) -> Result<(&str, &str)> {
    let Some((module_name, artifact_name)) = name.split_once('.') else {
        return Err(ScaffoldError::UnqualifiedName(name.to_string()));
    };
    let module_name = identifier(module_name, "module")?;
    let artifact_name = identifier(artifact_name, "artifact")?;
    if kind == ArtifactKind::Entity && RESERVED_ENTITY_NAMES.contains(&artifact_name) {
        return Err(ScaffoldError::ReservedEntityName(artifact_name.to_string()));
    }
    Ok((module_name, artifact_name))
}

fn identifier<'a>(value: &'a str, label: &'static str) -> Result<&'a str> {
    let valid = value
        .chars()
        .next()
        .is_some_and(|character| character.is_ascii_alphabetic())
        && value
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '_');
    if valid {
        Ok(value)
    } else {
        Err(ScaffoldError::InvalidIdentifier {
            label,
            value: value.to_string(),
        })
    }
}

/// Mendix names that collapse onto a Rust keyword are emitted as raw
/// identifiers (`pub mod r#type;`), which keeps the file name equal to the
/// snake-cased Mendix name. The four keywords Rust refuses to raw-escape have
/// no faithful spelling, so they are reported rather than renamed behind the
/// user's back.
fn rust_module_path(stem: &str) -> Result<String> {
    if matches!(stem, "crate" | "self" | "super" | "Self") {
        return Err(ScaffoldError::UnsupportedArtifactName(stem.to_string()));
    }
    Ok(if crate::is_rust_keyword(stem) {
        format!("r#{stem}")
    } else {
        stem.to_string()
    })
}

fn initialize_presentation(
    transaction: &mut Transaction,
    root: &Path,
    module_name: &str,
) -> Result<()> {
    // The layout is the frontend's, in its module's folder; one the
    // project already declares there or in Rust is not replaced.
    let directory = snake_case(module_name);
    let declared = root
        .join("frontend/src/components/layout")
        .join(&directory)
        .join("ApplicationLayout.tsx");
    let rust = root
        .join("src/ui/layouts")
        .join(&directory)
        .join("application_layout.rs");
    for existing in [&declared, &rust] {
        if transaction.content(existing)?.is_some() {
            return Err(ScaffoldError::FileExists(existing.display().to_string()));
        }
    }
    require_module(root, module_name)?;
    let version = forms_version(transaction, root)?;
    let document = crate::forms::layout_document(&version, &crate::forms::presentation_layout())?;
    crate::forms::add_forms(transaction, root, &[(module_name, document)])
}

#[cfg(test)]
mod tests {
    use super::version_order;
    use std::cmp::Ordering;

    #[test]
    fn modules_are_ordered_the_way_rustfmt_orders_them() {
        assert_eq!(version_order("page2", "page10"), Ordering::Less);
        assert_eq!(version_order("page10", "page2"), Ordering::Greater);
        assert_eq!(version_order("page02", "page2"), Ordering::Equal);
        assert_eq!(version_order("order", "order_line"), Ordering::Less);
        assert_eq!(version_order("order2", "order_line"), Ordering::Less);
        assert_eq!(version_order("billing", "main"), Ordering::Less);
        assert_eq!(
            version_order("v1_10_orders", "v1_9_orders"),
            Ordering::Greater
        );
        assert_eq!(version_order("sales", "sales"), Ordering::Equal);
    }
}

#[cfg(test)]
mod crud_tests {
    use super::*;
    use crate::forms::CrudKind;

    /// An edit page is shown in Atlas's popup layout when the project has
    /// it, and nowhere new when it does not.
    #[test]
    fn the_edit_page_opens_in_atlas_s_popup_when_the_project_has_it() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let mut transaction = Transaction::default();
        assert!(popup_layout(&mut transaction, root).unwrap().is_none());
        let layouts = root.join("frontend/src/components/layout/atlas_core");
        std::fs::create_dir_all(&layouts).unwrap();
        std::fs::write(layouts.join("PopupLayout.tsx"), "export default {};\n").unwrap();
        let popup = popup_layout(&mut transaction, root).unwrap().unwrap();
        assert_eq!(popup.layout, "Atlas_Core.PopupLayout");
        assert_eq!(popup.parameter, "Main");
    }

    /// An entity is read as Rust reads it, however its struct is laid out.
    #[test]
    fn an_entity_is_read_as_its_struct_declares_it() {
        let source = r#"
use mxrs::prelude::*;

/// A kind of animal.
#[entity(module = "Main", name = "Animal_Type")]
#[mxrs(index(code))]
pub struct AnimalType
{
    #[mxrs(length = 80, required)] pub code: MxString,
    /// How many legs.
    #[mxrs(
        default = 4
    )]
    pub(crate) legs : MxInteger,
    #[mxrs(name = "DOB")]
    pub born: MxDateTime,
    pub r#type: Size,
    pub tame: MxBool,
    pub picture: MxBinary,
    pub keepers: mxrs::prelude::ReferenceSet<Keeper>,
}

#[dto(module = "Main")]
pub struct Unrelated {
    pub name: MxString,
}
"#;
        let declared = declared_entity(source, "Main", "Animal_Type")
            .unwrap()
            .unwrap();
        let attributes: Vec<(&str, CrudKind)> = declared
            .attributes
            .iter()
            .map(|attribute| (attribute.name.as_str(), attribute.holds))
            .collect();
        assert_eq!(
            attributes,
            [
                ("Code", CrudKind::Text),
                ("Legs", CrudKind::Text),
                ("DOB", CrudKind::DateTime),
                ("Type", CrudKind::Choice),
                ("Tame", CrudKind::Boolean),
            ]
        );
        let left_out: Vec<&str> = declared
            .left_out
            .iter()
            .map(|(field, _)| field.as_str())
            .collect();
        assert_eq!(left_out, ["Picture", "Keepers"]);
        // Named by what the model calls it, in its own module, and an
        // entity: not the struct's identifier, and not a dto.
        assert!(
            declared_entity(source, "Main", "AnimalType")
                .unwrap()
                .is_none()
        );
        assert!(
            declared_entity(source, "Sales", "Animal_Type")
                .unwrap()
                .is_none()
        );
        assert!(
            declared_entity(source, "Main", "Unrelated")
                .unwrap()
                .is_none()
        );
        assert!(declared_entity("pub struct {", "Main", "X").is_err());
    }
}
