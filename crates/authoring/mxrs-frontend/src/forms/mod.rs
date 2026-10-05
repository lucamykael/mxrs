//! Pages, layouts and snippets as the frontend declares them: TSX.
//!
//! A page is the document Mendix stores for it, and its TSX states that
//! document: every stored document is an element named for its type
//! (`Forms$DivContainer` is `<DivContainer>`), every field a prop
//! (`Name` is `name`), and the widgets a document holds are its children.
//! Nothing is outside the vocabulary, because the vocabulary is the model's
//! own.
//!
//! What keeps that readable is `src/mxrs/elements.ts`: for each element, the
//! value every field has when a page says nothing about it. A page states
//! only what differs. The file belongs to the project — an import writes it
//! from the project's own documents — so what an unstated prop means never
//! depends on the version of mxrs that reads it.
//!
//! A pluggable widget stores its whole definition with every use. The
//! definition is declared once, in `src/widgets/<Name>.tsx`, and a page
//! uses the widget by that name, stating its properties by their keys.
//!
//! | file | declares |
//! |---|---|
//! | `src/mxrs/elements.ts` | each element's fields and their defaults |
//! | `src/widgets/*.tsx` | each pluggable widget's definition |
//! | `src/pages/<module>/*.tsx` | pages |
//! | `src/components/layout/<module>/*.tsx` | layouts |
//! | `src/components/snippets/<module>/*.tsx` | snippets |

mod read;
mod shapes;
mod tsx;
mod write;

use std::collections::{BTreeMap, HashMap};

use mxrs_ir::{NativeDocument, NativeValue};

pub use read::{read_elements, read_form, read_widget};
pub use shapes::{mine, render_elements};
pub use write::{render_form, render_widget};

/// The type of the document every pluggable widget is stored as.
pub(crate) const CUSTOM_WIDGET: &str = "CustomWidgets$CustomWidget";
pub(crate) const WIDGET_OBJECT: &str = "CustomWidgets$WidgetObject";
pub(crate) const WIDGET_PROPERTY: &str = "CustomWidgets$WidgetProperty";
pub(crate) const WIDGET_VALUE: &str = "CustomWidgets$WidgetValue";
pub(crate) const TEXT: &str = "Texts$Text";
pub(crate) const TRANSLATION: &str = "Texts$Translation";

/// The module the frontend's files import the elements from.
pub const ELEMENTS_MODULE: &str = "@/mxrs/elements";
/// The module that holds what the declarations are written with.
pub const FORMS_MODULE: &str = "@/mxrs/forms";
/// The folder, under `src/`, of the pluggable widgets' definitions.
pub const WIDGETS_FOLDER: &str = "widgets";

/// What a field holds when a document says nothing about it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FieldDefault {
    /// `null`, a boolean, a number or a text.
    Value(NativeValue),
    /// The element this component is, with every field at its own default.
    Element(String),
    /// A list with this marker and nothing in it.
    List(i32),
}

/// One field of an element.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Field {
    /// The field as the model stores it: `TabIndex`.
    pub key: String,
    /// The prop that states it: `tabIndex`.
    pub prop: String,
    pub default: FieldDefault,
}

/// One element: a document type, its fields in the order the model stores
/// them, and which of them is written as its children.
///
/// A type the model stores with different sets of fields — documents
/// written by different versions of Studio Pro — is one element for each:
/// `DivContainer`, `DivContainer_2`. A declaration says which by the
/// component it uses, so no field is ever stated as absent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Shape {
    /// The type as the model stores it: `Forms$DivContainer`.
    pub ty: String,
    /// The component that states it: `DivContainer`.
    pub component: String,
    pub fields: Vec<Field>,
    /// The key of the list written as the element's children.
    pub children: Option<String>,
    /// The key of the one field the element is mostly stated for. Where a
    /// prop holds this element and only that field is said, the prop is
    /// written as the field's value: `captionTemplate={{ en_US: "Save" }}`
    /// for a template that states its text and nothing else.
    pub main: Option<String>,
}

impl Shape {
    pub(crate) fn field(&self, key: &str) -> Option<&Field> {
        self.fields.iter().find(|field| field.key == key)
    }

    pub(crate) fn field_of_prop(&self, prop: &str) -> Option<&Field> {
        self.fields.iter().find(|field| field.prop == prop)
    }

    /// Whether `document` has exactly this element's fields, in order.
    pub(crate) fn fits(&self, document: &NativeDocument) -> bool {
        self.ty == document.ty
            && self.fields.len() == document.fields.len()
            && self
                .fields
                .iter()
                .zip(&document.fields)
                .all(|(field, (key, _))| &field.key == key)
    }
}

/// Every element a project's pages are written with.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Shapes {
    by_component: BTreeMap<String, Shape>,
    /// The components of each type, the first of them the one a type is
    /// when nothing says which.
    by_type: HashMap<String, Vec<String>>,
    /// Each element with every field at its default, by component.
    defaults: HashMap<String, NativeDocument>,
}

impl Shapes {
    /// The shapes, checked: a default that is an element names one of them,
    /// and no element's defaults hold the element itself.
    pub(crate) fn new(shapes: Vec<Shape>) -> Result<Self, String> {
        let mut by_component = BTreeMap::new();
        let mut by_type: HashMap<String, Vec<String>> = HashMap::new();
        for shape in shapes {
            by_type
                .entry(shape.ty.clone())
                .or_default()
                .push(shape.component.clone());
            if let Some(twice) = by_component.insert(shape.component.clone(), shape) {
                return Err(format!("{} is declared twice", twice.component));
            }
        }
        for components in by_type.values_mut() {
            components.sort();
        }
        let mut shapes = Self {
            by_component,
            by_type,
            defaults: HashMap::new(),
        };
        let components: Vec<String> = shapes.by_component.keys().cloned().collect();
        for component in components {
            shapes.default_of(&component, &mut Vec::new())?;
        }
        Ok(shapes)
    }

    fn default_of(
        &mut self,
        component: &str,
        open: &mut Vec<String>,
    ) -> Result<NativeDocument, String> {
        if let Some(found) = self.defaults.get(component) {
            return Ok(found.clone());
        }
        if open.iter().any(|other| other == component) {
            return Err(format!(
                "{component} holds itself by default, through {}",
                open.join(", ")
            ));
        }
        let Some(shape) = self.by_component.get(component).cloned() else {
            return Err(format!("no element {component} is declared"));
        };
        open.push(component.to_string());
        let mut document = NativeDocument::new(shape.ty.as_str());
        for field in &shape.fields {
            let value = match &field.default {
                FieldDefault::Value(value) => value.clone(),
                FieldDefault::Element(nested) => {
                    NativeValue::Document(self.default_of(nested, open)?)
                }
                FieldDefault::List(marker) => NativeValue::List(*marker, Vec::new()),
            };
            document.fields.push((field.key.clone(), value));
        }
        open.pop();
        self.defaults
            .insert(component.to_string(), document.clone());
        Ok(document)
    }

    /// The element a type is when nothing says which of its shapes.
    pub fn shape(&self, ty: &str) -> Option<&Shape> {
        self.by_component.get(self.by_type.get(ty)?.first()?)
    }

    /// The element `document` is: the one of its type with its fields.
    pub(crate) fn shape_of(&self, document: &NativeDocument) -> Option<&Shape> {
        self.by_type
            .get(&document.ty)?
            .iter()
            .filter_map(|component| self.by_component.get(component))
            .find(|shape| shape.fits(document))
    }

    pub(crate) fn component(&self, component: &str) -> Option<&Shape> {
        self.by_component.get(component)
    }

    pub fn iter(&self) -> impl Iterator<Item = &Shape> {
        self.by_component.values()
    }

    /// The element `shape` is, with every field at its default.
    pub(crate) fn default_for(&self, shape: &Shape) -> &NativeDocument {
        &self.defaults[&shape.component]
    }

    /// The element a type is, with every field at its default.
    pub(crate) fn default_document(&self, ty: &str) -> Option<&NativeDocument> {
        self.shape(ty).map(|shape| self.default_for(shape))
    }

    /// The element `value` is and the field it is mostly stated for, when
    /// it is an element that has one.
    pub(crate) fn main_of(&self, value: Option<&NativeValue>) -> Option<(&Shape, &Field)> {
        let Some(NativeValue::Document(document)) = value else {
            return None;
        };
        let shape = self.shape_of(document)?;
        let field = shape.field(shape.main.as_deref()?)?;
        Some((shape, field))
    }

    /// The marker the translations of a text are listed with, when texts
    /// are stored the one way `{ en_US: "..." }` can state.
    pub(crate) fn text_marker(&self) -> Option<i32> {
        let text = self.shape(TEXT)?;
        let translation = self.shape(TRANSLATION)?;
        let [items] = text.fields.as_slice() else {
            return None;
        };
        let [language, value] = translation.fields.as_slice() else {
            return None;
        };
        match &items.default {
            FieldDefault::List(marker)
                if items.key == "Items"
                    && language.key == "LanguageCode"
                    && value.key == "Text" =>
            {
                Some(*marker)
            }
            _ => None,
        }
    }
}

/// A pluggable widget's definition, declared once and stored with each use.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WidgetDefinition {
    /// The component a page uses the widget by.
    pub name: String,
    /// The `CustomWidgets$CustomWidgetType` document.
    pub ty: NativeDocument,
    /// What a property holds when a use of the widget says nothing about
    /// it, where that is more than its type's own default: by the
    /// property's key, a nested object's after its property's
    /// (`columns.header`). Each is a value element, pointing nowhere yet.
    pub defaults: BTreeMap<String, NativeDocument>,
}

impl WidgetDefinition {
    /// The type of the value the property at `keys` holds.
    pub(crate) fn value_type(&self, keys: &str) -> Option<&NativeDocument> {
        let Some(NativeValue::Document(object_type)) = self.ty.get("ObjectType") else {
            return None;
        };
        let mut object_type = object_type;
        let mut found = None;
        for key in keys.split('.') {
            let Some(NativeValue::List(_, types)) = object_type.get("PropertyTypes") else {
                return None;
            };
            let property = types.iter().find_map(|ty| match ty {
                NativeValue::Document(ty) if ty.text("PropertyKey") == Some(key) => Some(ty),
                _ => None,
            })?;
            let Some(NativeValue::Document(value_type)) = property.get("ValueType") else {
                return None;
            };
            found = Some(value_type);
            match value_type.get("ObjectType") {
                Some(NativeValue::Document(nested)) => object_type = nested,
                _ => object_type = value_type,
            }
        }
        found
    }
}

/// What a project's pages are written with.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Vocabulary {
    pub shapes: Shapes,
    pub widgets: Vec<WidgetDefinition>,
}

impl Vocabulary {
    pub(crate) fn widget_of(&self, ty: &NativeDocument) -> Option<&WidgetDefinition> {
        self.widgets.iter().find(|widget| &widget.ty == ty)
    }
}

/// The field of a pluggable widget's value that a property of the type
/// `value_type` is stated in: a property of a use is written as that
/// field's value alone when nothing else of its value is said.
pub(crate) fn property_main(value_type: &NativeDocument) -> Option<&'static str> {
    Some(match value_type.text("Type")? {
        "Boolean" | "String" | "Integer" | "Decimal" | "Enumeration" => "PrimitiveValue",
        "TextTemplate" => "TextTemplate",
        "TranslatableString" => "TranslatableValue",
        "Expression" => "Expression",
        "Attribute" => "AttributeRef",
        "Action" => "Action",
        "Object" => "Objects",
        "DataSource" => "DataSource",
        "Icon" => "Icon",
        "Image" => "Image",
        "Microflow" => "Microflow",
        "Nanoflow" => "Nanoflow",
        "Widgets" => "Widgets",
        _ => return None,
    })
}

/// The element a property stated in `main` holds there, where properties
/// of that type hold one kind of element.
pub(crate) fn property_element(main: &str) -> Option<&'static str> {
    match main {
        "TextTemplate" => Some("Forms$ClientTemplate"),
        "AttributeRef" => Some("DomainModels$AttributeRef"),
        "TranslatableValue" => Some(TEXT),
        _ => None,
    }
}

/// What the field a property is stated in holds before the property says
/// anything: what its value holds there, or — where that is nothing and a
/// property of this type holds one kind of element — that element with
/// nothing said. So `header: { en_US: "Name" }` is a template whether or
/// not the uses of the widget so far gave the property one.
pub(crate) fn property_held(
    shapes: &Shapes,
    main: &str,
    held: Option<&NativeValue>,
) -> Option<NativeValue> {
    match held {
        Some(NativeValue::Null) => Some(
            property_element(main)
                .and_then(|ty| shapes.default_document(ty))
                .map_or(NativeValue::Null, |element| {
                    NativeValue::Document(element.clone())
                }),
        ),
        other => other.cloned(),
    }
}

/// What a property stated in `main` holds, when `value` is not that.
pub(crate) fn property_kind(main: &str, value: &NativeValue) -> Option<&'static str> {
    let element = |ty: &str| match value {
        NativeValue::Null => true,
        NativeValue::Document(document) => document.ty == ty,
        _ => false,
    };
    let (fits, expected) = match main {
        "TextTemplate" => (
            element("Forms$ClientTemplate"),
            "a template: its texts by language, or a <ClientTemplate>",
        ),
        "AttributeRef" => (
            element("DomainModels$AttributeRef"),
            "an attribute: its name, or an <AttributeRef>",
        ),
        "TranslatableValue" => (element(TEXT), "texts by language"),
        "Action" | "DataSource" | "Icon" => (
            matches!(value, NativeValue::Null)
                || matches!(value, NativeValue::Document(document) if document.ty != TEXT),
            "an element, or null",
        ),
        "Objects" | "Widgets" => (matches!(value, NativeValue::List(..)), "a list"),
        _ => (
            matches!(value, NativeValue::Text(_)),
            "a text — a number too, between quotes",
        ),
    };
    (!fits).then_some(expected)
}

/// Whether texts by language — `{ en_US: "..." }` — are what `field`
/// holds, where it holds `held` before anything is said: a text already, a
/// text by default, or nothing in particular.
pub(crate) fn texts_fit(shapes: &Shapes, field: &Field, held: Option<&NativeValue>) -> bool {
    if matches!(held, Some(NativeValue::Document(document)) if document.ty == TEXT) {
        return true;
    }
    match &field.default {
        FieldDefault::Value(NativeValue::Null) => true,
        FieldDefault::Element(component) => shapes
            .component(component)
            .is_some_and(|shape| shape.ty == TEXT),
        _ => false,
    }
}

/// Gives `value`, a pluggable widget's value with nothing said in it, the
/// default its property's type declares: in the field a property of that
/// type is stated in when that is a text, and as its primitive value
/// otherwise.
pub(crate) fn declare_default(value: &mut NativeDocument, value_type: &NativeDocument) {
    let Some(default) = value_type.text("DefaultValue") else {
        return;
    };
    let key = property_main(value_type)
        .filter(|key| *key == "Expression")
        .unwrap_or("PrimitiveValue");
    if let Some(NativeValue::Text(_)) = value.get(key) {
        value.set(key, default);
    }
}

/// The prop that states the field `key`: its name with a small first
/// letter. `key`, `ref` and `children` are React's own, so a field of that
/// name keeps its capital. `None` for a key no prop can state.
pub(crate) fn prop_of(key: &str) -> Option<String> {
    let mut letters = key.chars();
    let first = letters.next()?;
    let identifier = (first.is_ascii_alphabetic() || first == '_')
        && key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
    if !identifier || first.is_ascii_lowercase() {
        return None;
    }
    let lowered = format!("{}{}", first.to_ascii_lowercase(), letters.as_str());
    if matches!(lowered.as_str(), "key" | "ref" | "children") {
        return Some(key.to_string());
    }
    Some(lowered)
}

/// The function a form of type `ty` is declared with.
pub(crate) fn declarer(ty: &str) -> Option<&'static str> {
    match ty {
        "Forms$Page" => Some("page"),
        "Forms$Layout" => Some("layout"),
        "Forms$Snippet" => Some("snippet"),
        _ => None,
    }
}

/// The folder, under `src/`, that holds the forms of type `ty`.
pub fn folder(ty: &str) -> Option<&'static str> {
    match ty {
        "Forms$Page" => Some("pages"),
        "Forms$Layout" => Some("components/layout"),
        "Forms$Snippet" => Some("components/snippets"),
        _ => None,
    }
}

/// Every document of `root`, with where it is from the root.
pub(crate) fn walk<'a>(
    root: &'a NativeDocument,
    path: &str,
    visit: &mut impl FnMut(&str, &'a NativeDocument),
) {
    visit(path, root);
    for (key, value) in &root.fields {
        let nested = if path.is_empty() {
            key.clone()
        } else {
            format!("{path}.{key}")
        };
        match value {
            NativeValue::Document(document) => walk(document, &nested, visit),
            NativeValue::List(_, items) => {
                for (index, item) in items.iter().enumerate() {
                    if let NativeValue::Document(document) = item {
                        walk(document, &format!("{nested}[{index}]"), visit);
                    }
                }
            }
            _ => {}
        }
    }
}

/// `path` continued by the field `key`.
pub(crate) fn join(path: &str, key: &str) -> String {
    if path.is_empty() {
        key.to_string()
    } else {
        format!("{path}.{key}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(translations: &[(&str, &str)]) -> NativeDocument {
        NativeDocument::new(TEXT).with(
            "Items",
            NativeValue::List(
                3,
                translations
                    .iter()
                    .map(|(language, text)| {
                        NativeValue::Document(
                            NativeDocument::new(TRANSLATION)
                                .with("LanguageCode", *language)
                                .with("Text", *text),
                        )
                    })
                    .collect(),
            ),
        )
    }

    fn appearance(class: &str) -> NativeDocument {
        NativeDocument::new("Forms$Appearance")
            .with("Class", class)
            .with("Style", "")
    }

    fn container(name: &str, class: &str, widgets: Vec<NativeDocument>) -> NativeDocument {
        NativeDocument::new("Forms$DivContainer")
            .with("Appearance", appearance(class))
            .with("Name", name)
            .with("TabIndex", 0)
            .with(
                "Widgets",
                NativeValue::List(2, widgets.into_iter().map(NativeValue::from).collect()),
            )
    }

    fn label(name: &str, caption: &[(&str, &str)]) -> NativeDocument {
        NativeDocument::new("Forms$Label")
            .with("Appearance", appearance(""))
            .with("Caption", text(caption))
            .with("Name", name)
            .with("TabIndex", 0)
    }

    fn value_type(ty: &str, default: &str, object: Option<NativeDocument>) -> NativeDocument {
        NativeDocument::new("CustomWidgets$WidgetValueType")
            .with("DefaultValue", default)
            .with(
                "ObjectType",
                object.map_or(NativeValue::Null, NativeValue::Document),
            )
            .with("Type", ty)
    }

    fn property_type(key: &str, value: NativeDocument) -> NativeDocument {
        NativeDocument::new("CustomWidgets$WidgetPropertyType")
            .with("PropertyKey", key)
            .with("ValueType", value)
    }

    fn object_type(properties: Vec<NativeDocument>) -> NativeDocument {
        NativeDocument::new("CustomWidgets$WidgetObjectType").with(
            "PropertyTypes",
            NativeValue::List(2, properties.into_iter().map(NativeValue::from).collect()),
        )
    }

    fn value(primitive: &str, objects: Vec<NativeDocument>, pointer: &str) -> NativeDocument {
        NativeDocument::new(WIDGET_VALUE)
            .with(
                "Objects",
                NativeValue::List(2, objects.into_iter().map(NativeValue::from).collect()),
            )
            .with("PrimitiveValue", primitive)
            .with("TypePointer", NativeValue::Pointer(pointer.to_string()))
    }

    /// An object of the type at `type_path` holding `values` for the
    /// properties at those places of the type.
    fn object(type_path: &str, values: Vec<(usize, NativeDocument)>) -> NativeDocument {
        NativeDocument::new(WIDGET_OBJECT)
            .with(
                "Properties",
                NativeValue::List(
                    2,
                    values
                        .into_iter()
                        .map(|(index, value)| {
                            NativeValue::Document(
                                NativeDocument::new(WIDGET_PROPERTY)
                                    .with(
                                        "TypePointer",
                                        NativeValue::Pointer(format!(
                                            "{type_path}.PropertyTypes[{index}]"
                                        )),
                                    )
                                    .with("Value", value),
                            )
                        })
                        .collect(),
                ),
            )
            .with("TypePointer", NativeValue::Pointer(type_path.to_string()))
    }

    /// A grid with a flag, a text and columns, each column with a caption.
    fn grid(
        path: &str,
        name: &str,
        flag: &str,
        columns: &[&str],
        with_text: bool,
    ) -> NativeDocument {
        let ty = format!("{path}.Type.ObjectType");
        let column_type = format!("{ty}.PropertyTypes[2].ValueType.ObjectType");
        let columns = columns
            .iter()
            .map(|caption| {
                object(
                    &column_type,
                    vec![(
                        0,
                        value(
                            caption,
                            vec![],
                            &format!("{column_type}.PropertyTypes[0].ValueType"),
                        ),
                    )],
                )
            })
            .collect();
        let mut values = vec![(
            0,
            value(flag, vec![], &format!("{ty}.PropertyTypes[0].ValueType")),
        )];
        if with_text {
            values.push((
                1,
                value("", vec![], &format!("{ty}.PropertyTypes[1].ValueType")),
            ));
        }
        values.push((
            2,
            value("", columns, &format!("{ty}.PropertyTypes[2].ValueType")),
        ));
        NativeDocument::new(CUSTOM_WIDGET)
            .with("Name", name)
            .with("Object", object(&ty, values))
            .with(
                "Type",
                NativeDocument::new("CustomWidgets$CustomWidgetType")
                    .with(
                        "ObjectType",
                        object_type(vec![
                            property_type("advanced", value_type("Boolean", "false", None)),
                            property_type("emptyText", value_type("String", "", None)),
                            property_type(
                                "columns",
                                value_type(
                                    "Object",
                                    "",
                                    Some(object_type(vec![property_type(
                                        "header",
                                        value_type("String", "", None),
                                    )])),
                                ),
                            ),
                        ]),
                    )
                    .with("WidgetId", "com.example.widget.web.grid.Grid"),
            )
    }

    fn page(widgets: Vec<NativeDocument>) -> NativeDocument {
        NativeDocument::new("Forms$Page")
            .with(
                "AllowedModuleRoles",
                NativeValue::List(1, vec!["Sales.User".into()]),
            )
            .with("DefaultWidget", NativeValue::Pointer("Widgets[0]".into()))
            .with("Name", "Orders")
            .with(
                "Owner",
                NativeValue::Identity(NativeValue::NOTHING.to_string()),
            )
            .with(
                "Title",
                text(&[("en_US", "Orders"), ("nl_NL", "Bestellingen")]),
            )
            .with(
                "Widgets",
                NativeValue::List(2, widgets.into_iter().map(NativeValue::from).collect()),
            )
    }

    fn orders() -> NativeDocument {
        page(vec![
            container(
                "header",
                "card",
                vec![
                    label("title", &[("en_US", "Orders & \"more\"")]),
                    label("hint", &[]),
                ],
            ),
            container("body", "", vec![]),
            grid("Widgets[2]", "grid1", "true", &["Number", ""], true),
            // A use of the widget that does not store one of its properties.
            grid("Widgets[3]", "grid2", "false", &[], false),
        ])
    }

    fn vocabulary(documents: &[&NativeDocument]) -> Vocabulary {
        let mined = mine(documents);
        // What a build reads is the text.
        let shapes = read_elements(&render_elements(&mined.shapes), "elements.ts").unwrap();
        assert_eq!(shapes, mined.shapes);
        let widgets = mined
            .widgets
            .iter()
            .map(|widget| {
                let source = render_widget(widget, &mined).unwrap();
                let read = read_widget(&source, "widget.tsx", &shapes).unwrap();
                assert_eq!(&read, widget, "{source}");
                read
            })
            .collect();
        Vocabulary { shapes, widgets }
    }

    #[test]
    fn a_page_is_written_as_what_differs_and_read_back_whole() {
        let document = orders();
        let vocabulary = vocabulary(&[&document]);
        let source = render_form("Sales", &document, &vocabulary).unwrap();
        assert_eq!(
            source,
            r#"import { DivContainer, Label, Page } from "@/mxrs/elements";
import { missing, named, page, unset } from "@/mxrs/forms";
import { Grid } from "@/widgets/Grid";

export default page(
  "Sales",
  <Page
    name="Orders"
    allowedModuleRoles={["Sales.User"]}
    defaultWidget={named("header")}
    owner={unset}
    title={{ en_US: "Orders", nl_NL: "Bestellingen" }}
  >
    <DivContainer name="header" appearance="card">
      <Label name="title" caption={{ en_US: "Orders & \"more\"" }} />
      <Label name="hint" />
    </DivContainer>
    <DivContainer name="body" />
    <Grid name="grid1" properties={{ advanced: true, columns: [{ header: "Number" }, {}] }} />
    <Grid name="grid2" properties={{ emptyText: missing }} />
  </Page>,
);
"#,
            "{source}"
        );
        let (module, read) = read_form(&source, "Orders.tsx", &vocabulary).unwrap();
        assert_eq!(module, "Sales");
        assert_eq!(read.document, document);
        assert_eq!(read.name(), "Orders");
    }

    #[test]
    fn a_type_stored_with_other_fields_is_an_element_of_its_own() {
        let old = NativeDocument::new("Forms$Label")
            .with("Caption", text(&[("en_US", "Old")]))
            .with("Name", "old");
        let document = page(vec![
            label("a", &[]),
            label("b", &[]),
            container("c", "", vec![old]),
        ]);
        let vocabulary = vocabulary(&[&document]);
        let source = render_form("Sales", &document, &vocabulary).unwrap();
        assert!(
            source.contains("<Label_2 name=\"old\" caption={{ en_US: \"Old\" }} />"),
            "{source}"
        );
        assert_eq!(
            read_form(&source, "Orders.tsx", &vocabulary)
                .unwrap()
                .1
                .document,
            document
        );
    }

    #[test]
    fn what_a_page_cannot_state_is_refused_at_its_line() {
        let document = orders();
        let vocabulary = vocabulary(&[&document]);
        let source = render_form("Sales", &document, &vocabulary).unwrap();
        for (from, to, line, expected) in [
            (
                "name=\"body\"",
                "nme=\"body\"",
                18,
                "<DivContainer> has no prop `nme`",
            ),
            (
                "<Label name=\"hint\" />",
                "<Labl name=\"hint\" />",
                16,
                "an element imported",
            ),
            (
                "<Label name=\"hint\" />",
                "<Label name=\"hint\"><Label /></Label>",
                16,
                "holds no children",
            ),
            (
                "named(\"header\")",
                "named(\"nowhere\")",
                10,
                "no element is named `nowhere`",
            ),
            ("named(\"header\")", "named(\"\" + x)", 10, "a name"),
            (
                "advanced:",
                "advancd:",
                19,
                "the widget has no property `advancd`",
            ),
            ("name=\"body\"", "name=\"a &amp; b\"", 18, "write it as"),
            (
                "name=\"body\"",
                "name=\"body\" name=\"again\"",
                18,
                "each prop once",
            ),
            (
                "name=\"body\"",
                "name=\"body\" widgets={[]}",
                18,
                "written as the element's children",
            ),
            ("name=\"body\"", "{...rest}", 18, "a prop, not a spread"),
            (
                "name=\"body\"",
                "name=\"body\" tabIndex={1.5}",
                18,
                "a whole number",
            ),
            (
                "name=\"grid2\"",
                "name=\"grid2\" type={null}",
                20,
                "it is not stated",
            ),
            (
                "name=\"body\"",
                "name=\"body\" tabIndex=\"3\"",
                18,
                "`tabIndex` holds a number",
            ),
            ("name=\"body\"", "name={3}", 18, "`name` holds a text"),
            (
                "<Label name=\"hint\" />",
                "<Page name=\"Inner\" />",
                16,
                "is a file of its own",
            ),
            (
                "import { missing, named, page, unset }",
                "import { missing, page, unset }",
                10,
                "`named` is not imported",
            ),
            (
                "from \"@/widgets/Grid\"",
                "from \"@/widgets/Other\"",
                19,
                "an element imported",
            ),
            (
                "name=\"Orders\"",
                "name=\"Or ders\"",
                7,
                "the name the model knows it by",
            ),
            ("\"Sales\",", "\"\",", 6, "a module's name"),
            (
                "page(\n  \"Sales\"",
                "snippet(\n  \"Sales\"",
                5,
                "export default page",
            ),
        ] {
            assert!(source.contains(from), "{from}");
            let edited = source.replacen(from, to, 1);
            let error = read_form(&edited, "Orders.tsx", &vocabulary)
                .unwrap_err()
                .to_string();
            assert!(
                error.contains(&format!("Orders.tsx:{line}:")) && error.contains(expected),
                "{to}: {error}"
            );
        }
        // A form is declared by what it is.
        let layout = source
            .replace(
                "import { missing, named, page, unset }",
                "import { layout, missing, named, unset }",
            )
            .replace("export default page(", "export default layout(");
        let error = read_form(&layout, "Orders.tsx", &vocabulary)
            .unwrap_err()
            .to_string();
        assert!(error.contains("does not declare a Forms$Page"), "{error}");
    }

    #[test]
    fn a_new_use_of_a_widget_is_stored_as_its_uses_are() {
        // Every use holds a template in `emptyText`, with its own text.
        let templated = |path: &str, name: &str, caption: &str| {
            let mut widget = grid(path, name, "false", &[], true);
            let pointer = format!("{path}.Type.ObjectType.PropertyTypes[1].ValueType");
            let property = widget
                .document_mut("Object")
                .and_then(|object| object.list_mut("Properties"))
                .and_then(|properties| match &mut properties[1] {
                    NativeValue::Document(property) => property.document_mut("Value"),
                    _ => None,
                })
                .unwrap();
            *property = NativeDocument::new(WIDGET_VALUE)
                .with("Objects", NativeValue::List(2, vec![]))
                .with("PrimitiveValue", "")
                .with(
                    "TextTemplate",
                    NativeDocument::new("Forms$ClientTemplate").with("Template", caption),
                )
                .with("TypePointer", NativeValue::Pointer(pointer));
            widget
        };
        let document = page(vec![
            templated("Widgets[0]", "one", "Nothing here"),
            templated("Widgets[1]", "two", ""),
        ]);
        let vocabulary = vocabulary(&[&document]);
        // What the definition keeps is what the value is made of, not what
        // one use says in it.
        let defined = &vocabulary.widgets[0].defaults["emptyText"];
        assert_eq!(
            defined.get("TextTemplate"),
            Some(&NativeValue::Document(
                NativeDocument::new("Forms$ClientTemplate").with("Template", "")
            ))
        );
        let source = render_form("Sales", &document, &vocabulary).unwrap();
        assert!(source.contains("<Grid name=\"two\" />"), "{source}");
        let added = source.replace(
            "<Grid name=\"two\" />",
            "<Grid name=\"two\" />\n    <Grid name=\"three\" />",
        );
        let read = read_form(&added, "Orders.tsx", &vocabulary).unwrap().1;
        let expected = page(vec![
            templated("Widgets[0]", "one", "Nothing here"),
            templated("Widgets[1]", "two", ""),
            templated("Widgets[2]", "three", ""),
        ]);
        assert_eq!(read.document, expected);
    }

    /// A use of a widget with a template, an attribute and a number, each
    /// property holding `values` in the field a property of its type is for.
    fn card(
        path: &str,
        name: &str,
        title: Option<&str>,
        attribute: Option<&str>,
    ) -> NativeDocument {
        let ty = format!("{path}.Type.ObjectType");
        let value = |index: usize, template: NativeValue, attribute: NativeValue| {
            NativeDocument::new(WIDGET_VALUE)
                .with("AttributeRef", attribute)
                .with("Objects", NativeValue::List(2, vec![]))
                .with("PrimitiveValue", "")
                .with("TextTemplate", template)
                .with(
                    "TypePointer",
                    NativeValue::Pointer(format!("{ty}.PropertyTypes[{index}].ValueType")),
                )
        };
        let template = title.map_or(NativeValue::Null, |title| {
            NativeDocument::new("Forms$ClientTemplate")
                .with("Fallback", "")
                .with("Template", text(&[("en_US", title)]))
                .into()
        });
        let attribute = attribute.map_or(NativeValue::Null, |attribute| {
            NativeDocument::new("DomainModels$AttributeRef")
                .with("Attribute", attribute)
                .into()
        });
        NativeDocument::new(CUSTOM_WIDGET)
            .with("Name", name)
            .with(
                "Object",
                object(
                    &ty,
                    vec![
                        (0, value(0, template, NativeValue::Null)),
                        (1, value(1, NativeValue::Null, attribute)),
                        (2, value(2, NativeValue::Null, NativeValue::Null)),
                    ],
                ),
            )
            .with(
                "Type",
                NativeDocument::new("CustomWidgets$CustomWidgetType")
                    .with(
                        "ObjectType",
                        object_type(vec![
                            property_type("title", value_type("TextTemplate", "", None)),
                            property_type("attr", value_type("Attribute", "", None)),
                            property_type("count", value_type("Integer", "", None)),
                        ]),
                    )
                    .with("WidgetId", "com.example.widget.web.card.Card"),
            )
    }

    #[test]
    fn a_property_is_the_one_thing_its_type_is_for() {
        // Most uses leave the template and the attribute out, so a property
        // holds neither before it says anything.
        let document = page(vec![
            card(
                "Widgets[0]",
                "one",
                Some("Filled"),
                Some("Sales.Order.Number"),
            ),
            card("Widgets[1]", "two", None, None),
            card("Widgets[2]", "three", None, None),
        ]);
        let vocabulary = vocabulary(&[&document]);
        assert!(vocabulary.widgets[0].defaults.is_empty());
        let source = render_form("Sales", &document, &vocabulary).unwrap();
        assert!(
            source.contains(
                "<Card name=\"one\" properties={{ title: { en_US: \"Filled\" }, attr: \"Sales.Order.Number\" }} />"
            ),
            "{source}"
        );
        assert_eq!(
            read_form(&source, "Orders.tsx", &vocabulary)
                .unwrap()
                .1
                .document,
            document
        );
        // Said of a use that held nothing there, it is the same elements.
        let edited = source.replace(
            "<Card name=\"two\" />",
            "<Card name=\"two\" properties={{ title: { en_US: \"Filled\" }, attr: (\"Sales.Order.Number\" as string) }} />",
        );
        let read = read_form(&edited, "Orders.tsx", &vocabulary)
            .unwrap()
            .1
            .document;
        let expected = page(vec![
            card(
                "Widgets[0]",
                "one",
                Some("Filled"),
                Some("Sales.Order.Number"),
            ),
            card(
                "Widgets[1]",
                "two",
                Some("Filled"),
                Some("Sales.Order.Number"),
            ),
            card("Widgets[2]", "three", None, None),
        ]);
        assert_eq!(read, expected);
        // What a property of the type does not hold is refused by its key.
        for (to, expected) in [
            (
                "attr: 5",
                "`attr` states its `attribute`, which holds a text",
            ),
            (
                "attr: { en_US: \"x\" }",
                "`attr` states its `attribute`, which holds a text",
            ),
            ("count: 5", "`count` holds a text"),
            (
                "title: 5",
                "`title` states its `template`, which holds an element",
            ),
            ("count: [1]", "`count` does not hold a list"),
        ] {
            let edited = source.replace(
                "<Card name=\"two\" />",
                &format!("<Card name=\"two\" properties={{{{ {to} }}}} />"),
            );
            let error = read_form(&edited, "Orders.tsx", &vocabulary)
                .unwrap_err()
                .to_string();
            assert!(error.contains(expected), "{to}: {error}");
        }
    }

    #[test]
    fn a_prop_that_says_one_thing_of_its_element_is_read_as_that_thing() {
        let document = page(vec![
            container("a", "card", vec![]),
            container("b", "", vec![]),
            container("c", "", vec![]),
        ]);
        let vocabulary = vocabulary(&[&document]);
        let source = render_form("Sales", &document, &vocabulary).unwrap();
        assert!(
            source.contains("<DivContainer name=\"a\" appearance=\"card\" />"),
            "{source}"
        );
        let read = |to: &str| {
            read_form(
                &source.replace("appearance=\"card\"", to),
                "Orders.tsx",
                &vocabulary,
            )
        };
        // Nothing, however TypeScript is told to read it, is no element.
        let nulled = read("appearance={null as never}").unwrap().1.document;
        let Some(NativeValue::List(_, widgets)) = nulled.get("Widgets") else {
            panic!("{nulled:?}");
        };
        let NativeValue::Document(first) = &widgets[0] else {
            panic!("{widgets:?}");
        };
        assert_eq!(first.get("Appearance"), Some(&NativeValue::Null));
        for (to, expected) in [
            (
                "appearance={5}",
                "`appearance` states its `class`, which holds a text",
            ),
            (
                "appearance={{ en_US: \"x\" }}",
                "`appearance` states its `class`, which holds a text, not texts",
            ),
            ("appearance={[\"a\"]}", "`class` does not hold a list"),
        ] {
            let error = read(to).unwrap_err().to_string();
            assert!(error.contains(expected), "{to}: {error}");
        }
        // The field an element is stated for is one of its own, and no list.
        let elements = render_elements(&vocabulary.shapes);
        let listed = elements.replace(
            "  widgets: children(2),\n});",
            "  widgets: children(2),\n}, \"widgets\");",
        );
        assert_ne!(listed, elements);
        assert!(
            read_elements(&listed, "elements.ts")
                .unwrap_err()
                .to_string()
                .contains("that is no list")
        );
    }

    #[test]
    fn the_elements_file_is_read_as_it_is_written() {
        let document = orders();
        let mined = mine(&[&document]);
        let source = render_elements(&mined.shapes);
        assert!(
            source.contains(
                "export const DivContainer = element(\"Forms$DivContainer\", {\n  appearance: Appearance,\n  name: \"\",\n  tabIndex: 0,\n  widgets: children(2),\n});\n"
            ),
            "{source}"
        );
        // An element mostly stated for one field says which, and a prop
        // that says only that is the field's value.
        assert!(
            source.contains(
                "export const Appearance = element(\"Forms$Appearance\", {\n  class: \"\",\n  style: \"\",\n}, \"class\");\n"
            ),
            "{source}"
        );
        let unknown = source.replace("}, \"class\");", "}, \"colour\");");
        assert!(
            read_elements(&unknown, "elements.ts")
                .unwrap_err()
                .to_string()
                .contains("one of the element's fields")
        );
        for (from, to, expected) in [
            (
                "tabIndex: 0,",
                "tabIndex: zero(),",
                "`list(n)`, `children(n)` or `long(n)`",
            ),
            (
                "appearance: Appearance,",
                "appearance: Missing,",
                "an element declared above",
            ),
            (
                "name: \"\",",
                "name: \"\",\n  name: \"x\",",
                "each field once",
            ),
            ("name: \"\",", "Name: \"\",", "a field's name as a prop"),
            (
                "element(\"Forms$DivContainer\"",
                "element(\"DivContainer\"",
                "a stored type",
            ),
            (
                "export const DivContainer =",
                "export const divContainer =",
                "a component's name",
            ),
        ] {
            assert!(source.contains(from), "{from}");
            let error = read_elements(&source.replacen(from, to, 1), "elements.ts")
                .unwrap_err()
                .to_string();
            assert!(error.contains(expected), "{to}: {error}");
        }
    }
}
