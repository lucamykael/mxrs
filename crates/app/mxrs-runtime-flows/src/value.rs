//! Runtime values flowing through microflow variables.
//!
//! Ports the value semantics mxrb's native interpreter inherits from Ruby:
//! integers and decimals compare and mix across types, `+` concatenates
//! strings but never mixes strings with numbers, everything except `false`
//! and `empty` is truthy, and an unsupported combination is an error, never
//! a guess.

use std::collections::BTreeMap;
use std::time::{SystemTime, UNIX_EPOCH};

use mxrs_runtime::DATETIME_MEMBER_PREFIX;
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
    /// An opaque JSON payload (REST mapping results without an entity,
    /// JSON-object action arguments). mxrb holds the parsed Ruby hash the
    /// same way: expressions cannot traverse it, but it survives binding
    /// and serializes back out intact.
    Json(Value),
}

/// The type a request's text is read as, for a published REST operation's
/// path or query parameter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequestValue {
    /// Text as it is: a string, or an enumeration value's name.
    Text,
    /// A whole number of 32 bits.
    Integer,
    /// A whole number of 64 bits.
    Long,
    Decimal,
    Boolean,
    /// An ISO 8601 date or UTC date-time.
    DateTime,
}

impl RequestValue {
    /// What a value of the kind is, in a sentence: `an integer`.
    pub fn description(self) -> &'static str {
        match self {
            RequestValue::Text => "text",
            RequestValue::Integer => "an integer",
            RequestValue::Long => "a long integer",
            RequestValue::Decimal => "a decimal",
            RequestValue::Boolean => "a boolean",
            RequestValue::DateTime => "an ISO 8601 date",
        }
    }
}

impl FlowValue {
    /// The value a request's `text` is for a parameter of `kind`: empty
    /// when the request carries none, and why not when the text is not one.
    pub fn from_request(text: Option<&str>, kind: RequestValue) -> Result<FlowValue, String> {
        let Some(text) = text else {
            return Ok(FlowValue::Empty);
        };
        // A typed value said empty is no value, as one left out is.
        if kind != RequestValue::Text && text.trim().is_empty() {
            return Ok(FlowValue::Empty);
        }
        let refused = || format!("{text:?} is not {}", kind.description());
        Ok(match kind {
            RequestValue::Text => FlowValue::String(text.to_string()),
            RequestValue::Integer => FlowValue::Int(i64::from(
                text.trim().parse::<i32>().map_err(|_| refused())?,
            )),
            RequestValue::Long => FlowValue::Int(text.trim().parse().map_err(|_| refused())?),
            RequestValue::Decimal => FlowValue::Float(
                text.trim()
                    .parse::<f64>()
                    .ok()
                    .filter(|value| value.is_finite())
                    .ok_or_else(refused)?,
            ),
            RequestValue::Boolean => match text.trim().to_ascii_lowercase().as_str() {
                "true" => FlowValue::Bool(true),
                "false" => FlowValue::Bool(false),
                _ => return Err(refused()),
            },
            RequestValue::DateTime => {
                FlowValue::DateTime(crate::datetime::parse_iso(text).ok_or_else(refused)?)
            }
        })
    }

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
            FlowValue::Json(value) => value.to_string(),
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
            FlowValue::Json(_) => "json",
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

    /// Ruby `/` on Integers floors toward negative infinity. `i64::MIN / -1`
    /// overflows and is refused, never a panic.
    pub fn divide(&self, other: &FlowValue) -> Result<FlowValue, FlowError> {
        if let (FlowValue::Int(left), FlowValue::Int(right)) = (self, other) {
            if *right == 0 {
                return Err(FlowError::unsupported_expression("divided by 0"));
            }
            let quotient = left
                .checked_div(*right)
                .ok_or_else(|| FlowError::unsupported_expression("integer overflow"))?;
            let remainder = left
                .checked_rem(*right)
                .ok_or_else(|| FlowError::unsupported_expression("integer overflow"))?;
            let floored = if remainder != 0 && (*left < 0) != (*right < 0) {
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
            FlowValue::Int(value) => value
                .checked_neg()
                .map(FlowValue::Int)
                .ok_or_else(|| FlowError::unsupported_expression("integer overflow")),
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
            FlowValue::Json(value) => value.clone(),
        }
    }

    /// Store members hold JSON; this is the inverse direction for reads.
    /// A member tagged by [`FlowValue::to_member`] comes back as a
    /// [`FlowValue::DateTime`], so `formatDateTime($object/When, …)` works on
    /// stored values the way mxrb's live `Time` members do. Every other
    /// string stays a string, whatever it looks like.
    pub fn from_member(value: &Value) -> FlowValue {
        match value {
            Value::Null => FlowValue::Empty,
            Value::Bool(value) => FlowValue::Bool(*value),
            Value::Number(value) => value
                .as_i64()
                .map(FlowValue::Int)
                .or_else(|| value.as_f64().map(FlowValue::Float))
                .unwrap_or(FlowValue::Empty),
            Value::String(value) => match untag_datetime(value) {
                Some(seconds) => FlowValue::DateTime(seconds),
                None => FlowValue::String(value.clone()),
            },
            Value::Array(values) => {
                FlowValue::List(values.iter().map(FlowValue::from_member).collect())
            }
            Value::Object(_) => FlowValue::Json(value.clone()),
        }
    }

    /// The member payload written back into the store. Object references are
    /// stored as id strings and lists of objects as id arrays — the store's
    /// own association representation (`reference_ids`) — and datetimes carry
    /// [`DATETIME_MEMBER_PREFIX`] so reading them back cannot confuse them
    /// with text that merely looks like a timestamp.
    pub fn to_member(&self) -> Value {
        match self {
            FlowValue::Object(reference) => Value::String(reference.id.clone()),
            FlowValue::List(values) => {
                Value::Array(values.iter().map(FlowValue::to_member).collect())
            }
            FlowValue::DateTime(seconds) => {
                Value::String(format!("{DATETIME_MEMBER_PREFIX}{seconds}"))
            }
            other => other.to_json_shallow(),
        }
    }
}

/// Reads a tagged datetime member back to epoch seconds.
fn untag_datetime(text: &str) -> Option<f64> {
    let seconds: f64 = text.strip_prefix(DATETIME_MEMBER_PREFIX)?.parse().ok()?;
    seconds.is_finite().then_some(seconds)
}

/// Renders a stored member as the JSON a caller sees, undoing the datetime
/// tagging. Used wherever raw member maps cross a runtime boundary, so the
/// storage encoding never escapes into a response body.
pub fn member_to_json(value: &Value) -> Value {
    match value {
        Value::String(text) => match untag_datetime(text) {
            Some(seconds) => Value::String(crate::datetime::to_string(seconds)),
            None => value.clone(),
        },
        Value::Array(values) => Value::Array(values.iter().map(member_to_json).collect()),
        other => other.clone(),
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

    /// Every integer edge that would panic in debug and wrap in release is a
    /// named error instead. `i64::MIN / -1` and `-i64::MIN` are the two whole
    /// families of that: their true result is not representable.
    #[test]
    fn integer_overflow_edges_are_errors_and_never_panics() {
        let min = FlowValue::Int(i64::MIN);
        let minus_one = FlowValue::Int(-1);
        for result in [
            min.divide(&minus_one),
            min.negate(),
            min.subtract(&FlowValue::Int(1)),
            FlowValue::Int(i64::MAX).add(&FlowValue::Int(1)),
            min.multiply(&minus_one),
        ] {
            assert_eq!(
                result.unwrap_err(),
                FlowError::unsupported_expression("integer overflow")
            );
        }
        // Flooring division still works everywhere it is representable.
        assert_eq!(
            FlowValue::Int(-7).divide(&FlowValue::Int(2)).unwrap(),
            FlowValue::Int(-4)
        );
        assert_eq!(
            FlowValue::Int(i64::MIN).divide(&FlowValue::Int(1)).unwrap(),
            FlowValue::Int(i64::MIN)
        );
    }

    #[test]
    fn json_payloads_survive_a_store_round_trip_intact() {
        let payload = serde_json::json!({ "id": 7, "tags": ["a", "b"], "nested": { "ok": true } });
        let value = FlowValue::Json(payload.clone());
        let member = value.to_member();
        assert_eq!(member, payload);
        assert_eq!(FlowValue::from_member(&member), value);
        assert_eq!(value.to_json_shallow(), payload);
    }

    /// The whole point of tagging: a datetime round-trips as a datetime, and
    /// text that merely *looks* like one stays text. Sniffing used to make
    /// `$o/Note = '2025-09-21 13:54:56 UTC'` false and `<` an error.
    #[test]
    fn datetime_members_are_tagged_and_lookalike_text_is_left_alone() {
        let when = FlowValue::DateTime(1_758_462_896.0);
        let member = when.to_member();
        assert_eq!(
            member,
            Value::String(format!("{DATETIME_MEMBER_PREFIX}1758462896"))
        );
        assert_eq!(FlowValue::from_member(&member), when);

        let lookalike = FlowValue::String("2025-09-21 13:54:56 UTC".into());
        let stored = lookalike.to_member();
        assert_eq!(FlowValue::from_member(&stored), lookalike);

        // Rendering boundaries still emit the human form, never the tag.
        assert_eq!(when.mendix_string(), "2025-09-21 13:54:56 UTC");
        assert_eq!(
            when.to_json_shallow(),
            Value::String("2025-09-21 13:54:56 UTC".into())
        );
        assert_eq!(
            member_to_json(&member),
            Value::String("2025-09-21 13:54:56 UTC".into())
        );
        assert_eq!(member_to_json(&stored), stored);
        // Lists of datetimes detag element-wise.
        assert_eq!(
            member_to_json(&Value::Array(vec![member.clone()])),
            Value::Array(vec![Value::String("2025-09-21 13:54:56 UTC".into())])
        );
        // A truncated or non-numeric tag is text, not a silent zero.
        assert_eq!(
            FlowValue::from_member(&Value::String(DATETIME_MEMBER_PREFIX.into())),
            FlowValue::String(DATETIME_MEMBER_PREFIX.into())
        );
    }
}

#[cfg(test)]
mod request_tests {
    use super::*;

    /// A request's text is read as its parameter's type; text that is no
    /// value of it says why, and no text is empty.
    #[test]
    fn a_request_value_is_read_as_its_type() {
        let read = FlowValue::from_request;
        assert_eq!(
            read(Some("42"), RequestValue::Integer),
            Ok(FlowValue::Int(42))
        );
        assert_eq!(
            read(Some("2.5"), RequestValue::Decimal),
            Ok(FlowValue::Float(2.5))
        );
        assert_eq!(
            read(Some("TRUE"), RequestValue::Boolean),
            Ok(FlowValue::Bool(true))
        );
        assert_eq!(
            read(Some("1970-01-02"), RequestValue::DateTime),
            Ok(FlowValue::DateTime(86_400.0))
        );
        assert_eq!(
            read(Some("abc"), RequestValue::Text),
            Ok(FlowValue::String("abc".into()))
        );
        assert_eq!(read(None, RequestValue::Integer), Ok(FlowValue::Empty));
        assert_eq!(read(Some(""), RequestValue::Integer), Ok(FlowValue::Empty));
        assert!(read(Some("3000000000"), RequestValue::Integer).is_err());
        assert_eq!(
            read(Some("3000000000"), RequestValue::Long),
            Ok(FlowValue::Int(3_000_000_000))
        );
        assert!(read(Some("NaN"), RequestValue::Decimal).is_err());
        assert!(read(Some("2026-02-31"), RequestValue::DateTime).is_err());
        assert_eq!(
            read(Some("abc"), RequestValue::Integer),
            Err("\"abc\" is not an integer".to_string())
        );
    }
}
