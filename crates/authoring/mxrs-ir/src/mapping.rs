//! Import and export mappings: how the elements of a JSON structure become
//! objects of entities, and objects become JSON.
//!
//! A mapping's tree is its JSON structure's — each element's name, path,
//! occurrences and sample value are the structure's own — so a declaration
//! states only what the mapping adds: which entity an object is, through
//! which association it hangs off its parent, and which attribute a value
//! fills. [`MappingDecl::document`] writes the rest from the structure.

use crate::{ExportLevel, JsonElement, JsonElementType, JsonPrimitiveType, JsonStructureDecl};
use crate::{NativeDocument, NativeValue};

/// Which way a mapping goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MappingDirection {
    /// JSON into objects.
    Import,
    /// Objects into JSON.
    Export,
}

impl MappingDirection {
    fn prefix(self) -> &'static str {
        match self {
            MappingDirection::Import => "ImportMappings",
            MappingDirection::Export => "ExportMappings",
        }
    }

    fn document_type(self) -> String {
        match self {
            MappingDirection::Import => "ImportMappings$ImportMapping".to_string(),
            MappingDirection::Export => "ExportMappings$ExportMapping".to_string(),
        }
    }
}

/// How a mapping comes by the object of an element.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObjectHandling {
    Create,
    Find,
    /// The object the mapping is given: an export's root.
    Parameter,
    /// An export's backup when no object is found.
    Error,
    Custom,
}

impl ObjectHandling {
    pub fn as_str(self) -> &'static str {
        match self {
            ObjectHandling::Create => "Create",
            ObjectHandling::Find => "Find",
            ObjectHandling::Parameter => "Parameter",
            ObjectHandling::Error => "Error",
            ObjectHandling::Custom => "Custom",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "Create" => Self::Create,
            "Find" => Self::Find,
            "Parameter" => Self::Parameter,
            "Error" => Self::Error,
            "Custom" => Self::Custom,
            _ => return None,
        })
    }
}

/// What an export writes for an attribute without a value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum NullValueOption {
    #[default]
    LeaveOutElement,
    SendAsNil,
}

impl NullValueOption {
    pub fn as_str(self) -> &'static str {
        match self {
            NullValueOption::LeaveOutElement => "LeaveOutElement",
            NullValueOption::SendAsNil => "SendAsNil",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "LeaveOutElement" => Self::LeaveOutElement,
            "SendAsNil" => Self::SendAsNil,
            _ => return None,
        })
    }
}

/// The type of the attribute a value fills.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MappingValueType {
    String,
    Integer,
    Decimal,
    Boolean,
    DateTime,
    Enumeration(String),
}

impl MappingValueType {
    /// The type a value of `primitive` fills unless a mapping says another.
    pub fn of(primitive: JsonPrimitiveType) -> Self {
        match primitive {
            JsonPrimitiveType::Integer | JsonPrimitiveType::Long => Self::Integer,
            JsonPrimitiveType::Decimal => Self::Decimal,
            JsonPrimitiveType::Boolean => Self::Boolean,
            JsonPrimitiveType::DateTime => Self::DateTime,
            JsonPrimitiveType::String | JsonPrimitiveType::Unknown => Self::String,
        }
    }

    fn document(&self) -> NativeDocument {
        let named = |name: &str| NativeDocument::new(format!("DataTypes${name}Type"));
        match self {
            MappingValueType::String => named("String"),
            MappingValueType::Integer => named("Integer"),
            MappingValueType::Decimal => named("Decimal"),
            MappingValueType::Boolean => named("Boolean"),
            MappingValueType::DateTime => named("DateTime"),
            MappingValueType::Enumeration(enumeration) => {
                named("Enumeration").with("Enumeration", enumeration.as_str())
            }
        }
    }

    fn from_document(document: &NativeDocument) -> Option<Self> {
        let plain = document.fields.is_empty();
        Some(match document.ty.as_str() {
            "DataTypes$StringType" if plain => Self::String,
            "DataTypes$IntegerType" if plain => Self::Integer,
            "DataTypes$DecimalType" if plain => Self::Decimal,
            "DataTypes$BooleanType" if plain => Self::Boolean,
            "DataTypes$DateTimeType" if plain => Self::DateTime,
            "DataTypes$EnumerationType" if document.fields.len() == 1 => {
                Self::Enumeration(document.text("Enumeration")?.to_string())
            }
            _ => return None,
        })
    }
}

/// Through which association an object hangs off its parent's.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum MappingAssociation {
    /// `<Module>.<Entity>_<Parent>`, in the object's entity's module — the
    /// name Studio Pro gives the association it makes for the mapping.
    #[default]
    Named,
    /// None: a root, or an object under an array that is no entity.
    None,
    Other(String),
}

/// An element of the structure a mapping maps.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MappingElement {
    /// The last part of the element's path: a member's name, or
    /// `(Object)`, `(Array)`, `(Wrapper)`, `(Value)`.
    pub key: String,
    pub kind: MappingElementKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MappingElementKind {
    Object(ObjectMapping),
    Value(ValueMapping),
}

/// An object, array or wrapper of the structure, and the entity it is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObjectMapping {
    /// `None`: an array the mapping makes no object of.
    pub entity: Option<String>,
    pub association: MappingAssociation,
    /// `None`: the direction's own — an import creates, an export finds,
    /// and is given its root.
    pub handling: Option<ObjectHandling>,
    pub backup: Option<ObjectHandling>,
    pub allow_override: bool,
    pub children: Vec<MappingElement>,
}

impl ObjectMapping {
    pub fn new(entity: Option<String>) -> Self {
        Self {
            entity,
            association: MappingAssociation::Named,
            handling: None,
            backup: None,
            allow_override: false,
            children: Vec::new(),
        }
    }
}

/// A value of the structure, and the attribute it fills.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ValueMapping {
    /// `None`: the attribute named as the element is exposed.
    pub attribute: Option<String>,
    pub is_key: bool,
    /// The microflow that converts the value, by its qualified name.
    pub converter: String,
    /// `None`: the type the element's value holds.
    pub value_type: Option<MappingValueType>,
}

/// An import or export mapping of a module.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MappingDecl {
    pub direction: MappingDirection,
    pub name: String,
    pub documentation: String,
    /// The JSON structure it maps, by its qualified name.
    pub json_structure: String,
    pub elements: Vec<MappingElement>,
    /// Whether each value keeps the structure's sample value, as mappings
    /// Studio Pro makes now do.
    pub sample_values: bool,
    pub null_value_option: NullValueOption,
    pub excluded: bool,
    pub export_level: ExportLevel,
}

impl MappingDecl {
    pub fn new(
        direction: MappingDirection,
        name: impl Into<String>,
        json_structure: impl Into<String>,
    ) -> Self {
        Self {
            direction,
            name: name.into(),
            documentation: String::new(),
            json_structure: json_structure.into(),
            elements: Vec::new(),
            sample_values: true,
            null_value_option: NullValueOption::LeaveOutElement,
            excluded: false,
            export_level: ExportLevel::Hidden,
        }
    }

    /// The document the model stores for the mapping of `structure`, or
    /// which element the structure does not have.
    pub fn document(&self, structure: &JsonStructureDecl) -> Result<NativeDocument, String> {
        let mut elements = Vec::with_capacity(self.elements.len());
        for element in &self.elements {
            let found = structure
                .elements
                .iter()
                .find(|root| root.path == element.key)
                .ok_or_else(|| self.missing(&element.key))?;
            elements.push(NativeValue::Document(self.element(element, found, None)?));
        }
        let mut document = NativeDocument::new(self.direction.document_type())
            .with("Documentation", self.documentation.as_str())
            .with("Elements", NativeValue::List(2, elements))
            .with("Excluded", self.excluded)
            .with(
                "ExportLevel",
                match self.export_level {
                    ExportLevel::Hidden => "Hidden",
                    ExportLevel::Published => "Published",
                },
            );
        match self.direction {
            MappingDirection::Import => {
                document = document
                    .with("JsonStructure", self.json_structure.as_str())
                    .with("MappingSourceReference", NativeValue::Null)
                    .with("MessageDefinition", "")
                    .with("Name", self.name.as_str())
                    .with("OperationName", "")
                    .with(
                        "ParameterType",
                        NativeDocument::new("DataTypes$UnknownType"),
                    )
                    .with("PublicName", "")
                    .with("ServiceName", "")
                    .with("UseSubtransactionsForMicroflows", false);
            }
            MappingDirection::Export => {
                document = document
                    .with("IsHeaderParameter", false)
                    .with("JsonStructure", self.json_structure.as_str())
                    .with("MappingSourceReference", NativeValue::Null)
                    .with("MessageDefinition", "")
                    .with("Name", self.name.as_str())
                    .with("NullValueOption", self.null_value_option.as_str())
                    .with("OperationName", "")
                    .with("ParameterName", "")
                    .with("PublicName", "")
                    .with("ServiceName", "");
            }
        }
        Ok(document
            .with("WsdlFile", "")
            .with("XmlSchema", "")
            .with("XsdRootElementName", "")
            .with("MessageDefinition2", ""))
    }

    fn missing(&self, path: &str) -> String {
        format!(
            "mapping {}: its JSON structure {} has no element at `{path}`",
            self.name, self.json_structure
        )
    }

    /// The document of `mapping`, an element of `json`, whose parent is the
    /// object of `parent`'s entity.
    fn element(
        &self,
        mapping: &MappingElement,
        json: &JsonElement,
        parent: Option<&str>,
    ) -> Result<NativeDocument, String> {
        let ty = |kind: &str| format!("{}${kind}MappingElement", self.direction.prefix());
        match &mapping.kind {
            MappingElementKind::Object(object) => {
                if json.element_type == JsonElementType::Value {
                    return Err(format!(
                        "mapping {}: `{}` is a value, not an object",
                        self.name, json.path
                    ));
                }
                let entity = object.entity.as_deref();
                let mut children = Vec::with_capacity(object.children.len());
                for child in &object.children {
                    let path = format!("{}|{}", json.path, child.key);
                    let found = json
                        .children
                        .iter()
                        .find(|candidate| candidate.path == path)
                        .ok_or_else(|| self.missing(&path))?;
                    let owner = entity.or(parent);
                    children.push(NativeValue::Document(self.element(child, found, owner)?));
                }
                let association = match (&object.association, entity, parent) {
                    (MappingAssociation::Other(name), _, _) => name.clone(),
                    (MappingAssociation::Named, Some(entity), Some(parent)) => {
                        named_association(entity, parent)
                    }
                    _ => String::new(),
                };
                let root = parent.is_none();
                let (handling, backup) = match self.direction {
                    MappingDirection::Import => (ObjectHandling::Create, ObjectHandling::Create),
                    MappingDirection::Export if root => {
                        (ObjectHandling::Parameter, ObjectHandling::Error)
                    }
                    MappingDirection::Export => (ObjectHandling::Find, ObjectHandling::Error),
                };
                Ok(NativeDocument::new(ty("Object"))
                    .with("Children", NativeValue::List(2, children))
                    .with("Association", association)
                    .with("CustomHandlerCall", NativeValue::Null)
                    .with("Documentation", "")
                    .with("ElementType", json.element_type.as_str())
                    .with("Entity", entity.unwrap_or_default())
                    .with("ExposedName", json.exposed_name.as_str())
                    .with("IsDefaultType", json.is_default_type)
                    .with("JsonPath", json.path.as_str())
                    .with("MaxOccurs", NativeValue::Int32(json.max_occurs))
                    .with("MinOccurs", NativeValue::Int32(json.min_occurs))
                    .with("Nillable", json.nillable)
                    .with(
                        "ObjectHandling",
                        object.handling.unwrap_or(handling).as_str(),
                    )
                    .with(
                        "ObjectHandlingBackup",
                        object.backup.unwrap_or(backup).as_str(),
                    )
                    .with("ObjectHandlingBackupAllowOverride", object.allow_override)
                    .with("XmlPath", ""))
            }
            MappingElementKind::Value(value) => {
                if json.element_type != JsonElementType::Value {
                    return Err(format!(
                        "mapping {}: `{}` is not a value",
                        self.name, json.path
                    ));
                }
                let entity = parent.ok_or_else(|| {
                    format!(
                        "mapping {}: the value at `{}` is in no object of an entity",
                        self.name, json.path
                    )
                })?;
                let attribute = value.attribute.as_deref().unwrap_or(&json.exposed_name);
                let value_type = value
                    .value_type
                    .clone()
                    .unwrap_or_else(|| MappingValueType::of(json.primitive_type));
                Ok(NativeDocument::new(ty("Value"))
                    .with("Attribute", format!("{entity}.{attribute}"))
                    .with("Converter", value.converter.as_str())
                    .with("Documentation", "")
                    .with("ElementType", json.element_type.as_str())
                    .with("ExposedName", json.exposed_name.as_str())
                    .with("FractionDigits", NativeValue::Int32(json.fraction_digits))
                    .with("IsContent", false)
                    .with("IsKey", value.is_key)
                    .with("IsXmlAttribute", false)
                    .with("JsonPath", json.path.as_str())
                    .with("MaxLength", NativeValue::Int32(json.max_length))
                    .with("MaxOccurs", NativeValue::Int32(json.max_occurs))
                    .with("MinOccurs", NativeValue::Int32(json.min_occurs))
                    .with("Nillable", json.nillable)
                    .with(
                        "OriginalValue",
                        if self.sample_values {
                            json.original_value.as_str()
                        } else {
                            ""
                        },
                    )
                    .with("TotalDigits", NativeValue::Int32(json.total_digits))
                    .with("Type", value_type.document())
                    .with("XmlPath", "")
                    .with("XmlPrimitiveType", json.primitive_type.as_str()))
            }
        }
    }

    /// The fewest-words declaration of `document`, the stored mapping of
    /// `structure`, when one states that document again.
    pub fn from_document(document: &NativeDocument, structure: &JsonStructureDecl) -> Option<Self> {
        let direction = match document.ty.as_str() {
            "ImportMappings$ImportMapping" => MappingDirection::Import,
            "ExportMappings$ExportMapping" => MappingDirection::Export,
            _ => return None,
        };
        let mut declaration = Self::new(
            direction,
            document.text("Name")?,
            document.text("JsonStructure")?,
        );
        declaration.documentation = document.text("Documentation")?.to_string();
        declaration.excluded = matches!(document.get("Excluded")?, NativeValue::Bool(true));
        declaration.export_level = match document.text("ExportLevel")? {
            "Hidden" => ExportLevel::Hidden,
            "Published" => ExportLevel::Published,
            _ => return None,
        };
        if direction == MappingDirection::Export {
            declaration.null_value_option =
                NullValueOption::parse(document.text("NullValueOption")?)?;
        }
        let NativeValue::List(2, roots) = document.get("Elements")? else {
            return None;
        };
        for root in roots {
            let NativeValue::Document(root) = root else {
                return None;
            };
            declaration.elements.push(read_element(root, None, "")?);
        }
        // A value that keeps no sample value says the mapping keeps none.
        let mut samples = Vec::new();
        for root in roots {
            if let NativeValue::Document(root) = root {
                collect_samples(root, &mut samples);
            }
        }
        if samples.iter().any(|(path, sample)| {
            sample.is_empty()
                && structure
                    .elements
                    .iter()
                    .flat_map(JsonElement::walk)
                    .any(|element| &element.path == path && !element.original_value.is_empty())
        }) {
            declaration.sample_values = false;
        }
        (declaration.document(structure).ok()? == document.narrowed()).then_some(declaration)
    }
}

/// The association Studio Pro makes for an object of `entity` under one of
/// `parent`: `<Entity>_<Parent>`, in the entity's module.
fn named_association(entity: &str, parent: &str) -> String {
    let parent_name = parent.rsplit('.').next().unwrap_or(parent);
    format!("{entity}_{parent_name}")
}

fn collect_samples(document: &NativeDocument, samples: &mut Vec<(String, String)>) {
    if let (Some(path), Some(sample)) = (document.text("JsonPath"), document.text("OriginalValue"))
    {
        samples.push((path.to_string(), sample.to_string()));
    }
    if let Some(NativeValue::List(_, children)) = document.get("Children") {
        for child in children {
            if let NativeValue::Document(child) = child {
                collect_samples(child, samples);
            }
        }
    }
}

/// The declaration of a stored element whose parent's path is
/// `parent_path` and whose parent is an object of `parent`.
fn read_element(
    document: &NativeDocument,
    parent: Option<&str>,
    parent_path: &str,
) -> Option<MappingElement> {
    let path = document.text("JsonPath")?;
    let key = if parent_path.is_empty() {
        path.to_string()
    } else {
        path.strip_prefix(parent_path)?
            .strip_prefix('|')?
            .to_string()
    };
    if document.ty.ends_with("$ObjectMappingElement") {
        let entity = Some(document.text("Entity")?)
            .filter(|entity| !entity.is_empty())
            .map(str::to_string);
        let mut object = ObjectMapping::new(entity.clone());
        let association = document.text("Association")?;
        object.association = match (entity.as_deref(), parent) {
            _ if association.is_empty() => MappingAssociation::None,
            (Some(entity), Some(parent)) if association == named_association(entity, parent) => {
                MappingAssociation::Named
            }
            _ => MappingAssociation::Other(association.to_string()),
        };
        // `None` where nothing else would be written: the declaration then
        // says only an association the default would not give.
        if object.association == MappingAssociation::None && (entity.is_none() || parent.is_none())
        {
            object.association = MappingAssociation::Named;
        }
        object.handling = Some(ObjectHandling::parse(document.text("ObjectHandling")?)?);
        object.backup = Some(ObjectHandling::parse(
            document.text("ObjectHandlingBackup")?,
        )?);
        object.allow_override = matches!(
            document.get("ObjectHandlingBackupAllowOverride")?,
            NativeValue::Bool(true)
        );
        let NativeValue::List(2, children) = document.get("Children")? else {
            return None;
        };
        let owner = entity.as_deref().or(parent);
        for child in children {
            let NativeValue::Document(child) = child else {
                return None;
            };
            object.children.push(read_element(child, owner, path)?);
        }
        return Some(MappingElement {
            key,
            kind: MappingElementKind::Object(object),
        });
    }
    let entity = parent?;
    let attribute = document.text("Attribute")?;
    let attribute = attribute
        .strip_prefix(entity)?
        .strip_prefix('.')?
        .to_string();
    let NativeValue::Document(value_type) = document.get("Type")? else {
        return None;
    };
    Some(MappingElement {
        key,
        kind: MappingElementKind::Value(ValueMapping {
            attribute: Some(attribute),
            is_key: matches!(document.get("IsKey")?, NativeValue::Bool(true)),
            converter: document.text("Converter")?.to_string(),
            value_type: Some(MappingValueType::from_document(value_type)?),
        }),
    })
}

impl MappingDecl {
    /// The declaration with every choice that is the default left unsaid:
    /// what a developer reads and writes.
    pub fn simplified(mut self, structure: &JsonStructureDecl) -> Self {
        let direction = self.direction;
        for root in &mut self.elements {
            if let Some(json) = structure
                .elements
                .iter()
                .find(|element| element.path == root.key)
            {
                simplify(root, json, direction, true);
            }
        }
        self
    }
}

fn simplify(
    element: &mut MappingElement,
    json: &JsonElement,
    direction: MappingDirection,
    root: bool,
) {
    match &mut element.kind {
        MappingElementKind::Object(object) => {
            let (handling, backup) = match direction {
                MappingDirection::Import => (ObjectHandling::Create, ObjectHandling::Create),
                MappingDirection::Export if root => {
                    (ObjectHandling::Parameter, ObjectHandling::Error)
                }
                MappingDirection::Export => (ObjectHandling::Find, ObjectHandling::Error),
            };
            if object.handling == Some(handling) {
                object.handling = None;
            }
            if object.backup == Some(backup) {
                object.backup = None;
            }
            for child in &mut object.children {
                let path = format!("{}|{}", json.path, child.key);
                if let Some(found) = json.children.iter().find(|found| found.path == path) {
                    simplify(child, found, direction, false);
                }
            }
        }
        MappingElementKind::Value(value) => {
            if value.attribute.as_deref() == Some(json.exposed_name.as_str()) {
                value.attribute = None;
            }
            if value.value_type.as_ref() == Some(&MappingValueType::of(json.primitive_type)) {
                value.value_type = None;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn structure() -> JsonStructureDecl {
        JsonStructureDecl::derive(
            "JSON_Order",
            r#"{"number": "A-1", "total": 1.5, "lines": [{"sku": "S", "qty": 2}]}"#,
        )
        .unwrap()
    }

    fn import() -> MappingDecl {
        let mut lines = ObjectMapping::new(None);
        let mut line = ObjectMapping::new(Some("Sales.Line".into()));
        line.children.push(MappingElement {
            key: "sku".into(),
            kind: MappingElementKind::Value(ValueMapping {
                is_key: true,
                ..ValueMapping::default()
            }),
        });
        line.children.push(MappingElement {
            key: "qty".into(),
            kind: MappingElementKind::Value(ValueMapping {
                attribute: Some("Quantity".into()),
                ..ValueMapping::default()
            }),
        });
        lines.children.push(MappingElement {
            key: "(Object)".into(),
            kind: MappingElementKind::Object(line),
        });
        let mut root = ObjectMapping::new(Some("Sales.Order".into()));
        root.children.push(MappingElement {
            key: "number".into(),
            kind: MappingElementKind::Value(ValueMapping::default()),
        });
        root.children.push(MappingElement {
            key: "lines".into(),
            kind: MappingElementKind::Object(lines),
        });
        let mut mapping =
            MappingDecl::new(MappingDirection::Import, "IMM_Order", "Sales.JSON_Order");
        mapping.elements.push(MappingElement {
            key: "(Object)".into(),
            kind: MappingElementKind::Object(root),
        });
        mapping
    }

    /// A mapping's elements are its structure's, with the entity, the
    /// association and the attribute each is mapped to; an object under an
    /// array that is no entity hangs off the nearest entity.
    #[test]
    fn a_mapping_writes_what_its_structure_says_of_each_element() {
        let structure = structure();
        let document = import().document(&structure).unwrap();
        let root = document.at("Elements[0]").unwrap();
        assert_eq!(root.ty, "ImportMappings$ObjectMappingElement");
        assert_eq!(root.text("ExposedName"), Some("Root"));
        assert_eq!(root.text("Association"), Some(""));
        assert_eq!(root.text("ObjectHandling"), Some("Create"));
        let number = root.at("Children[0]").unwrap();
        assert_eq!(number.text("Attribute"), Some("Sales.Order.Number"));
        assert_eq!(number.text("OriginalValue"), Some("\"A-1\""));
        assert_eq!(number.text("XmlPrimitiveType"), Some("String"));
        let lines = root.at("Children[1]").unwrap();
        assert_eq!(lines.text("Entity"), Some(""));
        let line = lines.at("Children[0]").unwrap();
        assert_eq!(line.text("Association"), Some("Sales.Line_Order"));
        let quantity = line.at("Children[1]").unwrap();
        assert_eq!(quantity.text("Attribute"), Some("Sales.Line.Quantity"));
        assert_eq!(quantity.at("Type").unwrap().ty, "DataTypes$IntegerType");
        // The stored document reads back as the declaration, its defaults
        // unsaid.
        let read = MappingDecl::from_document(&document, &structure)
            .unwrap()
            .simplified(&structure);
        assert_eq!(read, import());
        // A name the structure has no element at is refused.
        let mut wrong = import();
        wrong.elements[0].key = "(Array)".into();
        assert_eq!(
            wrong.document(&structure).unwrap_err(),
            "mapping IMM_Order: its JSON structure Sales.JSON_Order has no element at `(Array)`"
        );
    }

    /// An export finds its objects and is given its root; a mapping Studio
    /// Pro made without sample values reads back as one.
    #[test]
    fn an_export_and_a_mapping_without_samples_read_back() {
        let structure = structure();
        let mut export = import();
        export.direction = MappingDirection::Export;
        export.name = "EM_Order".into();
        let document = export.document(&structure).unwrap();
        assert_eq!(
            document.at("Elements[0]").unwrap().text("ObjectHandling"),
            Some("Parameter")
        );
        assert_eq!(
            document
                .at("Elements[0].Children[1].Children[0]")
                .unwrap()
                .text("ObjectHandling"),
            Some("Find")
        );
        let read = MappingDecl::from_document(&document, &structure)
            .unwrap()
            .simplified(&structure);
        assert_eq!(read, export);
        let mut bare = import();
        bare.sample_values = false;
        let document = bare.document(&structure).unwrap();
        let read = MappingDecl::from_document(&document, &structure)
            .unwrap()
            .simplified(&structure);
        assert_eq!(read, bare);
    }
}
