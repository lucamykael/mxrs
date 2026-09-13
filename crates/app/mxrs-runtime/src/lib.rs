//! Runtime-neutral application execution core.
//!
//! The store and authorization engine are intentionally independent of HTTP,
//! SQLite, or Mendix's Java runtime. This makes transaction and security
//! semantics directly testable and lets later adapters share one fail-closed
//! implementation.

use std::collections::{BTreeMap, BTreeSet};

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
}

pub type Result<T> = std::result::Result<T, RuntimeError>;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ObjectValue {
    pub entity: String,
    pub id: String,
    pub members: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, Default)]
pub struct StoreSchema {
    entities: BTreeMap<String, EntitySchema>,
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
        self.entities.insert(
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
#[derive(Debug, Clone)]
pub struct Store {
    schema: StoreSchema,
    records: BTreeMap<String, BTreeMap<String, ObjectValue>>,
    committed: BTreeMap<(String, String), BTreeMap<String, Value>>,
}

impl Store {
    pub fn new(schema: StoreSchema) -> Self {
        Self {
            schema,
            records: BTreeMap::new(),
            committed: BTreeMap::new(),
        }
    }

    pub fn create(&mut self, entity: &str) -> Result<ObjectValue> {
        let definition = self
            .schema
            .entities
            .get(entity)
            .ok_or_else(|| RuntimeError::UnknownEntity(entity.to_string()))?;
        let value = ObjectValue {
            entity: entity.to_string(),
            id: uuid::Uuid::new_v4().to_string(),
            members: definition.defaults.clone(),
        };
        self.records
            .entry(entity.to_string())
            .or_default()
            .insert(value.id.clone(), value.clone());
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
            .flat_map(BTreeMap::values)
            .cloned()
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
            .cloned())
    }

    pub fn set_member(&mut self, entity: &str, id: &str, member: &str, value: Value) -> Result<()> {
        self.object_mut(entity, id)?
            .members
            .insert(member.to_string(), value);
        Ok(())
    }

    pub fn commit(&mut self, entity: &str, id: &str) -> Result<ObjectValue> {
        let object = self.object(entity, id)?.clone();
        if !self.is_transient(entity)? {
            self.committed
                .insert((entity.to_string(), id.to_string()), object.members.clone());
        }
        Ok(object)
    }

    pub fn rollback(&mut self, entity: &str, id: &str) -> Result<()> {
        let key = (entity.to_string(), id.to_string());
        if let Some(members) = self.committed.get(&key).cloned() {
            self.object_mut(entity, id)?.members = members;
        } else {
            self.records
                .entry(entity.to_string())
                .or_default()
                .remove(id);
        }
        Ok(())
    }

    pub fn delete(&mut self, entity: &str, id: &str) -> Result<ObjectValue> {
        let object = self
            .records
            .get_mut(entity)
            .and_then(|records| records.remove(id))
            .ok_or_else(|| RuntimeError::UnknownObject {
                entity: entity.to_string(),
                id: id.to_string(),
            })?;
        self.committed.remove(&(entity.to_string(), id.to_string()));
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
            .flat_map(BTreeMap::values)
            .filter(|candidate| {
                ids.contains(candidate.id.as_str())
                    || candidate
                        .members
                        .get(association)
                        .or_else(|| candidate.members.get(short))
                        .is_some_and(|value| reference_ids(value).contains(&start.id.as_str()))
            })
            .cloned()
            .collect::<Vec<_>>();
        related.sort_by(|left, right| left.id.cmp(&right.id));
        related.dedup_by(|left, right| left.id == right.id);
        related
    }

    pub fn transaction<T>(&mut self, operation: impl FnOnce(&mut Self) -> Result<T>) -> Result<T> {
        let snapshot = self.clone();
        match operation(self) {
            Ok(result) => {
                self.discard_uncommitted();
                Ok(result)
            }
            Err(error) => {
                *self = snapshot;
                Err(error)
            }
        }
    }

    fn discard_uncommitted(&mut self) {
        let transient = self
            .records
            .iter()
            .filter(|(entity, _)| self.is_transient(entity).unwrap_or(false))
            .map(|(entity, records)| (entity.clone(), records.clone()))
            .collect::<BTreeMap<_, _>>();
        self.records = transient;
        for ((entity, id), members) in &self.committed {
            self.records.entry(entity.clone()).or_default().insert(
                id.clone(),
                ObjectValue {
                    entity: entity.clone(),
                    id: id.clone(),
                    members: members.clone(),
                },
            );
        }
    }

    fn object(&self, entity: &str, id: &str) -> Result<&ObjectValue> {
        self.records
            .get(entity)
            .and_then(|records| records.get(id))
            .ok_or_else(|| RuntimeError::UnknownObject {
                entity: entity.to_string(),
                id: id.to_string(),
            })
    }

    fn object_mut(&mut self, entity: &str, id: &str) -> Result<&mut ObjectValue> {
        self.records
            .get_mut(entity)
            .and_then(|records| records.get_mut(id))
            .ok_or_else(|| RuntimeError::UnknownObject {
                entity: entity.to_string(),
                id: id.to_string(),
            })
    }

    fn is_transient(&self, entity: &str) -> Result<bool> {
        self.schema
            .entities
            .get(entity)
            .map(|definition| definition.transient)
            .ok_or_else(|| RuntimeError::UnknownEntity(entity.to_string()))
    }
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

    pub fn entity_allowed(
        &self,
        entity: &str,
        action: EntityAction,
        member: Option<&str>,
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
            &context
        ));
        assert!(!policy.entity_allowed(
            "Sales.Order",
            EntityAction::Write,
            Some("Number"),
            &context
        ));
        assert!(policy.entity_allowed("Sales.Order", EntityAction::Write, None, &context));
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
}
