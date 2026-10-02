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
/// then application, infrastructure and the user interface — so a project reads
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
    /// Project security: user roles and the password policy.
    Security,
    /// Demo users. They join the project security declared before them,
    /// or — in a project that declares none — the one its model stores.
    DemoUser,
    TaskQueue,
    Microflow,
    /// The OQL documents views read from.
    ViewSource,
    Layout,
    Nanoflow,
    Navigation,
    /// Items declared apart from the navigation profile they join.
    NavigationItem,
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
    // `file!()` spells a path with the separator of the machine that
    // compiled it; compared as written, `a\b.rs` and `a/b.rs` would order
    // the same tree differently on Windows.
    declarations.sort_by_cached_key(|declaration| {
        (
            declaration.stage,
            declaration.file.replace('\\', "/"),
            declaration.line,
        )
    });
    declarations
}

/// Applies every declaration `crate_name` registered to `project`.
pub fn apply(crate_name: &str, project: &mut ProjectDecl) {
    for declaration in declarations(crate_name) {
        declaration.apply(project);
    }
}

/// Applies every declaration `crate_name` registered to `project`, with
/// what the frontend declares in its place among them: its navigation at
/// the navigation stage, so the items Rust pages add for themselves join
/// it. A navigation the frontend declares is the project's only one.
pub fn apply_with_frontend(
    crate_name: &str,
    project: &mut ProjectDecl,
    frontend: mxrs_frontend::FrontendDecl,
) -> Result<(), mxrs_frontend::FrontendError> {
    let mut navigation = frontend.navigation;
    let declarations = declarations(crate_name);
    if navigation.is_some() {
        let rust = declarations
            .iter()
            .find(|declaration| declaration.stage == Stage::Navigation);
        if let Some(rust) = rust {
            return Err(mxrs_frontend::FrontendError::Duplicate(format!(
                "{}:{}",
                rust.file, rust.line
            )));
        }
        if project.navigation.is_some() {
            return Err(mxrs_frontend::FrontendError::Duplicate(
                "the project's entry point".to_string(),
            ));
        }
    }
    for declaration in declarations {
        if declaration.stage > Stage::Navigation
            && let Some(declared) = navigation.take()
        {
            project.navigation = Some(declared);
        }
        declaration.apply(project);
    }
    if let Some(declared) = navigation {
        project.navigation = Some(declared);
    }
    Ok(())
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
