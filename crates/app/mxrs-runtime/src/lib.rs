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

use std::collections::{BTreeMap, BTreeSet, VecDeque};
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
    /// What a save was refused for: the entity's validation rules the
    /// object broke, or the feedback a before-commit flow gave as it
    /// rejected it.
    #[error("validation failed: {}", violations_text(.0))]
    Validation(Vec<Violation>),
}

/// One thing wrong with an object a page saves: the member and the message
/// the model states for it, and the object it is about when a flow
/// committed another one than the page's.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Violation {
    pub member: String,
    pub message: String,
    /// The id of the object the violation is about; `None` when it is the
    /// object the request saves.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub object: Option<String>,
}

fn violations_text(violations: &[Violation]) -> String {
    violations
        .iter()
        .map(|violation| violation.message.as_str())
        .collect::<Vec<_>>()
        .join("; ")
}

/// A validation rule of an entity, as the model states it on an attribute.
#[derive(Debug, Clone, PartialEq)]
pub struct ValidationRule {
    pub member: String,
    pub kind: RuleKind,
    /// The model's message, or empty for the rule's own words.
    pub message: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum RuleKind {
    Required,
    Unique,
    MaxLength(usize),
    Range {
        minimum: Option<Bound>,
        maximum: Option<Bound>,
    },
    /// A regular expression, as the model's `RegularExpressions` document
    /// states it.
    Pattern(String),
    /// Equal to a value, or to another member of the object.
    EqualsTo(Bound),
}

/// What a rule compares a member with: a value the model states, or
/// another member of the same object.
#[derive(Debug, Clone, PartialEq)]
pub enum Bound {
    Value(String),
    Member(String),
}

impl Bound {
    fn of(&self, record: &BTreeMap<String, Value>) -> Option<Value> {
        match self {
            Bound::Value(value) => Some(Value::String(value.clone())),
            Bound::Member(member) => record.get(member).cloned(),
        }
    }
}

fn number_of(value: &Value) -> Option<f64> {
    match value {
        Value::Number(number) => number.as_f64(),
        Value::String(text) => text
            .strip_prefix(DATETIME_MEMBER_PREFIX)
            .unwrap_or(text)
            .trim()
            .parse()
            .ok(),
        _ => None,
    }
}

impl std::fmt::Display for Bound {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Bound::Value(value) => formatter.write_str(value),
            Bound::Member(member) => formatter.write_str(member),
        }
    }
}

impl ValidationRule {
    /// What the rule says when it is broken: the model's message, or its own.
    fn message(&self) -> String {
        if !self.message.trim().is_empty() {
            return self.message.clone();
        }
        let member = &self.member;
        match &self.kind {
            RuleKind::Required => format!("{member} is required"),
            RuleKind::Unique => format!("{member} must be unique"),
            RuleKind::MaxLength(length) => format!("{member} may hold at most {length} characters"),
            RuleKind::Range { minimum, maximum } => match (minimum, maximum) {
                (Some(low), Some(high)) => format!("{member} must be between {low} and {high}"),
                (Some(low), None) => format!("{member} must be at least {low}"),
                (None, Some(high)) => format!("{member} must be at most {high}"),
                (None, None) => format!("{member} is out of range"),
            },
            RuleKind::Pattern(_) => format!("{member} is not in the required format"),
            RuleKind::EqualsTo(bound) => format!("{member} must be equal to {bound}"),
        }
    }

    /// Whether the object breaks the rule; `others` are the values the
    /// other objects of the entity hold in the member, for uniqueness, and
    /// `numeric` whether the member's attribute holds a number.
    fn broken(&self, record: &BTreeMap<String, Value>, others: &[&Value], numeric: bool) -> bool {
        let value = record.get(&self.member);
        let empty = value.is_none_or(|value| match value {
            Value::Null => true,
            Value::String(text) => text.is_empty(),
            _ => false,
        });
        match &self.kind {
            RuleKind::Required => empty,
            // As the database's unique index compares: a number by its value
            // (5 is 5.0), text exactly as it is written.
            RuleKind::Unique => {
                !empty
                    && value.is_some_and(|value| {
                        others.iter().any(|other| {
                            *other == value
                                || numeric
                                    && number_of(value)
                                        .zip(number_of(other))
                                        .is_some_and(|(left, right)| left == right)
                        })
                    })
            }
            RuleKind::MaxLength(length) => value
                .and_then(Value::as_str)
                .is_some_and(|text| text.chars().count() > *length),
            RuleKind::Range { minimum, maximum } => {
                let Some(number) = value.and_then(number_of) else {
                    return false;
                };
                let bound = |bound: &Option<Bound>| {
                    bound
                        .as_ref()
                        .and_then(|bound| bound.of(record))
                        .and_then(|value| number_of(&value))
                };
                bound(minimum).is_some_and(|low| number < low)
                    || bound(maximum).is_some_and(|high| number > high)
            }
            RuleKind::EqualsTo(bound) => {
                let Some(expected) = bound.of(record) else {
                    return !empty;
                };
                // A number by its value; text, even text of digits, as written.
                let numbers = numeric
                    .then(|| value.and_then(number_of).zip(number_of(&expected)))
                    .flatten();
                match numbers {
                    Some((left, right)) => left != right,
                    _ => {
                        let text = |value: &Value| match value {
                            Value::String(text) => text.clone(),
                            Value::Null => String::new(),
                            other => other.to_string(),
                        };
                        value.map(text).unwrap_or_default() != text(&expected)
                    }
                }
            }
            RuleKind::Pattern(pattern) => value.and_then(Value::as_str).is_some_and(|text| {
                !text.is_empty()
                    && regex::Regex::new(pattern).is_ok_and(|pattern| !pattern.is_match(text))
            }),
        }
    }
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
    /// What each attribute holds, where the model says.
    kinds: BTreeMap<String, MemberKind>,
    /// What the model asks of an object before it is committed.
    rules: Vec<ValidationRule>,
    /// The entity this one specializes, when the project declares it.
    generalization: Option<String>,
}

/// Whether the runtime reads a regular expression: the model's are Java's,
/// and what this one does not have (look-around, back-references) is said.
pub fn pattern_reads(expression: &str) -> std::result::Result<(), String> {
    regex::Regex::new(expression).map(|_| ()).map_err(|error| {
        error
            .to_string()
            .lines()
            .last()
            .unwrap_or_default()
            .trim()
            .to_string()
    })
}

/// What an attribute holds, as far as a value written from outside a flow
/// has to be checked against it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MemberKind {
    Text {
        length: Option<usize>,
    },
    Boolean,
    Integer,
    Decimal,
    DateTime,
    /// An enumeration, a binary, a hashed string: text this runtime does
    /// not look into.
    Other,
}

impl MemberKind {
    fn numeric(&self) -> bool {
        matches!(self, MemberKind::Integer | MemberKind::Decimal)
    }
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
                kinds: BTreeMap::new(),
                rules: Vec::new(),
                generalization: None,
            },
        );
        self
    }

    /// The validation rules of an entity, checked at a page's save.
    pub fn rules(mut self, entity: &str, rules: Vec<ValidationRule>) -> Self {
        if let Some(schema) = Arc::make_mut(&mut self.entities).get_mut(entity) {
            schema.rules = rules;
        }
        self
    }

    pub fn rules_of(&self, entity: &str) -> &[ValidationRule] {
        self.entities
            .get(entity)
            .map_or(&[], |schema| schema.rules.as_slice())
    }

    /// States what the attributes of an entity already declared hold.
    pub fn members(mut self, entity: &str, kinds: BTreeMap<String, MemberKind>) -> Self {
        if let Some(schema) = Arc::make_mut(&mut self.entities).get_mut(entity) {
            schema.kinds = kinds;
        }
        self
    }

    pub fn contains(&self, entity: &str) -> bool {
        self.entities.contains_key(entity)
    }

    /// Declares that `entity` specializes `generalization`: what is asked
    /// of the generalization — a retrieve, a find — is asked of it too.
    pub fn generalization(mut self, entity: &str, generalization: &str) -> Self {
        if let Some(schema) = Arc::make_mut(&mut self.entities).get_mut(entity) {
            schema.generalization = Some(generalization.to_string());
        }
        self
    }

    /// The entity `entity` specializes, if any.
    pub fn generalization_of(&self, entity: &str) -> Option<&str> {
        self.entities
            .get(entity)
            .and_then(|schema| schema.generalization.as_deref())
    }

    /// Whether an object of `entity` is one of `ancestor`: the entity
    /// itself or one of its specializations.
    pub fn is_a(&self, entity: &str, ancestor: &str) -> bool {
        let mut current = Some(entity);
        for _ in 0..64 {
            match current {
                Some(name) if name == ancestor => return true,
                Some(name) => current = self.generalization_of(name),
                None => return false,
            }
        }
        false
    }

    /// `entity` and every entity that specializes it, the entity first.
    pub fn family(&self, entity: &str) -> Vec<String> {
        let mut family = vec![entity.to_string()];
        family.extend(
            self.entities
                .keys()
                .filter(|name| name.as_str() != entity && self.is_a(name, entity))
                .cloned(),
        );
        family
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

    /// Creates the object a form was shown before it existed, under the
    /// identity the form knows it by — so saving it twice is saving one
    /// object.
    pub fn create_as(&mut self, entity: &str, id: &str) -> Result<ObjectValue> {
        if uuid::Uuid::parse_str(id).is_err() {
            return Err(RuntimeError::Transaction(
                "a new object is known by a UUID".into(),
            ));
        }
        let mut object = self.create(entity)?;
        self.rollback(entity, &object.id)?;
        object.id = id.to_string();
        put_record(&mut self.records, Arc::new(object.clone()));
        self.mark_dirty(entity, id);
        Ok(object)
    }

    /// The objects of `entity` the unit of work holds, its
    /// specializations' among them, as a Mendix retrieve answers.
    pub fn retrieve(&self, entity: &str) -> Result<Vec<ObjectValue>> {
        if !self.schema.contains(entity) {
            return Err(RuntimeError::UnknownEntity(entity.to_string()));
        }
        Ok(self
            .schema
            .family(entity)
            .iter()
            .filter_map(|member| self.records.get(member))
            .flat_map(|records| records.values())
            .map(|object| object.as_ref().clone())
            .collect())
    }

    /// The objects of an entity as last committed, in no unit of work's
    /// state.
    pub fn retrieve_committed(&self, entity: &str) -> Vec<ObjectValue> {
        self.schema
            .family(entity)
            .iter()
            .filter_map(|member| self.committed.get(member))
            .flat_map(|records| records.values())
            .map(|object| object.as_ref().clone())
            .collect()
    }

    /// The members of an object that differ from what was last committed
    /// of it — every member of one never committed.
    pub fn uncommitted_members(&self, entity: &str, id: &str) -> BTreeMap<String, Value> {
        let entity = &self.concrete(entity, id);
        let Some(current) = self.records.get(entity).and_then(|records| records.get(id)) else {
            return BTreeMap::new();
        };
        let committed = self
            .committed
            .get(entity)
            .and_then(|records| records.get(id));
        current
            .members
            .iter()
            .filter(|(member, value)| {
                committed.is_none_or(|committed| committed.members.get(*member) != Some(*value))
            })
            .map(|(member, value)| (member.clone(), value.clone()))
            .collect()
    }

    /// Whether an object holds what was not committed: new, or changed
    /// since its last commit.
    pub fn is_dirty(&self, entity: &str, id: &str) -> bool {
        let entity = &self.concrete(entity, id);
        self.dirty.contains(&(entity.to_string(), id.to_string()))
    }

    /// The object of `entity` with `id` — of the entity itself or one of
    /// its specializations, which keeps its own entity.
    pub fn find(&self, entity: &str, id: &str) -> Result<Option<ObjectValue>> {
        if !self.schema.contains(entity) {
            return Err(RuntimeError::UnknownEntity(entity.to_string()));
        }
        if let Some(object) = self.records.get(entity).and_then(|records| records.get(id)) {
            return Ok(Some(object.as_ref().clone()));
        }
        Ok(self
            .schema
            .family(entity)
            .iter()
            .skip(1)
            .find_map(|member| {
                self.records
                    .get(member)
                    .and_then(|records| records.get(id))
                    .map(|object| object.as_ref().clone())
            }))
    }

    /// The entity of the object with `id` that `entity` names: `entity`
    /// itself, or the specialization of it the object is.
    fn concrete(&self, entity: &str, id: &str) -> String {
        let holds = |member: &String| {
            [&self.records, &self.committed].iter().any(|map| {
                map.get(member)
                    .is_some_and(|records| records.contains_key(id))
            })
        };
        if self
            .records
            .get(entity)
            .is_some_and(|records| records.contains_key(id))
        {
            return entity.to_string();
        }
        self.schema
            .family(entity)
            .into_iter()
            .find(holds)
            .unwrap_or_else(|| entity.to_string())
    }

    pub fn set_member(&mut self, entity: &str, id: &str, member: &str, value: Value) -> Result<()> {
        let entity = &self.concrete(entity, id);
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
        let entity = &self.concrete(entity, id);
        let object = self.object(entity, id)?.clone();
        put_record(&mut self.committed, object.clone());
        Arc::make_mut(&mut self.dirty).remove(&(entity.to_string(), id.to_string()));
        Ok(object.as_ref().clone())
    }

    pub fn rollback(&mut self, entity: &str, id: &str) -> Result<()> {
        let entity = &self.concrete(entity, id);
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
        let entity = &self.concrete(entity, id);
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
    pub fn schema(&self) -> &StoreSchema {
        &self.schema
    }

    pub fn is_committed(&self, entity: &str, id: &str) -> bool {
        let entity = &self.concrete(entity, id);
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

/// What the runtime runs by name, as the user who asked for it: a flow's
/// own retrieves and commits are held to that user's rights.
pub trait Action: Send + Sync {
    fn execute(
        &self,
        store: &mut Store,
        arguments: &Value,
        context: &SecurityContext,
    ) -> Result<Value>;
}

/// A closure is an action that has no use for who runs it.
impl<F> Action for F
where
    F: Fn(&mut Store, &Value) -> Result<Value> + Send + Sync,
{
    fn execute(&self, store: &mut Store, arguments: &Value, _: &SecurityContext) -> Result<Value> {
        self(store, arguments)
    }
}

/// How an object a page saves is committed when the entity's event
/// handlers are to run: a flow engine commits it between its before- and
/// after-handlers and answers the effects those flows asked for. Without
/// one, the store commits the object as it is.
pub trait Lifecycle: Send + Sync {
    fn commit(
        &self,
        store: &mut Store,
        entity: &str,
        id: &str,
        context: &SecurityContext,
    ) -> Result<Vec<Value>>;
}

pub struct Runtime {
    store: Store,
    security: SecurityPolicy,
    actions: BTreeMap<String, Box<dyn Action>>,
    lifecycle: Option<Box<dyn Lifecycle>>,
    held: HeldObjects,
}

/// What a flow made of an object it handed a page without committing it:
/// the members it set, and whether the object is new.
#[derive(Debug, Clone, PartialEq)]
struct Held {
    user: Option<String>,
    members: BTreeMap<String, Value>,
    new: bool,
}

/// The objects flows handed pages uncommitted, kept for the user each was
/// handed to — as the Mendix client keeps an object a microflow made — until
/// a page saves or deletes it. A unit of work ends with its flow, so without
/// them what the flow set would be gone by the time the page saves.
#[derive(Debug, Default)]
struct HeldObjects {
    objects: BTreeMap<(String, String), Held>,
    /// Oldest first, so the runtime keeps a bounded number.
    order: VecDeque<(String, String)>,
}

/// How many objects handed over uncommitted the runtime keeps at once.
const HELD_LIMIT: usize = 4096;

impl HeldObjects {
    /// What the caller was handed of an object, if it was handed one.
    fn get(&self, entity: &str, id: &str, context: &SecurityContext) -> Option<&Held> {
        self.objects
            .get(&(entity.to_string(), id.to_string()))
            .filter(|held| held.user == context.user)
    }

    fn keep(&mut self, entity: &str, id: &str, held: Held) {
        let key = (entity.to_string(), id.to_string());
        self.order.retain(|kept| kept != &key);
        self.order.push_back(key.clone());
        self.objects.insert(key, held);
        while self.order.len() > HELD_LIMIT {
            if let Some(oldest) = self.order.pop_front() {
                self.objects.remove(&oldest);
            }
        }
    }

    fn release(&mut self, entity: &str, id: &str) {
        let key = (entity.to_string(), id.to_string());
        self.order.retain(|kept| kept != &key);
        self.objects.remove(&key);
    }
}

/// Every object (`{entity, id}`) a flow's answer names.
fn named_objects(value: &Value, found: &mut BTreeSet<(String, String)>) {
    match value {
        Value::Object(map) => {
            if let (Some(entity), Some(id)) = (
                map.get("entity").and_then(Value::as_str),
                map.get("id").and_then(Value::as_str),
            ) {
                found.insert((entity.to_string(), id.to_string()));
            }
            map.values().for_each(|value| named_objects(value, found));
        }
        Value::Array(values) => values.iter().for_each(|value| named_objects(value, found)),
        _ => {}
    }
}

/// Says of each object an answer names that is new to the store, that it
/// is: the page saves it as the new object it is.
fn mark_new(value: &mut Value, new: &BTreeSet<(String, String)>) {
    match value {
        Value::Object(map) => {
            let named = map
                .get("entity")
                .and_then(Value::as_str)
                .zip(map.get("id").and_then(Value::as_str))
                .map(|(entity, id)| (entity.to_string(), id.to_string()));
            if named.is_some_and(|named| new.contains(&named)) && map.contains_key("members") {
                map.insert("new".to_string(), Value::Bool(true));
            }
            map.values_mut().for_each(|value| mark_new(value, new));
        }
        Value::Array(values) => values.iter_mut().for_each(|value| mark_new(value, new)),
        _ => {}
    }
}

impl Runtime {
    pub fn new(store: Store, security: SecurityPolicy) -> Self {
        Self {
            store,
            security,
            actions: BTreeMap::new(),
            lifecycle: None,
            held: HeldObjects::default(),
        }
    }

    /// Commits what a page saves through the entity's event handlers.
    pub fn set_lifecycle(&mut self, lifecycle: impl Lifecycle + 'static) {
        self.lifecycle = Some(Box::new(lifecycle));
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
        // An object a page gives a flow is one its user may read, with the
        // changes its form holds and its user may make — over what a flow
        // made of it before, when one handed it to the page.
        let given = given_objects(&self.store, &self.security, &self.held, arguments, context)?;
        let (mut answer, handed) = self.store.transaction(|store| {
            for object in &given {
                if object.create {
                    store.create_as(&object.entity, &object.id)?;
                }
                for (member, value) in object.restored.iter().chain(&object.values) {
                    store.set_member(&object.entity, &object.id, member, value.clone())?;
                }
            }
            let answer = action.execute(store, arguments, context)?;
            // What the page holds once the flow has run, uncommitted: the
            // objects it was given and those the flow answers or opens a
            // page with.
            let mut named = BTreeSet::new();
            named_objects(&answer, &mut named);
            named.extend(
                given
                    .iter()
                    .map(|object| (object.entity.clone(), object.id.clone())),
            );
            let handed: Vec<((String, String), Option<Held>)> = named
                .into_iter()
                .filter(|(entity, _)| store.schema().contains(entity))
                .map(|(entity, id)| {
                    let held = (store.find(&entity, &id)?.is_some()
                        && store.is_dirty(&entity, &id))
                    .then(|| Held {
                        user: context.user.clone(),
                        members: store.uncommitted_members(&entity, &id),
                        new: !store.is_committed(&entity, &id),
                    });
                    Ok(((entity, id), held))
                })
                .collect::<Result<_>>()?;
            Ok((answer, handed))
        })?;
        let mut new = BTreeSet::new();
        for ((entity, id), held) in handed {
            match held {
                Some(held) => {
                    if held.new {
                        new.insert((entity.clone(), id.clone()));
                    }
                    self.held.keep(&entity, &id, held);
                }
                None => self.held.release(&entity, &id),
            }
        }
        mark_new(&mut answer, &new);
        // What comes back to the page is what its user may read.
        Ok(readable(&self.store, &self.security, answer, context))
    }

    /// What a page does with the objects it shows, by the operation's name:
    /// `retrieve` the objects of an entity the caller may read, `create` a
    /// blank one for a form, `save` the members a form changed and commit
    /// them, or `delete` one. Each is checked against the entity's access
    /// rules member by member, and a value against what its attribute
    /// holds. A save is checked against the entity's validation rules, and
    /// committed through its event handlers when the runtime has a
    /// lifecycle to run them.
    pub fn data(
        &mut self,
        operation: &str,
        arguments: &Value,
        context: &SecurityContext,
    ) -> Result<Value> {
        let text = |key: &str| {
            arguments
                .get(key)
                .and_then(Value::as_str)
                .ok_or_else(|| RuntimeError::Transaction(format!("{operation} needs `{key}`")))
        };
        let entity = text("entity")?;
        let schema = self
            .store
            .schema
            .entities
            .get(entity)
            .cloned()
            .ok_or_else(|| RuntimeError::UnknownEntity(entity.to_string()))?;
        let security = &self.security;
        let refused = |action: &str, member: Option<&str>| RuntimeError::NotAuthorized {
            action: action.to_string(),
            resource: match member {
                Some(member) => format!("{entity}.{member}"),
                None => entity.to_string(),
            },
        };
        // An object as its caller may see it: the members it may read.
        let seen = |object: &ObjectValue| {
            let mut visible = object.clone();
            visible.members.retain(|member, _| {
                security.entity_allowed(
                    entity,
                    EntityAction::Read,
                    Some(member),
                    Some(&object.members),
                    context,
                )
            });
            shown(&visible)
        };
        match operation {
            "retrieve" => {
                if !security.entity_allowed(entity, EntityAction::Read, None, None, context) {
                    return Err(refused("read", None));
                }
                // As the page's source says: within its XPath constraint,
                // in its sort order, and so many from where it is.
                let constraint = arguments
                    .get("constraint")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|constraint| !constraint.is_empty());
                if let Some(constraint) = constraint
                    && !crate::xpath::is_list_supported(constraint)
                {
                    return Err(RuntimeError::Transaction(format!(
                        "unsupported XPath constraint {constraint:?}: the runtime reads comparisons of the entity's own members with a literal or the current user, joined by and/or/not — not an association path, a page's variable or a token such as [%CurrentDateTime%]"
                    )));
                }
                // The constraint and the order see what the caller may read
                // of each object, so neither tells of a member it may not.
                let visible = |object: &ObjectValue| -> BTreeMap<String, Value> {
                    object
                        .members
                        .iter()
                        .filter(|(member, _)| {
                            security.entity_allowed(
                                entity,
                                EntityAction::Read,
                                Some(member),
                                Some(&object.members),
                                context,
                            )
                        })
                        .map(|(member, value)| (member.clone(), value.clone()))
                        .collect()
                };
                let mut objects: Vec<(ObjectValue, BTreeMap<String, Value>)> = self
                    .store
                    .retrieve(entity)?
                    .into_iter()
                    .filter(|object| {
                        security.entity_allowed(
                            entity,
                            EntityAction::Read,
                            None,
                            Some(&object.members),
                            context,
                        )
                    })
                    .map(|object| {
                        let seen = visible(&object);
                        (object, seen)
                    })
                    .filter(|(_, seen)| {
                        constraint.is_none_or(|constraint| {
                            crate::xpath::evaluate(constraint, seen, context) == Some(true)
                        })
                    })
                    .collect();
                let sort: Vec<(String, bool)> = arguments
                    .get("sort")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(|item| {
                        Some((
                            item.get("attribute")?.as_str()?.to_string(),
                            item.get("descending")
                                .and_then(Value::as_bool)
                                .unwrap_or(false),
                        ))
                    })
                    .collect();
                if let Some((path, _)) = sort.iter().find(|(attribute, _)| attribute.contains('/'))
                {
                    return Err(RuntimeError::Transaction(format!(
                        "unsupported sort by {path:?}: the runtime sorts by the entity's own members, not along an association"
                    )));
                }
                if !sort.is_empty() {
                    objects.sort_by(|(_, left), (_, right)| {
                        for (attribute, descending) in &sort {
                            let order = compare_members(left.get(attribute), right.get(attribute));
                            if order != std::cmp::Ordering::Equal {
                                return if *descending { order.reverse() } else { order };
                            }
                        }
                        std::cmp::Ordering::Equal
                    });
                }
                let total = objects.len();
                let offset = arguments
                    .get("offset")
                    .and_then(Value::as_u64)
                    .map_or(0, |offset| offset as usize);
                let limit = arguments
                    .get("limit")
                    .and_then(Value::as_u64)
                    .map(|limit| limit as usize);
                let objects: Vec<Value> = objects
                    .iter()
                    .skip(offset)
                    .take(limit.unwrap_or(usize::MAX))
                    .map(|(object, _)| seen(object))
                    .collect();
                Ok(serde_json::json!({ "objects": objects, "total": total }))
            }
            "create" => {
                if !security.entity_allowed(entity, EntityAction::Create, None, None, context) {
                    return Err(refused("create", None));
                }
                // Shown to the form and not kept: the object exists once it
                // is saved, as the new one the form says it is.
                self.store.transaction(|store| {
                    let object = store.create(entity)?;
                    store.rollback(entity, &object.id)?;
                    let mut blank = seen(&object);
                    blank["new"] = Value::Bool(true);
                    Ok(blank)
                })
            }
            "save" => {
                let id = text("id")?;
                let new = arguments.get("new").and_then(Value::as_bool) == Some(true);
                let members = arguments
                    .get("members")
                    .and_then(Value::as_object)
                    .ok_or_else(|| RuntimeError::Transaction("save needs `members`".into()))?;
                let existing = self.store.find(entity, id)?;
                // What a flow made of the object before handing it to the
                // page is saved with it, and what the user changed over it.
                let held = self.held.get(entity, id, context).cloned();
                let record = match (&existing, &held, new) {
                    (Some(object), held, _) => {
                        let mut record = object.members.clone();
                        record.extend(held.iter().flat_map(|held| held.members.clone()));
                        record
                    }
                    (None, Some(held), _) if held.new => {
                        let mut record = schema.defaults.clone();
                        record.extend(held.members.clone());
                        record
                    }
                    (None, _, true) => {
                        if !security.entity_allowed(
                            entity,
                            EntityAction::Create,
                            None,
                            None,
                            context,
                        ) {
                            return Err(refused("create", None));
                        }
                        schema.defaults.clone()
                    }
                    // Not a new object, and gone: saving it would bring back
                    // what someone deleted.
                    (None, _, false) => {
                        return Err(RuntimeError::UnknownObject {
                            entity: entity.to_string(),
                            id: id.to_string(),
                        });
                    }
                };
                let values = written(security, &schema, entity, &record, members, context)?;
                let was_held = held.is_some();
                let restored = held.map(|held| held.members).unwrap_or_default();
                let lifecycle = self.lifecycle.as_deref();
                let (saved, effects) = self.store.transaction(|store| {
                    let id = match existing {
                        Some(object) => object.id,
                        None => store.create_as(entity, id)?.id,
                    };
                    for (member, value) in restored.into_iter().chain(values) {
                        store.set_member(entity, &id, &member, value)?;
                    }
                    // The lifecycle validates between the entity's
                    // before-handlers and the commit, as a flow's commit does.
                    let effects = match lifecycle {
                        Some(lifecycle) => lifecycle.commit(store, entity, &id, context)?,
                        None => {
                            validate_object(store, entity, &id)?;
                            store.commit(entity, &id)?;
                            Vec::new()
                        }
                    };
                    let saved =
                        store
                            .find(entity, &id)?
                            .ok_or_else(|| RuntimeError::UnknownObject {
                                entity: entity.to_string(),
                                id: id.clone(),
                            })?;
                    Ok((saved, effects))
                })?;
                // Saved, what the flow made of it is the store's; what it
                // made for another user stays theirs.
                if was_held {
                    self.held.release(entity, &saved.id);
                }
                let mut answer = seen(&saved);
                if !effects.is_empty() {
                    answer["effects"] = Value::Array(effects);
                }
                Ok(answer)
            }
            "delete" => {
                let id = text("id")?;
                let record = self.store.find(entity, id)?.map(|object| object.members);
                if !security.entity_allowed(
                    entity,
                    EntityAction::Delete,
                    None,
                    record.as_ref(),
                    context,
                ) {
                    return Err(refused("delete", None));
                }
                let deleted = self.store.transaction(|store| store.delete(entity, id))?;
                self.held.release(entity, id);
                Ok(serde_json::json!({ "entity": deleted.entity, "id": deleted.id }))
            }
            other => Err(RuntimeError::UnknownAction(format!("data/{other}"))),
        }
    }

    pub fn store(&self) -> &Store {
        &self.store
    }

    pub fn store_mut(&mut self) -> &mut Store {
        &mut self.store
    }
}

/// The object as it stands in the store, against its entity's validation
/// rules: every rule broken is answered at once, as the Mendix client
/// shows them all under their inputs. A page's save and a flow's commit
/// both ask this before the object is committed.
pub fn validate_object(store: &Store, entity: &str, id: &str) -> Result<()> {
    let entity = &store.concrete(entity, id);
    let rules = store.schema().rules_of(entity);
    if rules.is_empty() {
        return Ok(());
    }
    let Some(object) = store.find(entity, id)? else {
        return Ok(());
    };
    // Unique is the database's index: what other objects hold once
    // committed, not what a unit of work has yet to commit — over the
    // generalization's whole family, which shares the index.
    let others: Vec<ObjectValue> = if rules.iter().any(|rule| rule.kind == RuleKind::Unique) {
        let mut root: &str = entity;
        for _ in 0..64 {
            match store.schema().generalization_of(root) {
                Some(parent) => root = parent,
                None => break,
            }
        }
        store
            .retrieve_committed(root)
            .into_iter()
            .filter(|other| other.id != id)
            .collect()
    } else {
        Vec::new()
    };
    let kinds = store
        .schema()
        .entities
        .get(entity)
        .map(|schema| &schema.kinds);
    let violations: Vec<Violation> = rules
        .iter()
        .filter(|rule| {
            let held: Vec<&Value> = others
                .iter()
                .filter_map(|other| other.members.get(&rule.member))
                .collect();
            let numeric = kinds
                .and_then(|kinds| kinds.get(&rule.member))
                .is_some_and(MemberKind::numeric);
            rule.broken(&object.members, &held, numeric)
        })
        .map(|rule| Violation {
            member: rule.member.clone(),
            message: rule.message(),
            object: Some(id.to_string()),
        })
        .collect();
    if violations.is_empty() {
        Ok(())
    } else {
        Err(RuntimeError::Validation(violations))
    }
}

/// What a page writes of an object, member by member: each one the entity
/// has, the caller may write, and holds a value of its attribute's kind —
/// one that does not is refused as that member's violation.
fn written(
    security: &SecurityPolicy,
    schema: &EntitySchema,
    entity: &str,
    record: &BTreeMap<String, Value>,
    members: &serde_json::Map<String, Value>,
    context: &SecurityContext,
) -> Result<Vec<(String, Value)>> {
    let mut values = Vec::with_capacity(members.len());
    let mut violations = Vec::new();
    for (member, value) in members {
        if !schema.kinds.contains_key(member)
            && !schema.defaults.contains_key(member)
            && !record.contains_key(member)
        {
            return Err(RuntimeError::Transaction(format!(
                "{entity} has no member {member}"
            )));
        }
        if !security.entity_allowed(
            entity,
            EntityAction::Write,
            Some(member),
            Some(record),
            context,
        ) {
            return Err(RuntimeError::NotAuthorized {
                action: "write".to_string(),
                resource: format!("{entity}.{member}"),
            });
        }
        match stored(value, schema.kinds.get(member), record.get(member)) {
            Ok(value) => values.push((member.clone(), value)),
            Err(reason) => violations.push(Violation {
                member: member.clone(),
                message: format!("{member}: {reason}"),
                object: None,
            }),
        }
    }
    if violations.is_empty() {
        Ok(values)
    } else {
        Err(RuntimeError::Validation(violations))
    }
}

/// An object a page gives a flow, as the flow is to find it.
struct Given {
    entity: String,
    id: String,
    /// A form's new object: it exists once the flow's unit of work does.
    create: bool,
    /// What a flow made of it before, when one handed it to the page.
    restored: Vec<(String, Value)>,
    values: Vec<(String, Value)>,
}

/// The objects a page gives a flow (`{entity, id}`, with the members its
/// form changed and whether it is new): each must be one the caller may
/// read — or, new, may create — and what it changed, what it may write.
fn given_objects(
    store: &Store,
    security: &SecurityPolicy,
    held: &HeldObjects,
    arguments: &Value,
    context: &SecurityContext,
) -> Result<Vec<Given>> {
    let Some(arguments) = arguments.as_object() else {
        return Ok(Vec::new());
    };
    let mut given = Vec::new();
    for value in arguments.values() {
        let (Some(entity), Some(id)) = (
            value.get("entity").and_then(Value::as_str),
            value.get("id").and_then(Value::as_str),
        ) else {
            continue;
        };
        let schema = store
            .schema()
            .entities
            .get(entity)
            .cloned()
            .ok_or_else(|| RuntimeError::UnknownEntity(entity.to_string()))?;
        let new = value.get("new").and_then(Value::as_bool) == Some(true);
        let existing = store.find(entity, id)?;
        let kept = held.get(entity, id, context);
        let restored: Vec<(String, Value)> =
            kept.iter().flat_map(|kept| kept.members.clone()).collect();
        let record = match (&existing, new) {
            (None, _) if kept.is_some_and(|kept| kept.new) => {
                let mut record = schema.defaults.clone();
                record.extend(restored.iter().cloned());
                record
            }
            (Some(object), _) => {
                if !security.entity_allowed(
                    entity,
                    EntityAction::Read,
                    None,
                    Some(&object.members),
                    context,
                ) {
                    return Err(RuntimeError::NotAuthorized {
                        action: "read".to_string(),
                        resource: entity.to_string(),
                    });
                }
                let mut record = object.members.clone();
                record.extend(restored.iter().cloned());
                record
            }
            (None, true) => {
                if !security.entity_allowed(entity, EntityAction::Create, None, None, context) {
                    return Err(RuntimeError::NotAuthorized {
                        action: "create".to_string(),
                        resource: entity.to_string(),
                    });
                }
                schema.defaults.clone()
            }
            (None, false) => {
                return Err(RuntimeError::UnknownObject {
                    entity: entity.to_string(),
                    id: id.to_string(),
                });
            }
        };
        let values = match value.get("members").and_then(Value::as_object) {
            Some(members) => written(security, &schema, entity, &record, members, context)?,
            None => Vec::new(),
        };
        given.push(Given {
            entity: entity.to_string(),
            id: id.to_string(),
            create: existing.is_none(),
            restored,
            values,
        });
    }
    Ok(given)
}

/// What a flow answers, as its caller may read it: every object in it
/// (`{entity, id, members}`) keeps only the members the caller may read.
fn readable(
    store: &Store,
    security: &SecurityPolicy,
    value: Value,
    context: &SecurityContext,
) -> Value {
    match value {
        Value::Array(values) => Value::Array(
            values
                .into_iter()
                .map(|value| readable(store, security, value, context))
                .collect(),
        ),
        Value::Object(mut map) => {
            let shown = match (
                map.get("entity").and_then(Value::as_str),
                map.get("id").and_then(Value::as_str),
                map.get("members"),
            ) {
                (Some(entity), Some(id), Some(Value::Object(members))) => {
                    let record = store
                        .find(entity, id)
                        .ok()
                        .flatten()
                        .map(|object| object.members)
                        .unwrap_or_else(|| {
                            members
                                .iter()
                                .map(|(k, v)| (k.clone(), v.clone()))
                                .collect()
                        });
                    Some(
                        members
                            .iter()
                            .filter(|(member, _)| {
                                security.entity_allowed(
                                    entity,
                                    EntityAction::Read,
                                    Some(member),
                                    Some(&record),
                                    context,
                                )
                            })
                            .map(|(member, value)| (member.clone(), value.clone()))
                            .collect::<serde_json::Map<_, _>>(),
                    )
                }
                _ => None,
            };
            if let Some(shown) = shown {
                map.insert("members".to_string(), Value::Object(shown));
                Value::Object(map)
            } else {
                Value::Object(
                    map.into_iter()
                        .map(|(key, value)| (key, readable(store, security, value, context)))
                        .collect(),
                )
            }
        }
        other => other,
    }
}

/// The order of two members as a list sorts them: numbers by size, dates by
/// instant, texts by their characters; what is absent comes first.
fn compare_members(left: Option<&Value>, right: Option<&Value>) -> std::cmp::Ordering {
    // A total order: what is absent, then booleans, numbers, dates, texts
    // (as a person reads them, then by case), then anything else.
    let key = |value: Option<&Value>| -> (u8, f64, String) {
        match value {
            None | Some(Value::Null) => (0, 0.0, String::new()),
            Some(Value::Bool(flag)) => (1, f64::from(u8::from(*flag)), String::new()),
            Some(Value::Number(number)) => (2, number.as_f64().unwrap_or(0.0), String::new()),
            Some(Value::String(text)) => match text
                .strip_prefix(DATETIME_MEMBER_PREFIX)
                .and_then(|seconds| seconds.parse::<f64>().ok())
            {
                Some(seconds) => (3, seconds, String::new()),
                None => (4, 0.0, text.clone()),
            },
            Some(other) => (5, 0.0, other.to_string()),
        }
    };
    let (left_rank, left_number, left_text) = key(left);
    let (right_rank, right_number, right_text) = key(right);
    left_rank
        .cmp(&right_rank)
        .then_with(|| left_number.total_cmp(&right_number))
        .then_with(|| left_text.to_lowercase().cmp(&right_text.to_lowercase()))
        .then_with(|| left_text.cmp(&right_text))
}

/// An object as a page shows it: a date and time is text a browser reads
/// (`2026-10-06T12:00:00Z`), not the tag the store keeps it under.
pub fn shown(object: &ObjectValue) -> Value {
    let members: serde_json::Map<String, Value> = object
        .members
        .iter()
        .map(|(member, value)| {
            let instant = value
                .as_str()
                .and_then(|text| text.strip_prefix(DATETIME_MEMBER_PREFIX))
                .and_then(|seconds| seconds.parse::<f64>().ok())
                .filter(|seconds| seconds.is_finite());
            let value = match instant {
                Some(seconds) => Value::String(iso_instant(seconds.floor() as i64)),
                None => value.clone(),
            };
            (member.clone(), value)
        })
        .collect();
    serde_json::json!({ "entity": object.entity, "id": object.id, "members": members })
}

/// What a form sent for a member, as the store keeps what the member's
/// attribute holds — or why it cannot be that.
fn stored(
    value: &Value,
    kind: Option<&MemberKind>,
    current: Option<&Value>,
) -> std::result::Result<Value, String> {
    let blank = value.is_null() || value.as_str().is_some_and(|text| text.trim().is_empty());
    // A member the model says nothing about holds a date where it holds one now.
    let dated = current
        .and_then(Value::as_str)
        .is_some_and(|text| text.starts_with(DATETIME_MEMBER_PREFIX));
    match kind {
        Some(MemberKind::Text { length }) => match value {
            Value::Null => Ok(Value::String(String::new())),
            Value::String(text) => match length {
                Some(length) if text.chars().count() > *length => {
                    Err(format!("longer than {length} characters"))
                }
                _ => Ok(value.clone()),
            },
            _ => Err("not a text".to_string()),
        },
        Some(MemberKind::Boolean) => match value {
            Value::Bool(_) => Ok(value.clone()),
            Value::String(text) if text == "true" => Ok(Value::Bool(true)),
            Value::String(text) if text == "false" => Ok(Value::Bool(false)),
            _ => Err("neither true nor false".to_string()),
        },
        Some(MemberKind::Integer) if blank => Ok(Value::Null),
        Some(MemberKind::Integer) => value
            .as_i64()
            .or_else(|| value.as_str().and_then(|text| text.trim().parse().ok()))
            .map(Value::from)
            .ok_or_else(|| "not a whole number".to_string()),
        Some(MemberKind::Decimal) if blank => Ok(Value::Null),
        Some(MemberKind::Decimal) => value
            .as_f64()
            .or_else(|| value.as_str().and_then(|text| text.trim().parse().ok()))
            .filter(|number| number.is_finite())
            .and_then(serde_json::Number::from_f64)
            .map(Value::Number)
            .ok_or_else(|| "not a number".to_string()),
        Some(MemberKind::DateTime) if blank => Ok(Value::String(String::new())),
        None if dated && blank => Ok(Value::String(String::new())),
        Some(MemberKind::DateTime) => instant(value),
        None if dated => instant(value),
        Some(MemberKind::Other) => match value {
            Value::Null => Ok(Value::String(String::new())),
            Value::String(_) => Ok(value.clone()),
            _ => Err("not a text".to_string()),
        },
        None => Ok(value.clone()),
    }
}

fn instant(value: &Value) -> std::result::Result<Value, String> {
    value
        .as_str()
        .and_then(instant_seconds)
        .map(|seconds| Value::String(format!("{DATETIME_MEMBER_PREFIX}{seconds}")))
        .ok_or_else(|| "not a date (YYYY-MM-DD or YYYY-MM-DDTHH:MM:SSZ)".to_string())
}

/// `seconds` since the epoch as `YYYY-MM-DDTHH:MM:SSZ`.
fn iso_instant(seconds: i64) -> String {
    let days = seconds.div_euclid(86_400);
    let of_day = seconds.rem_euclid(86_400);
    // Civil date from a day count (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let day_of_era = z.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_index + 2) / 5 + 1;
    let month = if month_index < 10 {
        month_index + 3
    } else {
        month_index - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        of_day / 3_600,
        of_day % 3_600 / 60,
        of_day % 60
    )
}

/// The seconds since the epoch a date (`YYYY-MM-DD`, midnight UTC) or an
/// instant (`YYYY-MM-DDTHH:MM[:SS[.fff]]` with `Z`, an offset or neither,
/// which is UTC) names — a real one: the 31st of February is none.
fn instant_seconds(text: &str) -> Option<i64> {
    let (date, time) = match text.split_once('T') {
        Some((date, time)) => (date, time),
        None => (text, "00:00:00Z"),
    };
    let mut parts = date.split('-');
    let number = |part: Option<&str>, digits: usize| {
        let part = part?;
        (part.len() == digits && part.bytes().all(|byte| byte.is_ascii_digit()))
            .then(|| part.parse::<i64>().ok())
            .flatten()
    };
    let (year, month, day) = (
        number(parts.next(), 4)?,
        number(parts.next(), 2)?,
        number(parts.next(), 2)?,
    );
    if parts.next().is_some() || year == 0 || !(1..=12).contains(&month) {
        return None;
    }
    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let days = match month {
        2 if leap => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    };
    if !(1..=days).contains(&day) {
        return None;
    }
    // The offset, when the time states one.
    let (time, offset) = if let Some(time) = time.strip_suffix('Z') {
        (time, 0)
    } else if let Some(at) = time.rfind(['+', '-']) {
        let (time, zone) = time.split_at(at);
        let mut zone_parts = zone[1..].split(':');
        let (hours, minutes) = (number(zone_parts.next(), 2)?, number(zone_parts.next(), 2)?);
        if zone_parts.next().is_some() || hours > 14 || minutes > 59 {
            return None;
        }
        let seconds = hours * 3_600 + minutes * 60;
        (
            time,
            if zone.starts_with('-') {
                -seconds
            } else {
                seconds
            },
        )
    } else {
        (time, 0)
    };
    let mut clock = time.split(':');
    let hour = number(clock.next(), 2)?;
    let minute = number(clock.next(), 2)?;
    let second = match clock.next() {
        Some(second) => number(Some(second.split('.').next()?), 2)?,
        None => 0,
    };
    if clock.next().is_some() || hour > 23 || minute > 59 || second > 59 {
        return None;
    }
    let year = year - i64::from(month <= 2);
    let era = year.div_euclid(400);
    let year_of_era = year.rem_euclid(400);
    let day_of_year = (153 * (if month > 2 { month - 3 } else { month + 9 }) + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    Some(
        (era * 146_097 + day_of_era - 719_468) * 86_400 + hour * 3_600 + minute * 60 + second
            - offset,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A list is retrieved as its source asks: within its XPath constraint,
    /// in its sort order, so many from where it is — and told how many
    /// there are in all. A constraint outside what the runtime reads is
    /// refused, not answered with nothing.
    #[test]
    fn a_list_is_retrieved_as_its_source_asks() {
        let defaults = BTreeMap::from([
            ("Name".to_string(), Value::String(String::new())),
            ("Amount".to_string(), Value::from(0)),
            ("Active".to_string(), Value::Bool(true)),
        ]);
        let mut runtime = Runtime::new(
            Store::new(StoreSchema::default().entity("Main.Sale", defaults, false)),
            SecurityPolicy::default(),
        );
        let context = SecurityContext::default();
        for (name, amount, active) in [("Food", 30, true), ("Vet", 200, true), ("Toys", 45, false)]
        {
            let blank = runtime
                .data(
                    "create",
                    &serde_json::json!({ "entity": "Main.Sale" }),
                    &context,
                )
                .unwrap();
            runtime
                .data(
                    "save",
                    &serde_json::json!({
                        "entity": "Main.Sale", "id": blank["id"], "new": true,
                        "members": { "Name": name, "Amount": amount, "Active": active },
                    }),
                    &context,
                )
                .unwrap();
        }
        let names = |answer: &Value| {
            answer["objects"]
                .as_array()
                .unwrap()
                .iter()
                .map(|object| object["members"]["Name"].as_str().unwrap().to_string())
                .collect::<Vec<_>>()
        };
        let listed = runtime
            .data(
                "retrieve",
                &serde_json::json!({
                    "entity": "Main.Sale",
                    "constraint": "[Active = true()]",
                    "sort": [{ "attribute": "Amount", "descending": true }],
                }),
                &context,
            )
            .unwrap();
        assert_eq!(names(&listed), vec!["Vet", "Food"]);
        assert_eq!(listed["total"], 2);
        let paged = runtime
            .data(
                "retrieve",
                &serde_json::json!({
                    "entity": "Main.Sale",
                    "sort": [{ "attribute": "Name", "descending": false }],
                    "offset": 1, "limit": 1,
                }),
                &context,
            )
            .unwrap();
        assert_eq!(names(&paged), vec!["Toys"]);
        assert_eq!(paged["total"], 3);
        let refused = runtime
            .data(
                "retrieve",
                &serde_json::json!({ "entity": "Main.Sale", "constraint": "[contains(Name, 'o')]" }),
                &context,
            )
            .unwrap_err()
            .to_string();
        assert!(
            refused.contains("unsupported XPath constraint"),
            "{refused}"
        );
    }

    /// A page's own operations: what it creates is kept once saved, a date
    /// stays a date, and what it deletes is gone.
    #[test]
    fn a_page_creates_saves_lists_and_deletes_objects() {
        let defaults = BTreeMap::from([
            ("Name".to_string(), Value::String(String::new())),
            ("Active".to_string(), Value::Bool(true)),
            (
                "BirthDate".to_string(),
                Value::String(format!("{DATETIME_MEMBER_PREFIX}0")),
            ),
        ]);
        let mut runtime = Runtime::new(
            Store::new(StoreSchema::default().entity("Main.Animal", defaults, false)),
            SecurityPolicy::default(),
        );
        let context = SecurityContext::default();
        let entity = serde_json::json!({ "entity": "Main.Animal" });

        let blank = runtime.data("create", &entity, &context).unwrap();
        assert_eq!(blank["members"]["Active"], true);
        assert_eq!(blank["members"]["BirthDate"], "1970-01-01T00:00:00Z");
        // Nothing is kept until the form is saved.
        let listed = runtime.data("retrieve", &entity, &context).unwrap();
        assert_eq!(listed["objects"].as_array().unwrap().len(), 0);

        let saved = runtime
            .data(
                "save",
                &serde_json::json!({
                    "entity": "Main.Animal",
                    "id": blank["id"],
                    "new": true,
                    "members": { "Name": "Rex", "BirthDate": "2020-02-29" },
                }),
                &context,
            )
            .unwrap();
        assert_eq!(saved["members"]["Name"], "Rex");
        assert_eq!(saved["members"]["BirthDate"], "2020-02-29T00:00:00Z");
        let id = saved["id"].as_str().unwrap().to_string();
        assert!(runtime.store().is_committed("Main.Animal", &id));

        // Saved again, it is the same object.
        let renamed = runtime
            .data(
                "save",
                &serde_json::json!({ "entity": "Main.Animal", "id": id, "members": { "Name": "Max" } }),
                &context,
            )
            .unwrap();
        assert_eq!(renamed["id"], id.as_str());
        let listed = runtime.data("retrieve", &entity, &context).unwrap();
        assert_eq!(listed["objects"][0]["members"]["Name"], "Max");
        assert_eq!(listed["objects"].as_array().unwrap().len(), 1);

        assert!(matches!(
            runtime.data(
                "save",
                &serde_json::json!({ "entity": "Main.Animal", "id": id, "members": { "Nope": 1 } }),
                &context,
            ),
            Err(RuntimeError::Transaction(_))
        ));
        runtime
            .data(
                "delete",
                &serde_json::json!({ "entity": "Main.Animal", "id": id }),
                &context,
            )
            .unwrap();
        let listed = runtime.data("retrieve", &entity, &context).unwrap();
        assert_eq!(listed["objects"].as_array().unwrap().len(), 0);
        assert!(matches!(
            runtime.data("explode", &entity, &context),
            Err(RuntimeError::UnknownAction(_))
        ));
        // What was deleted is not brought back by a form still open on it,
        // and a new object saved twice is one object.
        let stale = serde_json::json!({ "entity": "Main.Animal", "id": id, "members": { "Name": "Ghost" } });
        assert!(matches!(
            runtime.data("save", &stale, &context),
            Err(RuntimeError::UnknownObject { .. })
        ));
        let fresh = serde_json::json!({
            "entity": "Main.Animal",
            "id": uuid::Uuid::new_v4().to_string(),
            "new": true,
            "members": { "Name": "Twice" },
        });
        runtime.data("save", &fresh, &context).unwrap();
        runtime.data("save", &fresh, &context).unwrap();
        let listed = runtime.data("retrieve", &entity, &context).unwrap();
        assert_eq!(listed["objects"].as_array().unwrap().len(), 1);
    }

    /// A value written by a page is what its attribute holds, or is refused.
    #[test]
    fn a_page_writes_only_what_an_attribute_holds() {
        let kinds = BTreeMap::from([
            ("Name".to_string(), MemberKind::Text { length: Some(5) }),
            ("Active".to_string(), MemberKind::Boolean),
            ("Legs".to_string(), MemberKind::Integer),
            ("Weight".to_string(), MemberKind::Decimal),
            ("BirthDate".to_string(), MemberKind::DateTime),
        ]);
        let mut runtime = Runtime::new(
            Store::new(
                StoreSchema::default()
                    .entity("Main.Animal", BTreeMap::new(), false)
                    .members("Main.Animal", kinds),
            ),
            SecurityPolicy::default(),
        );
        let context = SecurityContext::default();
        let id = uuid::Uuid::new_v4().to_string();
        let mut save = |members: Value| {
            runtime.data(
                "save",
                &serde_json::json!({ "entity": "Main.Animal", "id": id, "new": true, "members": members }),
                &context,
            )
        };
        let saved = save(serde_json::json!({
            "Name": "Rex", "Active": "true", "Legs": "4", "Weight": "1.5",
            "BirthDate": "2020-02-29T10:00:00+02:00",
        }))
        .unwrap();
        assert_eq!(saved["members"]["Active"], true);
        assert_eq!(saved["members"]["Legs"], 4);
        assert_eq!(saved["members"]["Weight"], 1.5);
        assert_eq!(saved["members"]["BirthDate"], "2020-02-29T08:00:00Z");
        // Cleared, a number or a date holds nothing.
        let cleared = save(serde_json::json!({ "Legs": "", "BirthDate": null })).unwrap();
        assert_eq!(cleared["members"]["Legs"], Value::Null);
        assert_eq!(cleared["members"]["BirthDate"], "");
        for refused in [
            serde_json::json!({ "Name": "Too long" }),
            serde_json::json!({ "Name": { "a": 1 } }),
            serde_json::json!({ "Active": "yes" }),
            serde_json::json!({ "Legs": "four" }),
            serde_json::json!({ "Weight": "1,5" }),
            serde_json::json!({ "BirthDate": "2023-02-31" }),
            serde_json::json!({ "BirthDate": "2023-01-01T25:00:00Z" }),
            serde_json::json!({ "BirthDate": "99999999999-01-01" }),
        ] {
            // A value its attribute cannot hold is said under its input.
            assert!(
                matches!(save(refused.clone()), Err(RuntimeError::Validation(violations)) if violations.len() == 1),
                "{refused}"
            );
        }
        // A member the entity does not have is not the user's to put right.
        assert!(matches!(
            save(serde_json::json!({ "Nope": 1 })),
            Err(RuntimeError::Transaction(_))
        ));
        assert_eq!(instant_seconds("1970-01-01"), Some(0));
        assert_eq!(instant_seconds("1969-12-31T23:59:59Z"), Some(-1));
        assert_eq!(instant_seconds("2100-02-29"), None);
        assert_eq!(
            iso_instant(instant_seconds("2000-02-29T23:59:59.999Z").unwrap()),
            "2000-02-29T23:59:59Z"
        );
    }

    /// What a page saves is held to the entity's validation rules, every
    /// broken one answered at once with the model's message.
    fn rule(member: &str, kind: RuleKind, message: &str) -> ValidationRule {
        ValidationRule {
            member: member.to_string(),
            kind,
            message: message.to_string(),
        }
    }

    #[test]
    fn a_save_is_checked_against_the_entitys_validation_rules() {
        let mut runtime = Runtime::new(
            Store::new(
                StoreSchema::default()
                    .entity("Main.Customer", BTreeMap::new(), false)
                    .members(
                        "Main.Customer",
                        BTreeMap::from([
                            ("Name".to_string(), MemberKind::Text { length: None }),
                            ("Email".to_string(), MemberKind::Text { length: None }),
                            ("Age".to_string(), MemberKind::Integer),
                        ]),
                    )
                    .rules(
                        "Main.Customer",
                        vec![
                            rule("Name", RuleKind::Required, "Please name the customer"),
                            rule("Email", RuleKind::Unique, ""),
                            rule("Email", RuleKind::MaxLength(12), ""),
                            rule("Email", RuleKind::Pattern("^[^@]+@[^@]+$".into()), ""),
                            rule(
                                "Age",
                                RuleKind::Range {
                                    minimum: Some(Bound::Value("0".into())),
                                    maximum: Some(Bound::Value("150".into())),
                                },
                                "",
                            ),
                        ],
                    ),
            ),
            SecurityPolicy::default(),
        );
        let context = SecurityContext::default();
        let save = |runtime: &mut Runtime, id: &str, members: Value| {
            runtime.data(
                "save",
                &serde_json::json!({ "entity": "Main.Customer", "id": id, "new": true, "members": members }),
                &context,
            )
        };
        let first = uuid::Uuid::new_v4().to_string();
        save(
            &mut runtime,
            &first,
            serde_json::json!({ "Name": "Ana", "Email": "ana@pets.io", "Age": 30 }),
        )
        .unwrap();
        let second = uuid::Uuid::new_v4().to_string();
        let refused = save(
            &mut runtime,
            &second,
            serde_json::json!({ "Name": "", "Email": "ana@pets.io", "Age": 200 }),
        );
        let Err(RuntimeError::Validation(violations)) = refused else {
            panic!("{refused:?}");
        };
        assert_eq!(
            violations,
            vec![
                Violation {
                    member: "Name".into(),
                    message: "Please name the customer".into(),
                    object: Some(second.clone()),
                },
                Violation {
                    member: "Email".into(),
                    message: "Email must be unique".into(),
                    object: Some(second.clone()),
                },
                Violation {
                    member: "Age".into(),
                    message: "Age must be between 0 and 150".into(),
                    object: Some(second.clone()),
                },
            ]
        );
        for (members, member) in [
            (
                serde_json::json!({ "Name": "Bo", "Email": "a-very-long@x.io" }),
                "Email",
            ),
            (
                serde_json::json!({ "Name": "Bo", "Email": "not-mail" }),
                "Email",
            ),
        ] {
            let Err(RuntimeError::Validation(violations)) = save(&mut runtime, &second, members)
            else {
                panic!("accepted");
            };
            assert_eq!(violations.len(), 1);
            assert_eq!(violations[0].member, member);
        }
        // Nothing is kept of a refused save.
        assert!(
            runtime
                .store()
                .find("Main.Customer", &second)
                .unwrap()
                .is_none()
        );
        // An object may keep its own value: uniqueness is against the others.
        save(
            &mut runtime,
            &first,
            serde_json::json!({ "Name": "Ana", "Email": "ana@pets.io" }),
        )
        .unwrap();
    }

    /// Unique is the database's index: a number by its value whoever wrote
    /// it, text as written, and only what was committed.
    #[test]
    fn unique_compares_what_was_committed_as_its_index_would() {
        let kinds = BTreeMap::from([
            ("Code".to_string(), MemberKind::Text { length: None }),
            ("Price".to_string(), MemberKind::Decimal),
        ]);
        let mut runtime = Runtime::new(
            Store::new(
                StoreSchema::default()
                    .entity("Main.Item", BTreeMap::new(), false)
                    .members("Main.Item", kinds)
                    .rules(
                        "Main.Item",
                        vec![
                            rule("Code", RuleKind::Unique, ""),
                            rule("Price", RuleKind::Unique, ""),
                        ],
                    ),
            ),
            SecurityPolicy::default(),
        );
        // What a flow commits: an integer, as the flow wrote it.
        let store = runtime.store_mut();
        let first = store.create("Main.Item").unwrap();
        store
            .set_member("Main.Item", &first.id, "Code", Value::from("Abc"))
            .unwrap();
        store
            .set_member("Main.Item", &first.id, "Price", Value::from(5))
            .unwrap();
        store.commit("Main.Item", &first.id).unwrap();
        // Held by a unit of work and never committed, it is no one's.
        let draft = store.create("Main.Item").unwrap();
        store
            .set_member("Main.Item", &draft.id, "Code", Value::from("abc"))
            .unwrap();
        let context = SecurityContext::default();
        let mut save = |members: Value| {
            runtime.data(
                "save",
                &serde_json::json!({ "entity": "Main.Item", "id": uuid::Uuid::new_v4().to_string(), "new": true, "members": members }),
                &context,
            )
        };
        let Err(RuntimeError::Validation(violations)) = save(serde_json::json!({ "Price": "5.0" }))
        else {
            panic!("5.0 is 5");
        };
        assert_eq!(violations[0].member, "Price");
        save(serde_json::json!({ "Code": "abc", "Price": "6" })).unwrap();
    }

    /// Equal to a value compares a number by its value, and text — even
    /// text of digits — as it is written.
    #[test]
    fn equals_to_compares_text_as_written() {
        let equals = rule("Code", RuleKind::EqualsTo(Bound::Value("7".into())), "");
        let record = |value: Value| BTreeMap::from([("Code".to_string(), value)]);
        assert!(equals.broken(&record(Value::from("007")), &[], false));
        assert!(!equals.broken(&record(Value::from("7")), &[], false));
        assert!(!equals.broken(&record(Value::from(7.0)), &[], true));
    }

    /// A generalization answers for its specializations: a retrieve or a
    /// find of it holds theirs, a change by its name reaches them, and a
    /// unique attribute it declares is unique across the family.
    #[test]
    fn a_generalization_answers_for_its_specializations() {
        let kinds = || BTreeMap::from([("Name".to_string(), MemberKind::Text { length: None })]);
        let schema = StoreSchema::default()
            .entity("App.Animal", BTreeMap::new(), false)
            .members("App.Animal", kinds())
            .rules("App.Animal", vec![rule("Name", RuleKind::Unique, "")])
            .entity("App.Dog", BTreeMap::new(), false)
            .members("App.Dog", kinds())
            .rules("App.Dog", vec![rule("Name", RuleKind::Unique, "")])
            .generalization("App.Dog", "App.Animal");
        assert!(schema.is_a("App.Dog", "App.Animal"));
        assert!(!schema.is_a("App.Animal", "App.Dog"));
        let mut store = Store::new(schema);
        let cat = store.create("App.Animal").unwrap();
        store
            .set_member("App.Animal", &cat.id, "Name", Value::from("Tom"))
            .unwrap();
        store.commit("App.Animal", &cat.id).unwrap();
        let dog = store.create("App.Dog").unwrap();
        store
            .set_member("App.Animal", &dog.id, "Name", Value::from("Tom"))
            .unwrap();
        assert_eq!(store.retrieve("App.Animal").unwrap().len(), 2);
        assert_eq!(store.retrieve("App.Dog").unwrap().len(), 1);
        assert_eq!(
            store.find("App.Animal", &dog.id).unwrap().unwrap().entity,
            "App.Dog"
        );
        let Err(RuntimeError::Validation(violations)) =
            validate_object(&store, "App.Animal", &dog.id)
        else {
            panic!("a name the family holds is not unique");
        };
        assert_eq!(violations[0].member, "Name");
        store.commit("App.Animal", &dog.id).unwrap();
        assert!(store.is_committed("App.Dog", &dog.id));
    }

    /// An object a flow makes, or changes, and hands a page without
    /// committing is saved by the page with what the flow set — for the
    /// user it was handed to, and no one else.
    #[test]
    fn a_page_saves_what_a_flow_made_of_the_object_it_opened_it_with() {
        let blank = BTreeMap::from([
            ("Number".to_string(), Value::from("")),
            ("Status".to_string(), Value::from("")),
        ]);
        let mut runtime = Runtime::new(
            Store::new(StoreSchema::default().entity("Main.Order", blank, false)),
            SecurityPolicy::default(),
        );
        runtime.register_action("Main.ACT_Order_New", |store: &mut Store, _: &Value| {
            let order = store.create("Main.Order")?;
            store.set_member("Main.Order", &order.id, "Status", Value::from("Draft"))?;
            let order = store.find("Main.Order", &order.id)?.expect("created");
            Ok(serde_json::json!({ "result": Value::Null, "effects": [
                { "type": "open_page", "objects": [shown(&order)] },
            ] }))
        });
        runtime.register_action(
            "Main.ACT_Order_Reopen",
            |store: &mut Store, arguments: &Value| {
                let id = arguments["Order"]["id"].as_str().expect("given");
                store.set_member("Main.Order", id, "Status", Value::from("Open"))?;
                Ok(Value::Null)
            },
        );
        let owner = SecurityContext {
            user: Some("ana".into()),
            ..SecurityContext::default()
        };
        let answer = runtime
            .invoke("Main.ACT_Order_New", &serde_json::json!({}), &owner)
            .unwrap();
        let opened = &answer["effects"][0]["objects"][0];
        assert_eq!(opened["new"], true);
        let id = opened["id"].as_str().unwrap().to_string();
        let save = |runtime: &mut Runtime, context: &SecurityContext, members: Value| {
            runtime.data(
                "save",
                &serde_json::json!({ "entity": "Main.Order", "id": id, "new": true, "members": members }),
                context,
            )
        };
        // Another user is not handed what the flow made.
        let other = SecurityContext {
            user: Some("bo".into()),
            ..SecurityContext::default()
        };
        let mut elsewhere = Runtime::new(runtime.store().clone(), SecurityPolicy::default());
        std::mem::swap(&mut elsewhere.held, &mut runtime.held);
        let foreign = save(&mut elsewhere, &other, serde_json::json!({})).unwrap();
        assert_eq!(foreign["members"]["Status"], "");
        std::mem::swap(&mut elsewhere.held, &mut runtime.held);
        let saved = save(&mut runtime, &owner, serde_json::json!({ "Number": "A1" })).unwrap();
        assert_eq!(saved["members"]["Status"], "Draft");
        assert_eq!(saved["members"]["Number"], "A1");
        assert!(runtime.store().is_committed("Main.Order", &id));
        // A flow that changes a committed object the page then holds: the
        // change is saved with the page's.
        runtime
            .invoke(
                "Main.ACT_Order_Reopen",
                &serde_json::json!({ "Order": { "entity": "Main.Order", "id": id } }),
                &owner,
            )
            .unwrap();
        assert_eq!(
            runtime
                .store()
                .find("Main.Order", &id)
                .unwrap()
                .unwrap()
                .members["Status"],
            "Draft"
        );
        let saved = runtime
            .data(
                "save",
                &serde_json::json!({ "entity": "Main.Order", "id": id, "members": {} }),
                &owner,
            )
            .unwrap();
        assert_eq!(saved["members"]["Status"], "Open");
    }

    /// A save is committed through the lifecycle the runtime was given: the
    /// effects its flows ask for come back with the object, and what it
    /// refuses is not kept.
    #[test]
    fn a_save_is_committed_through_the_entitys_lifecycle() {
        struct Recording;
        impl Lifecycle for Recording {
            fn commit(
                &self,
                store: &mut Store,
                entity: &str,
                id: &str,
                _context: &SecurityContext,
            ) -> Result<Vec<Value>> {
                let name = store
                    .find(entity, id)?
                    .and_then(|object| object.members.get("Name").cloned())
                    .unwrap_or_default();
                if name == "Nope" {
                    return Err(RuntimeError::Validation(vec![Violation {
                        member: "Name".into(),
                        message: "Not that one".into(),
                        object: None,
                    }]));
                }
                store.set_member(
                    entity,
                    id,
                    "Name",
                    Value::String(format!("{} ✓", name.as_str().unwrap_or(""))),
                )?;
                store.commit(entity, id)?;
                Ok(vec![
                    serde_json::json!({ "type": "show_message", "message": "Saved" }),
                ])
            }
        }
        let mut runtime = Runtime::new(
            Store::new(
                StoreSchema::default()
                    .entity("Main.Customer", BTreeMap::new(), false)
                    .members(
                        "Main.Customer",
                        BTreeMap::from([("Name".to_string(), MemberKind::Text { length: None })]),
                    ),
            ),
            SecurityPolicy::default(),
        );
        runtime.set_lifecycle(Recording);
        let context = SecurityContext::default();
        let id = uuid::Uuid::new_v4().to_string();
        let save = |runtime: &mut Runtime, name: &str| {
            runtime.data(
                "save",
                &serde_json::json!({ "entity": "Main.Customer", "id": id, "new": true, "members": { "Name": name } }),
                &context,
            )
        };
        let saved = save(&mut runtime, "Ana").unwrap();
        assert_eq!(saved["members"]["Name"], "Ana ✓");
        assert_eq!(saved["effects"][0]["type"], "show_message");
        let refused = save(&mut runtime, "Nope");
        assert!(
            matches!(refused, Err(RuntimeError::Validation(_))),
            "{refused:?}"
        );
        assert_eq!(
            runtime
                .store()
                .find("Main.Customer", &id)
                .unwrap()
                .unwrap()
                .members["Name"],
            "Ana ✓"
        );
    }

    /// A page sees and writes what its user's role may, member by member.
    #[test]
    fn a_page_reads_and_writes_within_the_rights_of_its_role() {
        let defaults = BTreeMap::from([
            ("Name".to_string(), Value::String(String::new())),
            ("Secret".to_string(), Value::String("hidden".into())),
        ]);
        let rule = EntityRule {
            module_roles: BTreeSet::from(["Main.User".to_string()]),
            create: true,
            delete: false,
            default_member_right: None,
            member_rights: BTreeMap::from([
                ("Name".to_string(), MemberRight::Write),
                ("Secret".to_string(), MemberRight::None),
            ]),
            xpath: String::new(),
        };
        let security = SecurityPolicy {
            enabled: true,
            entities: BTreeMap::from([("Main.Animal".to_string(), vec![rule])]),
            ..Default::default()
        };
        let mut runtime = Runtime::new(
            Store::new(StoreSchema::default().entity("Main.Animal", defaults, false)),
            security,
        );
        let user = SecurityContext {
            module_roles: BTreeSet::from(["Main.User".to_string()]),
            ..Default::default()
        };
        let id = uuid::Uuid::new_v4().to_string();
        let save = |members: Value| serde_json::json!({ "entity": "Main.Animal", "id": id, "new": true, "members": members });
        // A member the role may not write is refused on a new object too.
        assert!(matches!(
            runtime.data("save", &save(serde_json::json!({ "Secret": "x" })), &user),
            Err(RuntimeError::NotAuthorized { .. })
        ));
        let saved = runtime
            .data("save", &save(serde_json::json!({ "Name": "Rex" })), &user)
            .unwrap();
        assert_eq!(saved["members"], serde_json::json!({ "Name": "Rex" }));
        let entity = serde_json::json!({ "entity": "Main.Animal" });
        let listed = runtime.data("retrieve", &entity, &user).unwrap();
        assert_eq!(
            listed["objects"][0]["members"],
            serde_json::json!({ "Name": "Rex" })
        );
        assert!(matches!(
            runtime.data(
                "delete",
                &serde_json::json!({ "entity": "Main.Animal", "id": id }),
                &user
            ),
            Err(RuntimeError::NotAuthorized { .. })
        ));
        // Someone with no role is told so, not shown an empty list.
        assert!(matches!(
            runtime.data("retrieve", &entity, &SecurityContext::default()),
            Err(RuntimeError::NotAuthorized { .. })
        ));
    }

    /// A role that may read some of a record, and a flow that echoes what it
    /// was given and what the store holds of it.
    fn guarded() -> (Runtime, SecurityContext, String) {
        let defaults = BTreeMap::from([
            ("Name".to_string(), Value::String(String::new())),
            ("Salary".to_string(), Value::from(0)),
        ]);
        let rule = EntityRule {
            module_roles: BTreeSet::from(["Main.User".to_string()]),
            create: true,
            delete: false,
            default_member_right: None,
            member_rights: BTreeMap::from([
                ("Name".to_string(), MemberRight::Write),
                ("Salary".to_string(), MemberRight::None),
            ]),
            xpath: "[Name != 'Boss']".to_string(),
        };
        let security = SecurityPolicy {
            enabled: true,
            entities: BTreeMap::from([("Main.Person".to_string(), vec![rule])]),
            documents: BTreeMap::from([(
                "Main.ACT_Echo".to_string(),
                BTreeSet::from(["Main.User".to_string()]),
            )]),
            ..Default::default()
        };
        let mut store = Store::new(
            StoreSchema::default()
                .entity("Main.Person", defaults, false)
                .members(
                    "Main.Person",
                    BTreeMap::from([
                        ("Name".to_string(), MemberKind::Text { length: None }),
                        ("Salary".to_string(), MemberKind::Integer),
                    ]),
                ),
        );
        let mut ids = Vec::new();
        for (name, salary) in [("Ann", 10), ("Boss", 900), ("Cid", 500)] {
            let object = store.create("Main.Person").unwrap();
            store
                .set_member("Main.Person", &object.id, "Name", Value::from(name))
                .unwrap();
            store
                .set_member("Main.Person", &object.id, "Salary", Value::from(salary))
                .unwrap();
            store.commit("Main.Person", &object.id).unwrap();
            ids.push(object.id);
        }
        let mut runtime = Runtime::new(store, security);
        runtime.register_action("Main.ACT_Echo", |store: &mut Store, arguments: &Value| {
            let person = &arguments["Person"];
            let object = store
                .find("Main.Person", person["id"].as_str().unwrap())?
                .unwrap();
            Ok(serde_json::json!({ "result": shown(&object) }))
        });
        let user = SecurityContext {
            module_roles: BTreeSet::from(["Main.User".to_string()]),
            ..Default::default()
        };
        (runtime, user, ids[1].clone())
    }

    /// A flow is given only objects its caller may read, with the changes
    /// the caller's form holds and may make; what comes back is what the
    /// caller may read of it.
    #[test]
    fn a_flow_is_given_what_its_caller_may_read_and_answers_what_it_may_see() {
        let (mut runtime, user, boss) = guarded();
        let people = runtime
            .data(
                "retrieve",
                &serde_json::json!({ "entity": "Main.Person" }),
                &user,
            )
            .unwrap();
        let ann = people["objects"]
            .as_array()
            .unwrap()
            .iter()
            .find(|person| person["members"]["Name"] == "Ann")
            .and_then(|person| person["id"].as_str())
            .unwrap()
            .to_string();
        let echo = |runtime: &mut Runtime, person: Value| {
            runtime.invoke(
                "Main.ACT_Echo",
                &serde_json::json!({ "Person": person }),
                &user,
            )
        };
        // Someone else's record, by its id: refused, not run.
        assert!(matches!(
            echo(
                &mut runtime,
                serde_json::json!({ "entity": "Main.Person", "id": boss })
            ),
            Err(RuntimeError::NotAuthorized { action, resource })
                if action == "read" && resource == "Main.Person"
        ));
        assert!(matches!(
            echo(
                &mut runtime,
                serde_json::json!({ "entity": "Main.Person", "id": "3f0b1a52-8b3e-4d6a-9f3c-2d7c1e5a9b77" })
            ),
            Err(RuntimeError::UnknownObject { .. })
        ));
        // The form's change reaches the flow; what it may not read does not
        // come back.
        let answer = echo(
            &mut runtime,
            serde_json::json!({ "entity": "Main.Person", "id": ann, "members": { "Name": "Annie" } }),
        )
        .unwrap();
        assert_eq!(
            answer["result"]["members"],
            serde_json::json!({ "Name": "Annie" })
        );
        // Not committed by the flow, the change is not kept.
        assert_eq!(
            runtime
                .store()
                .find("Main.Person", &ann)
                .unwrap()
                .unwrap()
                .members["Name"],
            "Ann"
        );
        // Nor may the form write what its role may not.
        assert!(matches!(
            echo(
                &mut runtime,
                serde_json::json!({ "entity": "Main.Person", "id": ann, "members": { "Salary": 1 } })
            ),
            Err(RuntimeError::NotAuthorized { .. })
        ));
        // A new object the form holds is the flow's to find.
        let fresh = uuid::Uuid::new_v4().to_string();
        let answer = echo(
            &mut runtime,
            serde_json::json!({ "entity": "Main.Person", "id": fresh, "new": true, "members": { "Name": "Dee" } }),
        )
        .unwrap();
        assert_eq!(answer["result"]["members"]["Name"], "Dee");
    }

    /// A list's constraint and order see what the caller may read, so they
    /// tell nothing of a member it may not.
    #[test]
    fn a_list_tells_nothing_of_what_its_caller_may_not_read() {
        let (mut runtime, user, _) = guarded();
        let retrieve = |runtime: &mut Runtime, query: Value| {
            let mut query = query;
            query["entity"] = Value::from("Main.Person");
            runtime.data("retrieve", &query, &user)
        };
        let probed = retrieve(
            &mut runtime,
            serde_json::json!({ "constraint": "[Salary > 100]" }),
        )
        .unwrap();
        assert_eq!(probed["total"], 0);
        let sorted = retrieve(
            &mut runtime,
            serde_json::json!({ "sort": [{ "attribute": "Salary", "descending": true }] }),
        )
        .unwrap();
        let names: Vec<&str> = sorted["objects"]
            .as_array()
            .unwrap()
            .iter()
            .map(|object| object["members"]["Name"].as_str().unwrap())
            .collect();
        // Every salary reads as absent: the order is the store's, untold.
        assert_eq!(names.len(), 2);
        for unsupported in [
            "[Main.Person_Company/Main.Company/Name = 'X']",
            "[Name = $currentObject/Name]",
            "[Name = '[%CurrentObject%]']",
            "[Born < '[%CurrentDateTime%]']",
        ] {
            assert!(
                matches!(
                    retrieve(
                        &mut runtime,
                        serde_json::json!({ "constraint": unsupported })
                    ),
                    Err(RuntimeError::Transaction(_))
                ),
                "{unsupported}"
            );
        }
        assert!(matches!(
            retrieve(
                &mut runtime,
                serde_json::json!({ "sort": [{ "attribute": "Main.Person_Company/Main.Company/Name", "descending": false }] })
            ),
            Err(RuntimeError::Transaction(_))
        ));
        assert!(crate::xpath::is_list_supported(
            "[Name = '[%CurrentUser%]' or Owner = $currentUser]"
        ));
        // Spelled otherwise, the token would be compared as text.
        assert!(!crate::xpath::is_list_supported(
            "[Owner = '[%currentuser%]']"
        ));
    }

    /// A list sorts by a total order: absent and empty alike first, then
    /// booleans, numbers, dates and texts as a person reads them.
    #[test]
    fn members_sort_in_a_total_order() {
        use std::cmp::Ordering::*;
        let null = Value::Null;
        assert_eq!(compare_members(None, Some(&null)), Equal);
        assert_eq!(compare_members(Some(&null), None), Equal);
        let values = [
            Value::Bool(true),
            Value::from(2),
            Value::from(10),
            Value::from("apple"),
            Value::from("Banana"),
        ];
        for (index, left) in values.iter().enumerate() {
            for (other, right) in values.iter().enumerate() {
                assert_eq!(
                    compare_members(Some(left), Some(right)),
                    index.cmp(&other),
                    "{left} {right}"
                );
            }
        }
    }
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
