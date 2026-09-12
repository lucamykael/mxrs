//! ES-module emitter that integrates specialized and generic widget bundles.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use mxrs_bson::{Document, parse_array};

use crate::{
    ComboBoxBundleCompiler, CompilerError, DataGridBundleCompiler, GalleryBundleCompiler,
    GenericWidgetBundleCompiler, ImageBundleCompiler,
};

type NativeRenderer<'a> = dyn Fn(&Document, Option<&str>, &str) -> Option<String> + 'a;
type ActionRenderer<'a> = dyn Fn(&Document) -> Option<String> + 'a;
type NanoflowRenderer<'a> = dyn Fn(&str) -> Option<String> + 'a;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageBundle {
    pub qualified_name: String,
    pub source: String,
    pub unsupported_widgets: Vec<String>,
    pub unsupported_custom_widgets: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum UsedBundle {
    ComboBox,
    DataGrid,
    Gallery,
    Image,
    Generic,
}

#[derive(Default)]
struct RenderState {
    used: BTreeSet<UsedBundle>,
    generic_widgets: BTreeMap<String, String>,
    unsupported: BTreeSet<String>,
    unsupported_custom: BTreeSet<String>,
}

/// Emits Runtime-loadable page and layout modules. Native Forms widgets are
/// deliberately delegated to a caller-supplied renderer; custom widgets are
/// dispatched through Data Grid 2, Gallery, Image, Combo Box, then generic.
pub struct PageBundleCompiler<'a> {
    documents: &'a [(String, Document)],
    project_path: &'a Path,
    native_renderer: Option<&'a NativeRenderer<'a>>,
    action_renderer: Option<&'a ActionRenderer<'a>>,
    nanoflow_renderer: Option<&'a NanoflowRenderer<'a>>,
    nanoflow_declarations: &'a str,
}

impl<'a> PageBundleCompiler<'a> {
    pub fn new(documents: &'a [(String, Document)], project_path: &'a Path) -> Self {
        Self {
            documents,
            project_path,
            native_renderer: None,
            action_renderer: None,
            nanoflow_renderer: None,
            nanoflow_declarations: "",
        }
    }

    pub fn with_native_renderer(mut self, renderer: &'a NativeRenderer<'a>) -> Self {
        self.native_renderer = Some(renderer);
        self
    }

    pub fn with_action_renderer(mut self, renderer: &'a ActionRenderer<'a>) -> Self {
        self.action_renderer = Some(renderer);
        self
    }

    pub fn with_nanoflow_renderer(
        mut self,
        renderer: &'a NanoflowRenderer<'a>,
        declarations: &'a str,
    ) -> Self {
        self.nanoflow_renderer = Some(renderer);
        self.nanoflow_declarations = declarations;
        self
    }

    pub fn compile_page(
        &self,
        module_name: &str,
        page: &Document,
    ) -> Result<PageBundle, CompilerError> {
        let qualified_name = format!("{module_name}.{}", page.get_str("Name").unwrap_or_default());
        let context = RenderContext::new(self, &qualified_name, "p");
        let arguments = page
            .get_document("FormCall")
            .ok()
            .map(|call| array_docs(call, "Arguments"))
            .unwrap_or_default();
        let mut slots = Vec::new();
        let mut names = BTreeSet::new();
        for argument in arguments {
            let slot = argument.get_str("Parameter").unwrap_or_default();
            if !qualified_name_valid(slot) {
                return Err(CompilerError::InvalidPageSlot {
                    name: slot.to_string(),
                });
            }
            if !names.insert(slot.to_string()) {
                return Err(CompilerError::DuplicatePageSlot {
                    name: slot.to_string(),
                });
            }
            let rendered = context.render_widgets(&array_docs(&argument, "Widgets"), None, "");
            slots.push((
                slot.to_string(),
                format!(
                    "renderKey => React.createElement(PageFragment, {{ renderKey }}, {rendered})"
                ),
            ));
        }
        let content = js_object_owned(&slots);
        let imports = context.imports();
        let title = translated_text(page.get_document("Title").ok());
        let classes = page
            .get_document("Appearance")
            .ok()
            .and_then(|appearance| appearance.get_str("Class").ok())
            .unwrap_or_default();
        let parameters = page_parameters(page);
        let source = format!(
            "import React from \"react\";\nimport {{ PageFragment }} from \"mendix/PageFragment\";\n{imports}\n{}\n\nexport const title = {};\nexport const classes = {};\nexport const autofocus = \"off\";\nexport const style = {{}};\nexport const parameters = {parameters};\nexport const content = {content};\n",
            self.nanoflow_declarations,
            js_string(&title),
            js_string(classes),
        );
        Ok(context.finish(source))
    }

    pub fn compile_layout(
        &self,
        module_name: &str,
        layout: &Document,
    ) -> Result<PageBundle, CompilerError> {
        let qualified_name = format!(
            "{module_name}.{}",
            layout.get_str("Name").unwrap_or_default()
        );
        let context = RenderContext::new(self, &qualified_name, "l");
        let widgets = layout
            .get_document("Content")
            .ok()
            .map(|content| array_docs(content, "Widgets"))
            .unwrap_or_default();
        let rendered = context.render_widgets(&widgets, None, "");
        let imports = context.imports();
        let source = format!(
            "import React from \"react\";\n{imports}\n{}\n\nexport const content = Object.assign({{}}, {{ \"Main\": {rendered} }});\n",
            self.nanoflow_declarations,
        );
        Ok(context.finish(source))
    }
}

struct RenderContext<'a, 'b> {
    compiler: &'b PageBundleCompiler<'a>,
    qualified_name: &'b str,
    key_prefix: &'b str,
    state: RefCell<RenderState>,
}

impl<'a, 'b> RenderContext<'a, 'b> {
    fn new(
        compiler: &'b PageBundleCompiler<'a>,
        qualified_name: &'b str,
        key_prefix: &'b str,
    ) -> Self {
        Self {
            compiler,
            qualified_name,
            key_prefix,
            state: RefCell::new(RenderState::default()),
        }
    }

    fn render_widgets(&self, widgets: &[Document], scope: Option<&str>, entity: &str) -> String {
        format!(
            "[{}]",
            widgets
                .iter()
                .map(|widget| self.render_widget(widget, scope, entity))
                .collect::<Vec<_>>()
                .join(", ")
        )
    }

    fn render_widget(&self, widget: &Document, scope: Option<&str>, entity: &str) -> String {
        if widget.get_str("$Type").ok() == Some("CustomWidgets$CustomWidget") {
            return self.render_custom_widget(widget, scope, entity);
        }
        if let Some(rendered) = self
            .compiler
            .native_renderer
            .and_then(|renderer| renderer(widget, scope, entity))
        {
            return rendered;
        }
        self.state.borrow_mut().unsupported.insert(
            widget
                .get_str("$Type")
                .unwrap_or("<missing $Type>")
                .to_string(),
        );
        "null".to_string()
    }

    fn render_custom_widget(&self, widget: &Document, scope: Option<&str>, entity: &str) -> String {
        let nested = |widgets: &[Document], nested_scope: &str, nested_entity: &str| {
            self.render_widgets(widgets, Some(nested_scope), nested_entity)
        };
        let grid =
            DataGridBundleCompiler::new(self.compiler.documents, self.qualified_name, widget)
                .with_widget_renderer(&nested);
        if grid.supported() {
            self.state.borrow_mut().used.insert(UsedBundle::DataGrid);
            return grid.render();
        }

        let gallery =
            GalleryBundleCompiler::new(self.compiler.documents, self.qualified_name, widget);
        if gallery.supported() {
            let nano = gallery.nanoflow_name().and_then(|name| {
                self.compiler
                    .nanoflow_renderer
                    .and_then(|renderer| renderer(name))
            });
            if gallery.nanoflow_name().is_some() && nano.is_none() {
                return self.unsupported_custom(widget);
            }
            let content = self.render_widgets(
                &gallery.content_widgets(),
                Some(&gallery.widget_key()),
                gallery.entity_name(),
            );
            let filters = gallery.filter_widgets();
            let rendered_filters = (!filters.is_empty()).then(|| {
                self.render_widgets(&filters, Some(&gallery.widget_key()), gallery.entity_name())
            });
            self.state.borrow_mut().used.insert(UsedBundle::Gallery);
            return gallery.render(&content, nano.as_deref(), rendered_filters.as_deref());
        }

        let action = |action: &Document| {
            self.compiler
                .action_renderer
                .and_then(|renderer| renderer(action))
        };
        let mut image =
            ImageBundleCompiler::new(self.compiler.documents, self.qualified_name, widget)
                .with_key_prefix(self.key_prefix)
                .with_action_renderer(&action);
        if let Some(scope) = scope {
            image = image.with_scope(scope);
        }
        if image.supported() {
            self.state.borrow_mut().used.insert(UsedBundle::Image);
            return image.render();
        }

        let combo = ComboBoxBundleCompiler::new(
            self.compiler.documents,
            self.qualified_name,
            widget,
            scope,
            entity,
        );
        if combo.supported() {
            self.state.borrow_mut().used.insert(UsedBundle::ComboBox);
            return combo.render();
        }

        let generic_nested = |widgets: &[Document]| self.render_widgets(widgets, scope, entity);
        let generic = GenericWidgetBundleCompiler::new(
            self.compiler.documents,
            self.compiler.project_path,
            self.qualified_name,
            widget,
            scope,
        )
        .with_key_prefix(self.key_prefix)
        .with_widget_renderer(&generic_nested)
        .with_action_renderer(&action);
        if generic.supported() {
            let mut state = self.state.borrow_mut();
            state.used.insert(UsedBundle::Generic);
            state.generic_widgets.insert(
                generic.component_name().to_string(),
                generic.module_path().to_string(),
            );
            drop(state);
            return generic.render();
        }
        self.unsupported_custom(widget)
    }

    fn unsupported_custom(&self, widget: &Document) -> String {
        let id = widget
            .get_document("Type")
            .ok()
            .and_then(|type_| type_.get_str("WidgetId").ok())
            .unwrap_or("<unknown custom widget>");
        self.state
            .borrow_mut()
            .unsupported_custom
            .insert(id.to_string());
        "null".to_string()
    }

    fn imports(&self) -> String {
        let state = self.state.borrow();
        if state.used.is_empty() {
            return String::new();
        }
        let mut imports = BTreeSet::new();
        let mut widgets = BTreeSet::new();
        imports.insert("import { asPluginWidgets } from \"mendix\";".to_string());
        if state.used.contains(&UsedBundle::DataGrid) {
            add_property_imports(
                &mut imports,
                &[
                    "DatabaseObjectListProperty",
                    "ExpressionProperty",
                    "ListAssociationProperty",
                    "ListAttributeProperty",
                    "ListExpressionProperty",
                    "SelectionProperty",
                    "TemplatedWidgetProperty",
                ],
            );
            imports.insert(
                "import Datagrid from \"../widgets/com/mendix/widget/web/datagrid/Datagrid.mjs\";"
                    .to_string(),
            );
            widgets.insert("Datagrid".to_string());
        }
        if state.used.contains(&UsedBundle::Gallery) {
            add_property_imports(
                &mut imports,
                &[
                    "DatabaseObjectListProperty",
                    "ExpressionProperty",
                    "MicroflowObjectListProperty",
                    "NanoflowObjectListProperty",
                    "SelectionProperty",
                    "TemplatedWidgetProperty",
                ],
            );
            imports.insert(
                "import { Gallery } from \"../widgets/com/mendix/widget/web/gallery/Gallery.mjs\";"
                    .to_string(),
            );
            widgets.insert("Gallery".to_string());
        }
        if state.used.contains(&UsedBundle::Image) {
            add_property_imports(
                &mut imports,
                &[
                    "ActionProperty",
                    "ExpressionProperty",
                    "WebStaticImageProperty",
                ],
            );
            imports.insert(
                "import { Image } from \"../widgets/com/mendix/widget/web/image/Image.mjs\";"
                    .to_string(),
            );
            widgets.insert("Image".to_string());
        }
        if state.used.contains(&UsedBundle::ComboBox) {
            add_property_imports(
                &mut imports,
                &[
                    "AssociationProperty",
                    "AttributeProperty",
                    "DatabaseObjectListProperty",
                    "ExpressionProperty",
                    "ListAttributeProperty",
                    "ListExpressionProperty",
                    "MicroflowObjectListProperty",
                    "SelectionProperty",
                ],
            );
            imports
                .insert("import { FormGroup } from \"mendix/widgets/web/FormGroup\";".to_string());
            imports.insert(
                "import Combobox from \"../widgets/com/mendix/widget/web/combobox/Combobox.mjs\";"
                    .to_string(),
            );
            widgets.insert("Combobox".to_string());
            widgets.insert("FormGroup".to_string());
        }
        if state.used.contains(&UsedBundle::Generic) {
            add_property_imports(
                &mut imports,
                &[
                    "ActionProperty",
                    "AttributeProperty",
                    "DatabaseObjectListProperty",
                    "ExpressionProperty",
                    "ListAttributeProperty",
                    "ListExpressionProperty",
                    "SelectionProperty",
                    "TemplatedWidgetProperty",
                ],
            );
            for (name, path) in &state.generic_widgets {
                imports.insert(format!(
                    "import * as {name}WidgetModule from \"../widgets/{path}.mjs\";"
                ));
                imports.insert(format!(
                    "const {name} = {name}WidgetModule[{}] || {name}WidgetModule.default;",
                    js_string(name)
                ));
                widgets.insert(name.clone());
            }
        }
        let mut lines = imports.into_iter().collect::<Vec<_>>();
        lines.push(format!(
            "const {{ {} }} = asPluginWidgets({{ {} }});",
            widgets
                .iter()
                .map(|name| format!("${name}"))
                .collect::<Vec<_>>()
                .join(", "),
            widgets.into_iter().collect::<Vec<_>>().join(", ")
        ));
        lines.join("\n")
    }

    fn finish(&self, source: String) -> PageBundle {
        let state = self.state.borrow();
        PageBundle {
            qualified_name: self.qualified_name.to_string(),
            source,
            unsupported_widgets: state.unsupported.iter().cloned().collect(),
            unsupported_custom_widgets: state.unsupported_custom.iter().cloned().collect(),
        }
    }
}

fn add_property_imports(imports: &mut BTreeSet<String>, names: &[&str]) {
    imports.extend(
        names
            .iter()
            .map(|name| format!("import {{ {name} }} from \"mendix/{name}\";")),
    );
}

fn page_parameters(page: &Document) -> String {
    let entries = array_docs(page, "Parameters")
        .into_iter()
        .filter_map(|parameter| {
            let name = parameter.get_str("Name").ok()?;
            let type_ = parameter.get_document("ParameterType").ok()?;
            (identifier(name) && type_.get_str("$Type").ok() == Some("DataTypes$ObjectType")).then(
                || {
                    (
                        format!("${name}"),
                        js_object(&[("kind", js_string("object"))]),
                    )
                },
            )
        })
        .collect::<Vec<_>>();
    js_object_owned(&entries)
}

fn translated_text(text: Option<&Document>) -> String {
    let items = text
        .map(|text| array_docs(text, "Items"))
        .unwrap_or_default();
    items
        .iter()
        .find(|item| item.get_str("LanguageCode").ok() == Some("en_US"))
        .or_else(|| items.first())
        .and_then(|item| item.get_str("Text").ok())
        .unwrap_or_default()
        .to_string()
}

fn array_docs(document: &Document, field: &str) -> Vec<Document> {
    parse_array(document.get_array(field).ok().map(Vec::as_slice))
        .items
        .into_iter()
        .filter_map(|value| value.as_document().cloned())
        .collect()
}

fn identifier(value: &str) -> bool {
    let mut chars = value.chars();
    chars
        .next()
        .is_some_and(|first| first == '_' || first == '$' || first.is_ascii_alphabetic())
        && chars.all(|character| {
            character == '_' || character == '$' || character.is_ascii_alphanumeric()
        })
}

fn qualified_name_valid(value: &str) -> bool {
    value.split('.').count() >= 2 && value.split('.').all(identifier)
}

fn js_string(value: &str) -> String {
    serde_json::to_string(value).expect("strings always serialize")
}

fn js_object(values: &[(&str, String)]) -> String {
    format!(
        "{{ {} }}",
        values
            .iter()
            .map(|(key, value)| format!("{}: {value}", js_string(key)))
            .collect::<Vec<_>>()
            .join(", ")
    )
}

fn js_object_owned(values: &[(String, String)]) -> String {
    format!(
        "{{ {} }}",
        values
            .iter()
            .map(|(key, value)| format!("{}: {value}", js_string(key)))
            .collect::<Vec<_>>()
            .join(", ")
    )
}

#[cfg(test)]
mod tests {
    use std::fs;

    use mxrs_bson::{Bson, doc};
    use tempfile::tempdir;

    use super::*;

    fn custom_widget(id: &str, name: &str) -> Document {
        doc! {
            "$Type": "CustomWidgets$CustomWidget",
            "Name": name,
            "Type": {
                "WidgetId": id,
                "ObjectType": {
                    "$ID": "object-type",
                    "PropertyTypes": mxrs_bson::build_array(Vec::new(), 2),
                },
            },
            "Object": {
                "TypePointer": "object-type",
                "Properties": mxrs_bson::build_array(Vec::new(), 2),
            },
        }
    }

    fn page(widgets: Vec<Document>) -> Document {
        doc! {
            "$Type": "Forms$Page",
            "Name": "Home",
            "Title": { "Items": mxrs_bson::build_array(vec![Bson::Document(doc! {
                "LanguageCode": "en_US", "Text": "Home",
            })], 3) },
            "Appearance": { "Class": "home" },
            "Parameters": mxrs_bson::build_array(vec![Bson::Document(doc! {
                "Name": "Order",
                "ParameterType": { "$Type": "DataTypes$ObjectType", "Entity": "Demo.Order" },
            })], 2),
            "FormCall": {
                "Arguments": mxrs_bson::build_array(vec![Bson::Document(doc! {
                    "Parameter": "Demo.Shell.Main",
                    "Widgets": mxrs_bson::build_array(widgets.into_iter().map(Bson::Document).collect(), 2),
                })], 2),
            },
        }
    }

    #[test]
    fn emits_page_module_and_registers_generic_widget() {
        let temp = tempdir().unwrap();
        fs::create_dir_all(temp.path().join("widgets/example")).unwrap();
        fs::write(
            temp.path().join("widgets/example/Clock.mjs"),
            "export default {};",
        )
        .unwrap();
        let project = temp.path().join("App.mpr");
        let documents = Vec::new();
        let page = page(vec![custom_widget("example.Clock", "clock")]);
        let bundle = PageBundleCompiler::new(&documents, &project)
            .compile_page("Demo", &page)
            .unwrap();
        assert_eq!(bundle.qualified_name, "Demo.Home");
        assert!(bundle.unsupported_widgets.is_empty());
        assert!(bundle.unsupported_custom_widgets.is_empty());
        for expected in [
            "import React",
            "ClockWidgetModule",
            "asPluginWidgets",
            "React.createElement($Clock",
            "export const title = \"Home\"",
            "export const classes = \"home\"",
            "\"$Order\": { \"kind\": \"object\" }",
            "\"Demo.Shell.Main\": renderKey =>",
        ] {
            assert!(
                bundle.source.contains(expected),
                "missing {expected}: {}",
                bundle.source
            );
        }
    }

    #[test]
    fn delegates_native_widgets_and_reports_both_unsupported_classes() {
        let temp = tempdir().unwrap();
        let project = temp.path().join("App.mpr");
        let documents = Vec::new();
        let native = |_widget: &Document, _scope: Option<&str>, _entity: &str| {
            Some("React.createElement(\"span\", null, \"ok\")".to_string())
        };
        let native_page = page(vec![doc! { "$Type": "Forms$DynamicText", "Name": "text" }]);
        let bundle = PageBundleCompiler::new(&documents, &project)
            .with_native_renderer(&native)
            .compile_page("Demo", &native_page)
            .unwrap();
        assert!(bundle.source.contains("React.createElement(\"span\""));
        assert!(bundle.unsupported_widgets.is_empty());

        let unsupported = page(vec![
            doc! { "$Type": "Forms$Unknown", "Name": "unknown" },
            custom_widget("missing.Widget", "missing"),
        ]);
        let bundle = PageBundleCompiler::new(&documents, &project)
            .compile_page("Demo", &unsupported)
            .unwrap();
        assert_eq!(bundle.unsupported_widgets, vec!["Forms$Unknown"]);
        assert_eq!(bundle.unsupported_custom_widgets, vec!["missing.Widget"]);
    }

    #[test]
    fn rejects_duplicate_slots_and_emits_layout_module() {
        let temp = tempdir().unwrap();
        let project = temp.path().join("App.mpr");
        let documents = Vec::new();
        let mut duplicate = page(Vec::new());
        let argument = duplicate
            .get_document("FormCall")
            .unwrap()
            .get_array("Arguments")
            .unwrap()[1]
            .as_document()
            .unwrap()
            .clone();
        duplicate
            .get_document_mut("FormCall")
            .unwrap()
            .get_array_mut("Arguments")
            .unwrap()
            .push(Bson::Document(argument));
        assert!(matches!(
            PageBundleCompiler::new(&documents, &project).compile_page("Demo", &duplicate),
            Err(CompilerError::DuplicatePageSlot { .. })
        ));

        let layout = doc! {
            "$Type": "Forms$Layout",
            "Name": "Shell",
            "Content": { "Widgets": mxrs_bson::build_array(Vec::new(), 2) },
        };
        let bundle = PageBundleCompiler::new(&documents, &project)
            .compile_layout("Demo", &layout)
            .unwrap();
        assert_eq!(bundle.qualified_name, "Demo.Shell");
        assert!(
            bundle
                .source
                .contains("export const content = Object.assign")
        );
    }
}
