//! Model-wide rename. Ports `lib/mxrb/semantic/renamer.rb`.

use std::collections::BTreeMap;

use mxrs_bson::{Bson, Document};
use mxrs_model::Project;

use crate::{
    ArtifactSummary, METADATA_FIELDS, RefactorError, Result, StringSite, build_index,
    is_declaration_field, map_strings, parsed_units, reference_spellings, replace_bounded, resolve,
};

/// One rewritten BSON string, addressed precisely enough to audit.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct RenameChange {
    pub unit_id: String,
    /// Dotted field path inside the unit's document, array indices included.
    pub path: String,
    pub before: String,
    pub after: String,
}

/// An inspectable preview plus the documents an apply would write.
#[derive(Debug, Clone)]
pub struct RenamePlan {
    pub source: ArtifactSummary,
    pub target: String,
    pub changes: Vec<RenameChange>,
    documents: BTreeMap<String, Document>,
}

impl RenamePlan {
    pub fn is_empty(&self) -> bool {
        self.changes.is_empty()
    }

    pub fn affected_units(&self) -> usize {
        self.documents.len()
    }

    /// The rewritten unit documents an apply would write — the mover's
    /// cross-module composition writes them inside its own transaction.
    pub(crate) fn documents(&self) -> &BTreeMap<String, Document> {
        &self.documents
    }

    /// Writes every rewritten unit in one transaction. Consuming `self`
    /// enforces mxrb's "a plan was already applied" guard at compile time
    /// rather than at runtime.
    pub fn apply(self, project: &mut Project) -> Result<()> {
        let mpr = project.mpr_mut();
        mpr.transaction(|mpr| {
            for (unit_id, document) in &self.documents {
                mpr.update_unit(unit_id, document.clone())?;
            }
            Ok(())
        })?;
        Ok(())
    }
}

/// Plans renaming `name` to `to`.
///
/// `to` may be a bare short name (`"Invoice"`), which keeps the artifact's
/// existing container, or a fully qualified one (`"Sales.Invoice"`). Either
/// way the result must keep the same qualification depth and the same
/// container: moving an artifact elsewhere is `move`'s job, and conflating the
/// two is how a rename quietly becomes a move.
pub fn plan_rename(project: &Project, name: &str, to: &str) -> Result<RenamePlan> {
    plan_rename_scoped(project, name, to, false)
}

/// Like [`plan_rename`], but `cross_module: true` additionally allows the
/// module prefix to change — mxrb's `Renamer#plan(..., cross_module: true)`,
/// reachable only through the mover's compose-with-relocate path so a plain
/// rename can never quietly relocate an artifact.
pub(crate) fn plan_rename_scoped(
    project: &Project,
    name: &str,
    to: &str,
    cross_module: bool,
) -> Result<RenamePlan> {
    let index = build_index(project)?;
    let source = resolve(&index, name)?;
    let target = qualified_target(
        &source.qualified_name,
        source.kind.as_str(),
        to,
        cross_module,
    )?;

    // A collision check that ignored the artifact itself would reject
    // renaming `Order` to `Order`, which is a no-op rather than a conflict.
    if let Some(existing) = index.resolve(&target)?
        && existing.key != source.key
    {
        return Err(RefactorError::NameCollision(target));
    }

    let spellings: Vec<(String, String)> = reference_spellings(&source.qualified_name)
        .into_iter()
        .zip(reference_spellings(&target))
        .collect();
    let short_name = source.name.clone();
    let target_short = target
        .rsplit('.')
        .next()
        .unwrap_or(target.as_str())
        .to_string();
    let object_id = source.id.clone();
    let source_unit = source.id.clone();
    let summary = ArtifactSummary::of(source);

    let mut changes = Vec::new();
    let mut documents = BTreeMap::new();
    for (unit_id, document) in parsed_units(project)? {
        // Declaration fields may only be rewritten inside the artifact's own
        // unit; elsewhere an identical `Name` belongs to something else.
        let target_object_id = (Some(&unit_id) == source_unit.as_ref())
            .then_some(object_id.as_deref())
            .flatten();
        let mut unit_changes = Vec::new();
        let mut path = Vec::new();
        let (mapped, changed) = map_strings(
            &Bson::Document(document),
            &mut path,
            false,
            target_object_id,
            &mut |site| {
                let replacement = rewrite(&site, &spellings, &short_name, &target_short);
                if replacement != site.value {
                    unit_changes.push(RenameChange {
                        unit_id: unit_id.clone(),
                        path: site.path.join("."),
                        before: site.value.to_string(),
                        after: replacement.clone(),
                    });
                }
                Some(replacement)
            },
        );
        if !changed {
            continue;
        }
        let Bson::Document(mapped) = mapped else {
            continue;
        };
        changes.append(&mut unit_changes);
        documents.insert(unit_id, mapped);
    }

    Ok(RenamePlan {
        source: summary,
        target,
        changes,
        documents,
    })
}

fn rewrite(
    site: &StringSite<'_>,
    spellings: &[(String, String)],
    short_name: &str,
    target_short: &str,
) -> String {
    if METADATA_FIELDS.contains(&site.field) {
        return site.value.to_string();
    }
    let replaced = spellings
        .iter()
        .fold(site.value.to_string(), |current, (from, to)| {
            replace_bounded(&current, from, to)
        });
    if replaced != site.value {
        return replaced;
    }
    // Nothing matched as a reference. The artifact's own declaration still
    // carries the bare short name, which is only safe to rewrite here.
    if site.in_target_object && is_declaration_field(site.field) && site.value == short_name {
        return target_short.to_string();
    }
    site.value.to_string()
}

/// Resolves the requested new name against the artifact's current one and
/// rejects the shapes mxrb rejects.
fn qualified_target(
    current: &str,
    kind: &'static str,
    requested: &str,
    cross_module: bool,
) -> Result<String> {
    let requested = requested.trim();
    if requested.is_empty() {
        return Err(RefactorError::EmptyName);
    }
    if !is_valid_name(requested) {
        return Err(RefactorError::InvalidName(requested.to_string()));
    }
    let old_parts: Vec<&str> = current.split('.').collect();
    let new_parts: Vec<&str> = requested.split('.').collect();
    let target = if new_parts.len() == 1 {
        let mut parts = old_parts[..old_parts.len() - 1].to_vec();
        parts.push(new_parts[0]);
        parts.join(".")
    } else {
        requested.to_string()
    };
    let target_parts: Vec<&str> = target.split('.').collect();
    if target_parts.len() != old_parts.len() {
        return Err(RefactorError::QualificationDepthChanged {
            kind,
            from: current.to_string(),
            to: target,
        });
    }
    if !cross_module
        && old_parts.len() > 1
        && target_parts[..target_parts.len() - 1] != old_parts[..old_parts.len() - 1]
    {
        return Err(RefactorError::CrossContainerRename {
            kind,
            from: current.to_string(),
            to: target,
        });
    }
    Ok(target)
}

/// Mendix identifiers start with a letter or underscore; dots separate
/// qualification. Mirrors mxrb's `/\A[A-Za-z_][A-Za-z0-9_.]*\z/`.
fn is_valid_name(value: &str) -> bool {
    let mut characters = value.chars();
    characters
        .next()
        .is_some_and(|first| first.is_ascii_alphabetic() || first == '_')
        && characters
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '_' | '.'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bare_new_name_keeps_the_existing_container() {
        assert_eq!(
            qualified_target("Sales.Order", "entity", "Invoice", false).unwrap(),
            "Sales.Invoice"
        );
        assert_eq!(
            qualified_target("Sales.Order.Number", "attribute", "Reference", false).unwrap(),
            "Sales.Order.Reference"
        );
        assert_eq!(
            qualified_target("Sales", "module", "Billing", false).unwrap(),
            "Billing"
        );
    }

    #[test]
    fn a_rename_that_would_relocate_or_reshape_the_name_is_refused() {
        assert!(matches!(
            qualified_target("Sales.Order", "entity", "Billing.Order", false),
            Err(RefactorError::CrossContainerRename { .. })
        ));
        assert!(matches!(
            qualified_target("Sales.Order", "entity", "Sales.Order.Line", false),
            Err(RefactorError::QualificationDepthChanged { .. })
        ));
        assert!(matches!(
            qualified_target("Sales.Order", "entity", "  ", false),
            Err(RefactorError::EmptyName)
        ));
        for invalid in ["9Lives", "has space", "has-dash", "$Order"] {
            assert!(
                matches!(
                    qualified_target("Sales.Order", "entity", invalid, false),
                    Err(RefactorError::InvalidName(_))
                ),
                "{invalid} was accepted"
            );
        }
        // A fully qualified name identical to the current container is fine.
        assert_eq!(
            qualified_target("Sales.Order", "entity", "Sales.Invoice", false).unwrap(),
            "Sales.Invoice"
        );
        // The mover's cross-module composition is the one caller allowed to
        // change the prefix; depth still may not change.
        assert_eq!(
            qualified_target("Sales.Order", "entity", "Billing.Order", true).unwrap(),
            "Billing.Order"
        );
        assert!(matches!(
            qualified_target("Sales.Order", "entity", "Sales.Order.Line", true),
            Err(RefactorError::QualificationDepthChanged { .. })
        ));
    }
}
