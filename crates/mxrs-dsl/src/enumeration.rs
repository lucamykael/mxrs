use mxrs_ir::{EnumerationDecl, EnumerationValueDecl};

/// Builder for one editable Mendix enumeration document.
pub struct EnumerationBuilder {
    decl: EnumerationDecl,
}

impl EnumerationBuilder {
    pub(crate) fn new(name: impl Into<String>) -> Self {
        Self {
            decl: EnumerationDecl::new(name),
        }
    }

    pub(crate) fn into_decl(self) -> EnumerationDecl {
        self.decl
    }

    pub fn documentation(&mut self, text: impl Into<String>) -> &mut Self {
        self.decl.documentation = text.into();
        self
    }

    pub fn value(&mut self, name: impl Into<String>) -> &mut EnumerationValueDecl {
        self.decl.values.push(EnumerationValueDecl::new(name));
        self.decl.values.last_mut().expect("just pushed")
    }
}
