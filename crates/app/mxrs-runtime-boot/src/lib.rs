//! Boots MXRS' own runtime from a Mendix project model.
//!
//! Ports the model-derivation half of mxrb's pure-Ruby runtime
//! (`lib/mxrb/runtime/access_control.rb` plus the store bootstrapping its
//! native executor performs): the domain model becomes a
//! [`StoreSchema`], `Security$ProjectSecurity` plus per-entity access rules
//! become a [`SecurityPolicy`], and every flow document lands in the policy's
//! document inventory. Flow *execution* deliberately lives elsewhere
//! (`mxrs-runtime-flows`); this crate stays derivation-only.
//!
//! Security mapping is fail-closed in both directions mxrb is: a present
//! security unit with an unknown `SecurityLevel` counts as enabled, and an
//! access rule's XPath constraint narrows the rule to the records it names
//! (`mxrs_runtime::xpath`) rather than widening it to the whole entity. A
//! constraint outside that evaluated subset denies every record and is
//! reported here, never silently dropped.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use mxrs_bson::{Bson, Document, parse_array};
use mxrs_model::{Attribute, AttributeType, Module, Project};
use mxrs_runtime::{EntityRule, MemberRight, SecurityContext, SecurityPolicy, StoreSchema};
use serde_json::Value;

/// One account `Security$ProjectSecurity` declares, with the password the
/// model itself holds.
///
/// Mendix keeps these passwords in the model in the clear, so they are read
/// from the built `.mpr` at boot and never leave this type: nothing writes
/// them to generated source, to a log, or to an error. [`LocalAccounts`] is
/// the only way to ask about them, and it only ever answers "yes, and these
/// are the caller's roles" or "no".
#[derive(Debug, Clone)]
struct Account {
    name: String,
    password: String,
    user_roles: BTreeSet<String>,
}

/// How a request signs in to this project: the accounts the model declares,
/// and who a caller with no credentials is.
#[derive(Debug, Clone, Default)]
pub struct LocalAccounts {
    /// `AdminUserName` / `AdminPassword` / `AdminUserRole`.
    administrator: Option<Account>,
    /// `DemoUsers`, carried only when `EnableDemoUsers` is set — Studio Pro's
    /// own switch for whether they may sign in at all.
    demo: Vec<Account>,
    /// `GuestUserRole`, carried only when `EnableGuestAccess` is set.
    guest_user_role: Option<String>,
}

impl LocalAccounts {
    /// The caller a `name`/`password` pair signs in as, or `None` when no
    /// declared account matches.
    ///
    /// The password comparison is constant-time, so a wrong password cannot
    /// be told from a wrong user name by how long the answer took. The
    /// returned context names the account's *user* roles; the policy expands
    /// them into module roles when it is asked a question.
    pub fn sign_in(&self, name: &str, password: &str) -> Option<SecurityContext> {
        let account = self
            .administrator
            .iter()
            .chain(&self.demo)
            .find(|account| account.name == name)?;
        constant_time_eq(account.password.as_bytes(), password.as_bytes()).then(|| {
            SecurityContext {
                user: Some(account.name.clone()),
                user_roles: account.user_roles.clone(),
                ..SecurityContext::default()
            }
        })
    }

    /// Who a caller with no credentials is: the guest user role the project
    /// declares, or a context holding no role at all when it declares none.
    ///
    /// The role-less context is deliberate. It is not "no caller, so no
    /// rules" — entity access judges it and, on a project with security on,
    /// denies it. A model that publishes an anonymous surface without
    /// granting anonymous callers anything has said nothing may be read
    /// through it, and that is what the caller gets.
    pub fn anonymous(&self) -> SecurityContext {
        SecurityContext {
            user_roles: self.guest_user_role.iter().cloned().collect(),
            ..SecurityContext::default()
        }
    }

    /// The `GuestUserRole` an anonymous caller runs as, when the project
    /// enables guest access.
    pub fn guest_user_role(&self) -> Option<&str> {
        self.guest_user_role.as_deref()
    }

    /// Whether any account can sign in at all. A project with none cannot
    /// authenticate a request, however it is addressed.
    pub fn can_sign_anyone_in(&self) -> bool {
        self.administrator.is_some() || !self.demo.is_empty()
    }
}

/// Compares two byte strings without an early exit, so the time taken does
/// not reveal how much of a password was right. Lengths are compared first
/// and deliberately leak: a password's length is not its content.
fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    left.iter()
        .zip(right)
        .fold(0u8, |difference, (left, right)| difference | (left ^ right))
        == 0
}

#[derive(Debug, thiserror::Error)]
pub enum BootError {
    #[error("cannot read project model: {0}")]
    Model(String),
    #[error("entity {entity} attribute {attribute}: invalid {kind:?} default {value:?}")]
    InvalidDefault {
        entity: String,
        attribute: String,
        kind: AttributeType,
        value: String,
    },
}

/// Everything the runtime needs from the model, plus an honest inventory of
/// what was and was not carried over.
#[derive(Debug)]
pub struct Boot {
    pub schema: StoreSchema,
    pub security: SecurityPolicy,
    /// How a request signs in: the accounts `Security$ProjectSecurity`
    /// declares, kept out of generated source by being read here instead.
    pub accounts: LocalAccounts,
    /// The decoded modules the schema and policy were derived from, kept so
    /// callers (the flow engine, `mxrs run`) never re-open the project.
    pub modules: Vec<Module>,
    pub entities: usize,
    pub documents: usize,
    /// `Entity.QualifiedName: xpath` pairs whose constraint is outside the
    /// subset `mxrs_runtime::xpath` evaluates. Those rules are still carried,
    /// but their constraint can never hold, so they deny every record. A
    /// constraint the evaluator *can* read is not listed here.
    pub skipped_xpath_rules: Vec<String>,
}

pub fn boot(path: impl AsRef<Path>) -> Result<Boot, BootError> {
    let project =
        Project::open(path.as_ref(), true).map_err(|error| BootError::Model(error.to_string()))?;
    let modules = project
        .modules()
        .map_err(|error| BootError::Model(error.to_string()))?;
    let security = project_security(&project)?;
    build(modules, security.as_ref())
}

/// The first `Security$ProjectSecurity` unit, parsed. mxrb locates it the
/// same way: a lazy scan over every unit's `$Type`.
fn project_security(project: &Project) -> Result<Option<Document>, BootError> {
    for unit in project
        .all_units()
        .map_err(|error| BootError::Model(error.to_string()))?
    {
        let doc = project
            .mpr()
            .parse_contents(&unit)
            .map_err(|error| BootError::Model(error.to_string()))?;
        if doc.get_str("$Type").ok() == Some("Security$ProjectSecurity") {
            return Ok(Some(doc));
        }
    }
    Ok(None)
}

pub fn build(modules: Vec<Module>, security: Option<&Document>) -> Result<Boot, BootError> {
    let mut schema = StoreSchema::default();
    let mut entities = 0;
    let mut documents = BTreeMap::new();
    let mut entity_rules: BTreeMap<String, Vec<EntityRule>> = BTreeMap::new();
    let mut skipped_xpath_rules = Vec::new();
    for module in &modules {
        let Some(module_name) = module.name.as_deref() else {
            continue;
        };
        for entity in module.entities() {
            let Some(entity_name) = entity.name.as_deref() else {
                continue;
            };
            let qualified = entity
                .qualified_name
                .clone()
                .unwrap_or_else(|| format!("{module_name}.{entity_name}"));
            let mut defaults = BTreeMap::new();
            for attribute in &entity.attributes {
                let (Some(name), Some(default)) = (
                    attribute.name.as_deref(),
                    attribute.default_value.as_deref(),
                ) else {
                    continue;
                };
                if let Some(value) = typed_default(attribute, default, &qualified)? {
                    defaults.insert(name.to_string(), value);
                }
            }
            // The relational schema (`schema::derive`) skips non-persistable
            // entities *and* OQL views, which have no table of their own.
            // Treating a view as durable here would hand `persistent_objects()`
            // rows the writer cannot place, failing the whole shutdown save —
            // so the two filters have to state the same rule.
            let transient = !entity.persistable || entity.oql_view();
            schema = schema.entity(qualified.clone(), defaults, transient);
            entities += 1;
            let mut rules = Vec::new();
            for rule in &entity.access_rules {
                let xpath = rule.xpath.trim();
                // A constraint the evaluator cannot read must not silently
                // become an unconstrained grant, so it is reported here at
                // boot as well as denied per record at authorization time.
                if !xpath.is_empty() && !mxrs_runtime::xpath::is_supported(xpath) {
                    skipped_xpath_rules.push(format!("{qualified}: {xpath}"));
                }
                rules.push(EntityRule {
                    module_roles: rule.roles.iter().cloned().collect(),
                    create: rule.create,
                    delete: rule.delete,
                    default_member_right: Some(member_right(&rule.default_rights)),
                    member_rights: rule
                        .members
                        .iter()
                        .map(|member| (member.name.clone(), member_right(&member.rights)))
                        .collect(),
                    xpath: xpath.to_string(),
                });
            }
            if !rules.is_empty() {
                entity_rules.insert(qualified, rules);
            }
        }
        for flow in module
            .microflows
            .iter()
            .chain(&module.nanoflows)
            .chain(&module.rules)
        {
            let Some(flow_name) = flow.name.as_deref() else {
                continue;
            };
            documents.insert(
                format!("{module_name}.{flow_name}"),
                flow.allowed_module_roles.iter().cloned().collect(),
            );
        }
    }
    let mut policy = SecurityPolicy {
        enabled: false,
        administrator_roles: BTreeSet::new(),
        user_role_modules: BTreeMap::new(),
        documents: BTreeMap::new(),
        entities: entity_rules,
    };
    let document_count = documents.len();
    policy.documents = documents;
    let mut accounts = LocalAccounts::default();
    if let Some(security) = security {
        apply_project_security(&mut policy, security);
        accounts = local_accounts(security);
    }
    Ok(Boot {
        schema,
        security: policy,
        accounts,
        modules,
        entities,
        documents: document_count,
        skipped_xpath_rules,
    })
}

/// Reads `Security$ProjectSecurity`'s sign-in material. An account with no
/// name or no password cannot be signed in to and is left out rather than
/// carried as an account that matches an empty credential.
fn local_accounts(security: &Document) -> LocalAccounts {
    let account = |name: &str, password: &str, user_roles: BTreeSet<String>| {
        (!name.is_empty() && !password.is_empty()).then(|| Account {
            name: name.to_string(),
            password: password.to_string(),
            user_roles,
        })
    };
    let administrator_role = security.get_str("AdminUserRole").unwrap_or_default();
    let administrator = account(
        security.get_str("AdminUserName").unwrap_or_default(),
        security.get_str("AdminPassword").unwrap_or_default(),
        BTreeSet::from([administrator_role.to_string()]),
    );
    // Studio Pro's own switch: demo users exist in the model whether or not
    // they may sign in, so an unchecked box means no demo account at all.
    let demo = if security.get_bool("EnableDemoUsers").unwrap_or(false) {
        documents_in(security, "DemoUsers")
            .into_iter()
            .filter_map(|user| {
                account(
                    user.get_str("UserName").unwrap_or_default(),
                    user.get_str("Password").unwrap_or_default(),
                    string_set(&user, "UserRoles"),
                )
            })
            .collect()
    } else {
        Vec::new()
    };
    let guest_user_role = security
        .get_bool("EnableGuestAccess")
        .unwrap_or(false)
        .then(|| security.get_str("GuestUserRole").unwrap_or_default())
        .filter(|role| !role.is_empty())
        .map(str::to_string);
    LocalAccounts {
        administrator,
        demo,
        guest_user_role,
    }
}

/// The documents inside a BSON array field, in declaration order.
fn documents_in(document: &Document, field: &str) -> Vec<Document> {
    match document.get(field) {
        Some(Bson::Array(items)) => parse_array(Some(items))
            .items
            .into_iter()
            .filter_map(|item| match item {
                Bson::Document(document) => Some(document),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    }
}

fn string_set(document: &Document, field: &str) -> BTreeSet<String> {
    match document.get(field) {
        Some(Bson::Array(items)) => parse_array(Some(items))
            .items
            .into_iter()
            .filter_map(|item| match item {
                Bson::String(value) => Some(value),
                _ => None,
            })
            .collect(),
        _ => BTreeSet::new(),
    }
}

fn apply_project_security(policy: &mut SecurityPolicy, security: &Document) {
    let level = security.get_str("SecurityLevel").unwrap_or_default();
    // mxrb: enabled unless the level names one of the known "off" values —
    // an unknown or missing level on a present unit fails closed as enabled.
    policy.enabled = !matches!(level, "CheckNothing" | "Off" | "None");
    let configured_administrator = security.get_str("AdminUserRole").unwrap_or_default();
    let roles = match security.get("UserRoles") {
        Some(Bson::Array(items)) => parse_array(Some(items)).items,
        _ => Vec::new(),
    };
    for role in roles {
        let Bson::Document(role) = role else { continue };
        let Ok(name) = role.get_str("Name") else {
            continue;
        };
        let module_roles = match role.get("ModuleRoles") {
            Some(Bson::Array(items)) => parse_array(Some(items))
                .items
                .into_iter()
                .filter_map(|item| match item {
                    Bson::String(value) => Some(value),
                    _ => None,
                })
                .collect(),
            _ => BTreeSet::new(),
        };
        if name == configured_administrator || role.get_bool("ManageAllRoles").unwrap_or(false) {
            policy.administrator_roles.insert(name.to_string());
        }
        policy
            .user_role_modules
            .insert(name.to_string(), module_roles);
    }
}

/// mxrb's `AccessControl::RIGHTS`: `None`/`ReadOnly`/`ReadWrite`, matched
/// case-insensitively. Anything else — including a missing or corrupted
/// value — is a **denial** (`RIGHTS.fetch(…, :none)` in mxrb), never a
/// fallback to broader rights.
fn member_right(rights: &str) -> MemberRight {
    match rights.to_ascii_lowercase().as_str() {
        "readonly" => MemberRight::Read,
        "readwrite" => MemberRight::Write,
        _ => MemberRight::None,
    }
}

/// A stored default is a string; the runtime store serves JSON. String-family
/// defaults stay verbatim (including empty), numeric and boolean defaults are
/// parsed, and an empty non-string default simply means "no default".
fn typed_default(
    attribute: &Attribute,
    default: &str,
    entity: &str,
) -> Result<Option<Value>, BootError> {
    let invalid = || BootError::InvalidDefault {
        entity: entity.to_string(),
        attribute: attribute.name.clone().unwrap_or_default(),
        kind: attribute.attribute_type,
        value: default.to_string(),
    };
    Ok(match attribute.attribute_type {
        AttributeType::String
        | AttributeType::HashString
        | AttributeType::Enum
        | AttributeType::DateTime
        | AttributeType::Binary => Some(Value::String(default.to_string())),
        AttributeType::Integer | AttributeType::Long | AttributeType::AutoNumber => {
            if default.is_empty() {
                None
            } else {
                Some(Value::from(default.parse::<i64>().map_err(|_| invalid())?))
            }
        }
        AttributeType::Float | AttributeType::Decimal => {
            if default.is_empty() {
                None
            } else {
                let number = default
                    .parse::<f64>()
                    .ok()
                    .and_then(serde_json::Number::from_f64);
                Some(Value::Number(number.ok_or_else(invalid)?))
            }
        }
        AttributeType::Boolean => {
            if default.is_empty() {
                None
            } else if default.eq_ignore_ascii_case("true") {
                Some(Value::Bool(true))
            } else if default.eq_ignore_ascii_case("false") {
                Some(Value::Bool(false))
            } else {
                return Err(invalid());
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use mxrs_bson::doc;
    use mxrs_model::entity::{AccessMember, AccessMemberKind, AccessRule, Entity};
    use mxrs_runtime::{EntityAction, SecurityContext};

    use super::*;

    fn entity(module: &str, name: &str) -> Entity {
        let mut entity = Entity::from_bson(&doc! {
            "$Type": "DomainModels$Entity",
            "Name": name,
        });
        entity.qualified_name = Some(format!("{module}.{name}"));
        entity
    }

    fn module_with(name: &str, entities: Vec<Entity>) -> Module {
        Module {
            id: String::new(),
            name: Some(name.to_string()),
            sort_index: None,
            from_app_store: false,
            app_store_guid: None,
            app_store_version: None,
            export_level: String::new(),
            domain_model: Some(mxrs_model::DomainModel {
                id: None,
                native_type: None,
                documentation: String::new(),
                entities,
                associations: Vec::new(),
                cross_associations: Vec::new(),
            }),
            pages: Vec::new(),
            microflows: Vec::new(),
            nanoflows: Vec::new(),
            rules: Vec::new(),
            menus: Vec::new(),
            module_roles: Vec::new(),
            artifact_units: Vec::new(),
        }
    }

    /// `schema::derive` in mxrs-runtime-sqlite gives an OQL view no table,
    /// so the store must not call it durable either: a committed view object
    /// would reach the relational writer with nowhere to go and fail the
    /// whole shutdown save, losing the session.
    #[test]
    fn oql_views_are_transient_like_the_relational_schema_treats_them() {
        let mut view = entity("Sales", "OrderSummary");
        view.persistable = true;
        view.oql_query = Some("SELECT 1 FROM Sales.Order".to_string());
        assert!(view.oql_view());
        let mut table = entity("Sales", "Order");
        table.persistable = true;

        let boot = build(vec![module_with("Sales", vec![view, table])], None).unwrap();
        let mut store = mxrs_runtime::Store::new(boot.schema.clone());
        for name in ["Sales.OrderSummary", "Sales.Order"] {
            let created = store.create(name).unwrap();
            store.commit(name, &created.id).unwrap();
        }
        let persistent = store.persistent_objects();
        assert_eq!(persistent.len(), 1);
        assert_eq!(persistent[0].entity, "Sales.Order");
    }

    #[test]
    fn defaults_become_typed_json_values_and_bad_defaults_fail_closed() {
        let mut order = entity("Sales", "Order");
        order.attributes = vec![
            attribute("Label", AttributeType::String, ""),
            attribute("Total", AttributeType::Decimal, "12.5"),
            attribute("Count", AttributeType::Integer, "3"),
            attribute("Open", AttributeType::Boolean, "TRUE"),
            attribute("Placed", AttributeType::DateTime, ""),
        ];
        let boot = build(vec![module_with("Sales", vec![order])], None).unwrap();
        assert_eq!(boot.entities, 1);
        assert!(boot.schema.contains("Sales.Order"));
        let mut store = mxrs_runtime::Store::new(boot.schema.clone());
        let created = store.create("Sales.Order").unwrap();
        assert_eq!(created.members["Label"], Value::String(String::new()));
        assert_eq!(created.members["Total"], serde_json::json!(12.5));
        assert_eq!(created.members["Count"], serde_json::json!(3));
        assert_eq!(created.members["Open"], Value::Bool(true));
        assert_eq!(created.members["Placed"], Value::String(String::new()));

        let mut broken = entity("Sales", "Broken");
        broken.attributes = vec![attribute("Count", AttributeType::Integer, "many")];
        let error = build(vec![module_with("Sales", vec![broken])], None).unwrap_err();
        assert!(matches!(error, BootError::InvalidDefault { .. }));
        assert!(error.to_string().contains("Sales.Broken"));
    }

    fn attribute(name: &str, kind: AttributeType, default: &str) -> Attribute {
        let mut attribute = Attribute::from_bson(&doc! { "Name": name });
        attribute.attribute_type = kind;
        attribute.default_value = Some(default.to_string());
        attribute
    }

    #[test]
    fn access_rules_map_to_entity_rules_and_xpath_rules_narrow_them_per_record() {
        let mut order = entity("Sales", "Order");
        order.access_rules = vec![
            AccessRule {
                id: None,
                roles: vec!["Sales.User".to_string()],
                create: true,
                delete: false,
                documentation: String::new(),
                default_rights: "ReadOnly".to_string(),
                members: vec![
                    AccessMember {
                        id: None,
                        name: "Total".to_string(),
                        reference: String::new(),
                        rights: "ReadWrite".to_string(),
                        kind: AccessMemberKind::Attribute,
                        raw: doc! {},
                    },
                    AccessMember {
                        id: None,
                        name: "Notes".to_string(),
                        reference: String::new(),
                        rights: "Corrupted".to_string(),
                        kind: AccessMemberKind::Attribute,
                        raw: doc! {},
                    },
                ],
                xpath: String::new(),
                xpath_caption: None,
                raw: doc! {},
            },
            AccessRule {
                id: None,
                roles: vec!["Sales.Owner".to_string()],
                create: true,
                delete: true,
                documentation: String::new(),
                default_rights: "ReadWrite".to_string(),
                members: vec![],
                xpath: "[Owner = $currentUser]".to_string(),
                xpath_caption: None,
                raw: doc! {},
            },
        ];
        let security = doc! {
            "$Type": "Security$ProjectSecurity",
            "SecurityLevel": "CheckEverything",
            "AdminUserRole": "Administrator",
            "UserRoles": mxrs_bson::build_array(vec![
                Bson::Document(doc! {
                    "$Type": "Security$UserRole",
                    "Name": "User",
                    "ManageAllRoles": false,
                    "ModuleRoles": mxrs_bson::build_array(vec![Bson::String("Sales.User".to_string())], 1),
                }),
                Bson::Document(doc! {
                    "$Type": "Security$UserRole",
                    "Name": "Administrator",
                    "ManageAllRoles": true,
                    "ModuleRoles": mxrs_bson::build_array(vec![], 1),
                }),
            ], 2),
        };
        let boot = build(vec![module_with("Sales", vec![order])], Some(&security)).unwrap();
        assert!(boot.security.enabled);
        // `[Owner = $currentUser]` is inside the evaluated subset, so it is
        // carried rather than reported — the report names only constraints
        // mxrs cannot read.
        assert!(boot.skipped_xpath_rules.is_empty());
        assert!(boot.security.administrator_roles.contains("Administrator"));
        let sales_user = SecurityContext {
            user: Some("alice".to_string()),
            user_roles: ["User".to_string()].into(),
            module_roles: BTreeSet::new(),
            variables: BTreeMap::new(),
        };
        assert!(boot.security.entity_allowed(
            "Sales.Order",
            EntityAction::Create,
            None,
            None,
            &sales_user
        ));
        assert!(!boot.security.entity_allowed(
            "Sales.Order",
            EntityAction::Delete,
            None,
            None,
            &sales_user
        ));
        assert!(boot.security.entity_allowed(
            "Sales.Order",
            EntityAction::Write,
            Some("Total"),
            None,
            &sales_user
        ));
        // An unknown member right is a denial (mxrb's RIGHTS.fetch(…, :none)),
        // never a fall-through to the rule's default.
        assert!(!boot.security.entity_allowed(
            "Sales.Order",
            EntityAction::Read,
            Some("Notes"),
            None,
            &sales_user
        ));
        // The owner rule is constrained, so it decides per record. Without a
        // record the question is about the entity, and the rule applies.
        let owner = SecurityContext {
            user: Some("bob".to_string()),
            user_roles: BTreeSet::new(),
            module_roles: ["Sales.Owner".to_string()].into(),
            variables: BTreeMap::new(),
        };
        assert!(boot.security.entity_allowed(
            "Sales.Order",
            EntityAction::Delete,
            None,
            None,
            &owner
        ));
        let his = BTreeMap::from([("Owner".to_string(), Value::String("bob".into()))]);
        let hers = BTreeMap::from([("Owner".to_string(), Value::String("alice".into()))]);
        assert!(boot.security.entity_allowed(
            "Sales.Order",
            EntityAction::Delete,
            None,
            Some(&his),
            &owner
        ));
        // Someone else's record: the only rule his roles reach does not
        // cover it, so the whole action is denied.
        assert!(!boot.security.entity_allowed(
            "Sales.Order",
            EntityAction::Delete,
            None,
            Some(&hers),
            &owner
        ));
    }

    /// A constraint the evaluator cannot read must deny every record and be
    /// reported, rather than quietly widening to the whole entity.
    #[test]
    fn an_unreadable_xpath_constraint_is_reported_and_denies_every_record() {
        let mut rule = AccessRule {
            id: None,
            roles: vec!["Sales.Owner".to_string()],
            create: true,
            delete: true,
            documentation: String::new(),
            default_rights: "ReadWrite".to_string(),
            members: vec![],
            xpath: "[contains(Name, 'x')]".to_string(),
            xpath_caption: None,
            raw: doc! {},
        };
        let build_with = |rule: AccessRule| {
            let mut entity = entity("Sales", "Order");
            entity.access_rules = vec![rule];
            build(
                vec![module_with("Sales", vec![entity])],
                Some(&doc! {
                    "$Type": "Security$ProjectSecurity",
                    "SecurityLevel": "CheckEverything",
                }),
            )
            .unwrap()
        };

        let boot = build_with(rule.clone());
        assert_eq!(
            boot.skipped_xpath_rules,
            ["Sales.Order: [contains(Name, 'x')]"]
        );
        let context = SecurityContext {
            user: Some("bob".to_string()),
            user_roles: BTreeSet::new(),
            module_roles: ["Sales.Owner".to_string()].into(),
            variables: BTreeMap::new(),
        };
        let record = BTreeMap::from([("Name".to_string(), Value::String("xyz".into()))]);
        assert!(!boot.security.entity_allowed(
            "Sales.Order",
            EntityAction::Delete,
            None,
            Some(&record),
            &context
        ));

        // A readable constraint is not reported at all — the report names
        // what mxrs will never honour, not every constrained rule.
        rule.xpath = "[Name = 'xyz']".to_string();
        let boot = build_with(rule);
        assert!(boot.skipped_xpath_rules.is_empty());
        assert!(boot.security.entity_allowed(
            "Sales.Order",
            EntityAction::Delete,
            None,
            Some(&record),
            &context
        ));
    }

    #[test]
    fn a_present_security_unit_with_an_unknown_level_counts_as_enabled() {
        for (level, enabled) in [
            ("CheckNothing", false),
            ("Off", false),
            ("None", false),
            ("CheckEverything", true),
            ("CheckFormsAndMicroflows", true),
            ("SomethingNew", true),
        ] {
            let security = doc! { "SecurityLevel": level };
            let boot = build(Vec::new(), Some(&security)).unwrap();
            assert_eq!(boot.security.enabled, enabled, "{level}");
        }
        assert!(!build(Vec::new(), None).unwrap().security.enabled);
    }
    /// The accounts a request can sign in as come from the model, and only the
    /// ones the project actually enables: Studio Pro's `EnableDemoUsers` box
    /// decides whether demo users may sign in at all, and `EnableGuestAccess`
    /// decides whether an anonymous caller has a role.
    #[test]
    fn local_accounts_follow_the_switches_the_project_declares() {
        let security = |enable_demo: bool, enable_guest: bool| {
            doc! {
                "$Type": "Security$ProjectSecurity",
                "SecurityLevel": "CheckEverything",
                "AdminUserRole": "Administrator",
                "AdminUserName": "MxAdmin",
                "AdminPassword": "let-me-in",
                "EnableDemoUsers": enable_demo,
                "EnableGuestAccess": enable_guest,
                "GuestUserRole": "Anonymous",
                "DemoUsers": mxrs_bson::build_array(
                    vec![
                        Bson::Document(doc! {
                            "$Type": "Security$DemoUserImpl",
                            "UserName": "demo_user",
                            "Password": "demo-secret",
                            "UserRoles": mxrs_bson::build_array(
                                vec![Bson::String("User".to_string())],
                                1,
                            ),
                        }),
                        // No password: nothing can be signed in to, so it is
                        // left out rather than matching an empty credential.
                        Bson::Document(doc! {
                            "$Type": "Security$DemoUserImpl",
                            "UserName": "demo_locked",
                            "Password": "",
                            "UserRoles": mxrs_bson::build_array(vec![], 1),
                        }),
                    ],
                    3,
                ),
            }
        };

        let accounts = build(Vec::new(), Some(&security(true, true)))
            .unwrap()
            .accounts;
        let administrator = accounts.sign_in("MxAdmin", "let-me-in").expect("admin");
        assert_eq!(administrator.user.as_deref(), Some("MxAdmin"));
        assert!(administrator.user_roles.contains("Administrator"));
        let demo = accounts.sign_in("demo_user", "demo-secret").expect("demo");
        assert!(demo.user_roles.contains("User"));
        assert!(accounts.sign_in("demo_locked", "").is_none());
        assert!(accounts.sign_in("MxAdmin", "wrong").is_none());
        assert!(accounts.sign_in("nobody", "let-me-in").is_none());
        assert!(accounts.can_sign_anyone_in());
        assert_eq!(accounts.guest_user_role(), Some("Anonymous"));
        assert!(accounts.anonymous().user_roles.contains("Anonymous"));
        // An anonymous caller is still a caller: no user, so nothing about the
        // request can claim to be one.
        assert!(accounts.anonymous().user.is_none());

        // Demo users switched off cannot sign in, however right the password.
        let restricted = build(Vec::new(), Some(&security(false, false)))
            .unwrap()
            .accounts;
        assert!(restricted.sign_in("demo_user", "demo-secret").is_none());
        assert!(restricted.sign_in("MxAdmin", "let-me-in").is_some());
        // Guest access off leaves an anonymous caller with no role, which
        // entity access then denies rather than waives.
        assert_eq!(restricted.guest_user_role(), None);
        assert!(restricted.anonymous().user_roles.is_empty());

        // A project with no security unit can sign nobody in.
        let none = build(Vec::new(), None).unwrap().accounts;
        assert!(!none.can_sign_anyone_in());
        assert!(none.sign_in("MxAdmin", "let-me-in").is_none());
    }

    #[test]
    fn a_password_is_compared_without_an_early_exit() {
        assert!(constant_time_eq(b"secret", b"secret"));
        assert!(!constant_time_eq(b"secret", b"secreT"));
        assert!(!constant_time_eq(b"secret", b"secre"));
        assert!(constant_time_eq(b"", b""));
    }
}
