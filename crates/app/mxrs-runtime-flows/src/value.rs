//! Runtime values flowing through microflow variables.
//!
//! Ports the value semantics mxrb's native interpreter inherits from Ruby:
//! integers and decimals compare and mix across types, `+` concatenates
//! strings but never mixes strings with numbers, everything except `false`
//! and `empty` is truthy, and an unsupported combination is an error, never
//! a guess.

use std::collections::BTreeMap;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;

use crate::FlowError;

/// A reference to a store object. mxrb variables hold live Ruby records;
/// this port keeps (entity, id) and reads members through the store, which
/// observes exactly the same uncommitted state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObjectRef {
    pub entity: String,
    pub id: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum FlowValue {
    /// Mendix `empty` / Ruby `nil`.
    Empty,
    Bool(bool),
    Int(i64),
    Float(f64),
    String(String),
    /// Seconds since the Unix epoch, UTC. mxrb uses the local clock; a pure
    /// std port formats in UTC — the only visible divergence is the zone of
    /// `formatDateTime([%CurrentDateTime%], …)` output.
    DateTime(f64),
    Object(ObjectRef),
    List(Vec<FlowValue>),
}

impl FlowValue {
    pub fn now() -> Self {
        let seconds = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|elapsed| elapsed.as_secs_f64())
            .unwrap_or(0.0);
        FlowValue::DateTime(seconds)
    }

    /// Ruby truthiness: only `false` and `nil` are false.
    pub fn truthy(&self) -> bool {
        !matches!(self, FlowValue::Bool(false) | FlowValue::Empty)
    }

    /// mxrb's `mendix_string`: Ruby `to_s` for the supported value kinds.
    pub fn mendix_string(&self) -> String {
        match self {
            FlowValue::Empty => String::new(),
            FlowValue::Bool(value) => value.to_string(),
            FlowValue::Int(value) => value.to_string(),
            FlowValue::Float(value) => float_to_string(*value),
            FlowValue::String(value) => value.clone(),
            FlowValue::DateTime(seconds) => crate::datetime::to_string(*seconds),
            FlowValue::Object(reference) => format!("{}/{}", reference.entity, reference.id),
            FlowValue::List(values) => format!("[{} value(s)]", values.len()),
        }
    }

    pub fn equals(&self, other: &FlowValue) -> bool {
        match (self, other) {
            (FlowValue::Int(left), FlowValue::Float(right))
            | (FlowValue::Float(right), FlowValue::Int(left)) => (*left as f64) == *right,
            (left, right) => left == right,
        }
    }

    pub fn compare(&self, other: &FlowValue) -> Result<std::cmp::Ordering, FlowError> {
        let unsupported = || {
            FlowError::unsupported_expression(format!(
                "cannot compare {} and {}",
                self.kind(),
                other.kind()
            ))
        };
        match (self, other) {
            (FlowValue::Int(left), FlowValue::Int(right)) => Ok(left.cmp(right)),
            (FlowValue::String(left), FlowValue::String(right)) => Ok(left.cmp(right)),
            (FlowValue::DateTime(left), FlowValue::DateTime(right)) => {
                left.partial_cmp(right).ok_or_else(unsupported)
            }
            (left, right) => {
                let (Some(left), Some(right)) = (left.as_float(), right.as_float()) else {
                    return Err(unsupported());
                };
                left.partial_cmp(&right).ok_or_else(unsupported)
            }
        }
    }

    pub fn as_float(&self) -> Option<f64> {
        match self {
            FlowValue::Int(value) => Some(*value as f64),
            FlowValue::Float(value) => Some(*value),
            _ => None,
        }
    }

    pub fn kind(&self) -> &'static str {
        match self {
            FlowValue::Empty => "empty",
            FlowValue::Bool(_) => "boolean",
            FlowValue::Int(_) => "integer",
            FlowValue::Float(_) => "decimal",
            FlowValue::String(_) => "string",
            FlowValue::DateTime(_) => "datetime",
            FlowValue::Object(_) => "object",
            FlowValue::List(_) => "list",
        }
    }

    pub fn add(&self, other: &FlowValue) -> Result<FlowValue, FlowError> {
        match (self, other) {
            (FlowValue::String(left), FlowValue::String(right)) => {
                Ok(FlowValue::String(format!("{left}{right}")))
            }
            (FlowValue::Int(left), FlowValue::Int(right)) => left
                .checked_add(*right)
                .map(FlowValue::Int)
                .ok_or_else(|| FlowError::unsupported_expression("integer overflow")),
            _ => self.numeric(other, "+", |left, right| left + right),
        }
    }

    pub fn subtract(&self, other: &FlowValue) -> Result<FlowValue, FlowError> {
        if let (FlowValue::Int(left), FlowValue::Int(right)) = (self, other) {
            return left
                .checked_sub(*right)
                .map(FlowValue::Int)
                .ok_or_else(|| FlowError::unsupported_expression("integer overflow"));
        }
        self.numeric(other, "-", |left, right| left - right)
    }

    pub fn multiply(&self, other: &FlowValue) -> Result<FlowValue, FlowError> {
        if let (FlowValue::Int(left), FlowValue::Int(right)) = (self, other) {
            return left
                .checked_mul(*right)
                .map(FlowValue::Int)
                .ok_or_else(|| FlowError::unsupported_expression("integer overflow"));
        }
        self.numeric(other, "*", |left, right| left * right)
    }

    /// Ruby `/` on Integers floors toward negative infinity.
    pub fn divide(&self, other: &FlowValue) -> Result<FlowValue, FlowError> {
        if let (FlowValue::Int(left), FlowValue::Int(right)) = (self, other) {
            if *right == 0 {
                return Err(FlowError::unsupported_expression("divided by 0"));
            }
            let quotient = left / right;
            let floored = if left % right != 0 && (*left < 0) != (*right < 0) {
                quotient - 1
            } else {
                quotient
            };
            return Ok(FlowValue::Int(floored));
        }
        self.numeric(other, "/", |left, right| left / right)
    }

    pub fn negate(&self) -> Result<FlowValue, FlowError> {
        match self {
            FlowValue::Int(value) => Ok(FlowValue::Int(-value)),
            FlowValue::Float(value) => Ok(FlowValue::Float(-value)),
            _ => Err(FlowError::unsupported_expression(format!(
                "cannot negate {}",
                self.kind()
            ))),
        }
    }

    fn numeric(
        &self,
        other: &FlowValue,
        operator: &str,
        apply: impl Fn(f64, f64) -> f64,
    ) -> Result<FlowValue, FlowError> {
        let (Some(left), Some(right)) = (self.as_float(), other.as_float()) else {
            return Err(FlowError::unsupported_expression(format!(
                "cannot apply {operator} to {} and {}",
                self.kind(),
                other.kind()
            )));
        };
        Ok(FlowValue::Float(apply(left, right)))
    }

    /// The JSON rendering used at runtime boundaries (HTTP action results,
    /// REST bodies). Object references render as their member map would in
    /// mxrb's `runtime_json` when a store is available — callers that need
    /// members expanded use [`crate::FlowEngine::value_to_json`].
    pub fn to_json_shallow(&self) -> Value {
        match self {
            FlowValue::Empty => Value::Null,
            FlowValue::Bool(value) => Value::Bool(*value),
            FlowValue::Int(value) => Value::from(*value),
            FlowValue::Float(value) => serde_json::Number::from_f64(*value)
                .map(Value::Number)
                .unwrap_or(Value::Null),
            FlowValue::String(value) => Value::String(value.clone()),
            FlowValue::DateTime(seconds) => Value::String(crate::datetime::to_string(*seconds)),
            FlowValue::Object(reference) => Value::String(reference.id.clone()),
            FlowValue::List(values) => {
                Value::Array(values.iter().map(FlowValue::to_json_shallow).collect())
            }
        }
    }

    /// Store members hold JSON; this is the inverse direction for reads.
    pub fn from_member(value: &Value) -> FlowValue {
        match value {
            Value::Null => FlowValue::Empty,
            Value::Bool(value) => FlowValue::Bool(*value),
            Value::Number(value) => value
                .as_i64()
                .map(FlowValue::Int)
                .or_else(|| value.as_f64().map(FlowValue::Float))
                .unwrap_or(FlowValue::Empty),
            Value::String(value) => FlowValue::String(value.clone()),
            Value::Array(values) => {
                FlowValue::List(values.iter().map(FlowValue::from_member).collect())
            }
            Value::Object(_) => FlowValue::Empty,
        }
    }

    /// The member payload written back into the store. Object references are
    /// stored as id strings and lists of objects as id arrays — the store's
    /// own association representation (`reference_ids`).
    pub fn to_member(&self) -> Value {
        match self {
            FlowValue::Object(reference) => Value::String(reference.id.clone()),
            FlowValue::List(values) => {
                Value::Array(values.iter().map(FlowValue::to_member).collect())
            }
            other => other.to_json_shallow(),
        }
    }
}

pub type Variables = BTreeMap<String, FlowValue>;

/// Ruby `Float#to_s`: integral floats keep one decimal (`2.0`), everything
/// else uses the shortest representation.
fn float_to_string(value: f64) -> String {
    if value.is_finite() && value.fract() == 0.0 && value.abs() < 1e15 {
        format!("{value:.1}")
    } else {
        value.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ruby_like_arithmetic_equality_and_truthiness() {
        assert!(FlowValue::Int(1).equals(&FlowValue::Float(1.0)));
        assert!(!FlowValue::Int(1).equals(&FlowValue::String("1".into())));
        assert_eq!(
            FlowValue::Int(2).add(&FlowValue::Int(3)).unwrap(),
            FlowValue::Int(5)
        );
        assert_eq!(
            FlowValue::Int(2).add(&FlowValue::Float(0.5)).unwrap(),
            FlowValue::Float(2.5)
        );
        assert_eq!(
            FlowValue::String("a".into())
                .add(&FlowValue::String("b".into()))
                .unwrap(),
            FlowValue::String("ab".into())
        );
        assert!(
            FlowValue::String("a".into())
                .add(&FlowValue::Int(1))
                .is_err()
        );
        assert_eq!(
            FlowValue::Int(7).divide(&FlowValue::Int(2)).unwrap(),
            FlowValue::Int(3)
        );
        assert!(FlowValue::Int(7).divide(&FlowValue::Int(0)).is_err());
        assert!(FlowValue::Int(0).truthy());
        assert!(!FlowValue::Empty.truthy());
        assert!(!FlowValue::Bool(false).truthy());
        assert_eq!(FlowValue::Float(2.0).mendix_string(), "2.0");
        assert_eq!(FlowValue::Float(2.5).mendix_string(), "2.5");
        assert_eq!(FlowValue::Bool(true).mendix_string(), "true");
    }
}
