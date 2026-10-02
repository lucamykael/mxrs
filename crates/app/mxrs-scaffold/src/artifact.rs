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

use std::path::{Path, PathBuf};

use crate::templates::snake_case;
use crate::transaction::Transaction;
use crate::{Result, ScaffoldError, io_error, page_templates, registry, templates};

/// Placeholder name on the scaffolded `ApplicationLayout` that scaffolded
/// pages attach their widgets to. Matches the `mxrs new` project scaffold.
const LAYOUT_PARAMETER: &str = "Main";

const RESERVED_ENTITY_NAMES: &[&str] = &["Owner", "ChangedBy", "CreatedDate", "ChangedDate"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArtifactKind {
    Entity,
    Enumeration,
    Constant,
    ScheduledEvent,
    UseCase,
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
        destination: "src/ui/layouts/<module>",
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
        destination: "src/ui/nanoflows/<module>",
        kind: ArtifactKind::Nanoflow,
    },
    ScaffoldCommand {
        name: "page",
        action: "new",
        argument: "<Module.Page>",
        summary: "Create a page declaration and its module layout",
        destination: "src/ui/pages/<module>",
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
];

impl ArtifactKind {
    /// Registry key prefix, matching mxrb's `"#{@kind}:#{@name}"` where the
    /// kind is the command name with dashes turned into underscores.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Entity => "entity",
            Self::Enumeration => "enumeration",
            Self::Constant => "constant",
            Self::ScheduledEvent => "scheduled_event",
            Self::UseCase => "use_case",
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
            demo_entity: None,
        }
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
        create_artifact(&mut transaction, &root, options, module_name, artifact_name)?;
    }

    let mut files = transaction.created().to_vec();
    let updated = transaction.updated().to_vec();
    if !options.dry_run {
        let key = format!("{}:{}", options.kind.as_str(), options.name);
        let recorded = files.clone();
        let registry_path = registry::stage(&mut transaction, &root, &key, &recorded)?;
        files.retain(|file| file != &registry_path);
        transaction.commit()?;
    }
    Ok(ScaffoldOutcome {
        kind: options.kind.as_str(),
        name: options.name.clone(),
        dry_run: options.dry_run,
        files,
        updated,
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
        && let Some(version) = text
            .lines()
            .filter_map(|line| line.trim().strip_prefix("mendix_version"))
            .filter_map(|value| value.trim_start().strip_prefix('='))
            .find_map(|value| quoted(value.trim()))
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

fn quoted(value: &str) -> Option<String> {
    let rest = value.strip_prefix('"')?;
    let end = rest.find('"')?;
    Some(rest[..end].to_string())
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
/// The page's own file also declares its navigation item, which extends the
/// Responsive profile rather than replacing its home page or any items the
/// application already declared.
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
    let template = page_templates::fetch(&template_name)?;
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
        create_concept_file(
            transaction,
            root,
            module_name,
            module_folder(ArtifactKind::Nanoflow),
            &templates::nanoflow_stem(&format!("NAN_Refresh{artifact_name}")),
            templates::page_chain_nanoflow(module_name, artifact_name, calls),
        )?;
    }

    let refresh = chain.map(|chain| {
        if chain.has_nanoflow() {
            templates::RefreshAction::Nanoflow
        } else {
            templates::RefreshAction::Microflow
        }
    });
    create_concept_file(
        transaction,
        root,
        module_name,
        module_folder(ArtifactKind::Page),
        &stem,
        templates::page_from_template(
            module_name,
            artifact_name,
            LAYOUT_PARAMETER,
            template.name,
            refresh,
            refresh_service.as_deref(),
            &options.page_roles,
        ),
    )
}

fn create_artifact(
    transaction: &mut Transaction,
    root: &Path,
    options: &ArtifactScaffold,
    module_name: &str,
    artifact_name: &str,
) -> Result<()> {
    require_module(root, module_name)?;
    if options.kind == ArtifactKind::Page {
        ensure_module_layout(transaction, root, module_name)?;
        if options.page_template.is_some() || options.page_chain.is_some() {
            return create_page_slice(transaction, root, options, module_name, artifact_name);
        }
    }
    if options.kind == ArtifactKind::Repository {
        return create_repository(transaction, root, module_name, artifact_name);
    }
    // A microflow is a method of the service of what it is about — the
    // place the importer would have put it.
    let service_docs = match options.kind {
        ArtifactKind::UseCase => Some(vec![format!(
            "Application service `{module_name}.{artifact_name}`."
        )]),
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
        ArtifactKind::Enumeration => templates::enumeration(module_name, artifact_name),
        ArtifactKind::Constant => templates::constant(module_name, artifact_name),
        ArtifactKind::ScheduledEvent => templates::scheduled_event(module_name, artifact_name),
        ArtifactKind::UseCase => templates::use_case(module_name, artifact_name),
        ArtifactKind::Nanoflow => templates::nanoflow(module_name, artifact_name),
        ArtifactKind::PublishedRest => templates::published_rest(module_name, artifact_name),
        ArtifactKind::ConsumedRest => templates::consumed_rest(module_name, artifact_name),
        ArtifactKind::JavaAction => templates::java_action(module_name, artifact_name),
        ArtifactKind::FunctionalTest => unreachable!("handled by the caller"),
        ArtifactKind::Evaluation | ArtifactKind::Ci => unreachable!("handled by the caller"),
        ArtifactKind::Validation => templates::validation(module_name, artifact_name),
        ArtifactKind::Integration => templates::integration(module_name, artifact_name),
        ArtifactKind::Repository => unreachable!("handled above"),
        ArtifactKind::Page => templates::page(
            module_name,
            artifact_name,
            LAYOUT_PARAMETER,
            &options.page_roles,
        ),
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
        ArtifactKind::Nanoflow => templates::nanoflow_stem(artifact_name),
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

/// mxrb's `ensure_presentation` creates the module's `ApplicationLayout`
/// before a page that references it. Ported for pages only: the mxrs family
/// directories are flat, so a nanoflow has no layout dependency to satisfy.
fn ensure_module_layout(
    transaction: &mut Transaction,
    root: &Path,
    module_name: &str,
) -> Result<()> {
    let family = root.join(format!("src/ui/layouts/{}/mod.rs", snake_case(module_name)));
    if transaction.content(&family)?.is_some() {
        return Ok(());
    }
    create_concept_file(
        transaction,
        root,
        module_name,
        "ui/layouts",
        "application_layout",
        templates::layouts(module_name, LAYOUT_PARAMETER),
    )
}

/// Which `<layer>/<concept>` folder a scaffolded artifact lives in. These are
/// the importer's own folders, so a scaffolded concept and an imported one are
/// the same file in the same place — the Mendix module is a folder *inside*
/// the concept, which `connect_concept_folder` appends.
fn module_folder(kind: ArtifactKind) -> &'static str {
    match kind {
        ArtifactKind::Entity => "domain/entities",
        ArtifactKind::Enumeration => "domain/enumerations",
        ArtifactKind::Constant | ArtifactKind::ScheduledEvent => "domain/documents",
        // The microflow a published operation calls is a service like any
        // other; `controllers/` is the route tables and handlers an import
        // generates from the published service itself.
        ArtifactKind::UseCase | ArtifactKind::Validation | ArtifactKind::PublishedRest => {
            "services"
        }
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
    // Layer-first: the module gets its folder inside each user-interface
    // concept, not a folder of its own.
    let directory = snake_case(module_name);
    let base = root.join("src/ui");
    let aggregator = base.join("layouts").join(&directory).join("mod.rs");
    let keeps = ["pages", "snippets", "nanoflows"]
        .map(|family| base.join(family).join(&directory).join(".keep"));
    if transaction.content(&aggregator)?.is_some()
        && keeps
            .iter()
            .map(|path| transaction.content(path))
            .collect::<Result<Vec<_>>>()?
            .iter()
            .all(Option::is_some)
    {
        return Err(ScaffoldError::FileExists(aggregator.display().to_string()));
    }
    require_module(root, module_name)?;
    let layout_file = base
        .join("layouts")
        .join(&directory)
        .join("application_layout.rs");
    // A layout file the module's own index does not know about was not put
    // there by this scaffold; refuse rather than adopt or overwrite it.
    if transaction.content(&aggregator)?.is_none() && transaction.content(&layout_file)?.is_some() {
        return Err(ScaffoldError::FileExists(layout_file.display().to_string()));
    }
    if transaction.content(&layout_file)?.is_none() {
        create_concept_file(
            transaction,
            root,
            module_name,
            "ui/layouts",
            "application_layout",
            templates::presentation_layout(module_name),
        )?;
    }
    for (family, keep) in ["pages", "snippets", "nanoflows"].iter().zip(keeps) {
        connect_concept_folder(transaction, root, module_name, &format!("ui/{family}"))?;
        if transaction.content(&keep)?.is_none() {
            transaction.create(keep, String::new())?;
        }
    }
    Ok(())
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
