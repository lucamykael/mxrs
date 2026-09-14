//! Reference-checked removal. Ports `lib/mxrb/semantic/remover.rb`.

use mxrs_model::Project;

use crate::{
    ArtifactSummary, CONTAINMENT_RELATION, RefactorError, Result, build_index,
    is_refactorable_unit, resolve,
};

/// Something that still points at the artifact being removed.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct IncomingReference {
    /// Qualified name of the referring artifact, or its index key when the
    /// referrer is not itself a named artifact.
    pub from: String,
    pub relation: String,
}

/// A removal preview.
///
/// Removal is refused while anything still references the artifact — a
/// dangling reference produces an `.mpr` Studio Pro will not open, so this is
/// a hard stop rather than a warning. It is equally refused while the unit
/// still has children, matching mxrb: deleting a parent whose children the
/// caller has not seen is a bigger operation than the one they asked for.
#[derive(Debug, Clone)]
pub struct RemovalPlan {
    pub artifact: ArtifactSummary,
    pub incoming: Vec<IncomingReference>,
    /// Units stored under this one. Non-empty means blocked, not cascaded.
    pub children: Vec<String>,
    unit_id: String,
}

impl RemovalPlan {
    /// Whether applying is allowed. Mirrors mxrb's `plan.safe?`, which its
    /// CLI turns into a nonzero exit.
    pub fn is_safe(&self) -> bool {
        self.incoming.is_empty() && self.children.is_empty()
    }

    pub fn apply(self, project: &mut Project) -> Result<()> {
        if !self.is_safe() {
            return Err(RefactorError::RemovalBlocked {
                name: self.artifact.qualified_name.clone(),
                incoming: self.incoming.len(),
                children: self.children.len(),
            });
        }
        let mpr = project.mpr_mut();
        mpr.transaction(|mpr| {
            mpr.delete_unit(&self.unit_id)?;
            Ok(())
        })?;
        Ok(())
    }
}

pub fn plan_remove(project: &Project, name: &str) -> Result<RemovalPlan> {
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

    let incoming = index
        .incoming(name)?
        .into_iter()
        .filter(|reference| reference.relation != CONTAINMENT_RELATION)
        // An artifact referring to itself does not keep itself alive.
        .filter(|reference| reference.from != artifact.key)
        .map(|reference| IncomingReference {
            // Prefer the referrer's qualified name; fall back to its index key
            // so a reference from something unnamed is still reported rather
            // than dropped.
            from: index
                .resolve(&reference.from)
                .ok()
                .flatten()
                .map_or_else(|| reference.from.clone(), |a| a.qualified_name.clone()),
            relation: reference.relation.clone(),
        })
        .collect();

    let children = project
        .mpr()
        .children_of(&unit_id)?
        .into_iter()
        .map(|unit| unit.unit_id)
        .collect();

    Ok(RemovalPlan {
        artifact: ArtifactSummary::of(artifact),
        incoming,
        children,
        unit_id,
    })
}
