//! Storage-independent project/module/entity declarations. Nothing in this
//! module knows about BSON field names, UUIDs, raw documents, export levels,
//! or other `.mpr` representation details. `mxrs-writer` lowers these values
//! into `mxrs-model` only at the persistence boundary.

use crate::flow::MicroflowDecl;
use crate::page::{LayoutDecl, PageDecl};
use crate::{ModuleRoleDecl, NavigationDecl, ProjectSecurityDecl};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// An author-level attribute kind.
///
/// The storage model uses a separate enum and is lowered by `mxrs-writer`:
///
/// ```compile_fail
/// let _ = mxrs_ir::AttributeDecl::new("Name", "String");
/// ```
pub enum AttributeType {
    String,
    Integer,
    Long,
    Float,
    Decimal,
    Boolean,
    DateTime,
    AutoNumber,
    HashString,
    Binary,
    Enumeration,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttributeDecl {
    pub name: String,
    pub documentation: String,
    pub attribute_type: AttributeType,
    pub default_value: Option<String>,
    pub length: Option<i32>,
    pub localize_date: Option<bool>,
    /// Qualified name of the backing enumeration for
    /// [`AttributeType::Enumeration`].
    pub enumeration: Option<String>,
    pub required: bool,
    pub unique: bool,
}

impl AttributeDecl {
    pub fn new(name: impl Into<String>, attribute_type: AttributeType) -> Self {
        Self {
            name: name.into(),
            documentation: String::new(),
            attribute_type,
            default_value: None,
            length: None,
            localize_date: None,
            enumeration: None,
            required: false,
            unique: false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// Association cardinality at the declaration boundary.
///
/// Raw strings cannot cross this boundary:
///
/// ```compile_fail
/// let _: mxrs_ir::AssociationType = "Reference";
/// ```
pub enum AssociationType {
    Reference,
    ReferenceSet,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AssociationOwner {
    #[default]
    Default,
    Both,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AssociationStorage {
    #[default]
    Column,
    Table,
}

#[derive(Debug, Clone)]
pub struct AssociationDecl {
    pub name: String,
    /// The referenced entity, as `"Entity"` (same module) or
    /// `"Module.Entity"` (cross-module).
    pub target: String,
    pub association_type: AssociationType,
    pub owner: AssociationOwner,
    pub storage: AssociationStorage,
    pub documentation: String,
}

#[derive(Debug, Clone)]
pub struct EntityDecl {
    pub name: String,
    pub documentation: String,
    pub persistable: bool,
    pub attributes: Vec<AttributeDecl>,
    pub associations: Vec<AssociationDecl>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnumerationValueDecl {
    pub name: String,
    /// Localized captions as `(language_code, text)` pairs.
    pub captions: Vec<(String, String)>,
}

impl EnumerationValueDecl {
    pub fn new(name: impl Into<String>) -> Self {
        let name = name.into();
        Self {
            captions: vec![("en_US".to_string(), name.clone())],
            name,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnumerationDecl {
    pub name: String,
    pub documentation: String,
    pub values: Vec<EnumerationValueDecl>,
}

impl EnumerationDecl {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            documentation: String::new(),
            values: vec![],
        }
    }
}

impl EntityDecl {
    pub fn new(name: impl Into<String>) -> Self {
        EntityDecl {
            name: name.into(),
            documentation: String::new(),
            persistable: true,
            attributes: vec![],
            associations: vec![],
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct ModuleDecl {
    pub name: String,
    pub entities: Vec<EntityDecl>,
    pub enumerations: Vec<EnumerationDecl>,
    pub microflows: Vec<MicroflowDecl>,
    /// Client-side flows. They share the semantic flow IR with microflows,
    /// but persist as `Microflows$Nanoflow` documents and have an independent
    /// identity namespace.
    pub nanoflows: Vec<MicroflowDecl>,
    pub pages: Vec<PageDecl>,
    pub layouts: Vec<LayoutDecl>,
    /// `None` preserves imported module security. `Some` is authoritative,
    /// including an explicitly empty role set.
    pub roles: Option<Vec<ModuleRoleDecl>>,
}

#[derive(Debug, Clone)]
pub struct ProjectDecl {
    pub mendix_version: String,
    pub modules: Vec<ModuleDecl>,
    /// `None` preserves imported project security verbatim. `Some` makes the
    /// typed declaration authoritative for its modeled fields.
    pub security: Option<ProjectSecurityDecl>,
    /// `None` preserves imported navigation verbatim. `Some` makes the
    /// declared profile collection authoritative.
    pub navigation: Option<NavigationDecl>,
}

impl ProjectDecl {
    /// Folds `declared` into the module of the same name, or appends it when
    /// the project has no such module yet.
    ///
    /// Exists so a project's declarations can be split across several source
    /// files without any of them having to restate the module's other
    /// artifacts, and so a declaration layer added on top of an imported
    /// project extends that module instead of declaring a second one under the
    /// same name — which the writer would then synchronize twice.
    ///
    /// `roles` replaces instead of appending: `Some` is authoritative for
    /// module security (see [`ModuleDecl::roles`]), so appending would let a
    /// declaration that deliberately empties the role set keep the roles it
    /// means to drop. A merge that declares no roles leaves the existing value
    /// untouched rather than resetting it to `None`.
    pub fn merge_module(&mut self, declared: ModuleDecl) {
        let Some(target) = self
            .modules
            .iter_mut()
            .find(|module| module.name == declared.name)
        else {
            self.modules.push(declared);
            return;
        };
        target.entities.extend(declared.entities);
        target.enumerations.extend(declared.enumerations);
        target.microflows.extend(declared.microflows);
        target.nanoflows.extend(declared.nanoflows);
        target.pages.extend(declared.pages);
        target.layouts.extend(declared.layouts);
        if let Some(roles) = declared.roles {
            target.roles = Some(roles);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ModuleRoleDecl;

    fn module(name: &str) -> ModuleDecl {
        ModuleDecl {
            name: name.to_string(),
            ..ModuleDecl::default()
        }
    }

    #[test]
    fn merging_appends_artifacts_of_a_known_module_and_pushes_an_unknown_one() {
        let mut project = ProjectDecl {
            mendix_version: "11.12.1".into(),
            modules: vec![module("Sales")],
            security: None,
            navigation: None,
        };
        project.modules[0].entities.push(EntityDecl::new("Order"));
        let mut declared = module("Sales");
        declared.entities.push(EntityDecl::new("Invoice"));
        declared
            .enumerations
            .push(EnumerationDecl::new("PaymentStatus"));
        project.merge_module(declared);
        assert_eq!(project.modules.len(), 1);
        let names = project.modules[0]
            .entities
            .iter()
            .map(|entity| entity.name.as_str())
            .collect::<Vec<_>>();
        assert_eq!(names, ["Order", "Invoice"]);
        assert_eq!(project.modules[0].enumerations.len(), 1);
        project.merge_module(module("Billing"));
        assert_eq!(project.modules.len(), 2);
        assert_eq!(project.modules[1].name, "Billing");
    }

    #[test]
    fn merging_replaces_declared_roles_but_never_clears_undeclared_ones() {
        let mut project = ProjectDecl {
            mendix_version: "11.12.1".into(),
            modules: vec![module("Sales")],
            security: None,
            navigation: None,
        };
        project.modules[0].roles = Some(vec![ModuleRoleDecl::new("User")]);
        project.merge_module(module("Sales"));
        assert_eq!(
            project.modules[0].roles.as_ref().map(|roles| roles.len()),
            Some(1)
        );
        let mut declared = module("Sales");
        declared.roles = Some(vec![]);
        project.merge_module(declared);
        assert_eq!(project.modules[0].roles.as_deref(), Some(&[][..]));
    }
}
