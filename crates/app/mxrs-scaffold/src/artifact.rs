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
//! mxrs scaffolds Rust into
//! `src/{domain,application,presentation}/modules/<module>/<family>/`, compiled
//! by `cargo`. Two consequences follow from that and are not stylistic:
//!
//! 1. Rust module paths must be identifiers, so directories are snake_cased
//!    (`src/domain/modules/sales`, not `.../Sales`). The Mendix name stays
//!    verbatim inside the generated declaration and in the registry key.
//! 2. Aggregators cannot be "a list of files to evaluate". Each one exposes an
//!    `apply` function plus a `const` table of function pointers, so adding an
//!    artifact is a two-line edit (`pub mod x;` and one table entry) with no
//!    ambiguity about where a line goes.
//!
//! Wiring into `build()` recognizes the two shapes mxrs itself generates (the
//! `mxrs new` scaffold and the `mxrs import` layout). Anything else fails with
//! [`ScaffoldError::UnrecognizedProjectBuild`] naming the file rather than
//! guessing where a call belongs in source the user restructured.

use std::path::{Path, PathBuf};

use crate::templates::{DECLARATIONS_LIST, DECLARE, FAMILIES_LIST, MODULES_LIST, snake_case};
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
        destination: "src/presentation/modules/<module>",
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
        destination: "src/application/modules/<module>/integrations",
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
        destination: "src/domain/modules/<module>/constants",
        kind: ArtifactKind::Constant,
    },
    ScaffoldCommand {
        name: "integration",
        action: "new",
        argument: "<Module.Adapter>",
        summary: "Create an integration adapter microflow",
        destination: "src/application/modules/<module>/integrations",
        kind: ArtifactKind::Integration,
    },
    ScaffoldCommand {
        name: "entity",
        action: "new",
        argument: "<Module.Entity>",
        summary: "Create a domain entity declaration",
        destination: "src/domain/modules/<module>/entities",
        kind: ArtifactKind::Entity,
    },
    ScaffoldCommand {
        name: "enumeration",
        action: "new",
        argument: "<Module.Enumeration>",
        summary: "Create an enumeration declaration",
        destination: "src/domain/modules/<module>/enumerations",
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
        destination: "src/application/modules/<module>/actions",
        kind: ArtifactKind::JavaAction,
    },
    ScaffoldCommand {
        name: "module",
        action: "new",
        argument: "<Module>",
        summary: "Create an editable module declaration layer",
        destination: "src/domain/modules/<module>",
        kind: ArtifactKind::Module,
    },
    ScaffoldCommand {
        name: "nanoflow",
        action: "new",
        argument: "<Module.Flow>",
        summary: "Create a client nanoflow declaration",
        destination: "src/presentation/modules/<module>/nanoflows",
        kind: ArtifactKind::Nanoflow,
    },
    ScaffoldCommand {
        name: "page",
        action: "new",
        argument: "<Module.Page>",
        summary: "Create a page declaration and its module layout",
        destination: "src/presentation/modules/<module>/pages",
        kind: ArtifactKind::Page,
    },
    ScaffoldCommand {
        name: "published-rest",
        action: "new",
        argument: "<Module.Handler>",
        summary: "Create a published REST handler microflow",
        destination: "src/application/modules/<module>/endpoints",
        kind: ArtifactKind::PublishedRest,
    },
    ScaffoldCommand {
        name: "repository",
        action: "new",
        argument: "<Module.Name>",
        summary: "Create a repository port and infrastructure adapter",
        destination: "src/{application,infrastructure}/repositories",
        kind: ArtifactKind::Repository,
    },
    ScaffoldCommand {
        name: "scheduled-event",
        action: "new",
        argument: "<Module.Event>",
        summary: "Create a scheduled event and its handler microflow",
        destination: "src/application/modules/<module>/jobs",
        kind: ArtifactKind::ScheduledEvent,
    },
    ScaffoldCommand {
        name: "security",
        action: "init",
        argument: "<Module>",
        summary: "Create module roles and project security",
        destination: "src/domain/modules/<module>/security",
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
        destination: "src/domain/security/demo_users",
        kind: ArtifactKind::DemoUser,
    },
    ScaffoldCommand {
        name: "validation",
        action: "new",
        argument: "<Module.Flow>",
        summary: "Create an application validation microflow",
        destination: "src/application/modules/<module>/validations",
        kind: ArtifactKind::Validation,
    },
    ScaffoldCommand {
        name: "use-case",
        action: "new",
        argument: "<Module.Flow>",
        summary: "Create an application use-case microflow",
        destination: "src/application/modules/<module>/use_cases",
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
            Self::Module => "module",
            Self::Presentation => "presentation",
        }
    }

    fn family(self) -> &'static str {
        match self {
            Self::Entity => "entities",
            Self::Enumeration => "enumerations",
            Self::Constant => "constants",
            Self::ScheduledEvent => "jobs",
            Self::UseCase => "use_cases",
            Self::Page => "pages",
            Self::Nanoflow => "nanoflows",
            Self::PublishedRest => "endpoints",
            Self::ConsumedRest => "integrations",
            Self::JavaAction => "actions",
            Self::FunctionalTest => "functional_tests",
            Self::Evaluation => "evaluations",
            Self::Validation => "validations",
            Self::Integration => "integrations",
            Self::Ci => "ci",
            Self::Repository => "repositories",
            Self::Security | Self::Module | Self::DemoUser => "security",
            Self::Presentation => "presentation",
        }
    }

    fn layer(self) -> &'static str {
        match self {
            Self::Entity | Self::Enumeration | Self::Constant | Self::Security | Self::Module => {
                "domain"
            }
            Self::Page | Self::Nanoflow | Self::Presentation => "presentation",
            Self::ScheduledEvent
            | Self::UseCase
            | Self::PublishedRest
            | Self::ConsumedRest
            | Self::JavaAction
            | Self::Validation
            | Self::Integration => "application",
            Self::FunctionalTest
            | Self::Evaluation
            | Self::Ci
            | Self::Repository
            | Self::DemoUser => {
                unreachable!("artifact is handled outside layered module families")
            }
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
/// `src/domain/modules`; the verbatim Mendix names appear in
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
    if let Some(entries) = optional_directory(&root.join("src/domain/modules"))? {
        for entry in entries {
            let path = entry
                .map_err(|error| io_error(&root.join("src/domain/modules"), error))?
                .path();
            if !is_hidden(&path)
                && path.join("mod.rs").is_file()
                && let Some(name) = path.file_name().and_then(|name| name.to_str())
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

fn create_module_layer(
    transaction: &mut Transaction,
    root: &Path,
    module_name: &str,
) -> Result<()> {
    let directory = snake_case(module_name);
    let aggregator = root.join(format!("src/domain/modules/{directory}/mod.rs"));
    if aggregator.is_file() {
        return Err(ScaffoldError::ModuleExists(
            aggregator.display().to_string(),
        ));
    }
    connect_modules_aggregator(transaction, root, "domain")?;
    transaction.create(&aggregator, templates::module_aggregator(module_name))?;
    let modules = root.join("src/domain/modules/mod.rs");
    let stem = rust_module_path(&directory)?;
    declare_child_module(transaction, &modules, &directory)?;
    append_list_entry(
        transaction,
        &modules,
        MODULES_LIST,
        &format!("{stem}::apply"),
    )
}

/// Ports mxrb's `scaffold_demo_user` to the Cargo-native layout: the
/// declaration lands in `src/domain/security/demo_users/`, the generated
/// password lands in a `0o600` `.env` at the project root (with an empty
/// `.env.example` key for sharing), and the demo-user aggregator is wired
/// into `build()` after `security::apply`. Role and entity references are
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
    let security = root.join("src/domain/security/mod.rs");
    let security_source = transaction
        .content(&security)?
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

    let aggregator = root.join("src/domain/security/demo_users/mod.rs");
    if transaction.content(&aggregator)?.is_none() {
        transaction.create(&aggregator, templates::demo_users_aggregator())?;
        declare_child_module(transaction, &security, "demo_users")?;
        connect_build(
            transaction,
            root,
            "security::demo_users::apply(&mut project);",
        )?;
    }
    let stem = snake_case(name);
    let file = aggregator.with_file_name(format!("{stem}.rs"));
    transaction.create(
        &file,
        templates::demo_user(name, &entity, &roles, &password_env),
    )?;
    declare_child_module(transaction, &aggregator, &stem)?;
    append_list_entry(
        transaction,
        &aggregator,
        DECLARATIONS_LIST,
        &format!("{}::{DECLARE}", rust_module_path(&stem)?),
    )?;

    if !options.dry_run {
        ensure_demo_user_secret(transaction, root, &password_env)?;
    }
    Ok(())
}

/// User roles the generated security declaration names: every
/// `security.role("Name", …)` (security-init template) or
/// `UserRoleDecl { name: "Name".to_string(), … }` /
/// `UserRoleDecl::new("Name")` (imported struct literal) occurrence under
/// `src/domain/security/mod.rs`.
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
        "src/domain/modules/{}/entities/{}.rs",
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

fn create_module_security(
    transaction: &mut Transaction,
    root: &Path,
    module_name: &str,
) -> Result<()> {
    require_module(root, module_name)?;
    create_family_file(
        transaction,
        root,
        module_name,
        ArtifactKind::Security,
        "module_roles",
        templates::module_roles(module_name),
    )?;
    let security = root.join("src/domain/security/mod.rs");
    if transaction.content(&security)?.is_none() {
        transaction.create(&security, templates::project_security(module_name))?;
        declare_child_module(transaction, &root.join("src/domain/mod.rs"), "security")?;
        connect_build(transaction, root, "security::apply(&mut project);")?;
    }
    Ok(())
}

/// A `--template`/`--chain` page is not one file but a slice: optionally a
/// backing entity and loader, the refresh flow(s) the chain names, and the
/// page itself. Mirrors mxrb's `scaffold_templated_page`/`page_support_specs`
/// with one deliberate omission: mxrb also writes a navigation entry per page
/// into `app/navigation/responsive/`, and mxrs has no navigation aggregator to
/// write into — its `mxrs new` scaffold declares navigation inline in
/// `build()`. Appending to that by hand is the guessing this crate refuses to
/// do elsewhere (see [`ScaffoldError::UnrecognizedProjectBuild`]), so the
/// generated page is reachable by reference but not linked into a menu.
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
        create_family_file(
            transaction,
            root,
            module_name,
            ArtifactKind::Entity,
            &stem,
            templates::page_chain_entity(module_name, artifact_name),
        )?;
        create_family_file(
            transaction,
            root,
            module_name,
            ArtifactKind::UseCase,
            &format!("act_load_{stem}"),
            templates::page_chain_loader(module_name, artifact_name),
        )?;
    }
    if chain.is_some_and(PageChain::has_microflow) {
        create_family_file(
            transaction,
            root,
            module_name,
            ArtifactKind::UseCase,
            &format!("act_refresh_{stem}"),
            templates::page_chain_action(module_name, artifact_name),
        )?;
    }
    if let Some(chain) = chain.filter(|chain| chain.has_nanoflow()) {
        create_family_file(
            transaction,
            root,
            module_name,
            ArtifactKind::Nanoflow,
            &format!("nan_refresh_{stem}"),
            templates::page_chain_nanoflow(module_name, artifact_name, chain.has_microflow()),
        )?;
    }

    let refresh = chain.map(|chain| {
        if chain.has_nanoflow() {
            templates::RefreshAction::Nanoflow
        } else {
            templates::RefreshAction::Microflow
        }
    });
    create_family_file(
        transaction,
        root,
        module_name,
        ArtifactKind::Page,
        &stem,
        templates::page_from_template(
            module_name,
            artifact_name,
            LAYOUT_PARAMETER,
            template.name,
            refresh,
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
        | ArtifactKind::DemoUser => {
            unreachable!("handled by the caller")
        }
    };
    create_family_file(
        transaction,
        root,
        module_name,
        options.kind,
        &snake_case(artifact_name),
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
    let port_root = root.join("src/application");
    let port_family = port_root.join("repositories");
    connect_plain_family(
        transaction,
        &root.join("src"),
        "application",
        "repositories",
    )?;
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
    let family = root.join(format!(
        "src/presentation/modules/{}/layouts/mod.rs",
        snake_case(module_name)
    ));
    if transaction.content(&family)?.is_some() {
        return Ok(());
    }
    connect_family(
        transaction,
        root,
        module_name,
        ArtifactKind::Page,
        "layouts",
    )?;
    let file = family.with_file_name("application_layout.rs");
    transaction.create(&file, templates::layouts(module_name, LAYOUT_PARAMETER))?;
    declare_child_module(transaction, &family, "application_layout")?;
    append_list_entry(
        transaction,
        &family,
        DECLARATIONS_LIST,
        &format!("application_layout::{DECLARE}"),
    )
}

fn create_family_file(
    transaction: &mut Transaction,
    root: &Path,
    module_name: &str,
    kind: ArtifactKind,
    stem: &str,
    source: String,
) -> Result<()> {
    let family = kind.family();
    let layer = kind.layer();
    connect_family(transaction, root, module_name, kind, family)?;
    let aggregator = root.join(format!(
        "src/{layer}/modules/{}/{family}/mod.rs",
        snake_case(module_name)
    ));
    let path = aggregator.with_file_name(format!("{stem}.rs"));
    let rust_path = rust_module_path(stem)?;
    transaction.create(&path, source)?;
    declare_child_module(transaction, &aggregator, stem)?;
    append_list_entry(
        transaction,
        &aggregator,
        DECLARATIONS_LIST,
        &format!("{rust_path}::{DECLARE}"),
    )
}

fn connect_family(
    transaction: &mut Transaction,
    root: &Path,
    module_name: &str,
    kind: ArtifactKind,
    family: &str,
) -> Result<()> {
    let layer = kind.layer();
    let directory = snake_case(module_name);
    let module = root.join(format!("src/{layer}/modules/{directory}/mod.rs"));
    if transaction.content(&module)?.is_none() {
        connect_modules_aggregator(transaction, root, layer)?;
        transaction.create(&module, templates::module_aggregator(module_name))?;
        let modules = root.join(format!("src/{layer}/modules/mod.rs"));
        let stem = rust_module_path(&directory)?;
        declare_child_module(transaction, &modules, &directory)?;
        append_list_entry(
            transaction,
            &modules,
            MODULES_LIST,
            &format!("{stem}::apply"),
        )?;
    }
    let aggregator = root.join(format!("src/{layer}/modules/{directory}/{family}/mod.rs"));
    if transaction.content(&aggregator)?.is_some() {
        return Ok(());
    }
    transaction.create(
        &aggregator,
        templates::family_aggregator(module_name, family),
    )?;
    declare_child_module(transaction, &module, family)?;
    append_list_entry(
        transaction,
        &module,
        FAMILIES_LIST,
        &format!("{family}::apply"),
    )
}

fn connect_modules_aggregator(
    transaction: &mut Transaction,
    root: &Path,
    layer: &str,
) -> Result<()> {
    let modules = root.join(format!("src/{layer}/modules/mod.rs"));
    if transaction.content(&modules)?.is_some() {
        return Ok(());
    }
    transaction.create(&modules, templates::modules_aggregator())?;
    let layer_module = root.join(format!("src/{layer}/mod.rs"));
    declare_child_module(transaction, &layer_module, "modules")?;
    match layer {
        "domain" => connect_build(transaction, root, "modules::apply(&mut project);"),
        "application" => connect_layer_apply(
            transaction,
            &layer_module,
            "modules::apply(&mut project);",
            IMPORTED_BUILD_TAIL,
        ),
        "presentation" => connect_layer_apply(
            transaction,
            &layer_module,
            "modules::apply(project);",
            "\n}\n",
        ),
        _ => unreachable!("known generated source layer"),
    }
}

const IMPORTED_BUILD_TAIL: &str = "\n    project\n}\n";
const FRESH_BUILD_TAIL: &str = "\n    project.build()\n}\n";

/// Adds one `apply` call to `src/domain/mod.rs`'s `build()`. Both recognized
/// tails are shapes mxrs itself generates; anything else is reported instead
/// of being rewritten, because a wrong guess here silently changes what the
/// next `cargo mxrs build` writes into the model.
fn connect_build(transaction: &mut Transaction, root: &Path, call: &str) -> Result<()> {
    let path = root.join("src/domain/mod.rs");
    let source = transaction
        .content(&path)?
        .ok_or_else(|| ScaffoldError::ProjectNotFound(path.display().to_string()))?;
    if source.contains(&format!("\n    {call}\n")) {
        return Ok(());
    }
    let updated = if let Some(head) = source.strip_suffix(IMPORTED_BUILD_TAIL) {
        format!("{head}\n    {call}{IMPORTED_BUILD_TAIL}")
    } else if let Some(head) = source.strip_suffix(FRESH_BUILD_TAIL) {
        format!("{head}\n    let mut project = project.build();\n    {call}{IMPORTED_BUILD_TAIL}")
    } else {
        return Err(ScaffoldError::UnrecognizedProjectBuild(
            path.display().to_string(),
        ));
    };
    transaction.write(&path, updated)
}

fn connect_layer_apply(
    transaction: &mut Transaction,
    path: &Path,
    call: &str,
    tail: &str,
) -> Result<()> {
    let source = transaction
        .content(path)?
        .ok_or_else(|| ScaffoldError::ProjectNotFound(path.display().to_string()))?;
    if source.contains(&format!("\n    {call}\n")) {
        return Ok(());
    }
    let Some(head) = source.strip_suffix(tail) else {
        return Err(ScaffoldError::UnrecognizedProjectBuild(
            path.display().to_string(),
        ));
    };
    transaction.write(path, format!("{head}\n    {call}{tail}"))
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
        Some(position) => lines.insert(position + 1, declaration),
        None => {
            let position = header_end(&lines);
            lines.splice(position..position, [declaration, String::new()]);
        }
    }
    transaction.write(aggregator, join(&lines))
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

/// Rewrites the whole table instead of splicing a line into an assumed shape.
/// `cargo fmt` collapses a short slice onto one line, and a scaffold that only
/// understood the shape it had originally emitted would start failing the
/// first time a user formatted their project.
fn append_list_entry(
    transaction: &mut Transaction,
    aggregator: &Path,
    list: &str,
    entry: &str,
) -> Result<()> {
    let missing = || ScaffoldError::AggregatorNotFound(aggregator.display().to_string());
    let source = transaction.content(aggregator)?.ok_or_else(missing)?;
    let mut lines = source.lines().map(str::to_string).collect::<Vec<_>>();
    let start = lines
        .iter()
        .position(|line| line.starts_with(&format!("const {list}:")))
        .ok_or_else(missing)?;
    let end = lines
        .iter()
        .skip(start)
        .position(|line| line.trim_end().ends_with("];"))
        .map(|offset| start + offset)
        .ok_or_else(missing)?;
    let declaration = lines[start..=end].join("\n");
    let (head, body) = declaration.split_once("= &[").ok_or_else(missing)?;
    let mut entries = body
        .trim_end()
        .strip_suffix("];")
        .ok_or_else(missing)?
        .split(',')
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
        .map(str::to_string)
        .collect::<Vec<_>>();
    if entries.iter().any(|existing| existing == entry) {
        return Ok(());
    }
    entries.push(entry.to_string());
    let mut replacement = vec![format!("{head}= &[")];
    replacement.extend(entries.into_iter().map(|entry| format!("    {entry},")));
    replacement.push("];".to_string());
    lines.splice(start..=end, replacement);
    transaction.write(aggregator, join(&lines))
}

fn join(lines: &[String]) -> String {
    let mut source = lines.join("\n");
    source.push('\n');
    source
}

fn require_module(root: &Path, module_name: &str) -> Result<()> {
    let aggregator = root.join(format!(
        "src/domain/modules/{}/mod.rs",
        snake_case(module_name)
    ));
    if aggregator.is_file() {
        return Ok(());
    }
    Err(ScaffoldError::ModuleNotFound(
        aggregator.display().to_string(),
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
    let directory = snake_case(module_name);
    let base = root.join(format!("src/presentation/modules/{directory}"));
    let aggregator = base.join("mod.rs");
    let keeps = ["pages", "snippets", "nanoflows"].map(|family| base.join(family).join(".keep"));
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
    let layout_family = base.join("layouts/mod.rs");
    let layout_file = base.join("layouts/application_layout.rs");
    if transaction.content(&layout_family)?.is_none()
        && transaction.content(&layout_file)?.is_some()
    {
        return Err(ScaffoldError::FileExists(layout_file.display().to_string()));
    }
    connect_family(
        transaction,
        root,
        module_name,
        ArtifactKind::Presentation,
        "layouts",
    )?;
    if transaction.content(&layout_file)?.is_none() {
        transaction.create(&layout_file, templates::presentation_layout(module_name))?;
        declare_child_module(transaction, &layout_family, "application_layout")?;
        append_list_entry(
            transaction,
            &layout_family,
            DECLARATIONS_LIST,
            &format!("application_layout::{DECLARE}"),
        )?;
    }
    for (family, keep) in ["pages", "snippets", "nanoflows"].iter().zip(keeps) {
        connect_family(
            transaction,
            root,
            module_name,
            ArtifactKind::Presentation,
            family,
        )?;
        if transaction.content(&keep)?.is_none() {
            transaction.create(keep, String::new())?;
        }
    }
    Ok(())
}
