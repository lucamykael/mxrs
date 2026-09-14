use mxrs_ir::{ConstantDecl, ConstantType};

/// Builder for one editable `Constants$Constant` document.
///
/// The value is always given as a string because that is how Mendix persists
/// `DefaultValue`; [`ConstantBuilder::value_type`] selects the declared type
/// independently. The two are not cross-checked — see [`ConstantDecl::value`].
pub struct ConstantBuilder {
    decl: ConstantDecl,
}

impl ConstantBuilder {
    pub(crate) fn new(name: impl Into<String>) -> Self {
        Self {
            decl: ConstantDecl::new(name, ConstantType::default()),
        }
    }

    pub(crate) fn into_decl(self) -> ConstantDecl {
        self.decl
    }

    pub fn documentation(&mut self, text: impl Into<String>) -> &mut Self {
        self.decl.documentation = text.into();
        self
    }

    pub fn value_type(&mut self, constant_type: ConstantType) -> &mut Self {
        self.decl.constant_type = constant_type;
        self
    }

    pub fn value(&mut self, value: impl Into<String>) -> &mut Self {
        self.decl.value = value.into();
        self
    }

    /// Makes the constant readable from client-side logic. Off by default,
    /// matching both Studio Pro and mxrb: a constant often holds a secret, so
    /// exposing it is an explicit act.
    pub fn exposed_to_client(&mut self, exposed: bool) -> &mut Self {
        self.decl.exposed_to_client = exposed;
        self
    }
}
