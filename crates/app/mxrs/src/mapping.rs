//! The model's integration mappings, as declarations a generated project can
//! read and a runtime boundary can apply.
//!
//! An export mapping describes the JSON document a Mendix object graph turns
//! into: which attributes appear, under which keys, and which associated
//! objects nest beneath them. A published REST operation that declares one
//! answers with that document instead of the entity's stored attributes, so
//! the generated project keeps a mapping declaration per mapping the model
//! publishes and applies it at the boundary.
//!
//! Applying it reads the store, including retrieves the mapping performs
//! itself to reach associated objects — so apply it through
//! [`mxrs::ports::FlowEngine::apply_export_mapping`][engine], which asks the
//! caller's entity-read rules about every object the document would carry.
//! [`ExportMapping::apply`] asks no rules and is named for the absence of a
//! caller to ask about.
//!
//! [engine]: crate::ports::FlowEngine::apply_export_mapping
//!
//! ```
//! use mxrs::mapping::{ExportMapping, ObjectMapping};
//!
//! // `(Array)|(Object)` at the root, one key renamed, one associated object
//! // nested — exactly what the model's element tree declares.
//! let mapping = ExportMapping::new(
//!     ObjectMapping::array("Sales.Order")
//!         .attribute("OrderNumber")
//!         .value("placed_at", "OrderDate")
//!         .child(
//!             "customer",
//!             "Sales.Order_Customer",
//!             ObjectMapping::object("Sales.Customer").attribute("Name"),
//!         ),
//! );
//! assert!(mapping.root().is_multiple());
//! ```

pub use mxrs_runtime_flows::{ExportMapping, NullValues, ObjectMapping, ValueMapping};

use mxrs_ir::declaration::ProjectDecl;
use mxrs_ir::{MappingDirection, NativeDocument};

/// The export mapping `qualified` — `Module.EM_Name` — as `crate_name`
/// declares it: the document its declaration writes against the JSON
/// structure it maps, both of them declared, shaped into what the boundary
/// applies. A project imported with both declared reads its runtime mapping
/// here, so the declaration is the mapping's one source.
///
/// # Panics
///
/// When `crate_name` does not declare the mapping and its structure, or
/// they state no document of objects — a mistake in the declarations the
/// project's own test of the mapping names.
pub fn declared(crate_name: &str, qualified: &str) -> ExportMapping {
    declared_document(crate_name, qualified)
        .and_then(|document| {
            ExportMapping::from_document(&document)
                .ok_or_else(|| format!("{qualified} maps no document of objects"))
        })
        .unwrap_or_else(|error| panic!("export mapping {error}"))
}

/// The document the export mapping `qualified` writes, or why there is none.
fn declared_document(crate_name: &str, qualified: &str) -> Result<NativeDocument, String> {
    let mut project = ProjectDecl {
        mendix_version: String::new(),
        modules: Vec::new(),
        security: None,
        navigation: None,
        demo_users: Vec::new(),
    };
    // Mappings and JSON structures are documents: nothing else need be
    // assembled to read them.
    for declaration in crate::registry::declarations(crate_name) {
        if declaration.stage() == crate::registry::Stage::Document {
            declaration.apply(&mut project);
        }
    }
    let find = |qualified: &str| {
        let (module, name) = qualified.split_once('.')?;
        project
            .modules
            .iter()
            .find(|candidate| candidate.name == module)
            .map(|module| (module, name.to_string()))
    };
    let (module, name) =
        find(qualified).ok_or_else(|| format!("{qualified}: its module declares nothing"))?;
    let mapping = module
        .mappings
        .iter()
        .find(|mapping| mapping.name == name && mapping.direction == MappingDirection::Export)
        .ok_or_else(|| format!("{qualified} is not declared"))?;
    let structure = find(&mapping.json_structure)
        .and_then(|(module, name)| {
            module
                .json_structures
                .iter()
                .find(|structure| structure.name == name)
        })
        .ok_or_else(|| {
            format!(
                "{qualified}: its JSON structure {} is not declared",
                mapping.json_structure
            )
        })?;
    mapping
        .document(structure)
        .map_err(|error| format!("{qualified}: {error}"))
}
