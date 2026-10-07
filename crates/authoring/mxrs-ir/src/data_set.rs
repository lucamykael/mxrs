//! Data sets: an OQL query a report runs, with the parameters it takes and
//! the module roles that may run it.
//!
//! What a role may do with a parameter is derived as Studio Pro derives it:
//! an object parameter carries its one constraint — that it is not empty —
//! disabled for every role, and a parameter of another type carries none.

use crate::{ExportLevel, NativeDocument, NativeValue};

/// What a data set's parameter is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DataSetParameterType {
    /// An object of the entity, by its qualified name.
    Object(String),
    /// A value of the enumeration, by its qualified name.
    Enumeration(String),
}

impl DataSetParameterType {
    fn document(&self) -> NativeDocument {
        match self {
            DataSetParameterType::Object(entity) => {
                NativeDocument::new("DataTypes$ObjectType").with("Entity", entity.as_str())
            }
            DataSetParameterType::Enumeration(enumeration) => {
                NativeDocument::new("DataTypes$EnumerationType")
                    .with("Enumeration", enumeration.as_str())
            }
        }
    }

    fn from_document(document: &NativeDocument) -> Option<Self> {
        match document.ty.as_str() {
            "DataTypes$ObjectType" => Some(Self::Object(document.text("Entity")?.to_string())),
            "DataTypes$EnumerationType" => {
                Some(Self::Enumeration(document.text("Enumeration")?.to_string()))
            }
            _ => None,
        }
    }
}

/// One parameter of a data set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DataSetParameter {
    pub name: String,
    pub ty: DataSetParameterType,
}

/// A data set of a module.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DataSetDecl {
    pub name: String,
    pub documentation: String,
    /// The OQL query, as written.
    pub query: String,
    pub parameters: Vec<DataSetParameter>,
    /// The module roles that may run it, by their qualified names.
    pub roles: Vec<String>,
    pub excluded: bool,
    pub export_level: ExportLevel,
}

impl DataSetDecl {
    pub fn new(name: impl Into<String>, query: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            documentation: String::new(),
            query: query.into(),
            parameters: Vec::new(),
            roles: Vec::new(),
            excluded: false,
            export_level: ExportLevel::Hidden,
        }
    }

    /// The document the model stores for the data set, its fields in the
    /// order Studio Pro stores them.
    pub fn document(&self) -> NativeDocument {
        let access = self
            .roles
            .iter()
            .map(|role| {
                let parameters = self
                    .parameters
                    .iter()
                    .filter(|parameter| matches!(parameter.ty, DataSetParameterType::Object(_)))
                    .map(|parameter| {
                        NativeValue::Document(
                            NativeDocument::new("DataSets$DataSetParameterAccess")
                                .with(
                                    "ConstraintAccessList",
                                    NativeValue::List(
                                        2,
                                        vec![NativeValue::Document(
                                            NativeDocument::new("DataSets$DataSetConstraintAccess")
                                                .with("ConstraintText", "(not empty)")
                                                .with("Enabled", false),
                                        )],
                                    ),
                                )
                                .with("ParameterName", parameter.name.as_str()),
                        )
                    })
                    .collect();
                NativeValue::Document(
                    NativeDocument::new("DataSets$DataSetModuleRoleAccess")
                        .with("ModuleRole", role.as_str())
                        .with("ParameterAccessList", NativeValue::List(2, parameters)),
                )
            })
            .collect();
        let parameters = self
            .parameters
            .iter()
            .map(|parameter| {
                NativeValue::Document(
                    NativeDocument::new("DataSets$DataSetParameter")
                        .with("Constraints", NativeValue::List(2, Vec::new()))
                        .with("Name", parameter.name.as_str())
                        .with("ParameterType", parameter.ty.document())
                        .with("ParameterTypeIsRange", false),
                )
            })
            .collect();
        NativeDocument::new("DataSets$DataSet")
            .with(
                "DataSetAccess",
                NativeDocument::new("DataSets$DataSetAccess")
                    .with("ModuleRoleAccessList", NativeValue::List(2, access)),
            )
            .with("Documentation", self.documentation.as_str())
            .with("Excluded", self.excluded)
            .with(
                "ExportLevel",
                match self.export_level {
                    ExportLevel::Hidden => "Hidden",
                    ExportLevel::Published => "Published",
                },
            )
            .with("Name", self.name.as_str())
            .with("Parameters", NativeValue::List(2, parameters))
            .with(
                "Source",
                NativeDocument::new("DataSets$OqlDataSetSource")
                    .with("IEIQ", false)
                    .with("Query", self.query.as_str()),
            )
    }

    /// The declaration a stored data set is, when [`Self::document`] states
    /// that document again.
    pub fn from_document(document: &NativeDocument) -> Option<Self> {
        if document.ty != "DataSets$DataSet" {
            return None;
        }
        let list = |value: &NativeValue| -> Option<Vec<NativeDocument>> {
            match value {
                NativeValue::List(2, items) => items
                    .iter()
                    .map(|item| match item {
                        NativeValue::Document(document) => Some(document.clone()),
                        _ => None,
                    })
                    .collect(),
                _ => None,
            }
        };
        let NativeValue::Document(source) = document.get("Source")? else {
            return None;
        };
        let NativeValue::Document(access) = document.get("DataSetAccess")? else {
            return None;
        };
        let mut declaration = Self::new(document.text("Name")?, source.text("Query")?);
        declaration.documentation = document.text("Documentation")?.to_string();
        declaration.excluded = matches!(document.get("Excluded")?, NativeValue::Bool(true));
        declaration.export_level = match document.text("ExportLevel")? {
            "Hidden" => ExportLevel::Hidden,
            "Published" => ExportLevel::Published,
            _ => return None,
        };
        for parameter in list(document.get("Parameters")?)? {
            let NativeValue::Document(ty) = parameter.get("ParameterType")? else {
                return None;
            };
            declaration.parameters.push(DataSetParameter {
                name: parameter.text("Name")?.to_string(),
                ty: DataSetParameterType::from_document(ty)?,
            });
        }
        for role in list(access.get("ModuleRoleAccessList")?)? {
            declaration.roles.push(role.text("ModuleRole")?.to_string());
        }
        (declaration.document() == document.narrowed()).then_some(declaration)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every role may pass every object parameter, its one constraint
    /// disabled; an enumeration parameter carries none.
    #[test]
    fn a_data_set_is_stored_as_studio_pro_stores_one() {
        let mut set = DataSetDecl::new("DataSet_Orders", "SELECT o.Number FROM Sales.Order AS o");
        set.parameters.push(DataSetParameter {
            name: "Customer".into(),
            ty: DataSetParameterType::Object("Sales.Customer".into()),
        });
        set.parameters.push(DataSetParameter {
            name: "Status".into(),
            ty: DataSetParameterType::Enumeration("Sales.Status".into()),
        });
        set.roles = vec!["Sales.User".into(), "Sales.Admin".into()];
        let document = set.document();
        let user = document
            .at("DataSetAccess.ModuleRoleAccessList[0]")
            .unwrap();
        assert_eq!(user.text("ModuleRole"), Some("Sales.User"));
        let Some(NativeValue::List(2, access)) = user.get("ParameterAccessList") else {
            panic!("a role's parameter access is a list");
        };
        assert_eq!(access.len(), 1);
        assert_eq!(
            document
                .at("DataSetAccess.ModuleRoleAccessList[1].ParameterAccessList[0].ConstraintAccessList[0]")
                .unwrap()
                .text("ConstraintText"),
            Some("(not empty)")
        );
        assert_eq!(DataSetDecl::from_document(&document), Some(set));
    }
}
