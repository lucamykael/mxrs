//! How generated source names the model.
//!
//! An entity is the struct that declares it, an attribute is that struct's
//! accessor, and a flow is the unit type carrying its Mendix name — each
//! declared in the file that owns it. A renderer therefore cannot know, while
//! it walks one page or one flow, how the things it references will be
//! spelled: that depends on every other file and on which short names are
//! still free in this one. So renderers write *references* — opaque tokens
//! naming a model element — and [`ModelNames::resolve`] turns a finished file
//! body into source plus the `use` lines it needs.

use std::collections::{BTreeMap, HashMap, HashSet};

use crate::entity_export::TypedEntityTarget;
use crate::{ExportError, Result};

/// Delimiters no rendered source can contain: every model string reaches the
/// output as a Rust string literal, which escapes control characters.
const OPEN: char = '\u{1}';
const CLOSE: char = '\u{2}';

fn reference(kind: char, target: &str) -> String {
    format!("{OPEN}{kind}{target}{CLOSE}")
}

/// The prefix of an identifier standing for a reference in
/// [`references_as_identifiers`]' output.
pub(crate) const REFERENCE_IDENTIFIER: &str = "__mxref_";

/// `body` with each reference spelled as an identifier Rust's grammar takes
/// wherever a reference can stand — a type, a path, a struct literal's
/// name or field: `__mxref_<kind>_<target in hex>`.
pub(crate) fn references_as_identifiers(body: &str) -> String {
    let mut out = String::with_capacity(body.len());
    let mut rest = body;
    while let Some(start) = rest.find(OPEN) {
        out.push_str(&rest[..start]);
        let after = &rest[start + OPEN.len_utf8()..];
        let Some(end) = after.find(CLOSE) else {
            out.push_str(&rest[start..]);
            return out;
        };
        let token = &after[..end];
        let mut chars = token.chars();
        let kind = chars.next().unwrap_or('?');
        out.push_str(REFERENCE_IDENTIFIER);
        out.push(kind);
        out.push('_');
        for byte in chars.as_str().bytes() {
            out.push_str(&format!("{byte:02x}"));
        }
        rest = &after[end + CLOSE.len_utf8()..];
    }
    out.push_str(rest);
    out
}

/// The `(kind, target)` an identifier from [`references_as_identifiers`]
/// stands for.
pub(crate) fn identifier_reference(identifier: &str) -> Option<(char, String)> {
    let rest = identifier.strip_prefix(REFERENCE_IDENTIFIER)?;
    let mut chars = rest.chars();
    let kind = chars.next()?;
    let hex = chars.as_str().strip_prefix('_')?;
    let bytes = (0..hex.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(hex.get(index..index + 2)?, 16).ok())
        .collect::<Option<Vec<u8>>>()?;
    Some((kind, String::from_utf8(bytes).ok()?))
}

/// The struct declaring entity `Module.Entity`.
pub(crate) fn entity(qualified_name: &str) -> String {
    reference('E', qualified_name)
}

/// The accessor for `attribute` of entity `Module.Entity`.
pub(crate) fn attribute(entity: &str, attribute: &str) -> String {
    reference('A', &format!("{entity}/{attribute}"))
}

/// The field of entity `Module.Entity` that declares `attribute`, as a
/// struct literal names it: the accessor's name alone.
pub(crate) fn field(entity: &str, attribute: &str) -> String {
    reference('F', &format!("{entity}/{attribute}"))
}

/// The macro calling Java action `Module.Action`.
pub(crate) fn java_action(qualified_name: &str) -> String {
    reference('J', qualified_name)
}

/// The variant naming value `Module.Enumeration.Value`.
pub(crate) fn enumeration_value(qualified_value: &str) -> String {
    reference('V', qualified_value)
}

/// The type naming microflow `Module.Flow`.
pub(crate) fn microflow(qualified_name: &str) -> String {
    reference('M', qualified_name)
}

/// The type naming nanoflow `Module.Flow`.
pub(crate) fn nanoflow(qualified_name: &str) -> String {
    reference('N', qualified_name)
}

/// The variant naming module role `Module.Role` in its module's roles enum.
pub(crate) fn role(qualified_name: &str) -> String {
    reference('R', qualified_name)
}

/// Where a module role is declared: the enum its module declares its roles
/// with, and the variant that is this one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RoleTarget {
    /// The Rust module the enum lives in.
    pub(crate) module_path: String,
    pub(crate) variant: String,
}

/// Where a flow's naming type is declared.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FlowTarget {
    /// The Rust module the type lives in.
    pub(crate) module_path: String,
    /// The type's identifier: the flow's Mendix name, spelled as one.
    pub(crate) marker: String,
}

/// A flow's Mendix name as the identifier of the type that names it. Mirrors
/// `marker_ident` in `mxrs-macros`, which declares that type — down to the
/// keyword list, or a flow would be declared under one name and referred to
/// by another.
pub(crate) fn flow_marker(name: &str) -> String {
    let mut ident: String = name
        .chars()
        .map(|character| {
            if character.is_alphanumeric() || character == '_' {
                character
            } else {
                '_'
            }
        })
        .collect();
    if ident.starts_with(|character: char| character.is_ascii_digit()) {
        ident.insert(0, '_');
    }
    if crate::rust_keyword(&ident) || ident == "_" {
        ident.push('_');
    }
    ident
}

/// Every nameable element of the imported model.
pub(crate) struct ModelNames<'a> {
    pub(crate) entities: &'a HashMap<String, TypedEntityTarget>,
    pub(crate) microflows: HashMap<String, FlowTarget>,
    pub(crate) nanoflows: HashMap<String, FlowTarget>,
    /// The module roles a Rust enum declares, by `Module.Role`.
    pub(crate) roles: HashMap<String, RoleTarget>,
    /// `Module.Enumeration.Value` → the enum declaring it and the variant
    /// that is that value.
    pub(crate) enumeration_values: HashMap<String, RoleTarget>,
    /// `Module.Action` → the macro that calls the Java action, and the
    /// module of contracts it is declared in.
    pub(crate) java_actions: HashMap<String, FlowTarget>,
}

/// One file body with its references spelled, and the imports that spelling
/// relies on.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Resolved {
    pub(crate) source: String,
    /// `use path::Name;` lines, sorted.
    pub(crate) imports: Vec<String>,
}

impl ModelNames<'_> {
    /// Spells every reference in `body`.
    ///
    /// A referenced type is imported by its short name when that name means
    /// one thing in this file; a name two references share, one in `taken`
    /// (the file's own items), one the preludes already give a meaning
    /// (an import would silently take `Ref` or `None` away from the code
    /// around it), or a lower-case one — which a `let` of the same name
    /// would otherwise be read as matching against — is spelled by its full
    /// path instead.
    pub(crate) fn resolve(&self, body: &str, taken: &[&str]) -> Result<Resolved> {
        let mut requests: Vec<(String, String)> = Vec::new();
        // A macro lives in a namespace of its own: importing one by its
        // lower-case name cannot change what a `let` matches.
        let mut macros: HashSet<String> = HashSet::new();
        let mut segments: Vec<Segment> = Vec::new();
        let mut rest = body;
        while let Some(start) = rest.find(OPEN) {
            segments.push(Segment::Text(rest[..start].to_string()));
            let after = &rest[start + OPEN.len_utf8()..];
            let end = after.find(CLOSE).ok_or_else(|| {
                ExportError::MarkerLayout("unterminated model reference".to_string())
            })?;
            let token = &after[..end];
            let (kind, target) = token.split_at(1);
            let (module_path, name, member) = self.locate(kind, target)?;
            if kind == "J" {
                macros.insert(name.clone());
            }
            // A field is named inside its entity's own struct literal: it
            // needs nothing imported.
            if !matches!(member, Member::Field(_)) {
                requests.push((name.clone(), module_path.clone()));
            }
            segments.push(Segment::Reference {
                module_path,
                name,
                member,
            });
            rest = &after[end + CLOSE.len_utf8()..];
        }
        segments.push(Segment::Text(rest.to_string()));

        let mut imported: BTreeMap<&str, &str> = BTreeMap::new();
        let mut colliding = HashSet::new();
        for (name, path) in &requests {
            if let Some(existing) = imported.insert(name, path)
                && existing != path
            {
                colliding.insert(name.as_str());
            }
        }
        imported.retain(|name, _| {
            !colliding.contains(name)
                && !taken.contains(name)
                && !prelude_name(name)
                && (macros.contains(*name)
                    || !name.starts_with(|character: char| {
                        character.is_lowercase() || character == '_'
                    }))
        });

        let mut source = String::with_capacity(body.len());
        for segment in &segments {
            match segment {
                Segment::Text(text) => source.push_str(text),
                Segment::Reference {
                    member: Member::Field(field),
                    ..
                } => source.push_str(field),
                Segment::Reference {
                    module_path,
                    name,
                    member,
                } => {
                    if imported.get(name.as_str()) != Some(&module_path.as_str()) {
                        source.push_str(module_path);
                        source.push_str("::");
                    }
                    source.push_str(name);
                    match member {
                        Member::None => {}
                        Member::Accessor(accessor) => {
                            source.push_str("::");
                            source.push_str(accessor);
                            source.push_str("()");
                        }
                        Member::Variant(variant) => {
                            source.push_str("::");
                            source.push_str(variant);
                        }
                        Member::Field(_) => unreachable!("written above"),
                    }
                }
            }
        }
        Ok(Resolved {
            source,
            imports: imported
                .into_iter()
                .map(|(name, path)| format!("use {path}::{name};"))
                .collect(),
        })
    }

    /// `(module path, type name, member)` for one reference.
    fn locate(&self, kind: &str, target: &str) -> Result<(String, String, Member)> {
        let missing =
            |what: &str| ExportError::MarkerLayout(format!("{what} {target:?} is not declared"));
        match kind {
            "E" => {
                let entity = self.entities.get(target).ok_or_else(|| missing("entity"))?;
                Ok((entity.module_path(), entity.type_name.clone(), Member::None))
            }
            "A" => {
                let (entity_name, attribute) =
                    target.split_once('/').ok_or_else(|| missing("attribute"))?;
                let entity = self
                    .entities
                    .get(entity_name)
                    .ok_or_else(|| missing("entity"))?;
                let accessor = entity
                    .attributes
                    .get(attribute)
                    .ok_or_else(|| missing("attribute"))?;
                Ok((
                    entity.module_path(),
                    entity.type_name.clone(),
                    Member::Accessor(accessor.clone()),
                ))
            }
            "F" => {
                let (entity_name, attribute) =
                    target.split_once('/').ok_or_else(|| missing("attribute"))?;
                let entity = self
                    .entities
                    .get(entity_name)
                    .ok_or_else(|| missing("entity"))?;
                let accessor = entity
                    .attributes
                    .get(attribute)
                    .ok_or_else(|| missing("attribute"))?;
                Ok((
                    entity.module_path(),
                    entity.type_name.clone(),
                    Member::Field(accessor.clone()),
                ))
            }
            "M" | "N" => {
                let flows = if kind == "M" {
                    &self.microflows
                } else {
                    &self.nanoflows
                };
                let flow = flows.get(target).ok_or_else(|| missing("flow"))?;
                Ok((flow.module_path.clone(), flow.marker.clone(), Member::None))
            }
            "J" => {
                let action = self
                    .java_actions
                    .get(target)
                    .ok_or_else(|| missing("Java action"))?;
                Ok((
                    action.module_path.clone(),
                    action.marker.clone(),
                    Member::None,
                ))
            }
            "V" => {
                let value = self
                    .enumeration_values
                    .get(target)
                    .ok_or_else(|| missing("enumeration value"))?;
                let (path, type_name) = value
                    .module_path
                    .rsplit_once("::")
                    .ok_or_else(|| missing("enumeration"))?;
                Ok((
                    path.to_string(),
                    type_name.to_string(),
                    Member::Variant(value.variant.clone()),
                ))
            }
            "R" => {
                let role = self.roles.get(target).ok_or_else(|| missing("role"))?;
                Ok((
                    role.module_path.clone(),
                    "Role".to_string(),
                    Member::Variant(role.variant.clone()),
                ))
            }
            _ => Err(ExportError::MarkerLayout(format!(
                "unknown model reference kind {kind:?}"
            ))),
        }
    }
}

/// Names a generated file already has a meaning for without importing
/// anything: what `mxrs::prelude` exports and what Rust's own prelude does.
/// An explicit `use` wins over both, so a model element spelled like one of
/// these is never imported by its short name.
pub(crate) fn prelude_name(name: &str) -> bool {
    MXRS_PRELUDE.contains(&name) || RUST_PRELUDE.contains(&name)
}

/// `mxrs::prelude`, upper-case names only: a lower-case name is never
/// imported short in the first place. Pinned against the facade's source by
/// `the_prelude_list_covers_the_facade_prelude`.
const MXRS_PRELUDE: &[&str] = &[
    "AggregateFunction",
    "ApplicationDefinition",
    "AssignAssociation",
    "AssignAttribute",
    "AssociationMarker",
    "AssociationRef",
    "AttributeMarker",
    "AttributeRef",
    "ButtonBuilder",
    "CallArgument",
    "ChangeKind",
    "CodeActionType",
    "Commit",
    "ConstantBuilder",
    "ConstantType",
    "ContainerBuilder",
    "DataType",
    "DataViewBuilder",
    "DemoUserBuilder",
    "EntityMarker",
    "EnumerationBuilder",
    "EnumerationMarker",
    "ExportLevel",
    "Expr",
    "FlowBuilder",
    "FlowVar",
    "ImageFormat",
    "JavaScriptPlatform",
    "JsonPrimitiveType",
    "LayoutBuilder",
    "LayoutGridBuilder",
    "LifecycleEvent",
    "ListChange",
    "LogSeverity",
    "MappingValueType",
    "MemberName",
    "MemberRights",
    "MendixType",
    "MenuActionDecl",
    "MenuBuilder",
    "MenuIconDecl",
    "MenuItemBuilder",
    "MessageKind",
    "MicroflowMarker",
    "MicroflowModuleBuilder",
    "MicroflowRef",
    "ModuleBuilder",
    "ModuleDecl",
    "Mx",
    "MxBinary",
    "MxBool",
    "MxDateTime",
    "MxDecimal",
    "MxEntity",
    "MxEnumeration",
    "MxFloat",
    "MxInteger",
    "MxList",
    "MxLong",
    "MxObject",
    "MxString",
    "NanoflowMarker",
    "NanoflowModuleBuilder",
    "NanoflowRef",
    "NativeDocument",
    "NativeValue",
    "NavigationBuilder",
    "NavigationItemBuilder",
    "NavigationProfileBuilder",
    "NullValueOption",
    "ObjectHandling",
    "OnOverlap",
    "PageBuilder",
    "ProjectBuilder",
    "Ref",
    "Reference",
    "ReferenceSet",
    "RenderExpr",
    "ScheduleUnit",
    "ScheduledEventBuilder",
    "ScheduledEventSchedule",
    "SecurityBuilder",
    "SecurityLevel",
    "SortOrder",
    "SystemMember",
    "TaskQueueBuilder",
    "TaskQueueConfig",
    "TaskQueueScope",
    "TypedAttributeMarker",
    "UserRoleBuilder",
    "Var",
    "Variable",
];

/// The types, traits and variants every Rust module starts with.
const RUST_PRELUDE: &[&str] = &[
    "AsMut",
    "AsRef",
    "Box",
    "Clone",
    "Copy",
    "Default",
    "DoubleEndedIterator",
    "Drop",
    "Eq",
    "Err",
    "ExactSizeIterator",
    "Extend",
    "Fn",
    "FnMut",
    "FnOnce",
    "From",
    "FromIterator",
    "Future",
    "Into",
    "IntoFuture",
    "IntoIterator",
    "Iterator",
    "None",
    "Ok",
    "Option",
    "Ord",
    "PartialEq",
    "PartialOrd",
    "Result",
    "Send",
    "Sized",
    "Some",
    "String",
    "Sync",
    "ToOwned",
    "ToString",
    "TryFrom",
    "TryInto",
    "Unpin",
    "Vec",
];

enum Segment {
    Text(String),
    Reference {
        module_path: String,
        name: String,
        member: Member,
    },
}

/// What of a referenced type the reference names.
enum Member {
    /// The type itself.
    None,
    /// An accessor function: `Order::number()`.
    Accessor(String),
    /// An enum variant: `Role::Administrator`.
    Variant(String),
    /// A struct field, named alone: `number` in `Order { number: ... }`.
    Field(String),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ModuleRoot;

    fn target(module: &str, file: &str, type_name: &str) -> TypedEntityTarget {
        TypedEntityTarget {
            root: ModuleRoot::Authored,
            module_stem: module.to_string(),
            file_stem: file.to_string(),
            type_name: type_name.to_string(),
            dto: false,
            attributes: HashMap::from([("Number".to_string(), "number".to_string())]),
        }
    }

    #[test]
    fn references_are_imported_once_and_spelled_short() {
        let entities =
            HashMap::from([("Sales.Order".to_string(), target("sales", "order", "Order"))]);
        let names = ModelNames {
            enumeration_values: HashMap::new(),
            java_actions: HashMap::new(),
            entities: &entities,
            microflows: HashMap::from([(
                "Sales.ACT_Ping".to_string(),
                FlowTarget {
                    module_path: "crate::services::sales::ping_service".to_string(),
                    marker: "ACT_Ping".to_string(),
                },
            )]),
            nanoflows: HashMap::new(),
            roles: HashMap::new(),
        };
        let body = format!(
            "Ref::<{}>::new(); {}.set(1); MicroflowRef::<{}>::new(); {}",
            entity("Sales.Order"),
            attribute("Sales.Order", "Number"),
            microflow("Sales.ACT_Ping"),
            entity("Sales.Order"),
        );
        let resolved = names.resolve(&body, &[]).unwrap();
        assert_eq!(
            resolved.source,
            "Ref::<Order>::new(); Order::number().set(1); MicroflowRef::<ACT_Ping>::new(); Order"
        );
        assert_eq!(
            resolved.imports,
            [
                "use crate::services::sales::ping_service::ACT_Ping;",
                "use crate::domain::entities::sales::order::Order;",
            ]
        );
    }

    #[test]
    fn ambiguous_taken_and_lower_case_names_are_spelled_by_path() {
        let entities = HashMap::from([
            ("Sales.Order".to_string(), target("sales", "order", "Order")),
            ("Crm.Order".to_string(), target("crm", "order", "Order")),
            ("Sales.Line".to_string(), target("sales", "line", "Line")),
        ]);
        let names = ModelNames {
            enumeration_values: HashMap::new(),
            java_actions: HashMap::new(),
            entities: &entities,
            microflows: HashMap::from([(
                "Sales.cleanup".to_string(),
                FlowTarget {
                    module_path: "crate::services::sales::imported".to_string(),
                    marker: "cleanup".to_string(),
                },
            )]),
            nanoflows: HashMap::new(),
            roles: HashMap::new(),
        };
        let body = format!(
            "{} {} {} {}",
            entity("Sales.Order"),
            entity("Crm.Order"),
            entity("Sales.Line"),
            microflow("Sales.cleanup"),
        );
        let resolved = names.resolve(&body, &["Line"]).unwrap();
        assert_eq!(
            resolved.source,
            "crate::domain::entities::sales::order::Order crate::domain::entities::crm::order::Order crate::domain::entities::sales::line::Line crate::services::sales::imported::cleanup"
        );
        assert!(resolved.imports.is_empty());
    }

    #[test]
    fn a_name_the_preludes_already_mean_is_never_imported_over_them() {
        let entities = HashMap::from([("Sales.Ref".to_string(), target("sales", "ref_", "Ref"))]);
        let names = ModelNames {
            enumeration_values: HashMap::new(),
            java_actions: HashMap::new(),
            entities: &entities,
            microflows: HashMap::from([(
                "Sales.None".to_string(),
                FlowTarget {
                    module_path: "crate::services::sales::none_service".to_string(),
                    marker: "None".to_string(),
                },
            )]),
            nanoflows: HashMap::new(),
            roles: HashMap::new(),
        };
        let body = format!(
            "Ref::<{}>::new(); call(MicroflowRef::<{}>::new(), None)",
            entity("Sales.Ref"),
            microflow("Sales.None"),
        );
        let resolved = names.resolve(&body, &[]).unwrap();
        assert_eq!(
            resolved.source,
            "Ref::<crate::domain::entities::sales::ref_::Ref>::new(); call(MicroflowRef::<crate::services::sales::none_service::None>::new(), None)"
        );
        assert!(resolved.imports.is_empty());
    }

    /// The list is a copy of what the facade exports, so it is checked
    /// against the facade's own source: a name added to `mxrs::prelude` and
    /// not here would be one an import could silently shadow.
    #[test]
    fn the_prelude_list_covers_the_facade_prelude() {
        let facade = include_str!("../../mxrs/src/lib.rs");
        let prelude = facade
            .split_once("pub mod prelude {")
            .and_then(|(_, rest)| rest.split_once("\n}\n"))
            .map(|(body, _)| body)
            .expect("the facade declares its prelude");
        let exported = prelude
            .split(|character: char| !(character.is_alphanumeric() || character == '_'))
            .filter(|word| word.starts_with(|character: char| character.is_uppercase()))
            .collect::<Vec<_>>();
        assert!(
            exported.len() > 50,
            "the prelude was not read: {exported:?}"
        );
        for name in exported {
            assert!(prelude_name(name), "`{name}` is exported by mxrs::prelude");
        }
        let mut sorted = MXRS_PRELUDE.to_vec();
        sorted.sort_unstable();
        assert_eq!(sorted, MXRS_PRELUDE, "kept sorted, so a duplicate shows");
    }

    #[test]
    fn a_reference_to_nothing_fails_loudly() {
        let entities = HashMap::new();
        let names = ModelNames {
            entities: &entities,
            microflows: HashMap::new(),
            nanoflows: HashMap::new(),
            roles: HashMap::new(),
            enumeration_values: HashMap::new(),
            java_actions: HashMap::new(),
        };
        let error = names.resolve(&entity("Sales.Missing"), &[]).unwrap_err();
        assert!(error.to_string().contains("Sales.Missing"), "{error}");
    }

    #[test]
    fn a_flow_is_named_by_its_mendix_name_spelled_as_an_identifier() {
        assert_eq!(flow_marker("ACT_CreateOrder"), "ACT_CreateOrder");
        assert_eq!(flow_marker("9Lives"), "_9Lives");
        assert_eq!(flow_marker("return"), "return_");
        assert_eq!(flow_marker("My Flow"), "My_Flow");
        // The same answers `marker_ident` gives in `mxrs-macros`, which the
        // facade's `authoring_names` test pins from the other side: words
        // Rust reserves in some edition are not left for one side to accept.
        assert_eq!(flow_marker("union"), "union_");
        assert_eq!(flow_marker("gen"), "gen_");
        assert_eq!(flow_marker("Self"), "Self_");
        assert_eq!(flow_marker("_"), "__");
    }
}
