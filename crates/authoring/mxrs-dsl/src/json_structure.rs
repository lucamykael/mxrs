//! A JSON structure, as a module declares it: its snippet, and what differs
//! from the elements Studio Pro derives from it.
//!
//! ```
//! # use mxrs_dsl::ModuleBuilder;
//! # let mut module = ModuleBuilder::new("Sales");
//! module.json_structure("JSON_Order", r#"{"number": "A-1", "lines": [{"sku": ""}]}"#, |json| {
//!     json.element("(Object)|number", |element| {
//!         element.max_length(20);
//!     });
//! });
//! ```

use mxrs_ir::{ExportLevel, JsonElement, JsonPrimitiveType, JsonStructureDecl};

pub struct JsonStructureBuilder {
    decl: JsonStructureDecl,
    qualified: String,
}

impl JsonStructureBuilder {
    pub(crate) fn new(module: &str, name: String, snippet: impl Into<String>) -> Self {
        let qualified = format!("{module}.{name}");
        let decl = JsonStructureDecl::derive(name, snippet)
            .unwrap_or_else(|error| panic!("JSON structure {qualified}: {error}"));
        Self { decl, qualified }
    }

    pub fn documentation(&mut self, value: impl Into<String>) -> &mut Self {
        self.decl.documentation = value.into();
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

    /// Changes the element at `path` — `(Object)|lines|(Object)|sku` — from
    /// what the snippet derives.
    ///
    /// # Panics
    ///
    /// When the snippet has no element at `path`.
    pub fn element(
        &mut self,
        path: &str,
        configure: impl FnOnce(&mut JsonElementBuilder<'_>),
    ) -> &mut Self {
        let qualified = &self.qualified;
        let element = self.decl.element_mut(path).unwrap_or_else(|| {
            panic!("JSON structure {qualified}: its snippet has no element at `{path}`")
        });
        configure(&mut JsonElementBuilder { element });
        self
    }

    pub(crate) fn into_decl(self) -> JsonStructureDecl {
        self.decl
    }
}

/// What a declaration changes of one element.
pub struct JsonElementBuilder<'a> {
    element: &'a mut JsonElement,
}

impl JsonElementBuilder<'_> {
    /// The name a mapping knows the element by.
    pub fn exposed_name(&mut self, value: impl Into<String>) -> &mut Self {
        self.element.exposed_name = value.into();
        self
    }

    pub fn exposed_item_name(&mut self, value: impl Into<String>) -> &mut Self {
        self.element.exposed_item_name = value.into();
        self
    }

    pub fn primitive_type(&mut self, value: JsonPrimitiveType) -> &mut Self {
        self.element.primitive_type = value;
        self
    }

    pub fn max_length(&mut self, value: i32) -> &mut Self {
        self.element.max_length = value;
        self
    }

    pub fn min_occurs(&mut self, value: i32) -> &mut Self {
        self.element.min_occurs = value;
        self
    }

    /// `-1`: as many as there are.
    pub fn max_occurs(&mut self, value: i32) -> &mut Self {
        self.element.max_occurs = value;
        self
    }

    pub fn nillable(&mut self, value: bool) -> &mut Self {
        self.element.nillable = value;
        self
    }

    pub fn is_default_type(&mut self, value: bool) -> &mut Self {
        self.element.is_default_type = value;
        self
    }

    pub fn fraction_digits(&mut self, value: i32) -> &mut Self {
        self.element.fraction_digits = value;
        self
    }

    pub fn total_digits(&mut self, value: i32) -> &mut Self {
        self.element.total_digits = value;
        self
    }

    /// The value the snippet gives, as JSON writes it.
    pub fn original_value(&mut self, value: impl Into<String>) -> &mut Self {
        self.element.original_value = value.into();
        self
    }

    pub fn error_message(&mut self, value: impl Into<String>) -> &mut Self {
        self.element.error_message = value.into();
        self
    }

    pub fn warning_message(&mut self, value: impl Into<String>) -> &mut Self {
        self.element.warning_message = value.into();
        self
    }
}
