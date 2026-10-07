//! Stable identities for artifacts created by `mxrs-writer`.
//!
//! A logical project name is first mapped into a UUIDv5 namespace under a
//! fixed mxrs namespace. Every new artifact is then derived from that project
//! namespace, its strongly typed [`ArtifactKind`], and its qualified name.
//! Existing `.mpr` artifacts keep their stored IDs; a writer opening an
//! existing project reconstructs this scope from its root ID and only calls
//! [`ProjectIdentity::artifact_id`] for artifacts that do not exist yet.

use std::fmt;

use thiserror::Error;
use uuid::Uuid;

const MXRS_NAMESPACE_NAME: &str = "https://github.com/lucamykael/mxrs/identity/v1";

/// The semantic role of an ID within a project namespace.
///
/// Including the role prevents equal qualified names in different artifact
/// families from colliding (for example, a module and a document both named
/// `Sales`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ArtifactKind {
    Module,
    DomainModel,
    Entity,
    Attribute,
    ValidationRule,
    Association,
    Microflow,
    Nanoflow,
    FlowParameter,
    FlowParameterType,
    FlowParameterCollection,
    Page,
    Layout,
    Enumeration,
    EnumerationValue,
    EnumerationCaption,
    AccessRule,
    AccessMember,
    EntityGeneralization,
    EntityIndex,
    EntityIndexMember,
    LifecycleCallback,
    EntitySource,
    OqlViewSource,
    OqlViewValue,
    Constant,
    /// The `Type` sub-document of a constant, which carries its own `$ID`
    /// distinct from the constant's.
    ConstantType,
    RegularExpression,
    TaskQueue,
    TaskQueueConfig,
    ScheduledEvent,
    /// The `Schedule` sub-document of a scheduled event, likewise separately
    /// identified.
    ScheduledEventSchedule,
    Menu,
    MenuCollection,
    MenuItem,
    MenuAction,
    MenuActionSettings,
    MenuIcon,
    MenuText,
    MenuTextTemplate,
    Translation,
    ProjectSettings,
    ProjectConversion,
    SystemTexts,
    ProjectSecurity,
    ModuleSecurity,
    ModuleSettings,
    JsonStructure,
    ImportMapping,
    ExportMapping,
    DataSet,
    ImageCollection,
    Image,
    ModuleRole,
    UserRole,
    DemoUser,
    PasswordPolicy,
    Navigation,
    DataStorage,
    JavaScriptAction,
    JavaAction,
}

impl fmt::Display for ArtifactKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Module => "module",
            Self::DomainModel => "domain-model",
            Self::Entity => "entity",
            Self::Attribute => "attribute",
            Self::ValidationRule => "validation-rule",
            Self::Association => "association",
            Self::Microflow => "microflow",
            Self::Nanoflow => "nanoflow",
            Self::FlowParameter => "flow-parameter",
            Self::FlowParameterType => "flow-parameter-type",
            Self::FlowParameterCollection => "flow-parameter-collection",
            Self::Page => "page",
            Self::Layout => "layout",
            Self::Enumeration => "enumeration",
            Self::EnumerationValue => "enumeration-value",
            Self::EnumerationCaption => "enumeration-caption",
            Self::AccessRule => "access-rule",
            Self::AccessMember => "access-member",
            Self::EntityGeneralization => "entity-generalization",
            Self::EntityIndex => "entity-index",
            Self::EntityIndexMember => "entity-index-member",
            Self::LifecycleCallback => "lifecycle-callback",
            Self::EntitySource => "entity-source",
            Self::OqlViewSource => "oql-view-source",
            Self::OqlViewValue => "oql-view-value",
            Self::Constant => "constant",
            Self::ConstantType => "constant-type",
            Self::RegularExpression => "regular-expression",
            Self::TaskQueue => "task-queue",
            Self::TaskQueueConfig => "task-queue-config",
            Self::ScheduledEvent => "scheduled-event",
            Self::ScheduledEventSchedule => "scheduled-event-schedule",
            Self::Menu => "menu",
            Self::MenuCollection => "menu-collection",
            Self::MenuItem => "menu-item",
            Self::MenuAction => "menu-action",
            Self::MenuActionSettings => "menu-action-settings",
            Self::MenuIcon => "menu-icon",
            Self::MenuText => "menu-text",
            Self::MenuTextTemplate => "menu-text-template",
            Self::Translation => "translation",
            Self::ProjectSettings => "project-settings",
            Self::ProjectConversion => "project-conversion",
            Self::SystemTexts => "system-texts",
            Self::ProjectSecurity => "project-security",
            Self::ModuleSecurity => "module-security",
            Self::ModuleSettings => "module-settings",
            Self::JsonStructure => "json-structure",
            Self::ImportMapping => "import-mapping",
            Self::ExportMapping => "export-mapping",
            Self::DataSet => "data-set",
            Self::ImageCollection => "image-collection",
            Self::Image => "image",
            Self::ModuleRole => "module-role",
            Self::UserRole => "user-role",
            Self::DemoUser => "demo-user",
            Self::PasswordPolicy => "password-policy",
            Self::Navigation => "navigation",
            Self::DataStorage => "data-storage",
            Self::JavaScriptAction => "javascript-action",
            Self::JavaAction => "java-action",
        })
    }
}

#[derive(Debug, Error)]
pub enum IdentityError {
    #[error("project root ID {value:?} is not a UUID: {source}")]
    InvalidProjectRoot {
        value: String,
        #[source]
        source: uuid::Error,
    },
}

/// A stable UUID namespace scoped to one Mendix project.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProjectIdentity {
    namespace: Uuid,
}

impl ProjectIdentity {
    /// Derives a stable project namespace from an author-visible logical name.
    pub fn for_project(logical_name: &str) -> Self {
        let mxrs_namespace = Uuid::new_v5(&Uuid::NAMESPACE_URL, MXRS_NAMESPACE_NAME.as_bytes());
        let namespace = Uuid::new_v5(&mxrs_namespace, logical_name.as_bytes());
        Self { namespace }
    }

    /// Uses an existing `.mpr` root ID as the namespace for new artifacts.
    pub fn from_project_root(root_id: &str) -> Result<Self, IdentityError> {
        let namespace =
            Uuid::parse_str(root_id).map_err(|source| IdentityError::InvalidProjectRoot {
                value: root_id.to_string(),
                source,
            })?;
        Ok(Self { namespace })
    }

    /// The stable root ID used when creating a fresh project.
    pub fn project_root_id(self) -> String {
        self.namespace.to_string()
    }

    /// Derives a stable ID for one new artifact.
    pub fn artifact_id(self, kind: ArtifactKind, qualified_name: &str) -> String {
        let identity = format!("{kind}:{qualified_name}");
        Uuid::new_v5(&self.namespace, identity.as_bytes()).to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_same_project_and_qualified_name_have_the_same_identity() {
        let first = ProjectIdentity::for_project("Shop");
        let second = ProjectIdentity::for_project("Shop");
        assert_eq!(first.project_root_id(), second.project_root_id());
        assert_eq!(
            first.project_root_id(),
            "34e4beda-a4ff-5088-8204-91aafdd544a6"
        );
        assert_eq!(
            first.artifact_id(ArtifactKind::Entity, "Sales.Order"),
            second.artifact_id(ArtifactKind::Entity, "Sales.Order")
        );
        assert_eq!(
            first.artifact_id(ArtifactKind::Entity, "Sales.Order"),
            "fd77a34f-de44-5bae-b956-49511fb8d7fe"
        );
    }

    #[test]
    fn project_kind_and_qualified_name_each_partition_the_identity_space() {
        let shop = ProjectIdentity::for_project("Shop");
        let crm = ProjectIdentity::for_project("CRM");
        let entity = shop.artifact_id(ArtifactKind::Entity, "Sales.Order");
        assert_ne!(entity, crm.artifact_id(ArtifactKind::Entity, "Sales.Order"));
        assert_ne!(
            entity,
            shop.artifact_id(ArtifactKind::Microflow, "Sales.Order")
        );
        assert_ne!(
            entity,
            shop.artifact_id(ArtifactKind::Entity, "Sales.Customer")
        );
    }

    #[test]
    fn an_existing_root_reconstructs_the_same_scope() {
        let fresh = ProjectIdentity::for_project("Shop");
        let reopened = ProjectIdentity::from_project_root(&fresh.project_root_id()).unwrap();
        assert_eq!(
            fresh.artifact_id(ArtifactKind::Attribute, "Sales.Order.Number"),
            reopened.artifact_id(ArtifactKind::Attribute, "Sales.Order.Number")
        );
    }

    #[test]
    fn an_invalid_existing_root_fails_loudly() {
        let error = ProjectIdentity::from_project_root("not-a-uuid").unwrap_err();
        assert!(error.to_string().contains("not-a-uuid"));
    }
}
