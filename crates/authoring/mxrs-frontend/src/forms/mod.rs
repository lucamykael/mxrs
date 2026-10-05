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
}

impl Shape {
    pub(crate) fn field(&self, key: &str) -> Option<&Field> {
        self.fields.iter().find(|field| field.key == key)
    }

    pub(crate) fn field_of_prop(&self, prop: &str) -> Option<&Field> {
        self.fields.iter().find(|field| field.prop == prop)
    }

    /// Whether `document` has exactly this element's fields, in order.
    fn fits(&self, document: &NativeDocument) -> bool {
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
            r#"import { Appearance, DivContainer, Label, Page, WidgetValue } from "@/mxrs/elements";
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
    <DivContainer name="header" appearance={<Appearance class="card" />}>
      <Label name="title" caption={{ en_US: "Orders & \"more\"" }} />
      <Label name="hint" />
    </DivContainer>
    <DivContainer name="body" />
    <Grid
      name="grid1"
      properties={{
        advanced: <WidgetValue primitiveValue="true" />,
        columns: <WidgetValue
          objects={[{ header: <WidgetValue primitiveValue="Number" /> }, {}]}
        />,
      }}
    />
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
                22,
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
                28,
                "it is not stated",
            ),
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
        ] {
            assert!(source.contains(from), "{from}");
            let error = read_elements(&source.replacen(from, to, 1), "elements.ts")
                .unwrap_err()
                .to_string();
            assert!(error.contains(expected), "{to}: {error}");
        }
    }
}
