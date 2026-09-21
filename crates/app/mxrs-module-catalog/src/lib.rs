//! The private module catalog and installer — mxrb's older, pre-official
//! module distribution mechanism (`lib/mxrb/marketplace.rb`), kept
//! separate from `mxrs-marketplace`'s official Mendix Content API client.
//! A [`Catalog`] is a small JSON document (local file or HTTPS URL) naming
//! modules by a `source`: a local directory, a git remote, or (in mxrb) a
//! slug bundled inside the mxrb gem itself.
//!
//! **Scope**: local-directory and git sources install and update fully,
//! offline-testable through an injectable git runner. A `builtin:` source
//! is refused with a named, honest error — mxrs bundles no built-in
//! module tree the way mxrb's gem does (`marketplace/modules/shared-kernel`
//! shipped inside the mxrb repository itself), and fabricating an
//! equivalent here would be inventing content, not porting it.
//!
//! The manifest filename inside an installed package (`mxrb-module.json`)
//! is kept exactly as mxrb spells it, so a catalog or git repository
//! authored for one tool installs identically through the other. The
//! lockfile path follows this project's own convention
//! (`.mxrs/modules.lock.json`, not mxrb's `.mxrb/modules.lock.json`).

pub mod catalog;
pub mod installer;
mod mendix_version;

pub use catalog::{Catalog, CatalogTransport, Entry};
pub use installer::{Installation, Installer};

#[derive(Debug, thiserror::Error)]
pub enum ModuleCatalogError {
    #[error("invalid marketplace catalog: {0}")]
    InvalidCatalog(String),

    #[error("marketplace catalogs must use HTTPS")]
    InsecureCatalogSource,

    #[error("catalog request failed with HTTP {0}")]
    CatalogRequestFailed(u16),

    #[error("cannot read marketplace catalog: {0}")]
    CatalogUnreadable(String),

    #[error("module {0:?} was not found in the catalog")]
    NotFound(String),

    #[error("module {name:?} version {version:?} was not found in the catalog")]
    VersionNotFound { name: String, version: String },

    #[error("could not fetch {name}: {message}")]
    FetchFailed { name: String, message: String },

    #[error(
        "mxrs bundles no built-in module tree; {0:?} is a builtin: source, which only mxrb's \
         gem can resolve. Point the catalog entry at a local directory or a git repository \
         instead."
    )]
    BuiltinModulesNotBundled(String),

    #[error("invalid built-in module slug {0:?}")]
    InvalidBuiltinSlug(String),

    #[error("invalid module manifest: {0}")]
    InvalidManifest(String),

    #[error("module manifest is missing: {0}")]
    MissingManifest(String),

    #[error("invalid module name {0:?}")]
    InvalidModuleName(String),

    #[error("unsafe module path {0:?}")]
    UnsafeModulePath(String),

    #[error("module file is missing: {0}")]
    MissingModuleFile(String),

    #[error("module package has no files")]
    EmptyPackage,

    #[error("module destination already exists: {0}")]
    DestinationExists(String),

    #[error("module update cannot change its name from {installed:?} to {replacement:?}")]
    RenameOnUpdate {
        installed: String,
        replacement: String,
    },

    #[error("module requires Mendix {required}, but the project uses {actual}")]
    IncompatibleMendixVersion { required: String, actual: String },

    #[error("missing dependencies: {0} — install them first")]
    MissingDependencies(String),

    #[error("no modules are installed (lock file missing)")]
    NoModulesInstalled,

    #[error("module {0:?} is not installed")]
    NotInstalled(String),

    #[error("cannot remove {module:?}: {dependents} depend on it")]
    HasDependents { module: String, dependents: String },

    #[error("invalid modules lock {path}: {message}")]
    InvalidLock { path: String, message: String },

    #[error("cannot access {path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },

    #[error("MPR error: {0}")]
    Mpr(#[from] mxrs_mpr::MprError),
}

pub type Result<T> = std::result::Result<T, ModuleCatalogError>;
