//! Runtime-neutral application execution core.
//!
//! The store and authorization engine are intentionally independent of HTTP,
//! SQLite, or Mendix's Java runtime. This makes transaction and security
//! semantics directly testable and lets later adapters share one fail-closed
//! implementation.
//!
//! StoreSchema currently declares defaults and persistence, not Mendix member
//! types, requiredness, association targets, or lifecycle hooks. Member names
//! and persisted identities are validated; JSON value types are deliberately
//! not inferred from defaults. Entity access rules must still be enforced by
//! callers using the policy, not by the raw store passed to native actions.

use std::collections::{BTreeMap, BTreeSet};
use std::panic::{AssertUnwindSafe, catch_unwind, resume_unwind};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum RuntimeError {
    #[error("unknown entity {0}")]
    UnknownEntity(String),
    #[error("unknown object {entity}/{id}")]
    UnknownObject { entity: String, id: String },
    #[error("not authorized to {action} {resource}")]
    NotAuthorized { action: String, resource: String },
    #[error("unknown action {0}")]
    UnknownAction(String),
    #[error("runtime transaction failed: {0}")]
    Transaction(String),
    #[error("invalid persistent runtime state: {0}")]
    InvalidPersistence(String),
}

pub mod xpath;

pub type Result<T> = std::result::Result<T, RuntimeError>;

/// Marks a member string as a datetime rather than text.
///
/// Store members are JSON, which has no datetime type, so a datetime written
/// by the flow interpreter has to survive the round trip as a string. Sniffing
/// the string back — "does it look like a rendered timestamp?" — silently
/// re-types any text a user happened to write in that shape, turning
/// `$o/Note = '2026-09-21 12:00:00 UTC'` false and `<` into an error. An
/// explicit tag cannot collide by accident: `U+0001` is not legal in an XML
/// or JSON model's text, so no authored value starts with it.
///
/// Members are tagged as `{prefix}{epoch seconds}`. Only member *storage* uses
/// this encoding; every rendering boundary (JSON results, REST bodies,
/// `formatDateTime`) emits the human form.
pub const DATETIME_MEMBER_PREFIX: &str = "\u{1}mxrs-datetime:";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ObjectValue {
    pub entity: String,
    pub id: String,
    pub members: BTreeMap<String, Value>,
}

/// Persistence adapters can reject malformed rows before migrating storage,
/// without having the project's entity schema available. Use one validator per
/// snapshot so IDs remain unambiguous across entity and association boundaries.
#[derive(Debug, Default)]
pub struct PersistentObjectValidator {
    identifiers: BTreeSet<String>,
}

impl PersistentObjectValidator {
    pub fn validate(&mut self, object: &ObjectValue) -> Result<()> {
        if !valid_name(&object.entity)
            || !valid_name(&object.id)
            || !object.members.keys().all(|member| valid_name(member))
        {
            return Err(RuntimeError::InvalidPersistence("entity, object ID, and member names must be nonempty and contain no control characters".into()));
        }
        if !self.identifiers.insert(object.id.clone()) {
            return Err(RuntimeError::InvalidPersistence(format!(
                "duplicate object {}/{}",
                object.entity, object.id
            )));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Default)]
pub struct StoreSchema {
    entities: Arc<BTreeMap<String, EntitySchema>>,
}

#[derive(Debug, Clone, Default)]
struct EntitySchema {
    defaults: BTreeMap<String, Value>,
    transient: bool,
}

impl StoreSchema {
    pub fn entity(
        mut self,
        name: impl Into<String>,
        defaults: BTreeMap<String, Value>,
        transient: bool,
    ) -> Self {
        Arc::make_mut(&mut self.entities).insert(
            name.into(),
            EntitySchema {
                defaults,
                transient,
            },
        );
        self
    }

    pub fn contains(&self, entity: &str) -> bool {
        self.entities.contains_key(entity)
    }
}

/// Unit-of-work store. New persistent records are visible within the current
/// unit of work but disappear at transaction completion unless committed.
///
/// The lifecycle follows `mxrb/lib/mxrb/runtime/native.rb:93-166`, including
/// session-local commits and rollback of transient objects. Unlike its
/// permissive object API, unknown entity/object mutations fail explicitly.
/// Read-only snapshots share immutable roots; writes copy map keys and only
/// the changed object's member payload. No transaction clones every payload.
#[derive(Debug, Clone)]
pub struct Store {
    schema: StoreSchema,
    records: Arc<RecordMap>,
    committed: Arc<RecordMap>,
    dirty: Arc<BTreeSet<(String, String)>>,
}

type EntityRecords = BTreeMap<String, Arc<ObjectValue>>;
type RecordMap = BTreeMap<String, Arc<EntityRecords>>;

impl Store {
    pub fn new(schema: StoreSchema) -> Self {
        Self {
            schema,
            records: Arc::default(),
            committed: Arc::default(),
            dirty: Arc::default(),
        }
    }

    pub fn create(&mut self, entity: &str) -> Result<ObjectValue> {
        let definition = self
            .schema
            .entities
            .get(entity)
            .ok_or_else(|| RuntimeError::UnknownEntity(entity.to_string()))?;
        if !valid_name(entity) || !definition.defaults.keys().all(|member| valid_name(member)) {
            return Err(RuntimeError::Transaction(
                "entity and member names must be nonempty and contain no control characters".into(),
            ));
        }
        let value = ObjectValue {
            entity: entity.to_string(),
            id: uuid::Uuid::new_v4().to_string(),
            members: definition.defaults.clone(),
        };
        put_record(&mut self.records, Arc::new(value.clone()));
        self.mark_dirty(entity, &value.id);
        Ok(value)
    }

    pub fn retrieve(&self, entity: &str) -> Result<Vec<ObjectValue>> {
        if !self.schema.contains(entity) {
            return Err(RuntimeError::UnknownEntity(entity.to_string()));
        }
        Ok(self
            .records
            .get(entity)
            .into_iter()
            .flat_map(|records| records.values())
            .map(|object| object.as_ref().clone())
            .collect())
    }

    pub fn find(&self, entity: &str, id: &str) -> Result<Option<ObjectValue>> {
        if !self.schema.contains(entity) {
            return Err(RuntimeError::UnknownEntity(entity.to_string()));
        }
        Ok(self
            .records
            .get(entity)
            .and_then(|records| records.get(id))
            .map(|object| object.as_ref().clone()))
    }

    pub fn set_member(&mut self, entity: &str, id: &str, member: &str, value: Value) -> Result<()> {
        if !valid_name(member) {
            return Err(RuntimeError::Transaction(
                "member names must be nonempty and contain no control characters".into(),
            ));
        }
        self.object_mut(entity, id)?
            .members
            .insert(member.to_string(), value);
        self.mark_dirty(entity, id);
        Ok(())
    }

    pub fn commit(&mut self, entity: &str, id: &str) -> Result<ObjectValue> {
        let object = self.object(entity, id)?.clone();
        put_record(&mut self.committed, object.clone());
        Arc::make_mut(&mut self.dirty).remove(&(entity.to_string(), id.to_string()));
        Ok(object.as_ref().clone())
    }

    pub fn rollback(&mut self, entity: &str, id: &str) -> Result<()> {
        self.object(entity, id)?;
        let key = (entity.to_string(), id.to_string());
        if let Some(object) = self
            .committed
            .get(entity)
            .and_then(|records| records.get(id))
            .cloned()
        {
            put_record(&mut self.records, object);
        } else {
            remove_record(&mut self.records, entity, id);
        }
        Arc::make_mut(&mut self.dirty).remove(&key);
        Ok(())
    }

    pub fn delete(&mut self, entity: &str, id: &str) -> Result<ObjectValue> {
        let object = self.object(entity, id)?.as_ref().clone();
        remove_record(&mut self.records, entity, id);
        remove_record(&mut self.committed, entity, id);
        Arc::make_mut(&mut self.dirty).remove(&(entity.to_string(), id.to_string()));
        Ok(object)
    }

    pub fn retrieve_association(&self, association: &str, start: &ObjectValue) -> Vec<ObjectValue> {
        let short = association.rsplit('.').next().unwrap_or(association);
        let ids = start
            .members
            .get(association)
            .or_else(|| start.members.get(short))
            .into_iter()
            .flat_map(reference_ids)
            .collect::<BTreeSet<_>>();
        let mut related = self
            .records
            .values()
            .flat_map(|records| records.values())
            .filter(|candidate| {
                ids.contains(candidate.id.as_str())
                    || candidate
                        .members
                        .get(association)
                        .or_else(|| candidate.members.get(short))
                        .is_some_and(|value| reference_ids(value).contains(&start.id.as_str()))
            })
            .map(|object| object.as_ref().clone())
            .collect::<Vec<_>>();
        related.sort_by(|left, right| left.id.cmp(&right.id));
        related.dedup_by(|left, right| left.id == right.id);
        related
    }

    /// Returns only committed, non-transient records in deterministic order.
    pub fn persistent_objects(&self) -> Vec<ObjectValue> {
        self.committed
            .iter()
            .filter(|(entity, _)| !self.is_transient(entity))
            .flat_map(|(_, records)| records.values())
            .map(|object| object.as_ref().clone())
            .collect()
    }

    /// Atomically replaces committed state loaded by a persistence adapter.
    /// Existing transient records remain session-local and are never loaded.
    pub fn restore_persistent(
        &mut self,
        objects: impl IntoIterator<Item = ObjectValue>,
    ) -> Result<()> {
        let mut replacement: RecordMap = BTreeMap::new();
        let mut validator = PersistentObjectValidator {
            identifiers: self
                .records
                .iter()
                .filter(|(entity, _)| self.is_transient(entity))
                .flat_map(|(_, records)| records.keys().cloned())
                .collect(),
        };
        for object in objects {
            let definition = self.schema.entities.get(&object.entity).ok_or_else(|| {
                RuntimeError::InvalidPersistence(format!("unknown entity {}", object.entity))
            })?;
            if definition.transient {
                return Err(RuntimeError::InvalidPersistence(format!(
                    "transient entity {} cannot be persisted",
                    object.entity
                )));
            }
            validator.validate(&object)?;
            Arc::make_mut(replacement.entry(object.entity.clone()).or_default())
                .insert(object.id.clone(), Arc::new(object));
        }
        let mut records = self.transient_records(&self.records);
        records.extend(
            replacement
                .iter()
                .map(|(entity, records)| (entity.clone(), records.clone())),
        );
        let mut committed = self.transient_records(&self.committed);
        committed.extend(replacement);
        self.records = Arc::new(records);
        self.committed = Arc::new(committed);
        self.dirty = Arc::default();
        Ok(())
    }

    pub fn transaction<T>(&mut self, operation: impl FnOnce(&mut Self) -> Result<T>) -> Result<T> {
        let snapshot = self.clone();
        match catch_unwind(AssertUnwindSafe(|| operation(self))) {
            Ok(Ok(result)) => {
                drop(snapshot);
                self.discard_uncommitted();
                Ok(result)
            }
            Ok(Err(error)) => {
                *self = snapshot;
                Err(error)
            }
            Err(panic) => {
                *self = snapshot;
                resume_unwind(panic)
            }
        }
    }

    /// Whether an object has ever been committed. Lifecycle semantics need
    /// this distinction: committing a fresh object fires create events,
    /// committing an already-committed one fires update events.
    pub fn is_committed(&self, entity: &str, id: &str) -> bool {
        self.committed
            .get(entity)
            .is_some_and(|records| records.contains_key(id))
    }

    /// Completes a root unit of work for callers that manage their own
    /// snapshot/rollback instead of using [`Store::transaction`]: uncommitted
    /// new records disappear and uncommitted changes revert, exactly like the
    /// end of a successful transaction.
    pub fn end_unit_of_work(&mut self) {
        self.discard_uncommitted();
    }

    fn discard_uncommitted(&mut self) {
        if self.dirty.is_empty() {
            return;
        }
        let dirty = std::mem::take(&mut self.dirty);
        for (entity, id) in dirty.iter() {
            if let Some(object) = self
                .committed
                .get(entity)
                .and_then(|records| records.get(id))
            {
                put_record(&mut self.records, object.clone());
            } else {
                remove_record(&mut self.records, entity, id);
            }
        }
    }

    fn object(&self, entity: &str, id: &str) -> Result<&Arc<ObjectValue>> {
        if !self.schema.contains(entity) {
            return Err(RuntimeError::UnknownEntity(entity.to_string()));
        }
        self.records
            .get(entity)
            .and_then(|records| records.get(id))
            .ok_or_else(|| RuntimeError::UnknownObject {
                entity: entity.to_string(),
                id: id.to_string(),
            })
    }

    fn object_mut(&mut self, entity: &str, id: &str) -> Result<&mut ObjectValue> {
        if !self.schema.contains(entity) {
            return Err(RuntimeError::UnknownEntity(entity.to_string()));
        }
        Arc::make_mut(&mut self.records)
            .get_mut(entity)
            .and_then(|records| Arc::make_mut(records).get_mut(id))
            .map(Arc::make_mut)
            .ok_or_else(|| RuntimeError::UnknownObject {
                entity: entity.to_string(),
                id: id.to_string(),
            })
    }

    fn is_transient(&self, entity: &str) -> bool {
        self.schema
            .entities
            .get(entity)
            .is_some_and(|definition| definition.transient)
    }

    fn mark_dirty(&mut self, entity: &str, id: &str) {
        if !self.is_transient(entity) {
            Arc::make_mut(&mut self.dirty).insert((entity.to_string(), id.to_string()));
        }
    }

    fn transient_records(&self, records: &RecordMap) -> RecordMap {
        records
            .iter()
            .filter(|(entity, _)| self.is_transient(entity))
            .map(|(entity, records)| (entity.clone(), records.clone()))
            .collect()
    }
}

fn put_record(records: &mut Arc<RecordMap>, object: Arc<ObjectValue>) {
    Arc::make_mut(
        Arc::make_mut(records)
            .entry(object.entity.clone())
            .or_default(),
    )
    .insert(object.id.clone(), object);
}

fn remove_record(records: &mut Arc<RecordMap>, entity: &str, id: &str) {
    if let Some(records) = Arc::make_mut(records).get_mut(entity) {
        Arc::make_mut(records).remove(id);
    }
}

fn valid_name(value: &str) -> bool {
    !value.trim().is_empty() && !value.chars().any(char::is_control)
}

fn reference_ids(value: &Value) -> Vec<&str> {
    match value {
        Value::String(value) => vec![value],
        Value::Array(values) => values.iter().filter_map(Value::as_str).collect(),
        _ => vec![],
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SecurityContext {
    pub user: Option<String>,
    pub user_roles: BTreeSet<String>,
    pub module_roles: BTreeSet<String>,
    /// Session values an XPath constraint can name as `$variable`. Kept on
    /// the context rather than passed alongside it so a constraint can never
    /// be evaluated against variables from a different caller.
    pub variables: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum MemberRight {
    None,
    Read,
    Write,
}

#[derive(Debug, Clone, Default)]
pub struct EntityRule {
    pub module_roles: BTreeSet<String>,
    pub create: bool,
    pub delete: bool,
    pub default_member_right: Option<MemberRight>,
    pub member_rights: BTreeMap<String, MemberRight>,
    /// The rule's XPath constraint, empty when it applies to every record.
    ///
    /// A constraint narrows *which records* the rule speaks for, so it can
    /// only be evaluated when there is a record in hand. Asking whether a
    /// role may create or read the entity at all is a question about no
    /// particular record, and there the rule applies unconstrained — the
    /// oracle's `next true unless evaluate_xpath && record`.
    pub xpath: String,
}

#[derive(Debug, Clone, Default)]
pub struct SecurityPolicy {
    pub enabled: bool,
    pub administrator_roles: BTreeSet<String>,
    pub user_role_modules: BTreeMap<String, BTreeSet<String>>,
    pub documents: BTreeMap<String, BTreeSet<String>>,
    pub entities: BTreeMap<String, Vec<EntityRule>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntityAction {
    Create,
    Read,
    Write,
    Delete,
}

impl SecurityPolicy {
    pub fn expand_context(&self, mut context: SecurityContext) -> SecurityContext {
        for role in &context.user_roles {
            if let Some(module_roles) = self.user_role_modules.get(role) {
                context.module_roles.extend(module_roles.iter().cloned());
            }
        }
        context
    }

    pub fn document_allowed(&self, document: &str, context: &SecurityContext) -> bool {
        if !self.enabled || self.administrator(context) {
            return true;
        }
        let context = self.expand_context(context.clone());
        self.documents
            .get(document)
            .is_some_and(|roles| !roles.is_disjoint(&context.module_roles))
    }

    /// Whether `context` may take `action` on `entity`, optionally narrowed to
    /// one `member` and one `record`.
    ///
    /// `record` is what makes XPath constraints decidable. Without it the
    /// question is about the entity rather than about any row, and a
    /// constrained rule applies unconstrained — the same split the oracle
    /// draws. With it, a rule whose constraint does not hold for that record
    /// does not speak for it, and a constraint this runtime cannot evaluate
    /// is a denial rather than a guess.
    pub fn entity_allowed(
        &self,
        entity: &str,
        action: EntityAction,
        member: Option<&str>,
        record: Option<&BTreeMap<String, Value>>,
        context: &SecurityContext,
    ) -> bool {
        if !self.enabled || self.administrator(context) {
            return true;
        }
        let context = self.expand_context(context.clone());
        self.entities.get(entity).is_some_and(|rules| {
            rules
                .iter()
                .filter(|rule| !rule.module_roles.is_disjoint(&context.module_roles))
                .filter(|rule| match record {
                    Some(record) if !rule.xpath.trim().is_empty() => {
                        xpath::evaluate(&rule.xpath, record, &context) == Some(true)
                    }
                    _ => true,
                })
                .any(|rule| match action {
                    EntityAction::Create => rule.create,
                    EntityAction::Delete => rule.delete,
                    EntityAction::Read => member.is_none_or(|name| {
                        rule.member_rights
                            .get(name)
                            .copied()
                            .or(rule.default_member_right)
                            .is_some_and(|right| right >= MemberRight::Read)
                    }),
                    EntityAction::Write => member.map_or_else(
                        || {
                            rule.default_member_right == Some(MemberRight::Write)
                                || rule
                                    .member_rights
                                    .values()
                                    .any(|right| *right == MemberRight::Write)
                        },
                        |name| {
                            rule.member_rights
                                .get(name)
                                .copied()
                                .or(rule.default_member_right)
                                == Some(MemberRight::Write)
                        },
                    ),
                })
        })
    }

    pub fn authorize_document(&self, document: &str, context: &SecurityContext) -> Result<()> {
        if self.document_allowed(document, context) {
            Ok(())
        } else {
            Err(RuntimeError::NotAuthorized {
                action: "execute".to_string(),
                resource: document.to_string(),
            })
        }
    }

    /// Whether `context` holds any of `module_roles` — the question a
    /// published REST service's `AllowedRoles` asks of its caller.
    ///
    /// The caller's user roles are expanded into module roles first, so a
    /// context that names only user roles still answers. An empty
    /// `module_roles` grants nobody: a service that allows no role is a
    /// service no caller may reach, not one everybody may.
    pub fn holds_any_module_role(&self, context: &SecurityContext, module_roles: &[&str]) -> bool {
        if self.administrator(context) {
            return true;
        }
        let context = self.expand_context(context.clone());
        module_roles
            .iter()
            .any(|role| context.module_roles.contains(*role))
    }

    fn administrator(&self, context: &SecurityContext) -> bool {
        !self.administrator_roles.is_disjoint(&context.user_roles)
    }
}

pub trait Action: Send + Sync {
    fn execute(&self, store: &mut Store, arguments: &Value) -> Result<Value>;
}

impl<F> Action for F
where
    F: Fn(&mut Store, &Value) -> Result<Value> + Send + Sync,
{
    fn execute(&self, store: &mut Store, arguments: &Value) -> Result<Value> {
        self(store, arguments)
    }
}

pub struct Runtime {
    store: Store,
    security: SecurityPolicy,
    actions: BTreeMap<String, Box<dyn Action>>,
}

impl Runtime {
    pub fn new(store: Store, security: SecurityPolicy) -> Self {
        Self {
            store,
            security,
            actions: BTreeMap::new(),
        }
    }

    pub fn register_action(&mut self, name: impl Into<String>, action: impl Action + 'static) {
        self.actions.insert(name.into(), Box::new(action));
    }

    pub fn invoke(
        &mut self,
        name: &str,
        arguments: &Value,
        context: &SecurityContext,
    ) -> Result<Value> {
        self.security.authorize_document(name, context)?;
        let action = self
            .actions
            .get(name)
            .ok_or_else(|| RuntimeError::UnknownAction(name.to_string()))?;
        self.store
            .transaction(|store| action.execute(store, arguments))
    }

    pub fn store(&self) -> &Store {
        &self.store
    }

    pub fn store_mut(&mut self) -> &mut Store {
        &mut self.store
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn schema() -> StoreSchema {
        StoreSchema::default()
            .entity(
                "Sales.Order",
                BTreeMap::from([("Status".to_string(), json!("New"))]),
                false,
            )
            .entity("Session.Filter", BTreeMap::new(), true)
    }

    #[test]
    fn uncommitted_persistent_values_are_discarded_at_transaction_end() {
        let mut store = Store::new(schema());
        store
            .transaction(|store| {
                store.create("Sales.Order")?;
                Ok(())
            })
            .unwrap();
        assert!(store.retrieve("Sales.Order").unwrap().is_empty());
    }

    #[test]
    fn commit_persists_and_rollback_restores_member_values() {
        let mut store = Store::new(schema());
        let order = store.create("Sales.Order").unwrap();
        store.commit("Sales.Order", &order.id).unwrap();
        store
            .set_member("Sales.Order", &order.id, "Status", json!("Paid"))
            .unwrap();
        store.rollback("Sales.Order", &order.id).unwrap();
        assert_eq!(
            store
                .find("Sales.Order", &order.id)
                .unwrap()
                .unwrap()
                .members["Status"],
            "New"
        );
    }

    #[test]
    fn transaction_failure_restores_the_entire_unit_of_work() {
        let mut store = Store::new(schema());
        let result: Result<()> = store.transaction(|store| {
            store.create("Sales.Order")?;
            Err(RuntimeError::Transaction("stop".to_string()))
        });
        assert_eq!(result, Err(RuntimeError::Transaction("stop".to_string())));
        assert!(store.retrieve("Sales.Order").unwrap().is_empty());
    }

    #[test]
    fn transient_values_survive_successful_transaction_boundaries() {
        let mut store = Store::new(schema());
        store
            .transaction(|store| {
                store.create("Session.Filter")?;
                Ok(())
            })
            .unwrap();
        assert_eq!(store.retrieve("Session.Filter").unwrap().len(), 1);
    }

    #[test]
    fn association_retrieval_resolves_direct_ids() {
        let mut store = Store::new(schema());
        let first = store.create("Sales.Order").unwrap();
        let second = store.create("Sales.Order").unwrap();
        store
            .set_member(
                "Sales.Order",
                &first.id,
                "Sales.Order_Related",
                json!([second.id]),
            )
            .unwrap();
        let first = store.find("Sales.Order", &first.id).unwrap().unwrap();
        assert_eq!(
            store.retrieve_association("Sales.Order_Related", &first)[0].id,
            second.id
        );
    }

    #[test]
    fn association_retrieval_resolves_inverse_ids() {
        let mut store = Store::new(schema());
        let first = store.create("Sales.Order").unwrap();
        let second = store.create("Sales.Order").unwrap();
        store
            .set_member(
                "Sales.Order",
                &second.id,
                "Sales.Order_Related",
                json!(first.id),
            )
            .unwrap();
        assert_eq!(
            store.retrieve_association("Sales.Order_Related", &first)[0].id,
            second.id
        );
    }

    #[test]
    fn security_expands_user_roles_and_fails_closed() {
        let policy = SecurityPolicy {
            enabled: true,
            user_role_modules: BTreeMap::from([(
                "User".to_string(),
                BTreeSet::from(["Sales.Reader".to_string()]),
            )]),
            documents: BTreeMap::from([(
                "Sales.ACT_Load".to_string(),
                BTreeSet::from(["Sales.Reader".to_string()]),
            )]),
            ..Default::default()
        };
        let context = SecurityContext {
            user_roles: BTreeSet::from(["User".to_string()]),
            ..Default::default()
        };
        assert!(policy.document_allowed("Sales.ACT_Load", &context));
        assert!(!policy.document_allowed("Sales.ACT_Delete", &context));
    }

    #[test]
    fn member_rights_override_default_rights() {
        let policy = SecurityPolicy {
            enabled: true,
            entities: BTreeMap::from([(
                "Sales.Order".to_string(),
                vec![EntityRule {
                    module_roles: BTreeSet::from(["Sales.User".to_string()]),
                    default_member_right: Some(MemberRight::Read),
                    member_rights: BTreeMap::from([("Status".to_string(), MemberRight::Write)]),
                    ..Default::default()
                }],
            )]),
            ..Default::default()
        };
        let context = SecurityContext {
            module_roles: BTreeSet::from(["Sales.User".to_string()]),
            ..Default::default()
        };
        assert!(policy.entity_allowed(
            "Sales.Order",
            EntityAction::Write,
            Some("Status"),
            None,
            &context
        ));
        assert!(!policy.entity_allowed(
            "Sales.Order",
            EntityAction::Write,
            Some("Number"),
            None,
            &context
        ));
        assert!(policy.entity_allowed("Sales.Order", EntityAction::Write, None, None, &context));
    }

    #[test]
    fn runtime_invocation_is_authorized_and_transactional() {
        let mut runtime = Runtime::new(Store::new(schema()), SecurityPolicy::default());
        runtime.register_action(
            "Sales.ACT_Create",
            |store: &mut Store, _arguments: &Value| {
                let order = store.create("Sales.Order")?;
                store.commit("Sales.Order", &order.id)?;
                Ok(json!({ "id": order.id }))
            },
        );
        let result = runtime
            .invoke(
                "Sales.ACT_Create",
                &Value::Null,
                &SecurityContext::default(),
            )
            .unwrap();
        assert!(result["id"].is_string());
        assert_eq!(runtime.store().retrieve("Sales.Order").unwrap().len(), 1);
    }
    /// `AllowedRoles` on a published service is a list of *module* roles, and
    /// a caller usually names only user roles — so the question has to expand
    /// the context first. An empty list allows nobody.
    #[test]
    fn holds_any_module_role_expands_user_roles_and_never_allows_an_empty_list() {
        let policy = SecurityPolicy {
            enabled: true,
            administrator_roles: BTreeSet::from(["Administrator".to_string()]),
            user_role_modules: BTreeMap::from([(
                "User".to_string(),
                BTreeSet::from(["Sales.User".to_string()]),
            )]),
            ..SecurityPolicy::default()
        };
        let user = SecurityContext {
            user_roles: BTreeSet::from(["User".to_string()]),
            ..SecurityContext::default()
        };

        assert!(policy.holds_any_module_role(&user, &["Sales.User"]));
        assert!(policy.holds_any_module_role(&user, &["Sales.Admin", "Sales.User"]));
        assert!(!policy.holds_any_module_role(&user, &["Sales.Admin"]));
        assert!(!policy.holds_any_module_role(&user, &[]));
        // Anonymous holds nothing, including on an empty list.
        assert!(!policy.holds_any_module_role(&SecurityContext::default(), &["Sales.User"]));
        assert!(!policy.holds_any_module_role(&SecurityContext::default(), &[]));
        // An administrator reaches every service, the same way it reaches
        // every entity.
        let administrator = SecurityContext {
            user_roles: BTreeSet::from(["Administrator".to_string()]),
            ..SecurityContext::default()
        };
        assert!(policy.holds_any_module_role(&administrator, &["Sales.Admin"]));
        assert!(policy.holds_any_module_role(&administrator, &[]));
    }
}

#[cfg(test)]
mod store_tests;

#[cfg(test)]
mod security_tests;
