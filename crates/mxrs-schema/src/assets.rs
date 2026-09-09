//! Embedded, version-specific schema assets. Raw content only — parsing and
//! using these for compile-time compatibility analysis is Phase 5's job
//! (`mxrs-compiler-domain`, porting `compiler/{runtime_model_schema,
//! compatibility_analyzer}.rb`); this crate just makes the bytes available
//! without any runtime file I/O.
//!
//! Ports `lib/mxrb/compiler/schemas/{runtime-11.json,system-model-11.12.1.b64}`
//! from mxrb. Only the 11.x assets are embedded per the locked MVP scope —
//! widening to the full 5.21-11.12 matrix means adding the corresponding
//! files here alongside version-dispatch logic.

/// The Studio Pro 11 runtime metamodel schema (JSON).
pub const RUNTIME_SCHEMA_11: &str = include_str!("../assets/runtime-11.json");

/// Base64-encoded seed system-model documents for a fresh 11.12.1 project.
/// The decoded bytes are gzip-compressed (magic bytes `1f 8b 08`) — whoever
/// consumes this (e.g. a future `mxrs-scaffold`) needs to inflate it before
/// BSON-decoding the contained documents.
pub const SYSTEM_MODEL_SEED_11_12_1: &str = include_str!("../assets/system-model-11.12.1.b64");

/// The fresh-project bootstrap template for 11.12.1: a JSON manifest of
/// `Settings$ProjectSettings`/`Texts$SystemTextCollection`/
/// `Projects$ProjectConversion` native units, extracted from a real
/// Studio-Pro-created blank project (`lib/mxrb/templates/project/11.12.1.json`
/// in mxrb). See `project_template` for the parsed form.
pub const PROJECT_TEMPLATE_11_12_1: &str = include_str!("../assets/project-template-11.12.1.json");

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine;

    #[test]
    fn runtime_schema_is_well_formed_json() {
        assert!(RUNTIME_SCHEMA_11.trim_start().starts_with('{'));
    }

    #[test]
    fn system_model_seed_is_valid_base64() {
        // The asset wraps base64 across many lines; strip whitespace first.
        let cleaned: String = SYSTEM_MODEL_SEED_11_12_1
            .chars()
            .filter(|c| !c.is_whitespace())
            .collect();
        let decoded = base64::engine::general_purpose::STANDARD.decode(cleaned);
        assert!(decoded.is_ok());
        assert!(!decoded.unwrap().is_empty());
    }
}
