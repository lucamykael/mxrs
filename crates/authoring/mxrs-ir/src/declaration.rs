//! Storage-independent project/module/entity declarations. Nothing in this
//! module knows about BSON field names, UUIDs, raw documents, or other `.mpr`
//! representation details. `mxrs-writer` lowers these values
//! into `mxrs-model` only at the persistence boundary.

use crate::flow::MicroflowDecl;
use crate::page::{LayoutDecl, PageDecl};
use crate::{
    DemoUserDecl, ModuleRoleDecl, NavigationDecl, NavigationItemDecl, NavigationProfileDecl,
    ProjectSecurityDecl,
};

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

/// What a module role may do with one entity member, or with every member a
/// rule does not name. Mirrors Mendix's `AccessRights`/
/// `DefaultMemberAccessRights` values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MemberRights {
    #[default]
    None,
    ReadOnly,
    ReadWrite,
}

/// Whether a [`MemberAccessDecl`] names an attribute or an association: the
/// two land in different fields of the native `DomainModels$MemberAccess`
/// document and cannot be told apart from the name alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccessMemberKind {
    Attribute,
    Association,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemberAccessDecl {
    /// Qualified as `"Module.Entity.Attribute"` or `"Module.Association"` —
    /// the builder resolves it from a marker, so an unqualified spelling
    /// cannot reach the writer.
    pub reference: String,
    pub kind: AccessMemberKind,
    pub rights: MemberRights,
}

/// One entity access rule: what a set of module roles may do with an entity.
///
/// `members` is the explicit list only. mxrb additionally supports a
/// `read:`/`write: :all` shorthand that expands to every member at
/// declaration time; here that expansion is the writer's job, driven by
/// [`AccessRuleDecl::default_rights`], so a rule declared once keeps meaning
/// the same thing after an attribute is added.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccessRuleDecl {
    /// Module role names, unqualified (`"User"`), as declared by
    /// [`crate::ModuleRoleDecl`].
    pub roles: Vec<String>,
    pub documentation: String,
    pub allow_create: bool,
    pub allow_delete: bool,
    pub default_rights: MemberRights,
    pub members: Vec<MemberAccessDecl>,
    pub xpath_constraint: String,
    pub xpath_caption: Option<String>,
}

impl AccessRuleDecl {
    pub fn new(roles: Vec<String>) -> Self {
        Self {
            roles,
            documentation: String::new(),
            allow_create: false,
            allow_delete: false,
            default_rights: MemberRights::default(),
            members: vec![],
            xpath_constraint: String::new(),
            xpath_caption: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EntityImageDecl {
    None,
    Reference(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EntitySourceDecl {
    Stored,
    OqlView { source_document: String },
}

#[derive(Debug, Clone)]
pub struct EntityDecl {
    pub name: String,
    pub documentation: String,
    pub persistable: bool,
    /// `None` preserves an imported image. `Some` is authoritative, including
    /// [`EntityImageDecl::None`] for an explicit clear.
    pub image: Option<EntityImageDecl>,
    /// `None` preserves an imported entity/source kind. Fresh entities default
    /// to [`EntitySourceDecl::Stored`].
    pub source: Option<EntitySourceDecl>,
    pub attributes: Vec<AttributeDecl>,
    pub associations: Vec<AssociationDecl>,
    /// `None` preserves whatever access rules an imported entity already has.
    /// `Some` is authoritative, including an explicitly empty rule set — the
    /// same split [`ModuleDecl::roles`] draws for module security.
    pub access_rules: Option<Vec<AccessRuleDecl>>,
    /// `None` preserves the imported inheritance document. `Some` is
    /// authoritative and represents either a root entity (including its
    /// system-member flags) or one explicit parent entity.
    pub inheritance: Option<EntityInheritanceDecl>,
    /// `None` preserves imported indexes; `Some` replaces them, including an
    /// explicitly empty list.
    pub indexes: Option<Vec<EntityIndexDecl>>,
    /// `None` preserves imported callbacks; `Some` replaces them, including
    /// an explicitly empty list.
    pub lifecycle: Option<Vec<LifecycleDecl>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SystemMembersDecl {
    pub owner: bool,
    pub created_date: bool,
    pub changed_date: bool,
    pub changed_by: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EntityInheritanceDecl {
    Root(SystemMembersDecl),
    Generalizes(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SystemMember {
    CreatedDate,
    ChangedDate,
    Owner,
    ChangedBy,
}

impl SystemMember {
    pub const fn native_name(self) -> &'static str {
        match self {
            Self::CreatedDate => "CreatedDate",
            Self::ChangedDate => "ChangedDate",
            Self::Owner => "Owner",
            Self::ChangedBy => "ChangedBy",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IndexMemberDecl {
    Attribute {
        name: String,
        ascending: bool,
    },
    System {
        member: SystemMember,
        ascending: bool,
    },
}

impl IndexMemberDecl {
    pub fn name(&self) -> &str {
        match self {
            Self::Attribute { name, .. } => name,
            Self::System { member, .. } => member.native_name(),
        }
    }

    pub const fn ascending(&self) -> bool {
        match self {
            Self::Attribute { ascending, .. } | Self::System { ascending, .. } => *ascending,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntityIndexDecl {
    pub members: Vec<IndexMemberDecl>,
    pub include_offline: bool,
}

impl EntityIndexDecl {
    pub fn new() -> Self {
        Self {
            members: vec![],
            include_offline: false,
        }
    }
}

impl Default for EntityIndexDecl {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LifecycleEvent {
    BeforeCommit,
    AfterCommit,
    BeforeDelete,
    AfterDelete,
}

impl LifecycleEvent {
    pub const fn native_parts(self) -> (&'static str, &'static str) {
        match self {
            Self::BeforeCommit => ("Before", "Commit"),
            Self::AfterCommit => ("After", "Commit"),
            Self::BeforeDelete => ("Before", "Delete"),
            Self::AfterDelete => ("After", "Delete"),
        }
    }

    pub const fn rust_name(self) -> &'static str {
        match self {
            Self::BeforeCommit => "before_commit",
            Self::AfterCommit => "after_commit",
            Self::BeforeDelete => "before_delete",
            Self::AfterDelete => "after_delete",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LifecycleDecl {
    pub event: LifecycleEvent,
    pub handler: String,
    pub pass_event_object: bool,
    pub raise_error_on_false: bool,
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
            image: None,
            source: None,
            attributes: vec![],
            associations: vec![],
            access_rules: None,
            inheritance: None,
            indexes: None,
            lifecycle: None,
        }
    }
}

/// The value type of a [`ConstantDecl`].
///
/// Mirrors mxrb's `Writer::CONSTANT_TYPE_MAP` exactly — the five `DataTypes$*`
/// types its `constant_doc` accepts. A constant's declared value is always
/// carried as a string because that is how `DefaultValue` is persisted; the
/// type only selects the `Type` sub-document.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ConstantType {
    #[default]
    String,
    Integer,
    Boolean,
    Decimal,
    DateTime,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConstantDecl {
    pub name: String,
    pub documentation: String,
    pub constant_type: ConstantType,
    /// `Some` is persisted verbatim as `DefaultValue`. `None` preserves an
    /// imported value, allowing generated projects to keep credentials in
    /// the lossless model snapshot instead of copying them into Rust source.
    /// Values are not validated against `constant_type`: Mendix persists all
    /// five supported kinds as strings.
    pub value: Option<String>,
    pub exposed_to_client: bool,
}

impl ConstantDecl {
    pub fn new(name: impl Into<String>, constant_type: ConstantType) -> Self {
        Self {
            name: name.into(),
            documentation: String::new(),
            constant_type,
            value: Some(String::new()),
            exposed_to_client: false,
        }
    }
}

/// Visibility of a module document to consumers outside its module.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ExportLevel {
    #[default]
    Hidden,
    Published,
}

/// An editable Mendix regular-expression document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegularExpressionDecl {
    pub name: String,
    pub documentation: String,
    pub expression: String,
    pub excluded: bool,
    pub export_level: ExportLevel,
}

impl RegularExpressionDecl {
    pub fn new(name: impl Into<String>, expression: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            documentation: String::new(),
            expression: expression.into(),
            excluded: false,
            export_level: ExportLevel::Hidden,
        }
    }
}

/// Legacy interval unit retained by Mendix alongside the modern schedule.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScheduleUnit {
    Milliseconds,
    Seconds,
    Minutes,
    Hours,
    Days,
    Weeks,
    Months,
    Years,
}

/// The four closed modern schedule shapes in the Mendix 11 metamodel.
///
/// This is deliberately independent of [`ScheduleUnit`]: real projects may
/// retain a legacy `IntervalType`/`Interval` pair while using a different
/// modern schedule shape.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScheduledEventSchedule {
    None,
    Minute {
        multiplier: i64,
    },
    Hour {
        multiplier: i64,
        minute_offset: i64,
    },
    Day {
        hour_of_day: i64,
        minute_of_hour: i64,
    },
    Week {
        hour_of_day: i64,
        minute_of_hour: i64,
        monday: bool,
        tuesday: bool,
        wednesday: bool,
        thursday: bool,
        friday: bool,
        saturday: bool,
        sunday: bool,
    },
}

/// What the runtime does when a run is still in progress at the next trigger.
///
/// mxrb keeps this an open string defaulting to `"SkipNext"`; these are the
/// two values attested in its own fixtures. Typed rather than free-form so an
/// unattested value cannot reach the `.mpr` silently — adding one is a
/// deliberate change backed by evidence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OnOverlap {
    #[default]
    SkipNext,
    DelayNext,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScheduledEventDecl {
    pub name: String,
    pub documentation: String,
    pub excluded: bool,
    pub export_level: ExportLevel,
    /// Qualified name of the microflow to run, as `"Microflow"` (same module)
    /// or `"Module.Microflow"`.
    pub microflow: String,
    pub unit: ScheduleUnit,
    pub interval: i64,
    /// RFC 3339 UTC instant. Kept human-readable in generated Rust and parsed
    /// strictly at the persistence boundary.
    pub start_at: String,
    pub time_zone: String,
    pub schedule: ScheduledEventSchedule,
    pub on_overlap: OnOverlap,
    pub enabled: bool,
}

/// Localized user-facing text, keyed by Mendix language code (`en_US`,
/// `pt_BR`, ...). A map keeps generated declarations deterministic while
/// retaining every translation as editable text.
pub type LocalizedText = std::collections::BTreeMap<String, String>;

/// Icon variants supported by standalone Mendix menu documents.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MenuIconDecl {
    /// Numeric code from the legacy Mendix glyph font.
    Glyph(i64),
    /// Qualified image from an icon collection.
    Image(String),
}

/// A typed destination for a standalone menu item.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MenuActionDecl {
    None {
        disabled_during_execution: bool,
    },
    OpenPage {
        page: String,
        disabled_during_execution: bool,
        pages_to_close: Option<u32>,
        title_override: Option<LocalizedText>,
    },
    CreateObjectAndOpenPage {
        entity: String,
        page: String,
        disabled_during_execution: bool,
        pages_to_close: Option<u32>,
        title_override: Option<LocalizedText>,
    },
}

impl Default for MenuActionDecl {
    fn default() -> Self {
        Self::None {
            disabled_during_execution: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct MenuItemDecl {
    pub caption: LocalizedText,
    pub alternative_text: Option<LocalizedText>,
    pub action: MenuActionDecl,
    pub icon: Option<MenuIconDecl>,
    pub items: Vec<MenuItemDecl>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MenuDecl {
    pub name: String,
    pub documentation: String,
    pub excluded: bool,
    pub export_level: ExportLevel,
    pub items: Vec<MenuItemDecl>,
}

impl MenuDecl {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            documentation: String::new(),
            excluded: false,
            export_level: ExportLevel::Hidden,
            items: vec![],
        }
    }
}

impl ScheduledEventDecl {
    /// Defaults mirror `Writer#scheduled_event_doc`: UTC, enabled, skip an
    /// overlapping run, and an interval of one `unit`.
    pub fn new(name: impl Into<String>, microflow: impl Into<String>, unit: ScheduleUnit) -> Self {
        Self {
            name: name.into(),
            documentation: String::new(),
            excluded: false,
            export_level: ExportLevel::Hidden,
            microflow: microflow.into(),
            unit,
            interval: 1,
            start_at: "2000-01-01T00:00:00.000Z".into(),
            time_zone: "UTC".into(),
            schedule: match unit {
                ScheduleUnit::Minutes => ScheduledEventSchedule::Minute { multiplier: 1 },
                ScheduleUnit::Hours => ScheduledEventSchedule::Hour {
                    multiplier: 1,
                    minute_offset: 0,
                },
                ScheduleUnit::Days => ScheduledEventSchedule::Day {
                    hour_of_day: 0,
                    minute_of_hour: 0,
                },
                ScheduleUnit::Milliseconds
                | ScheduleUnit::Seconds
                | ScheduleUnit::Weeks
                | ScheduleUnit::Months
                | ScheduleUnit::Years => ScheduledEventSchedule::None,
            },
            on_overlap: OnOverlap::default(),
            enabled: true,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct ModuleDecl {
    pub name: String,
    pub entities: Vec<EntityDecl>,
    pub oql_view_sources: Vec<OqlViewSourceDecl>,
    pub enumerations: Vec<EnumerationDecl>,
    pub constants: Vec<ConstantDecl>,
    pub regular_expressions: Vec<RegularExpressionDecl>,
    pub task_queues: Vec<crate::TaskQueueDecl>,
    pub scheduled_events: Vec<ScheduledEventDecl>,
    pub menus: Vec<MenuDecl>,
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OqlViewSourceDecl {
    pub name: String,
    pub query: String,
    pub documentation: String,
    pub excluded: bool,
    pub export_level: ExportLevel,
}

impl OqlViewSourceDecl {
    pub fn new(name: impl Into<String>, query: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            query: query.into(),
            documentation: String::new(),
            excluded: false,
            export_level: ExportLevel::Hidden,
        }
    }
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
    /// Demo users declared in a project that declares no security of its
    /// own. They join the security the model already stores, which stays
    /// otherwise untouched; with a declared `security` they are listed there
    /// instead.
    pub demo_users: Vec<DemoUserDecl>,
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
        target.oql_view_sources.extend(declared.oql_view_sources);
        target.enumerations.extend(declared.enumerations);
        target.constants.extend(declared.constants);
        target.task_queues.extend(declared.task_queues);
        target
            .regular_expressions
            .extend(declared.regular_expressions);
        target.scheduled_events.extend(declared.scheduled_events);
        target.menus.extend(declared.menus);
        target.microflows.extend(declared.microflows);
        target.nanoflows.extend(declared.nanoflows);
        target.pages.extend(declared.pages);
        target.layouts.extend(declared.layouts);
        if let Some(roles) = declared.roles {
            target.roles = Some(roles);
        }
    }

    /// Returns the module of this name, declaring an empty one when the
    /// project has none yet.
    ///
    /// Generated declaration layers need "the module I am about to extend"
    /// without restating it. Open-coding that as
    /// `iter_mut().find(..).expect(..)` after a conditional `push` puts a
    /// panic into generated product code for a condition the caller cannot
    /// see — so the find-or-create lives here, where it is total.
    pub fn module_mut(&mut self, name: &str) -> &mut ModuleDecl {
        if let Some(position) = self.modules.iter().position(|module| module.name == name) {
            return &mut self.modules[position];
        }
        self.modules.push(ModuleDecl {
            name: name.to_string(),
            ..Default::default()
        });
        self.modules
            .last_mut()
            .expect("a module was just pushed onto the project")
    }

    /// Appends a navigation item to `profile`, declaring the navigation and
    /// the profile when the project does not have them.
    ///
    /// A scaffolded page entry must not assume the application still declares
    /// the profile it was generated against. Mendix profiles are
    /// Responsive/Tablet/Phone and the generated Rust is meant to be edited,
    /// so "the Responsive profile exists" is a guess, not an invariant —
    /// and a guess is exactly what must not become a panic in a user's build.
    pub fn navigation_item(&mut self, profile: &str, item: NavigationItemDecl) -> &mut Self {
        let navigation = self.navigation.get_or_insert_with(NavigationDecl::default);
        if !navigation
            .profiles
            .iter()
            .any(|declared| declared.name == profile)
        {
            navigation
                .profiles
                .push(NavigationProfileDecl::new(profile));
        }
        for declared in navigation
            .profiles
            .iter_mut()
            .filter(|declared| declared.name == profile)
        {
            declared.items.push(item.clone());
        }
        self
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

    /// Both accessors exist so generated declaration layers never open-code
    /// find-or-create with an `expect`. Totality is the whole contract: the
    /// panic they replace fired inside a user's build, not inside mxrs.
    #[test]
    fn module_mut_finds_or_declares_without_disturbing_the_others() {
        let mut project = ProjectDecl {
            mendix_version: "11.12.1".into(),
            modules: vec![module("Sales"), module("CRM")],
            security: None,
            navigation: None,
            demo_users: vec![],
        };
        project
            .module_mut("Sales")
            .entities
            .push(EntityDecl::new("Order"));
        project
            .module_mut("Billing")
            .entities
            .push(EntityDecl::new("Invoice"));

        let names: Vec<&str> = project
            .modules
            .iter()
            .map(|module| module.name.as_str())
            .collect();
        assert_eq!(names, ["Sales", "CRM", "Billing"]);
        assert_eq!(project.modules[0].entities.len(), 1);
        assert!(project.modules[1].entities.is_empty());
        assert_eq!(project.modules[2].entities.len(), 1);
        // A second call returns the same module rather than a duplicate.
        project
            .module_mut("Billing")
            .entities
            .push(EntityDecl::new("Credit"));
        assert_eq!(project.modules.len(), 3);
        assert_eq!(project.modules[2].entities.len(), 2);
    }

    #[test]
    fn navigation_items_land_in_a_declared_profile_or_create_one() {
        let item = |page: &str| NavigationItemDecl {
            caption: Default::default(),
            page: Some(page.to_string()),
            microflow: None,
            icon: None,
            items: vec![],
        };
        // A project that declares no navigation at all still accepts an item.
        let mut bare = ProjectDecl {
            mendix_version: "11.12.1".into(),
            modules: vec![],
            security: None,
            navigation: None,
            demo_users: vec![],
        };
        bare.navigation_item("Responsive", item("Main.Home"));
        let navigation = bare.navigation.as_ref().expect("navigation was declared");
        assert_eq!(navigation.profiles.len(), 1);
        assert_eq!(navigation.profiles[0].name, "Responsive");
        assert_eq!(navigation.profiles[0].items.len(), 1);

        // An existing profile keeps its home page and earlier items.
        let mut existing = NavigationProfileDecl::new("Responsive");
        existing.home_page = Some("Main.Home".to_string());
        existing.items.push(item("Main.First"));
        let mut project = ProjectDecl {
            mendix_version: "11.12.1".into(),
            modules: vec![],
            security: None,
            navigation: Some(NavigationDecl {
                profiles: vec![NavigationProfileDecl::new("Phone"), existing],
            }),
            demo_users: vec![],
        };
        project.navigation_item("Responsive", item("Main.Second"));
        let navigation = project.navigation.as_ref().unwrap();
        assert_eq!(navigation.profiles.len(), 2);
        assert_eq!(navigation.profiles[0].name, "Phone");
        assert!(navigation.profiles[0].items.is_empty());
        assert_eq!(
            navigation.profiles[1].home_page.as_deref(),
            Some("Main.Home")
        );
        let pages: Vec<Option<&str>> = navigation.profiles[1]
            .items
            .iter()
            .map(|item| item.page.as_deref())
            .collect();
        assert_eq!(pages, [Some("Main.First"), Some("Main.Second")]);

        // A project that renamed Responsive away gets the profile declared
        // rather than a panic — the case that used to abort a user's build.
        let mut renamed = ProjectDecl {
            mendix_version: "11.12.1".into(),
            modules: vec![],
            security: None,
            navigation: Some(NavigationDecl {
                profiles: vec![NavigationProfileDecl::new("Phone")],
            }),
            demo_users: vec![],
        };
        renamed.navigation_item("Responsive", item("Main.Home"));
        let names: Vec<&str> = renamed
            .navigation
            .as_ref()
            .unwrap()
            .profiles
            .iter()
            .map(|profile| profile.name.as_str())
            .collect();
        assert_eq!(names, ["Phone", "Responsive"]);
    }

    #[test]
    fn merging_appends_artifacts_of_a_known_module_and_pushes_an_unknown_one() {
        let mut project = ProjectDecl {
            mendix_version: "11.12.1".into(),
            modules: vec![module("Sales")],
            security: None,
            navigation: None,
            demo_users: vec![],
        };
        project.modules[0].entities.push(EntityDecl::new("Order"));
        let mut declared = module("Sales");
        declared.entities.push(EntityDecl::new("Invoice"));
        declared
            .enumerations
            .push(EnumerationDecl::new("PaymentStatus"));
        declared
            .regular_expressions
            .push(RegularExpressionDecl::new("OrderCode", "[A-Z]+"));
        project.merge_module(declared);
        assert_eq!(project.modules.len(), 1);
        let names = project.modules[0]
            .entities
            .iter()
            .map(|entity| entity.name.as_str())
            .collect::<Vec<_>>();
        assert_eq!(names, ["Order", "Invoice"]);
        assert_eq!(project.modules[0].enumerations.len(), 1);
        assert_eq!(project.modules[0].regular_expressions.len(), 1);
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
            demo_users: vec![],
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
