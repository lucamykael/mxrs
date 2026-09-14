//! Unit relocation. Ports `lib/mxrb/semantic/mover.rb`'s same-container half.
//!
//! mxrb's mover also handles the cross-module case by delegating to its
//! renamer with `cross_module: true`, because moving an artifact between
//! modules changes its qualified name and therefore every reference to it.
//! That composition is available here too — `plan_move` refuses a
//! cross-module target and names `rename` as the operation that changes a
//! qualified name — rather than silently doing half of each. Doing both in one
//! step is a worthwhile follow-up; doing it implicitly is not.

use mxrs_model::Project;

use crate::{ArtifactSummary, RefactorError, Result, build_index, is_refactorable_unit, resolve};

/// Where a unit currently sits and where it would sit.
#[derive(Debug, Clone)]
pub struct MovePlan {
    pub artifact: ArtifactSummary,
    pub before_container: String,
    pub after_container: String,
    /// Containment slot the unit is filed under, e.g. `"Documents"`.
    pub containment: String,
    unit_id: String,
}

impl MovePlan {
    /// A move to the container the artifact already sits in. mxrb reports
    /// this as "Already in container" rather than as an error, and so does
    /// this: asking for a state that already holds is not a failure.
    pub fn is_empty(&self) -> bool {
        self.before_container == self.after_container
    }

    pub fn apply(self, project: &mut Project) -> Result<()> {
        if self.is_empty() {
            return Ok(());
        }
        let mpr = project.mpr_mut();
        mpr.transaction(|mpr| {
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

    // Moving between modules would change the artifact's qualified name, and
    // every reference to it with it. That is a rename, and pretending
    // otherwise would leave the model referring to a name nothing has.
    if let (Some(from), Some(to)) = (artifact.module.as_deref(), module_of(&index, destination))
        && from != to
    {
        return Err(RefactorError::CrossContainerRename {
            kind: artifact.kind.as_str(),
            from: artifact.qualified_name.clone(),
            to: format!("{to}.{}", artifact.name),
        });
    }

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
        unit_id,
    })
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
