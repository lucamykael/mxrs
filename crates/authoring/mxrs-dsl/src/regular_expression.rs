use mxrs_ir::{ExportLevel, RegularExpressionDecl};

/// Builder for an editable Mendix regular-expression document.
pub struct RegularExpressionBuilder {
    declaration: RegularExpressionDecl,
}

impl RegularExpressionBuilder {
    pub(crate) fn new(name: impl Into<String>, expression: impl Into<String>) -> Self {
        Self {
            declaration: RegularExpressionDecl::new(name, expression),
        }
    }

    pub(crate) fn into_decl(self) -> RegularExpressionDecl {
        self.declaration
    }

    pub fn documentation(&mut self, text: impl Into<String>) -> &mut Self {
        self.declaration.documentation = text.into();
        self
    }

    pub fn excluded(&mut self, excluded: bool) -> &mut Self {
        self.declaration.excluded = excluded;
        self
    }

    pub fn export_level(&mut self, level: ExportLevel) -> &mut Self {
        self.declaration.export_level = level;
        self
    }
}
