//! A data set, as a module declares it:
//!
//! ```
//! # use mxrs_dsl::ModuleBuilder;
//! # let mut module = ModuleBuilder::new("Sales");
//! module.data_set("DataSet_Orders", "SELECT o.Number FROM Sales.Order AS o", |set| {
//!     set.object_parameter("Customer", "Sales.Customer")
//!         .roles(["Sales.User", "Sales.Admin"]);
//! });
//! ```

use mxrs_ir::{DataSetDecl, DataSetParameter, DataSetParameterType, ExportLevel};

pub struct DataSetBuilder {
    decl: DataSetDecl,
}

impl DataSetBuilder {
    pub(crate) fn new(name: impl Into<String>, query: impl Into<String>) -> Self {
        Self {
            decl: DataSetDecl::new(name, query),
        }
    }

    /// A parameter that is an object of `entity`, by its qualified name.
    pub fn object_parameter(
        &mut self,
        name: impl Into<String>,
        entity: impl Into<String>,
    ) -> &mut Self {
        self.decl.parameters.push(DataSetParameter {
            name: name.into(),
            ty: DataSetParameterType::Object(entity.into()),
        });
        self
    }

    /// A parameter that is a value of `enumeration`, by its qualified name.
    pub fn enumeration_parameter(
        &mut self,
        name: impl Into<String>,
        enumeration: impl Into<String>,
    ) -> &mut Self {
        self.decl.parameters.push(DataSetParameter {
            name: name.into(),
            ty: DataSetParameterType::Enumeration(enumeration.into()),
        });
        self
    }

    /// The module roles that may run it, by their qualified names.
    pub fn roles<R: Into<String>>(&mut self, roles: impl IntoIterator<Item = R>) -> &mut Self {
        self.decl.roles.extend(roles.into_iter().map(Into::into));
        self
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

    pub(crate) fn into_decl(self) -> DataSetDecl {
        self.decl
    }
}
