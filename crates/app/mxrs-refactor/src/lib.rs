//! Reference-safe model refactoring over an existing `.mpr` — the mxrs
//! counterpart of mxrb's `rename`, `remove` and `move` commands
//! (`lib/mxrb/semantic/{renamer,remover,mover}.rb`).
//!
//! **Plan first, mutate only on request.** Every operation returns a plan
//! describing exactly what would change, down to the individual BSON string
//! and the path it sits at. Nothing touches the file until [`RenamePlan::apply`]
//! / [`RemovalPlan::apply`] / [`MovePlan::apply`] is called, and applying runs
//! inside an `mxrs-mpr` transaction so a failure part-way leaves the `.mpr`
//! as it was. This mirrors mxrb, whose CLI previews by default and writes only
//! under `--apply`, and it is the reason the preview is a value rather than
//! printed text: the same plan drives both the human rendering and the apply.
//!
//! **No query language.** Renaming is a textual substitution over decoded BSON
//! strings, guarded by identifier boundaries, exactly as mxrb does it. That is
//! a deliberate limit, not an oversight: Mendix stores references as qualified
//! name strings scattered across document types this workspace does not model
//! exhaustively, so a substitution that is visible in the preview is safer
//! than a typed rewrite that silently misses the documents it does not know
//! about. The preview exists so the substitution can be audited before it
//! lands.
//!
//! What that limit costs, stated plainly: a rename cannot distinguish a
//! reference to `Sales.Order` from the same text inside a document type whose
//! meaning is unknown to us. Free-text fields are excluded by name
//! ([`METADATA_FIELDS`]) because a rename should not rewrite prose, but any
//! other field holding the qualified name is rewritten. Read the preview.

use std::collections::BTreeMap;

use mxrs_bson::{Bson, Document};
use mxrs_model::Project;
use mxrs_semantic::{Artifact, ArtifactKind, SemanticIndex};

mod mover;
mod remover;
mod renamer;

pub use mover::{MovePlan, plan_move};
pub use remover::{IncomingReference, RemovalPlan, plan_remove};
pub use renamer::{RenameChange, RenamePlan, plan_rename};

/// Free-text fields a rename never rewrites. Mirrors mxrb's
/// `Renamer::METADATA_FIELDS`: documentation and captions are prose written by
/// a human, and a rename that edited them would be changing meaning rather
/// than fixing a reference.
pub const METADATA_FIELDS: &[&str] = &[
    "Documentation",
    "documentation",
    "Caption",
    "caption",
    "Text",
    "text",
];

#[derive(Debug, thiserror::Error)]
pub enum RefactorError {
    #[error("semantic index error: {0}")]
    Semantic(#[from] mxrs_semantic::SemanticError),

    #[error("model error: {0}")]
    Model(#[from] mxrs_model::ModelError),

    #[error("MPR I/O error: {0}")]
    Mpr(#[from] mxrs_mpr::MprError),

    #[error("new name cannot be empty")]
    EmptyName,

    #[error("invalid Mendix name {0:?}")]
    InvalidName(String),

    #[error("{kind} rename must keep its qualification depth ({from} -> {to})")]
    QualificationDepthChanged {
        kind: &'static str,
        from: String,
        to: String,
    },

    #[error(
        "{kind} rename cannot move the artifact to another container ({from} -> {to}); use `move` for that"
    )]
    CrossContainerRename {
        kind: &'static str,
        from: String,
        to: String,
    },

    #[error("Mendix artifact {0:?} already exists")]
    NameCollision(String),

    #[error(
        "{kind} {name:?} is not a removable or movable unit; it is part of another document, and changing it is a typed domain-model mutation"
    )]
    NotAUnit { name: String, kind: &'static str },

    #[error("container {0:?} was not found in this project")]
    UnknownContainer(String),

    #[error("plan was already applied")]
    AlreadyApplied,

    #[error(
        "refusing to remove {name:?}: {incoming} artifact(s) still reference it and it has {children} child unit(s)"
    )]
    RemovalBlocked {
        name: String,
        incoming: usize,
        children: usize,
    },
}

pub type Result<T> = std::result::Result<T, RefactorError>;

/// The subset of an [`Artifact`] a plan needs to describe itself, owned so a
/// plan outlives the index it was built from.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ArtifactSummary {
    pub qualified_name: String,
    pub kind: String,
    pub unit_id: Option<String>,
}

impl ArtifactSummary {
    fn of(artifact: &Artifact) -> Self {
        Self {
            qualified_name: artifact.qualified_name.clone(),
            kind: artifact.kind.as_str().to_string(),
            unit_id: artifact.id.clone(),
        }
    }
}

/// Opens the project's semantic index and resolves `name`, turning the two
/// failure modes every refactoring shares — unknown and ambiguous — into
/// errors before any planning work happens.
fn resolve<'index>(index: &'index SemanticIndex, name: &str) -> Result<&'index Artifact> {
    Ok(index.require(name)?)
}

pub(crate) fn build_index(project: &Project) -> Result<SemanticIndex> {
    Ok(SemanticIndex::build(project)?)
}

/// Replaces `needle` with `replacement` in `haystack`, but only where the
/// match is not part of a longer identifier.
///
/// Hand-rolled rather than a regex because the guard is exactly two character
/// classes and the needle is arbitrary user text that would otherwise need
/// escaping. `Sales.Order` must not match inside `Sales.OrderLine`, and
/// `Order` must not match inside `ReorderPoint`.
pub(crate) fn replace_bounded(haystack: &str, needle: &str, replacement: &str) -> String {
    if needle.is_empty() || !haystack.contains(needle) {
        return haystack.to_string();
    }
    let bytes = haystack.as_bytes();
    let mut out = String::with_capacity(haystack.len());
    let mut cursor = 0;
    while let Some(offset) = haystack[cursor..].find(needle) {
        let start = cursor + offset;
        let end = start + needle.len();
        let before_ok = start == 0 || !is_identifier_byte(bytes[start - 1]);
        let after_ok = end == bytes.len() || !is_identifier_byte(bytes[end]);
        if before_ok && after_ok {
            out.push_str(&haystack[cursor..start]);
            out.push_str(replacement);
        } else {
            out.push_str(&haystack[cursor..end]);
        }
        cursor = end;
    }
    out.push_str(&haystack[cursor..]);
    out
}

fn is_identifier_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

/// Mendix spells a member reference two ways: `Module.Entity.Attribute` in
/// some documents and `Module.Entity/Attribute` in others (XPath-flavoured
/// ones). A rename that only handled the dotted form would leave the other
/// half dangling, so both are substituted — same pair mxrb builds in
/// `replace_qualified`.
pub(crate) fn reference_spellings(qualified_name: &str) -> Vec<String> {
    let mut spellings = vec![qualified_name.to_string()];
    if qualified_name.matches('.').count() == 2
        && let Some(position) = qualified_name.rfind('.')
    {
        let mut slashed = qualified_name.to_string();
        slashed.replace_range(position..=position, "/");
        spellings.push(slashed);
    }
    spellings
}

/// Fields whose value *is* the artifact's own name rather than a reference to
/// it. Only rewritten on the artifact's own object, so an unrelated document
/// that happens to have a `Name` equal to the short name is left alone.
pub(crate) fn is_declaration_field(field: &str) -> bool {
    matches!(field, "Name" | "name" | "$QualifiedName")
}

/// Every unit in the project, parsed once, keyed by unit id. Both rename and
/// removal need the whole file; reading it once keeps a plan O(units) rather
/// than O(units × artifacts).
pub(crate) fn parsed_units(project: &Project) -> Result<BTreeMap<String, Document>> {
    let mut units = BTreeMap::new();
    for unit in project.all_units()? {
        let document = project.mpr().parse_contents(&unit)?;
        units.insert(unit.unit_id.clone(), document);
    }
    Ok(units)
}

/// Context handed to [`map_strings`]' visitor for one string.
pub(crate) struct StringSite<'a> {
    pub value: &'a str,
    /// Owning field name, i.e. the last path segment.
    pub field: &'a str,
    /// Dotted field path inside the unit, array indices included.
    pub path: &'a [String],
    /// Whether this string sits inside the artifact's own object — the only
    /// place a declaration field may be rewritten.
    pub in_target_object: bool,
}

/// Walks a decoded BSON value, applying `visit` to every string, and returns
/// the rewritten value plus whether anything changed.
///
/// The visitor receives the live path rather than the caller reconstructing it
/// afterwards: matching rewritten strings back to their locations by value
/// would mis-attribute any string that already equalled the replacement.
pub(crate) fn map_strings(
    value: &Bson,
    path: &mut Vec<String>,
    in_target_object: bool,
    target_object_id: Option<&str>,
    visit: &mut dyn FnMut(StringSite<'_>) -> Option<String>,
) -> (Bson, bool) {
    match value {
        Bson::Document(document) => {
            let here = in_target_object
                || target_object_id.is_some_and(|id| {
                    document
                        .get("$ID")
                        .and_then(mxrs_bson::extract_id)
                        .is_some_and(|actual| actual == id)
                });
            let mut changed = false;
            let mut out = Document::new();
            for (key, child) in document {
                path.push(key.clone());
                let (mapped, child_changed) =
                    map_strings(child, path, here, target_object_id, visit);
                path.pop();
                changed |= child_changed;
                out.insert(key, mapped);
            }
            (Bson::Document(out), changed)
        }
        Bson::Array(items) => {
            let mut changed = false;
            let mut out = Vec::with_capacity(items.len());
            for (index, child) in items.iter().enumerate() {
                path.push(index.to_string());
                // An array element is never itself "the target object" for
                // declaration-field purposes; only documents carry `$ID`.
                let (mapped, child_changed) =
                    map_strings(child, path, in_target_object, target_object_id, visit);
                path.pop();
                changed |= child_changed;
                out.push(mapped);
            }
            (Bson::Array(out), changed)
        }
        Bson::String(text) => {
            let field = path.last().map(String::as_str).unwrap_or_default();
            let site = StringSite {
                value: text,
                field,
                path,
                in_target_object,
            };
            match visit(site) {
                Some(replacement) if replacement != *text => (Bson::String(replacement), true),
                _ => (value.clone(), false),
            }
        }
        other => (other.clone(), false),
    }
}

/// Kinds that are one document in one storage unit, and can therefore be
/// moved or removed by operating on that unit.
///
/// Mirrors mxrb's `Remover::EMBEDDED_KINDS` exclusion, inverted: modules,
/// entities, attributes and associations are all refused. The last three are
/// fields inside their entity's domain-model document rather than units of
/// their own, and a module is a container whose removal is a domain-model
/// mutation, not a unit delete. mxrb says "removal requires a typed
/// domain-model mutation" for exactly these, and refusing is better than
/// deleting a unit that means something else.
pub(crate) fn is_refactorable_unit(kind: ArtifactKind) -> bool {
    matches!(
        kind,
        ArtifactKind::Microflow
            | ArtifactKind::Nanoflow
            | ArtifactKind::Rule
            | ArtifactKind::Page
            | ArtifactKind::Layout
            | ArtifactKind::Snippet
            | ArtifactKind::Enumeration
            | ArtifactKind::Constant
            | ArtifactKind::ScheduledEvent
            | ArtifactKind::Menu
            | ArtifactKind::JavaAction
            | ArtifactKind::JavaScriptAction
            | ArtifactKind::Workflow
    )
}

/// Structural containment, not usage.
///
/// The index records a `contains` edge from every container to what it holds,
/// so a microflow always has at least one incoming edge — from its own module.
/// Counting those as references would make nothing removable, which is why
/// "is anything still using this?" means "is there a non-containment edge?".
pub(crate) const CONTAINMENT_RELATION: &str = "contains";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounded_replacement_never_matches_inside_a_longer_identifier() {
        assert_eq!(
            replace_bounded("Sales.Order", "Sales.Order", "Sales.Invoice"),
            "Sales.Invoice"
        );
        // The prefix of a longer name must survive untouched.
        assert_eq!(
            replace_bounded("Sales.OrderLine", "Sales.Order", "Sales.Invoice"),
            "Sales.OrderLine"
        );
        assert_eq!(
            replace_bounded("ReorderPoint", "Order", "Invoice"),
            "ReorderPoint"
        );
        // Several independent occurrences in one expression all move.
        assert_eq!(
            replace_bounded(
                "$Sales.Order/Total > 0 and $Sales.Order/Active",
                "Sales.Order",
                "Sales.Invoice"
            ),
            "$Sales.Invoice/Total > 0 and $Sales.Invoice/Active"
        );
        assert_eq!(
            replace_bounded("nothing here", "Order", "Invoice"),
            "nothing here"
        );
    }

    #[test]
    fn a_member_reference_is_substituted_in_both_spellings() {
        assert_eq!(
            reference_spellings("Sales.Order.Number"),
            ["Sales.Order.Number", "Sales.Order/Number"]
        );
        // Two-part names have only one spelling.
        assert_eq!(reference_spellings("Sales.Order"), ["Sales.Order"]);
    }
}
