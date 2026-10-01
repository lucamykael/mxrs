//! Self-registration of declarations.
//!
//! An application does not list what it contains. Each declaration —
//! `#[mxrs::entity]`, `#[mxrs::microflow]`, a page — submits itself here, and
//! `#[mxrs::application]` assembles the model from whatever its own crate
//! submitted. Adding an artifact is therefore one file and one `pub mod`
//! line; there is no aggregator to keep in step with the tree.

use mxrs_ir::ProjectDecl;

/// Where a declaration takes its place when the model is assembled.
///
/// The order is the order the layers used to be composed in — domain first,
/// then application, infrastructure and presentation — so a project reads
/// the same to the writer whether it lists its declarations or registers
/// them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Stage {
    /// Constants, regular expressions, scheduled events and menus.
    Document,
    /// Persistable entities, data transfer objects and views.
    Entity,
    Enumeration,
    /// One module's roles.
    ModuleSecurity,
    /// Project security: user roles, password policy, demo users.
    Security,
    TaskQueue,
    Microflow,
    /// The OQL documents views read from.
    ViewSource,
    Layout,
    Nanoflow,
    Navigation,
    Page,
}

/// One declaration an application crate contributes to its model.
pub struct Declaration {
    stage: Stage,
    module_path: &'static str,
    file: &'static str,
    line: u32,
    apply: fn(&mut ProjectDecl),
}

impl Declaration {
    /// Called by the declaration macros; `module_path`, `file` and `line`
    /// are the expansion site's.
    pub const fn new(
        stage: Stage,
        module_path: &'static str,
        file: &'static str,
        line: u32,
        apply: fn(&mut ProjectDecl),
    ) -> Self {
        Self {
            stage,
            module_path,
            file,
            line,
            apply,
        }
    }

    pub fn stage(&self) -> Stage {
        self.stage
    }

    /// The Rust module the declaration was written in.
    pub fn module_path(&self) -> &'static str {
        self.module_path
    }

    /// The crate the declaration belongs to.
    pub fn crate_name(&self) -> &'static str {
        crate_of(self.module_path)
    }

    pub fn file(&self) -> &'static str {
        self.file
    }

    pub fn line(&self) -> u32 {
        self.line
    }

    pub fn apply(&self, project: &mut ProjectDecl) {
        (self.apply)(project);
    }
}

inventory::collect!(Declaration);

/// The crate a `module_path!()` value belongs to.
pub fn crate_of(module_path: &str) -> &str {
    module_path.split("::").next().unwrap_or(module_path)
}

/// Every declaration `crate_name` registered, in assembly order.
///
/// The linker decides the order declarations are discovered in, so it is
/// never relied on: declarations sort by stage, then by where they were
/// written. That makes the assembled model a function of the source tree
/// alone — the same on every machine and in every build.
pub fn declarations(crate_name: &str) -> Vec<&'static Declaration> {
    let mut declarations = inventory::iter::<Declaration>
        .into_iter()
        .filter(|declaration| declaration.crate_name() == crate_name)
        .collect::<Vec<_>>();
    declarations.sort_by_key(|declaration| (declaration.stage, declaration.file, declaration.line));
    declarations
}

/// Applies every declaration `crate_name` registered to `project`.
pub fn apply(crate_name: &str, project: &mut ProjectDecl) {
    for declaration in declarations(crate_name) {
        declaration.apply(project);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_module_path_names_its_crate_first() {
        assert_eq!(crate_of("shop::domain::entities::sales::order"), "shop");
        assert_eq!(crate_of("shop"), "shop");
    }

    #[test]
    fn stages_follow_the_layer_composition_order() {
        assert!(Stage::Document < Stage::Entity);
        assert!(Stage::Entity < Stage::Security);
        assert!(Stage::Security < Stage::Microflow);
        assert!(Stage::Microflow < Stage::Page);
    }
}
