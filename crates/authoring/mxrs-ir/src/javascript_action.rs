//! JavaScript actions: code a nanoflow runs in the browser. The model
//! stores what the action takes and returns; its JavaScript is the
//! project's, in `javascriptsource/<module>/actions/<Name>.js`.

use crate::{ExportLevel, NativeDocument, NativeValue};

/// What a JavaScript action takes or returns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CodeActionType {
    Void,
    Boolean,
    DateTime,
    Decimal,
    Integer,
    String,
    /// An object of the entity, by its qualified name.
    Object(String),
    /// A list of objects of the entity.
    List(String),
    /// A value of the enumeration, by its qualified name.
    Enumeration(String),
}

impl CodeActionType {
    /// The type a stored document states, when it is one of these.
    pub fn from_document(document: &NativeDocument) -> Option<Self> {
        let entity = |document: &NativeDocument| document.text("Entity").map(str::to_string);
        Some(match document.ty.as_str() {
            "CodeActions$VoidType" => Self::Void,
            "CodeActions$BooleanType" => Self::Boolean,
            "CodeActions$DateTimeType" => Self::DateTime,
            "CodeActions$DecimalType" => Self::Decimal,
            "CodeActions$IntegerType" => Self::Integer,
            "CodeActions$StringType" => Self::String,
            "CodeActions$ConcreteEntityType" => Self::Object(entity(document)?),
            "CodeActions$ListType" => match document.get("Parameter")? {
                NativeValue::Document(parameter)
                    if parameter.ty == "CodeActions$ConcreteEntityType" =>
                {
                    Self::List(entity(parameter)?)
                }
                _ => return None,
            },
            "CodeActions$EnumerationType" => {
                Self::Enumeration(document.text("Enumeration")?.to_string())
            }
            _ => return None,
        })
    }

    /// The document the model stores for the type.
    pub fn document(&self) -> NativeDocument {
        let named = |ty: &str| NativeDocument::new(format!("CodeActions${ty}Type"));
        match self {
            CodeActionType::Void => named("Void"),
            CodeActionType::Boolean => named("Boolean"),
            CodeActionType::DateTime => named("DateTime"),
            CodeActionType::Decimal => named("Decimal"),
            CodeActionType::Integer => named("Integer"),
            CodeActionType::String => named("String"),
            CodeActionType::Object(entity) => {
                named("ConcreteEntity").with("Entity", entity.as_str())
            }
            CodeActionType::List(entity) => named("List").with(
                "Parameter",
                named("ConcreteEntity").with("Entity", entity.as_str()),
            ),
            CodeActionType::Enumeration(enumeration) => {
                named("Enumeration").with("Enumeration", enumeration.as_str())
            }
        }
    }
}

/// Where a JavaScript action runs: in the web client, a native mobile app,
/// or both.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum JavaScriptPlatform {
    #[default]
    All,
    Web,
    Native,
}

impl JavaScriptPlatform {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "All" => Some(Self::All),
            "Web" => Some(Self::Web),
            "Native" => Some(Self::Native),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            JavaScriptPlatform::All => "All",
            JavaScriptPlatform::Web => "Web",
            JavaScriptPlatform::Native => "Native",
        }
    }
}

/// One parameter of a JavaScript action.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JavaScriptActionParameter {
    pub name: String,
    pub ty: CodeActionType,
    pub description: String,
    pub category: String,
    pub required: bool,
}

impl JavaScriptActionParameter {
    pub fn new(name: impl Into<String>, ty: CodeActionType) -> Self {
        Self {
            name: name.into(),
            ty,
            description: String::new(),
            category: String::new(),
            required: true,
        }
    }

    fn from_document(document: &NativeDocument) -> Option<Self> {
        let ty = match document.get("ParameterType")? {
            NativeValue::Document(kind) if kind.ty == "CodeActions$BasicParameterType" => {
                match kind.get("Type")? {
                    NativeValue::Document(ty) => CodeActionType::from_document(ty)?,
                    _ => return None,
                }
            }
            _ => return None,
        };
        Some(Self {
            name: document.text("Name")?.to_string(),
            ty,
            description: document.text("Description")?.to_string(),
            category: document.text("Category")?.to_string(),
            required: match document.get("IsRequired")? {
                NativeValue::Bool(required) => *required,
                _ => return None,
            },
        })
    }

    fn document(&self) -> NativeDocument {
        NativeDocument::new("JavaScriptActions$JavaScriptActionParameter")
            .with("Category", self.category.as_str())
            .with("Description", self.description.as_str())
            .with("IsRequired", self.required)
            .with("Name", self.name.as_str())
            .with(
                "ParameterType",
                NativeDocument::new("CodeActions$BasicParameterType")
                    .with("Type", self.ty.document()),
            )
    }
}

/// A JavaScript action of a module: its name, what it takes and returns,
/// and where it runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JavaScriptActionDecl {
    pub name: String,
    pub documentation: String,
    pub parameters: Vec<JavaScriptActionParameter>,
    pub return_type: CodeActionType,
    /// The name a call gives the value it returns unless it says another.
    pub return_name: String,
    pub platform: JavaScriptPlatform,
    pub excluded: bool,
    pub export_level: ExportLevel,
}

impl JavaScriptActionDecl {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            documentation: String::new(),
            parameters: Vec::new(),
            return_type: CodeActionType::Void,
            return_name: "ReturnValueName".to_string(),
            platform: JavaScriptPlatform::All,
            excluded: false,
            export_level: ExportLevel::Hidden,
        }
    }

    /// The declaration a stored action is, when [`Self::document`] states
    /// that document again: an action this cannot say — a type parameter,
    /// a toolbox icon, a parameter of another kind — is none.
    pub fn from_document(document: &NativeDocument) -> Option<Self> {
        if document.ty != "JavaScriptActions$JavaScriptAction" {
            return None;
        }
        let parameters = match document.get("Parameters")? {
            NativeValue::List(_, items) => items
                .iter()
                .map(|item| match item {
                    NativeValue::Document(parameter) => {
                        JavaScriptActionParameter::from_document(parameter)
                    }
                    _ => None,
                })
                .collect::<Option<Vec<_>>>()?,
            _ => return None,
        };
        let return_type = match document.get("JavaReturnType")? {
            NativeValue::Document(ty) => CodeActionType::from_document(ty)?,
            _ => return None,
        };
        let declaration = Self {
            name: document.text("Name")?.to_string(),
            documentation: document.text("Documentation")?.to_string(),
            parameters,
            return_type,
            return_name: document.text("ActionDefaultReturnName")?.to_string(),
            platform: JavaScriptPlatform::parse(document.text("Platform")?)?,
            excluded: matches!(document.get("Excluded")?, NativeValue::Bool(true)),
            export_level: match document.text("ExportLevel")? {
                "Hidden" => ExportLevel::Hidden,
                "Published" => ExportLevel::Published,
                _ => return None,
            },
        };
        (declaration.document() == *document).then_some(declaration)
    }

    /// The document the model stores for the action, its fields in the
    /// order Mendix stores them.
    pub fn document(&self) -> NativeDocument {
        NativeDocument::new("JavaScriptActions$JavaScriptAction")
            .with("ActionDefaultReturnName", self.return_name.as_str())
            .with("Documentation", self.documentation.as_str())
            .with("Excluded", self.excluded)
            .with(
                "ExportLevel",
                match self.export_level {
                    ExportLevel::Hidden => "Hidden",
                    ExportLevel::Published => "Published",
                },
            )
            .with("JavaReturnType", self.return_type.document())
            .with("MicroflowActionInfo", NativeValue::Null)
            .with("Name", self.name.as_str())
            .with(
                "Parameters",
                NativeValue::List(
                    2,
                    self.parameters
                        .iter()
                        .map(|parameter| NativeValue::Document(parameter.document()))
                        .collect(),
                ),
            )
            .with("Platform", self.platform.as_str())
            .with("TypeParameters", NativeValue::List(2, Vec::new()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The document holds what Studio Pro stores for a new action, its
    /// fields in Studio Pro's order.
    #[test]
    fn an_action_is_stored_as_studio_pro_stores_one() {
        let mut action = JavaScriptActionDecl::new("OpenMap");
        action.parameters.push(JavaScriptActionParameter::new(
            "Address",
            CodeActionType::Object("Sales.Address".into()),
        ));
        action.return_type = CodeActionType::List("Sales.Pin".into());
        let document = action.document();
        assert_eq!(
            document
                .fields
                .iter()
                .map(|(key, _)| key.as_str())
                .collect::<Vec<_>>(),
            [
                "ActionDefaultReturnName",
                "Documentation",
                "Excluded",
                "ExportLevel",
                "JavaReturnType",
                "MicroflowActionInfo",
                "Name",
                "Parameters",
                "Platform",
                "TypeParameters",
            ]
        );
        let returned = document.at("JavaReturnType.Parameter").unwrap();
        assert_eq!(returned.ty, "CodeActions$ConcreteEntityType");
        let parameter = document.at("Parameters[0].ParameterType.Type").unwrap();
        assert_eq!(
            parameter.get("Entity"),
            Some(&NativeValue::Text("Sales.Address".into()))
        );
        // The document reads back as the declaration that states it; one
        // with a toolbox icon says what the declaration cannot.
        assert_eq!(JavaScriptActionDecl::from_document(&document), Some(action));
        let mut iconed = document.clone();
        iconed.set(
            "MicroflowActionInfo",
            NativeDocument::new("CodeActions$MicroflowActionInfo").with("Caption", "Open"),
        );
        assert_eq!(JavaScriptActionDecl::from_document(&iconed), None);
    }
}
