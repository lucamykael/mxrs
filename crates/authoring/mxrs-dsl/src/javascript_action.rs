//! A code action's contract, as a module declares it:
//!
//! ```
//! # use mxrs_dsl::{ModuleBuilder, MxBool, MxString};
//! # let mut module = ModuleBuilder::new("Sales");
//! module.javascript_action("OpenMap", |action| {
//!     action.takes::<MxString>("Address").returns::<MxBool>();
//! });
//! module.java_action("Pad", |action| {
//!     action.takes::<MxString>("Value").returns::<MxString>();
//! });
//! ```

use mxrs_expr::{
    MxBool, MxDateTime, MxDecimal, MxFloat, MxInteger, MxList, MxLong, MxObject, MxString,
};
use mxrs_ir::{
    CodeActionParameter, CodeActionType, EntityMarker, ExportLevel, JavaActionDecl,
    JavaScriptActionDecl, JavaScriptPlatform,
};

/// A value a code action can take or return.
pub trait CodeActionValue {
    fn code_action_type() -> CodeActionType;
}

macro_rules! code_action_values {
    ($($ty:ty => $variant:ident),+ $(,)?) => {$(
        impl CodeActionValue for $ty {
            fn code_action_type() -> CodeActionType {
                CodeActionType::$variant
            }
        }
    )+};
}

code_action_values!(
    MxString => String,
    MxBool => Boolean,
    MxInteger => Integer,
    MxLong => Integer,
    MxDecimal => Decimal,
    MxFloat => Decimal,
    MxDateTime => DateTime,
);

impl<M: EntityMarker> CodeActionValue for MxObject<M> {
    fn code_action_type() -> CodeActionType {
        CodeActionType::Object(M::qualified_name())
    }
}

impl<M: EntityMarker> CodeActionValue for MxList<M> {
    fn code_action_type() -> CodeActionType {
        CodeActionType::List(M::qualified_name())
    }
}

/// What a Java and a JavaScript action's builders say alike.
macro_rules! code_action_builder {
    ($builder:ident, $decl:ty) => {
        impl $builder {
            pub(crate) fn new(name: impl Into<String>) -> Self {
                Self {
                    decl: <$decl>::new(name),
                }
            }

            pub fn documentation(&mut self, value: impl Into<String>) -> &mut Self {
                self.decl.documentation = value.into();
                self
            }

            /// A parameter a call must give: `takes::<MxObject<Order>>("Order")`.
            pub fn takes<T: CodeActionValue>(&mut self, name: impl Into<String>) -> &mut Self {
                self.takes_type(name, T::code_action_type())
            }

            /// A parameter a call may leave out.
            pub fn takes_optional<T: CodeActionValue>(
                &mut self,
                name: impl Into<String>,
            ) -> &mut Self {
                self.takes_optional_type(name, T::code_action_type())
            }

            /// A parameter of a type no Rust type names, such as an enumeration's.
            pub fn takes_type(&mut self, name: impl Into<String>, ty: CodeActionType) -> &mut Self {
                self.decl
                    .parameters
                    .push(CodeActionParameter::new(name, ty));
                self
            }

            /// A parameter a call may leave out, of a type no Rust type names.
            pub fn takes_optional_type(
                &mut self,
                name: impl Into<String>,
                ty: CodeActionType,
            ) -> &mut Self {
                let mut parameter = CodeActionParameter::new(name, ty);
                parameter.required = false;
                self.decl.parameters.push(parameter);
                self
            }

            /// What the action returns, of a type no Rust type names.
            pub fn returns_type(&mut self, ty: CodeActionType) -> &mut Self {
                self.decl.return_type = ty;
                self
            }

            /// What the action returns; nothing unless said.
            pub fn returns<T: CodeActionValue>(&mut self) -> &mut Self {
                self.decl.return_type = T::code_action_type();
                self
            }

            /// The name a call gives what the action returns unless it says another.
            pub fn return_name(&mut self, value: impl Into<String>) -> &mut Self {
                self.decl.return_name = value.into();
                self
            }

            pub fn excluded(&mut self, value: bool) -> &mut Self {
                self.decl.excluded = value;
                self
            }

            pub fn export_level(&mut self, value: ExportLevel) -> &mut Self {
                self.decl.export_level = value;
                self
            }

            pub(crate) fn into_decl(self) -> $decl {
                self.decl
            }
        }
    };
}

pub struct JavaScriptActionBuilder {
    decl: JavaScriptActionDecl,
}

code_action_builder!(JavaScriptActionBuilder, JavaScriptActionDecl);

impl JavaScriptActionBuilder {
    /// Where the action runs; both web and native unless said.
    pub fn platform(&mut self, value: JavaScriptPlatform) -> &mut Self {
        self.decl.platform = value;
        self
    }
}

pub struct JavaActionBuilder {
    decl: JavaActionDecl,
}

code_action_builder!(JavaActionBuilder, JavaActionDecl);
