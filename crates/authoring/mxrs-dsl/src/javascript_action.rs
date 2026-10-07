//! A JavaScript action's contract, as a module declares it:
//!
//! ```
//! # use mxrs_dsl::{ModuleBuilder, MxBool, MxString};
//! # let mut module = ModuleBuilder::new("Sales");
//! module.javascript_action("OpenMap", |action| {
//!     action.takes::<MxString>("Address").returns::<MxBool>();
//! });
//! ```

use mxrs_expr::{
    MxBool, MxDateTime, MxDecimal, MxFloat, MxInteger, MxList, MxLong, MxObject, MxString,
};
use mxrs_ir::{
    CodeActionType, EntityMarker, ExportLevel, JavaScriptActionDecl, JavaScriptActionParameter,
    JavaScriptPlatform,
};

/// A value a JavaScript action can take or return.
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

pub struct JavaScriptActionBuilder {
    decl: JavaScriptActionDecl,
}

impl JavaScriptActionBuilder {
    pub(crate) fn new(name: impl Into<String>) -> Self {
        Self {
            decl: JavaScriptActionDecl::new(name),
        }
    }

    pub fn documentation(&mut self, value: impl Into<String>) -> &mut Self {
        self.decl.documentation = value.into();
        self
    }

    /// Where the action runs; both web and native unless said.
    pub fn platform(&mut self, value: JavaScriptPlatform) -> &mut Self {
        self.decl.platform = value;
        self
    }

    /// A parameter a call must give: `takes::<MxObject<Order>>("Order")`.
    pub fn takes<T: CodeActionValue>(&mut self, name: impl Into<String>) -> &mut Self {
        self.takes_type(name, T::code_action_type())
    }

    /// A parameter a call may leave out.
    pub fn takes_optional<T: CodeActionValue>(&mut self, name: impl Into<String>) -> &mut Self {
        let mut parameter = JavaScriptActionParameter::new(name, T::code_action_type());
        parameter.required = false;
        self.decl.parameters.push(parameter);
        self
    }

    /// A parameter of a type no Rust type names, such as an enumeration's.
    pub fn takes_type(&mut self, name: impl Into<String>, ty: CodeActionType) -> &mut Self {
        self.decl
            .parameters
            .push(JavaScriptActionParameter::new(name, ty));
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

    pub(crate) fn into_decl(self) -> JavaScriptActionDecl {
        self.decl
    }
}
