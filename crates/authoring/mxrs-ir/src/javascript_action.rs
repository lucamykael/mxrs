//! Code actions: Java actions, which a microflow runs on the server, and
//! JavaScript actions, which a nanoflow runs in the browser. The model
//! stores what an action takes and returns; its code is the project's, in
//! `javasource/<module>/actions/<Name>.java` or
//! `javascriptsource/<module>/actions/<Name>.js`.

use crate::{ExportLevel, NativeDocument, NativeValue};

/// What a code action takes or returns.
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

/// One parameter of a code action.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodeActionParameter {
    pub name: String,
    pub ty: CodeActionType,
    pub description: String,
    pub category: String,
    pub required: bool,
}

impl CodeActionParameter {
    pub fn new(name: impl Into<String>, ty: CodeActionType) -> Self {
        Self {
            name: name.into(),
            ty,
            description: String::new(),
            category: String::new(),
            required: true,
        }
    }

    fn from_document(document: &NativeDocument, family: Family) -> Option<Self> {
        if document.ty != family.parameter() {
            return None;
        }
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

    fn document(&self, family: Family) -> NativeDocument {
        NativeDocument::new(family.parameter())
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
    pub parameters: Vec<CodeActionParameter>,
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
        let shape = Shape::from_document(document, Family::JavaScript)?;
        let declaration = Self {
            name: shape.name,
            documentation: shape.documentation,
            parameters: shape.parameters,
            return_type: shape.return_type,
            return_name: shape.return_name,
            platform: JavaScriptPlatform::parse(document.text("Platform")?)?,
            excluded: shape.excluded,
            export_level: shape.export_level,
        };
        (declaration.document() == *document).then_some(declaration)
    }

    /// The document the model stores for the action, its fields in the
    /// order Mendix stores them.
    pub fn document(&self) -> NativeDocument {
        Shape {
            name: self.name.clone(),
            documentation: self.documentation.clone(),
            parameters: self.parameters.clone(),
            return_type: self.return_type.clone(),
            return_name: self.return_name.clone(),
            excluded: self.excluded,
            export_level: self.export_level,
        }
        .document(Family::JavaScript, Some(self.platform))
    }
}

/// A Java action of a module: its name and what it takes and returns. Its
/// Java is the project's, and mxrs runs the Rust registered for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JavaActionDecl {
    pub name: String,
    pub documentation: String,
    pub parameters: Vec<CodeActionParameter>,
    pub return_type: CodeActionType,
    /// The name a call gives the value it returns unless it says another.
    pub return_name: String,
    pub excluded: bool,
    pub export_level: ExportLevel,
}

impl JavaActionDecl {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            documentation: String::new(),
            parameters: Vec::new(),
            return_type: CodeActionType::Void,
            return_name: "ReturnValueName".to_string(),
            excluded: false,
            export_level: ExportLevel::Hidden,
        }
    }

    /// The declaration a stored action is, when [`Self::document`] states
    /// that document again: an action this cannot say — a type parameter,
    /// a toolbox icon, a microflow parameter — is none.
    pub fn from_document(document: &NativeDocument) -> Option<Self> {
        let shape = Shape::from_document(document, Family::Java)?;
        let declaration = Self {
            name: shape.name,
            documentation: shape.documentation,
            parameters: shape.parameters,
            return_type: shape.return_type,
            return_name: shape.return_name,
            excluded: shape.excluded,
            export_level: shape.export_level,
        };
        (declaration.document() == *document).then_some(declaration)
    }

    /// The document the model stores for the action, its fields in the
    /// order Mendix stores them.
    pub fn document(&self) -> NativeDocument {
        Shape {
            name: self.name.clone(),
            documentation: self.documentation.clone(),
            parameters: self.parameters.clone(),
            return_type: self.return_type.clone(),
            return_name: self.return_name.clone(),
            excluded: self.excluded,
            export_level: self.export_level,
        }
        .document(Family::Java, None)
    }
}

/// Which kind of code action a document is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Family {
    Java,
    JavaScript,
}

impl Family {
    fn action(self) -> &'static str {
        match self {
            Family::Java => "JavaActions$JavaAction",
            Family::JavaScript => "JavaScriptActions$JavaScriptAction",
        }
    }

    fn parameter(self) -> &'static str {
        match self {
            Family::Java => "JavaActions$JavaActionParameter",
            Family::JavaScript => "JavaScriptActions$JavaScriptActionParameter",
        }
    }
}

/// What Java and JavaScript actions store alike.
struct Shape {
    name: String,
    documentation: String,
    parameters: Vec<CodeActionParameter>,
    return_type: CodeActionType,
    return_name: String,
    excluded: bool,
    export_level: ExportLevel,
}

impl Shape {
    fn from_document(document: &NativeDocument, family: Family) -> Option<Self> {
        if document.ty != family.action() {
            return None;
        }
        let parameters = match document.get("Parameters")? {
            NativeValue::List(_, items) => items
                .iter()
                .map(|item| match item {
                    NativeValue::Document(parameter) => {
                        CodeActionParameter::from_document(parameter, family)
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
        Some(Self {
            name: document.text("Name")?.to_string(),
            documentation: document.text("Documentation")?.to_string(),
            parameters,
            return_type,
            return_name: document.text("ActionDefaultReturnName")?.to_string(),
            excluded: matches!(document.get("Excluded")?, NativeValue::Bool(true)),
            export_level: match document.text("ExportLevel")? {
                "Hidden" => ExportLevel::Hidden,
                "Published" => ExportLevel::Published,
                _ => return None,
            },
        })
    }

    /// The document, a JavaScript action's with where it runs.
    fn document(self, family: Family, platform: Option<JavaScriptPlatform>) -> NativeDocument {
        let mut document = NativeDocument::new(family.action())
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
                        .map(|parameter| NativeValue::Document(parameter.document(family)))
                        .collect(),
                ),
            );
        if let Some(platform) = platform {
            document = document.with("Platform", platform.as_str());
        }
        document.with("TypeParameters", NativeValue::List(2, Vec::new()))
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
        action.parameters.push(CodeActionParameter::new(
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

    /// A Java action is stored as a JavaScript action is, without where it
    /// runs, and one kind never reads back as the other.
    #[test]
    fn a_java_action_is_stored_as_studio_pro_stores_one() {
        let mut action = JavaActionDecl::new("Pad");
        action
            .parameters
            .push(CodeActionParameter::new("Value", CodeActionType::String));
        action.return_type = CodeActionType::String;
        let document = action.document();
        assert_eq!(document.ty, "JavaActions$JavaAction");
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
                "TypeParameters",
            ]
        );
        assert_eq!(
            document.at("Parameters[0]").unwrap().ty,
            "JavaActions$JavaActionParameter"
        );
        assert_eq!(JavaActionDecl::from_document(&document), Some(action));
        assert_eq!(JavaScriptActionDecl::from_document(&document), None);
    }
}
