//! Selective Mendix Java proxy generation.
//!
//! Scans a project's user-owned `javasource/**/*.java`, finds the generated
//! proxies those sources reference, and materializes only the missing files.
//! Existing files are never overwritten. The implementation ports mxrb's
//! `compiler/java_proxy_generator.rb`, including entity dependency closure,
//! System-model entities/enumerations, microflows, constants and the Java
//! action registrar.

mod generator;
mod source_model;

pub use generator::JavaProxyGenerator;

#[derive(Debug, thiserror::Error)]
pub enum JavaGenError {
    #[error(transparent)]
    Mpr(#[from] mxrs_mpr::MprError),
    #[error(transparent)]
    SystemModel(#[from] mxrs_schema::SystemModelError),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("MPR has no Mendix product version")]
    MissingVersion,
}

pub type Result<T> = std::result::Result<T, JavaGenError>;
