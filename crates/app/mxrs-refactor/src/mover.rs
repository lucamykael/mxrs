//! Unit relocation. Ports `lib/mxrb/semantic/mover.rb`, both halves.
//!
//! A same-module move relocates one unit and touches nothing else. A
//! cross-module move changes the artifact's qualified name — and therefore
//! every reference to it — so, exactly like mxrb, it composes the renamer
//! (`cross_module: true`) with the relocation and applies both inside one
//! transaction: rewritten documents first, then the containment row. Neither
//! half is ever done implicitly on its own.

use mxrs_model::Project;

use crate::renamer::{RenamePlan, plan_rename_scoped};
use crate::{ArtifactSummary, RefactorError, Result, build_index, is_refactorable_unit, resolve};

/// Where a unit currently sits and where it would sit. A cross-module move
/// additionally carries the rename plan whose rewritten references keep the
/// model consistent — mxrb's `CrossModuleMovePlan`.
#[derive(Debug, Clone)]
pub struct MovePlan {
    pub artifact: ArtifactSummary,
    pub before_container: String,
    pub after_container: String,
    /// Containment slot the unit is filed under, e.g. `"Documents"`.
    pub containment: String,
    /// `Some` iff the destination lives in another module.
    pub rename: Option<RenamePlan>,
    unit_id: String,
}

impl MovePlan {
    /// A move to the container the artifact already sits in. mxrb reports
    /// this as "Already in container" rather than as an error, and so does
    /// this: asking for a state that already holds is not a failure. A
    /// cross-module move is never empty (mxrb's `CrossModuleMovePlan#empty?`
    /// is always false).
    pub fn is_empty(&self) -> bool {
        self.rename.is_none() && self.before_container == self.after_container
    }

    /// The artifact's qualified name after the move.
    pub fn target_name(&self) -> &str {
        self.rename
            .as_ref()
            .map(|rename| rename.target.as_str())
            .unwrap_or(self.artifact.qualified_name.as_str())
    }

    pub fn apply(self, project: &mut Project) -> Result<()> {
        if self.is_empty() {
            return Ok(());
        }
        let mpr = project.mpr_mut();
        mpr.transaction(|mpr| {
            // Rewritten references land first, then the containment row —
            // the same single-transaction order as mxrb's
            // `CrossModuleMovePlan#apply!`.
            if let Some(rename) = &self.rename {
                for (unit_id, document) in rename.documents() {
                    mpr.update_unit(unit_id, document.clone())?;
                }
            }
            mpr.relocate_unit(&self.unit_id, &self.after_container, &self.containment)?;
            Ok(())
        })?;
        Ok(())
    }
}

/// Plans moving `name`'s unit under `container`.
///
/// `container` names the destination artifact (typically a module or folder);
/// it is resolved through the same semantic index as `name`, so a typo is an
/// "unknown artifact" error rather than a unit filed under a nonexistent
/// parent.
pub fn plan_move(project: &Project, name: &str, container: &str) -> Result<MovePlan> {
    let index = build_index(project)?;
    let artifact = resolve(&index, name)?;
    if !is_refactorable_unit(artifact.kind) {
        return Err(RefactorError::NotAUnit {
            name: artifact.qualified_name.clone(),
            kind: artifact.kind.as_str(),
        });
    }
    let Some(unit_id) = artifact.id.clone() else {
        return Err(RefactorError::NotAUnit {
            name: artifact.qualified_name.clone(),
            kind: artifact.kind.as_str(),
        });
    };

    let destination = index
        .resolve(container)?
        .ok_or_else(|| RefactorError::UnknownContainer(container.to_string()))?;
    let destination_id = destination
        .id
        .clone()
        .ok_or_else(|| RefactorError::UnknownContainer(container.to_string()))?;

    // mxrb restricts destinations to modules and folders; folders are not
    // indexed artifacts here, so a module is the only resolvable container.
    if destination.kind != mxrs_semantic::ArtifactKind::Module {
        return Err(RefactorError::NotAContainer {
            name: destination.qualified_name.clone(),
            kind: destination.kind.as_str(),
        });
    }

    // Moving a unit under its own descendant would detach the subtree from
    // the model graph — mxrb refuses it up front and so does this.
    if descendant_ids(project, &unit_id)?.contains(&destination_id) {
        return Err(RefactorError::MoveIntoDescendant {
            name: artifact.qualified_name.clone(),
            container: destination.qualified_name.clone(),
        });
    }

    // Moving between modules changes the artifact's qualified name, and
    // every reference to it with it — so the plan composes the renamer's
    // cross-module mode with the relocation (mxrb's `plan_cross_module`).
    let rename = match (artifact.module.as_deref(), module_of(&index, destination)) {
        (Some(from), Some(to)) if from != to => Some(plan_rename_scoped(
            project,
            &artifact.qualified_name,
            &format!("{to}.{}", artifact.name),
            true,
        )?),
        _ => None,
    };

    let unit = project
        .mpr()
        .unit(&unit_id)?
        .ok_or_else(|| RefactorError::NotAUnit {
            name: artifact.qualified_name.clone(),
            kind: artifact.kind.as_str(),
        })?;

    Ok(MovePlan {
        artifact: ArtifactSummary::of(artifact),
        before_container: unit.container_id.clone(),
        after_container: destination_id,
        containment: unit.containment_name.clone(),
        rename,
        unit_id,
    })
}

/// Every unit stored beneath `root_id`, excluding `root_id` itself.
fn descendant_ids(project: &Project, root_id: &str) -> Result<Vec<String>> {
    let mpr = project.mpr();
    let mut ids = Vec::new();
    let mut frontier = vec![root_id.to_string()];
    while let Some(parent) = frontier.pop() {
        for child in mpr.children_of(&parent)? {
            if !ids.contains(&child.unit_id) {
                ids.push(child.unit_id.clone());
                frontier.push(child.unit_id);
            }
        }
    }
    Ok(ids)
}

/// A module is its own module; anything else reports the module it lives in.
fn module_of<'a>(
    _index: &'a mxrs_semantic::SemanticIndex,
    artifact: &'a mxrs_semantic::Artifact,
) -> Option<&'a str> {
    if artifact.kind == mxrs_semantic::ArtifactKind::Module {
        Some(artifact.qualified_name.as_str())
    } else {
        artifact.module.as_deref()
    }
}
