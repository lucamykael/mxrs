//! Typed marshalling between the authoring type markers and the runtime's
//! [`FlowValue`] — the value half of the service and action ports described
//! in this project's ports design.
//!
//! The `Mx*` types (`MxString`, `MxBool`, …) are phantom markers: they carry
//! no data and exist to type the authoring boundary. At a port boundary the
//! hand-written side needs real Rust values, so [`PortValue`] associates
//! each marker with its runtime representation and the two conversions:
//!
//! | marker | runtime representation |
//! |---|---|
//! | `MxString` | `String` |
//! | `MxBool` | `bool` |
//! | `MxInteger` | `i32` |
//! | `MxLong` | `i64` |
//! | `MxFloat` / `MxDecimal` | `f64` |
//! | `MxDateTime` | `f64` seconds since the Unix epoch, UTC |
//! | `MxObject<M>` | [`ObjectHandle<M>`] |
//! | `MxList<M>` | `Vec<ObjectHandle<M>>` |
//!
//! `MxBinary` and the bare `MxEnumeration` marker have no port
//! representation yet: binaries do not travel through microflow variables,
//! and enumeration values surface as their member-name strings until typed
//! enum ports land.
//!
//! Mendix `empty` never converts into a bare runtime value — model
//! optionality explicitly with [`PortValue::to_flow_optional`] and
//! [`PortValue::from_flow_optional`], which map `empty` to `None`.
//!
//! The flow-engine surface a port implementation talks to is re-exported
//! here ([`FlowEngine`], [`Variables`], [`JavaAction`], [`Adapter`], …), so
//! hand-written code can already drive a flow with typed values:
//!
//! ```
//! use mxrs::ports::{FlowEngine, PortValue, Variables};
//! use mxrs::prelude::*;
//! # fn call(engine: &FlowEngine, store: &mut mxrs::Store) -> Result<String, Box<dyn std::error::Error>> {
//! let mut arguments = Variables::new();
//! arguments.insert("input".to_string(), MxString::to_flow("value".into()));
//! let (result, _) = engine.call(store, "App.Echo", arguments, None)?;
//! Ok(MxString::from_flow(result)?)
//! # }
//! ```

use std::fmt;
use std::marker::PhantomData;

use mxrs_expr::{
    MendixType, MxBool, MxDateTime, MxDecimal, MxFloat, MxInteger, MxList, MxLong, MxObject,
    MxString,
};
use mxrs_ir::EntityMarker;
pub use mxrs_runtime_flows::{
    Adapter, AdapterKind, Execution, FlowEngine, FlowError, FlowValue, JavaAction, ObjectRef,
    Variables,
};

/// A typed handle to one runtime object: the `(entity, id)` pair the flow
/// engine's variables hold, carrying the entity as a compile-time marker
/// instead of a string. Members are read through the runtime store, which
/// observes the same uncommitted state the engine does.
pub struct ObjectHandle<M: EntityMarker> {
    id: String,
    marker: PhantomData<M>,
}

impl<M: EntityMarker> ObjectHandle<M> {
    /// Wraps a store object id as a handle to an `M` instance.
    pub fn new(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            marker: PhantomData,
        }
    }

    /// The store object id this handle points at.
    pub fn id(&self) -> &str {
        &self.id
    }

    /// The qualified `Module.Entity` name of the handle's entity type.
    pub fn entity() -> String {
        M::qualified_name()
    }
}

impl<M: EntityMarker> Clone for ObjectHandle<M> {
    fn clone(&self) -> Self {
        Self::new(self.id.clone())
    }
}

impl<M: EntityMarker> fmt::Debug for ObjectHandle<M> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ObjectHandle")
            .field("entity", &M::qualified_name())
            .field("id", &self.id)
            .finish()
    }
}

impl<M: EntityMarker> PartialEq for ObjectHandle<M> {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}

impl<M: EntityMarker> Eq for ObjectHandle<M> {}

/// A [`FlowValue`] did not have the shape a port's declared type requires.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PortValueError {
    expected: String,
    found: String,
}

impl PortValueError {
    fn new(expected: impl Into<String>, found: impl Into<String>) -> Self {
        Self {
            expected: expected.into(),
            found: found.into(),
        }
    }

    fn mismatch(expected: impl Into<String>, found: &FlowValue) -> Self {
        Self::new(expected, found.kind())
    }

    /// What the port's declared type required.
    pub fn expected(&self) -> &str {
        &self.expected
    }

    /// What the runtime value actually was.
    pub fn found(&self) -> &str {
        &self.found
    }
}

impl fmt::Display for PortValueError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "expected {}, found {}", self.expected, self.found)
    }
}

impl std::error::Error for PortValueError {}

/// The runtime representation of one authoring type marker, and the two
/// conversions a generated port needs at its boundary.
pub trait PortValue: MendixType {
    /// The Rust value hand-written code works with for this Mendix type.
    type Runtime;

    /// Lowers a runtime value into the engine's [`FlowValue`].
    fn to_flow(value: Self::Runtime) -> FlowValue;

    /// Recovers the runtime value, rejecting any other [`FlowValue`] shape
    /// — including `empty`; see [`PortValue::from_flow_optional`].
    fn from_flow(value: FlowValue) -> Result<Self::Runtime, PortValueError>;

    /// Lowers an optional value; `None` becomes Mendix `empty`.
    fn to_flow_optional(value: Option<Self::Runtime>) -> FlowValue {
        match value {
            Some(value) => Self::to_flow(value),
            None => FlowValue::Empty,
        }
    }

    /// Recovers an optional value; Mendix `empty` becomes `None`.
    fn from_flow_optional(value: FlowValue) -> Result<Option<Self::Runtime>, PortValueError> {
        match value {
            FlowValue::Empty => Ok(None),
            other => Self::from_flow(other).map(Some),
        }
    }
}

impl PortValue for MxString {
    type Runtime = String;

    fn to_flow(value: Self::Runtime) -> FlowValue {
        FlowValue::String(value)
    }

    fn from_flow(value: FlowValue) -> Result<Self::Runtime, PortValueError> {
        match value {
            FlowValue::String(value) => Ok(value),
            other => Err(PortValueError::mismatch("string", &other)),
        }
    }
}

impl PortValue for MxBool {
    type Runtime = bool;

    fn to_flow(value: Self::Runtime) -> FlowValue {
        FlowValue::Bool(value)
    }

    fn from_flow(value: FlowValue) -> Result<Self::Runtime, PortValueError> {
        match value {
            FlowValue::Bool(value) => Ok(value),
            other => Err(PortValueError::mismatch("boolean", &other)),
        }
    }
}

impl PortValue for MxInteger {
    type Runtime = i32;

    fn to_flow(value: Self::Runtime) -> FlowValue {
        FlowValue::Int(i64::from(value))
    }

    fn from_flow(value: FlowValue) -> Result<Self::Runtime, PortValueError> {
        match value {
            FlowValue::Int(value) => i32::try_from(value).map_err(|_| {
                PortValueError::new("integer", format!("long {value} out of integer range"))
            }),
            other => Err(PortValueError::mismatch("integer", &other)),
        }
    }
}

impl PortValue for MxLong {
    type Runtime = i64;

    fn to_flow(value: Self::Runtime) -> FlowValue {
        FlowValue::Int(value)
    }

    fn from_flow(value: FlowValue) -> Result<Self::Runtime, PortValueError> {
        match value {
            FlowValue::Int(value) => Ok(value),
            other => Err(PortValueError::mismatch("long", &other)),
        }
    }
}

/// Floats and decimals accept integer runtime values too — the engine mixes
/// the two numeric kinds exactly like mxrb inherits from Ruby.
macro_rules! float_port {
    ($marker:ty, $expected:literal) => {
        impl PortValue for $marker {
            type Runtime = f64;

            fn to_flow(value: Self::Runtime) -> FlowValue {
                FlowValue::Float(value)
            }

            fn from_flow(value: FlowValue) -> Result<Self::Runtime, PortValueError> {
                match value {
                    FlowValue::Float(value) => Ok(value),
                    FlowValue::Int(value) => Ok(value as f64),
                    other => Err(PortValueError::mismatch($expected, &other)),
                }
            }
        }
    };
}

float_port!(MxFloat, "float");
float_port!(MxDecimal, "decimal");

impl PortValue for MxDateTime {
    /// Seconds since the Unix epoch, UTC — the engine's own representation.
    type Runtime = f64;

    fn to_flow(value: Self::Runtime) -> FlowValue {
        FlowValue::DateTime(value)
    }

    fn from_flow(value: FlowValue) -> Result<Self::Runtime, PortValueError> {
        match value {
            FlowValue::DateTime(value) => Ok(value),
            other => Err(PortValueError::mismatch("datetime", &other)),
        }
    }
}

impl<M: EntityMarker> PortValue for MxObject<M> {
    type Runtime = ObjectHandle<M>;

    fn to_flow(value: Self::Runtime) -> FlowValue {
        FlowValue::Object(ObjectRef {
            entity: M::qualified_name(),
            id: value.id,
        })
    }

    fn from_flow(value: FlowValue) -> Result<Self::Runtime, PortValueError> {
        match value {
            FlowValue::Object(reference) if reference.entity == M::qualified_name() => {
                Ok(ObjectHandle::new(reference.id))
            }
            FlowValue::Object(reference) => Err(PortValueError::new(
                format!("object of {}", M::qualified_name()),
                format!("object of {}", reference.entity),
            )),
            other => Err(PortValueError::mismatch(
                format!("object of {}", M::qualified_name()),
                &other,
            )),
        }
    }
}

impl<M: EntityMarker> PortValue for MxList<M> {
    type Runtime = Vec<ObjectHandle<M>>;

    fn to_flow(value: Self::Runtime) -> FlowValue {
        FlowValue::List(
            value
                .into_iter()
                .map(<MxObject<M> as PortValue>::to_flow)
                .collect(),
        )
    }

    fn from_flow(value: FlowValue) -> Result<Self::Runtime, PortValueError> {
        match value {
            FlowValue::List(values) => values
                .into_iter()
                .map(<MxObject<M> as PortValue>::from_flow)
                .collect(),
            other => Err(PortValueError::mismatch(
                format!("list of {}", M::qualified_name()),
                &other,
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Order;
    impl EntityMarker for Order {
        const MODULE: &'static str = "Sales";
        const NAME: &'static str = "Order";
    }
    struct Customer;
    impl EntityMarker for Customer {
        const MODULE: &'static str = "Sales";
        const NAME: &'static str = "Customer";
    }

    #[test]
    fn scalars_round_trip_and_reject_other_shapes() {
        assert_eq!(
            MxString::from_flow(MxString::to_flow("a".into())).unwrap(),
            "a"
        );
        assert!(MxBool::from_flow(MxBool::to_flow(true)).unwrap());
        assert_eq!(MxInteger::from_flow(MxInteger::to_flow(7)).unwrap(), 7);
        assert_eq!(
            MxLong::from_flow(MxLong::to_flow(1 << 40)).unwrap(),
            1 << 40
        );
        assert_eq!(MxFloat::from_flow(MxFloat::to_flow(1.5)).unwrap(), 1.5);
        assert_eq!(MxDecimal::from_flow(FlowValue::Int(3)).unwrap(), 3.0);
        assert_eq!(
            MxDateTime::from_flow(MxDateTime::to_flow(1000.5)).unwrap(),
            1000.5
        );

        let error = MxString::from_flow(FlowValue::Bool(true)).unwrap_err();
        assert_eq!(error.expected(), "string");
        assert_eq!(error.found(), "boolean");
        assert_eq!(error.to_string(), "expected string, found boolean");
    }

    #[test]
    fn integer_rejects_out_of_range_longs() {
        let error = MxInteger::from_flow(FlowValue::Int(i64::from(i32::MAX) + 1)).unwrap_err();
        assert_eq!(error.expected(), "integer");
        assert!(error.found().contains("out of integer range"), "{error}");
    }

    #[test]
    fn empty_only_converts_through_the_optional_helpers() {
        assert!(MxString::from_flow(FlowValue::Empty).is_err());
        assert_eq!(
            MxString::from_flow_optional(FlowValue::Empty).unwrap(),
            None
        );
        assert_eq!(
            MxString::from_flow_optional(FlowValue::String("a".into())).unwrap(),
            Some("a".to_string())
        );
        assert_eq!(MxString::to_flow_optional(None), FlowValue::Empty);
        assert_eq!(
            MxString::to_flow_optional(Some("a".into())),
            FlowValue::String("a".into())
        );
    }

    #[test]
    fn object_handles_carry_the_entity_and_reject_mismatches() {
        let handle = ObjectHandle::<Order>::new("id-1");
        let value = <MxObject<Order>>::to_flow(handle.clone());
        assert_eq!(
            value,
            FlowValue::Object(ObjectRef {
                entity: "Sales.Order".into(),
                id: "id-1".into()
            })
        );
        assert_eq!(<MxObject<Order>>::from_flow(value.clone()).unwrap(), handle);

        let error = <MxObject<Customer>>::from_flow(value).unwrap_err();
        assert_eq!(error.expected(), "object of Sales.Customer");
        assert_eq!(error.found(), "object of Sales.Order");

        let error = <MxObject<Order>>::from_flow(FlowValue::Int(1)).unwrap_err();
        assert_eq!(error.found(), "integer");
    }

    #[test]
    fn lists_convert_each_element_and_fail_on_the_first_mismatch() {
        let handles = vec![
            ObjectHandle::<Order>::new("a"),
            ObjectHandle::<Order>::new("b"),
        ];
        let value = <MxList<Order>>::to_flow(handles.clone());
        assert_eq!(<MxList<Order>>::from_flow(value).unwrap(), handles);

        let mixed = FlowValue::List(vec![
            FlowValue::Object(ObjectRef {
                entity: "Sales.Order".into(),
                id: "a".into(),
            }),
            FlowValue::String("not an object".into()),
        ]);
        assert!(<MxList<Order>>::from_flow(mixed).is_err());
        assert!(<MxList<Order>>::from_flow(FlowValue::Int(2)).is_err());
    }
}
