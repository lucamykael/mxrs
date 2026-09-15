//! Typed Mendix expressions for the authoring boundary.
//!
//! Values constructed here render to canonical Mendix expression source,
//! while their phantom type prevents invalid comparisons and assignments
//! from reaching the storage-independent IR as unchecked strings.

use std::fmt;
use std::marker::PhantomData;

use mxrs_ir::flow::FlowReturnType;
use mxrs_ir::{AssociationMarker, AttributeMarker, EntityMarker, Member};

pub trait MendixType: 'static {}

macro_rules! mendix_types {
    ($($name:ident),+ $(,)?) => {$(
        #[derive(Debug, Clone, Copy)]
        pub struct $name;
        impl MendixType for $name {}
    )+};
}

mendix_types!(
    MxString,
    MxInteger,
    MxLong,
    MxFloat,
    MxDecimal,
    MxBool,
    MxDateTime,
    MxBinary,
    MxEnumeration,
);

#[derive(Debug, Clone, Copy)]
pub struct MxObject<M: EntityMarker>(PhantomData<M>);
impl<M: EntityMarker> MendixType for MxObject<M> {}

#[derive(Debug, Clone, Copy)]
pub struct MxList<M: EntityMarker>(PhantomData<M>);
impl<M: EntityMarker> MendixType for MxList<M> {}

/// An attribute marker whose Mendix value type is known.
pub trait TypedAttributeMarker: AttributeMarker {
    type Value: MendixType;
}

#[derive(Debug, PartialEq, Eq)]
pub struct Expr<T: MendixType> {
    source: String,
    marker: PhantomData<T>,
}

impl<T: MendixType> Clone for Expr<T> {
    fn clone(&self) -> Self {
        Self::new(self.source.clone())
    }
}

impl<T: MendixType> Expr<T> {
    fn new(source: impl Into<String>) -> Self {
        Self {
            source: source.into(),
            marker: PhantomData,
        }
    }

    pub fn variable(name: impl AsRef<str>) -> Self {
        Self::new(format!("${}", name.as_ref()))
    }

    pub fn eq(self, other: impl IntoExpr<T>) -> Expr<MxBool> {
        binary(self, "=", other.into_expr())
    }

    pub fn ne(self, other: impl IntoExpr<T>) -> Expr<MxBool> {
        binary(self, "!=", other.into_expr())
    }
}

impl<T: OrderedType> Expr<T> {
    pub fn gt(self, other: impl IntoExpr<T>) -> Expr<MxBool> {
        binary(self, ">", other.into_expr())
    }

    pub fn ge(self, other: impl IntoExpr<T>) -> Expr<MxBool> {
        binary(self, ">=", other.into_expr())
    }

    pub fn lt(self, other: impl IntoExpr<T>) -> Expr<MxBool> {
        binary(self, "<", other.into_expr())
    }

    pub fn le(self, other: impl IntoExpr<T>) -> Expr<MxBool> {
        binary(self, "<=", other.into_expr())
    }
}

impl Expr<MxString> {
    pub fn contains(self, needle: impl IntoExpr<MxString>) -> Expr<MxBool> {
        Expr::new(format!(
            "contains({}, {})",
            self.source,
            needle.into_expr().source
        ))
    }

    pub fn is_empty(self) -> Expr<MxBool> {
        Expr::new(format!("isEmpty({})", self.source))
    }
}

impl Expr<MxBool> {
    pub fn and(self, other: impl IntoExpr<MxBool>) -> Self {
        binary(self, "and", other.into_expr())
    }

    pub fn or(self, other: impl IntoExpr<MxBool>) -> Self {
        binary(self, "or", other.into_expr())
    }

    #[must_use]
    pub fn negate(self) -> Self {
        Self::new(format!("not({})", self.source))
    }
}

impl<T: MendixType> fmt::Display for Expr<T> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.source)
    }
}

pub trait IntoExpr<T: MendixType> {
    fn into_expr(self) -> Expr<T>;
}

impl<T: MendixType> IntoExpr<T> for Expr<T> {
    fn into_expr(self) -> Expr<T> {
        self
    }
}

impl<T: MendixType> IntoExpr<T> for &Expr<T> {
    fn into_expr(self) -> Expr<T> {
        self.clone()
    }
}

pub trait RenderExpr {
    fn render(&self) -> String;
}

/// An expression whose Mendix persistence return type is known. Flow end
/// events need both source text and this metadata; source text alone cannot
/// distinguish e.g. Integer from Long or an object from a list.
pub trait TypedRenderExpr: RenderExpr {
    fn flow_return_type(&self) -> FlowReturnType;
}

pub trait MendixReturnType: MendixType {
    fn flow_return_type() -> FlowReturnType;
}

/// Chooses the authoring variable returned by a flow call: an expression for
/// scalars, an entity variable for objects, or an iterable variable for lists.
pub trait FlowResultType: MendixReturnType {
    type Variable: TypedRenderExpr;
    fn result_variable(name: String) -> Self::Variable;
}

macro_rules! primitive_return_types {
    ($($ty:ty => $variant:ident),+ $(,)?) => {
        $(
            impl MendixReturnType for $ty {
                fn flow_return_type() -> FlowReturnType {
                    FlowReturnType::$variant
                }
            }
            impl FlowResultType for $ty {
                type Variable = Expr<Self>;
                fn result_variable(name: String) -> Self::Variable { Expr::variable(name) }
            }
        )+
    };
}

primitive_return_types!(
    MxString => String,
    MxInteger => Integer,
    MxLong => Long,
    MxFloat => Float,
    MxDecimal => Decimal,
    MxBool => Boolean,
    MxDateTime => DateTime,
    MxBinary => Binary,
);

impl<M: EntityMarker> MendixReturnType for MxObject<M> {
    fn flow_return_type() -> FlowReturnType {
        FlowReturnType::Object(M::qualified_name())
    }
}

impl<M: EntityMarker> MendixReturnType for MxList<M> {
    fn flow_return_type() -> FlowReturnType {
        FlowReturnType::List(M::qualified_name())
    }
}

impl<M: EntityMarker> FlowResultType for MxObject<M> {
    type Variable = Var<M>;
    fn result_variable(name: String) -> Self::Variable {
        Var::new(name)
    }
}

impl<M: EntityMarker> FlowResultType for MxList<M> {
    type Variable = ListVar<M>;
    fn result_variable(name: String) -> Self::Variable {
        ListVar::new(name)
    }
}

impl<T: MendixType> RenderExpr for Expr<T> {
    fn render(&self) -> String {
        self.source.clone()
    }
}

impl<T: MendixReturnType> TypedRenderExpr for Expr<T> {
    fn flow_return_type(&self) -> FlowReturnType {
        T::flow_return_type()
    }
}

pub trait OrderedType: MendixType {}
impl OrderedType for MxString {}
impl OrderedType for MxInteger {}
impl OrderedType for MxLong {}
impl OrderedType for MxFloat {}
impl OrderedType for MxDecimal {}
impl OrderedType for MxDateTime {}

fn binary<T: MendixType, U: MendixType>(left: Expr<T>, op: &str, right: Expr<T>) -> Expr<U> {
    Expr::new(format!("({} {op} {})", left.source, right.source))
}

pub fn string(value: impl AsRef<str>) -> Expr<MxString> {
    let escaped = value.as_ref().replace('\'', "''");
    Expr::new(format!("'{escaped}'"))
}

pub fn integer(value: i32) -> Expr<MxInteger> {
    Expr::new(value.to_string())
}

pub fn long(value: i64) -> Expr<MxLong> {
    Expr::new(value.to_string())
}

pub fn float(value: f64) -> Expr<MxFloat> {
    Expr::new(format_number(value))
}

pub fn decimal(value: f64) -> Expr<MxDecimal> {
    Expr::new(format_number(value))
}

pub fn boolean(value: bool) -> Expr<MxBool> {
    Expr::new(value.to_string())
}

fn format_number(value: f64) -> String {
    let rendered = value.to_string();
    if rendered.contains(['.', 'e', 'E']) {
        rendered
    } else {
        format!("{rendered}.0")
    }
}

impl IntoExpr<MxString> for &str {
    fn into_expr(self) -> Expr<MxString> {
        string(self)
    }
}

impl IntoExpr<MxString> for String {
    fn into_expr(self) -> Expr<MxString> {
        string(self)
    }
}

impl IntoExpr<MxBool> for bool {
    fn into_expr(self) -> Expr<MxBool> {
        boolean(self)
    }
}

impl IntoExpr<MxInteger> for i32 {
    fn into_expr(self) -> Expr<MxInteger> {
        integer(self)
    }
}

impl IntoExpr<MxLong> for i64 {
    fn into_expr(self) -> Expr<MxLong> {
        long(self)
    }
}

impl IntoExpr<MxFloat> for f64 {
    fn into_expr(self) -> Expr<MxFloat> {
        float(self)
    }
}

impl IntoExpr<MxDecimal> for f64 {
    fn into_expr(self) -> Expr<MxDecimal> {
        decimal(self)
    }
}

#[derive(Debug)]
pub struct Var<M: EntityMarker> {
    name: String,
    marker: PhantomData<M>,
}

impl<M: EntityMarker> Var<M> {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            marker: PhantomData,
        }
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn attribute<A>(&self) -> Expr<A::Value>
    where
        A: TypedAttributeMarker<Entity = M>,
    {
        Expr::new(format!("${}/{}", self.name, A::NAME))
    }

    pub fn expression(&self) -> Expr<MxObject<M>> {
        Expr::new(format!("${}", self.name))
    }
}

impl<M: EntityMarker> Clone for Var<M> {
    fn clone(&self) -> Self {
        Self::new(self.name.clone())
    }
}

impl<M: EntityMarker> RenderExpr for Var<M> {
    fn render(&self) -> String {
        format!("${}", self.name)
    }
}

impl<M: EntityMarker> TypedRenderExpr for Var<M> {
    fn flow_return_type(&self) -> FlowReturnType {
        FlowReturnType::Object(M::qualified_name())
    }
}

impl<M: EntityMarker> IntoExpr<MxObject<M>> for Var<M> {
    fn into_expr(self) -> Expr<MxObject<M>> {
        self.expression()
    }
}

impl<M: EntityMarker> IntoExpr<MxObject<M>> for &Var<M> {
    fn into_expr(self) -> Expr<MxObject<M>> {
        self.expression()
    }
}

#[derive(Debug)]
pub struct ListVar<M: EntityMarker> {
    name: String,
    marker: PhantomData<M>,
}

impl<M: EntityMarker> ListVar<M> {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            marker: PhantomData,
        }
    }

    pub fn name(&self) -> &str {
        &self.name
    }
}

impl<M: EntityMarker> RenderExpr for ListVar<M> {
    fn render(&self) -> String {
        format!("${}", self.name)
    }
}

impl<M: EntityMarker> Clone for ListVar<M> {
    fn clone(&self) -> Self {
        Self::new(self.name.clone())
    }
}

impl<M: EntityMarker> IntoExpr<MxList<M>> for ListVar<M> {
    fn into_expr(self) -> Expr<MxList<M>> {
        Expr::variable(self.name)
    }
}

impl<M: EntityMarker> IntoExpr<MxList<M>> for &ListVar<M> {
    fn into_expr(self) -> Expr<MxList<M>> {
        Expr::variable(&self.name)
    }
}

impl<T: TypedRenderExpr + ?Sized> TypedRenderExpr for &T {
    fn flow_return_type(&self) -> FlowReturnType {
        T::flow_return_type(self)
    }
}

impl<T: RenderExpr + ?Sized> RenderExpr for &T {
    fn render(&self) -> String {
        T::render(self)
    }
}

impl<M: EntityMarker> TypedRenderExpr for ListVar<M> {
    fn flow_return_type(&self) -> FlowReturnType {
        FlowReturnType::List(M::qualified_name())
    }
}

pub struct MemberAssignment<M: EntityMarker> {
    member: Member,
    marker: PhantomData<M>,
}

impl<M: EntityMarker> MemberAssignment<M> {
    pub fn into_member(self) -> Member {
        self.member
    }
}

pub fn attribute<A>(value: impl IntoExpr<A::Value>) -> MemberAssignment<A::Entity>
where
    A: TypedAttributeMarker,
{
    MemberAssignment {
        member: Member::attribute(A::NAME, value.into_expr().to_string()),
        marker: PhantomData,
    }
}

pub fn association<A>(value: &Var<A::To>) -> MemberAssignment<A::From>
where
    A: AssociationMarker,
{
    MemberAssignment {
        member: Member::association(A::NAME, value.render()),
        marker: PhantomData,
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

    struct Total;
    impl AttributeMarker for Total {
        type Entity = Order;
        const NAME: &'static str = "Total";
    }
    impl TypedAttributeMarker for Total {
        type Value = MxDecimal;
    }

    #[test]
    fn renders_literals_and_typed_member_comparisons_canonically() {
        assert_eq!(string("O'Reilly").to_string(), "'O''Reilly'");
        let order = Var::<Order>::new("order");
        assert_eq!(
            order.attribute::<Total>().gt(100.0).to_string(),
            "($order/Total > 100.0)"
        );
    }
}
