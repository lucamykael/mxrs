//! ES-module emitter that integrates specialized and generic widget bundles.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use mxrs_bson::{Document, parse_array};
use mxrs_compiler_support::operation_id;

use crate::{
    ComboBoxBundleCompiler, CompilerError, DataGridBundleCompiler, GalleryBundleCompiler,
    GenericWidgetBundleCompiler, ImageBundleCompiler,
};

type NativeRenderer<'a> = dyn Fn(&Document, Option<&str>, &str) -> Option<String> + 'a;
type ActionRenderer<'a> = dyn Fn(&Document) -> Option<String> + 'a;
type NanoflowRenderer<'a> = dyn Fn(&str) -> Option<String> + 'a;
type NanoflowDeclarations<'a> = dyn Fn() -> String + 'a;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageBundle {
    pub qualified_name: String,
    pub source: String,
    pub unsupported_widgets: Vec<String>,
    pub unsupported_custom_widgets: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum UsedBundle {
    ActionButton,
    BoundText,
    ComboBox,
    Container,
    DataView,
    DataGrid,
    FormInput,
    FileManager,
    Gallery,
    Image,
    ListView,
    NativeImage,
    NavigationList,
    ReferenceSelector,
    ReferenceSetSelector,
    ScrollContainer,
    Generic,
}

#[derive(Default)]
struct RenderState {
    used: BTreeSet<UsedBundle>,
    form_widgets: BTreeSet<String>,
    generic_widgets: BTreeMap<String, String>,
    unsupported: BTreeSet<String>,
    unsupported_custom: BTreeSet<String>,
}

/// Emits Runtime-loadable page and layout modules. Native Forms widgets not
/// handled by the built-in renderer can be delegated to a caller-supplied
/// renderer; custom widgets are dispatched through specialized bundles, then
/// the generic compiler.
pub struct PageBundleCompiler<'a> {
    documents: &'a [(String, Document)],
    project_path: &'a Path,
    native_renderer: Option<&'a NativeRenderer<'a>>,
    action_renderer: Option<&'a ActionRenderer<'a>>,
    nanoflow_renderer: Option<&'a NanoflowRenderer<'a>>,
    nanoflow_declarations: &'a str,
    nanoflow_declarations_renderer: Option<&'a NanoflowDeclarations<'a>>,
    package_modules: RefCell<BTreeMap<String, bool>>,
}

/// Owning project adapter used by acceptance tooling and callers that want
/// to compile every page without assembling `(module, document)` pairs.
pub struct ProjectPageBundleCompiler {
    project_path: PathBuf,
    documents: Vec<(String, Document)>,
    flow_index: mxrs_compiler_flow::ProjectFlowIndex,
}

struct ProjectNanoflowCache<'a> {
    index: &'a mxrs_compiler_flow::ProjectFlowIndex,
    project_root: Option<&'a Path>,
    programs: RefCell<BTreeMap<String, Option<(String, String)>>>,
    requested: RefCell<BTreeSet<String>>,
}

impl<'a> ProjectNanoflowCache<'a> {
    fn new(
        index: &'a mxrs_compiler_flow::ProjectFlowIndex,
        project_root: Option<&'a Path>,
    ) -> Self {
        Self {
            index,
            project_root,
            programs: RefCell::new(BTreeMap::new()),
            requested: RefCell::new(BTreeSet::new()),
        }
    }

    fn reset(&self) {
        self.requested.borrow_mut().clear();
    }

    fn reference(&self, name: &str) -> Option<String> {
        self.requested.borrow_mut().insert(name.to_string());
        if let Some(cached) = self.programs.borrow().get(name) {
            return cached.as_ref().map(|(reference, _)| reference.clone());
        }
        let mut compiler =
            mxrs_compiler_flow::nanoflow::NanoflowCompiler::new(self.index, self.project_root);
        let reference = compiler.reference(name);
        let compiled = reference.map(|reference| (reference, compiler.declarations()));
        self.programs
            .borrow_mut()
            .insert(name.to_string(), compiled.clone());
        compiled.map(|(reference, _)| reference)
    }

    fn declarations(&self) -> String {
        let requested = self.requested.borrow();
        let programs = self.programs.borrow();
        let mut seen = BTreeSet::new();
        let mut lines = Vec::new();
        for name in requested.iter() {
            let Some(Some((_, declarations))) = programs.get(name) else {
                continue;
            };
            for line in declarations.lines() {
                if seen.insert(line.to_string()) {
                    lines.push(line);
                }
            }
        }
        lines.join("\n")
    }
}

impl ProjectPageBundleCompiler {
    pub fn new(project: &mxrs_model::Project) -> Result<Self, CompilerError> {
        let units = project.all_units()?;
        let parent_by_id = units
            .iter()
            .map(|unit| (unit.unit_id.clone(), unit.container_id.clone()))
            .collect::<BTreeMap<_, _>>();
        let module_by_id = project
            .modules()?
            .into_iter()
            .filter_map(|module| module.name.map(|name| (module.id, name)))
            .collect::<BTreeMap<_, _>>();
        let parsed = units
            .iter()
            .map(|unit| {
                project
                    .mpr()
                    .parse_contents(unit)
                    .map(|document| (unit.clone(), document))
                    .map_err(mxrs_model::ModelError::from)
            })
            .collect::<Result<Vec<_>, _>>()?;
        let documents: Vec<(String, Document)> = parsed
            .into_iter()
            .map(|(unit, document)| {
                let owner = owning_module(&unit.container_id, &parent_by_id, &module_by_id)
                    .unwrap_or_default();
                (owner, document)
            })
            .collect();
        let flow_index = mxrs_compiler_flow::ProjectFlowIndex::from_documents(&documents);
        Ok(Self {
            project_path: project.mpr().path().to_path_buf(),
            documents,
            flow_index,
        })
    }

    pub fn documents(&self) -> &[(String, Document)] {
        &self.documents
    }

    pub fn compile_pages(&self) -> Vec<Result<PageBundle, CompilerError>> {
        let programs = ProjectNanoflowCache::new(&self.flow_index, self.project_path.parent());
        self.documents
            .iter()
            .filter(|(_, document)| document.get_str("$Type").ok() == Some("Forms$Page"))
            .map(|(module, page)| {
                programs.reset();
                let render = |name: &str| programs.reference(name);
                let declarations = || programs.declarations();
                PageBundleCompiler::new(&self.documents, &self.project_path)
                    .with_nanoflow_programs(&render, &declarations)
                    .compile_page(module, page)
            })
            .collect()
    }

    pub fn compile_layouts(&self) -> Vec<Result<PageBundle, CompilerError>> {
        let programs = ProjectNanoflowCache::new(&self.flow_index, self.project_path.parent());
        self.documents
            .iter()
            .filter(|(_, document)| document.get_str("$Type").ok() == Some("Forms$Layout"))
            .map(|(module, layout)| {
                programs.reset();
                let render = |name: &str| programs.reference(name);
                let declarations = || programs.declarations();
                PageBundleCompiler::new(&self.documents, &self.project_path)
                    .with_nanoflow_programs(&render, &declarations)
                    .compile_layout(module, layout)
            })
            .collect()
    }
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
            nanoflow_declarations_renderer: None,
            package_modules: RefCell::new(BTreeMap::new()),
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

    pub fn with_nanoflow_programs(
        mut self,
        renderer: &'a NanoflowRenderer<'a>,
        declarations: &'a NanoflowDeclarations<'a>,
    ) -> Self {
        self.nanoflow_renderer = Some(renderer);
        self.nanoflow_declarations_renderer = Some(declarations);
        self
    }

    fn rendered_nanoflow_declarations(&self) -> String {
        self.nanoflow_declarations_renderer
            .map(|renderer| renderer())
            .unwrap_or_else(|| self.nanoflow_declarations.to_string())
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
            self.rendered_nanoflow_declarations(),
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
            self.rendered_nanoflow_declarations(),
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
        if let Some(rendered) = self.render_builtin_native(widget, scope, entity) {
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

    fn render_builtin_native(
        &self,
        widget: &Document,
        scope: Option<&str>,
        entity: &str,
    ) -> Option<String> {
        let type_name = widget.get_str("$Type").ok()?;
        match type_name {
            "Forms$ActionButton" => Some(self.render_action_button(widget, scope, entity)),
            "Forms$LayoutGrid" => Some(self.render_element(
                "div",
                widget,
                &array_docs(widget, "Rows"),
                scope,
                entity,
                "mx-layoutgrid mx-layoutgrid-fluid",
            )),
            "Forms$LayoutGridRow" => Some(self.render_element(
                "div",
                widget,
                &array_docs(widget, "Columns"),
                scope,
                entity,
                "row",
            )),
            "Forms$LayoutGridColumn" => {
                let classes = [
                    "col".to_string(),
                    grid_weight_class("md", widget.get_i32("Weight").unwrap_or(-1)),
                    grid_weight_class("sm", widget.get_i32("TabletWeight").unwrap_or(-1)),
                    grid_weight_class("xs", widget.get_i32("PhoneWeight").unwrap_or(-1)),
                ]
                .into_iter()
                .filter(|value| !value.is_empty())
                .collect::<Vec<_>>()
                .join(" ");
                Some(self.render_element(
                    "div",
                    widget,
                    &array_docs(widget, "Widgets"),
                    scope,
                    entity,
                    &classes,
                ))
            }
            "Forms$DivContainer" | "Forms$Container" => {
                let action = widget.get_document("OnClickAction").ok();
                if action.is_some_and(|action| {
                    action.get_str("$Type").is_ok_and(|type_name| {
                        !type_name.is_empty() && type_name != "Forms$NoAction"
                    })
                }) {
                    let action = action?;
                    let property = self
                        .compiler
                        .action_renderer
                        .and_then(|renderer| renderer(action))
                        .or_else(|| self.builtin_action_property(widget, action, scope, entity));
                    if let Some(property) = property {
                        self.state.borrow_mut().used.insert(UsedBundle::Container);
                        let key = self.widget_key(widget);
                        return Some(format!(
                            "React.createElement($Container, {})",
                            js_object(&[
                                ("key", js_string(&key)),
                                ("$widgetId", js_string(&key)),
                                ("class", js_string(&css_class(widget))),
                                ("renderMode", js_string("div")),
                                (
                                    "content",
                                    self.render_widgets(
                                        &array_docs(widget, "Widgets"),
                                        scope,
                                        entity,
                                    ),
                                ),
                                ("onClick", property),
                            ])
                        ));
                    }
                }
                Some(self.render_element(
                    "div",
                    widget,
                    &array_docs(widget, "Widgets"),
                    scope,
                    entity,
                    "",
                ))
            }
            "Forms$DataView" => self.render_data_view(widget, scope, entity),
            "Forms$DataGrid" => self.render_native_data_grid(widget, scope, entity),
            "Forms$TextBox" => self.render_text_box(widget, scope, entity),
            "Forms$TextArea" => self.render_text_area(widget, scope, entity),
            "Forms$CheckBox" => self.render_simple_input(widget, scope, entity, "CheckBox"),
            "Forms$DatePicker" => self.render_date_picker(widget, scope, entity),
            "Forms$DropDown" => self.render_drop_down(widget, scope, entity),
            "Forms$ListView" => self.render_list_view(widget, scope, entity),
            "Forms$StaticImageViewer" => self.render_static_image(widget),
            "Forms$NavigationList" => self.render_navigation_list(widget, scope, entity),
            "Forms$ScrollContainer" => self.render_scroll_container(widget, scope, entity),
            "Forms$FileManager" => self.render_file_manager(widget, scope),
            "Forms$ReferenceSelector" => {
                self.render_association_selector(widget, scope, entity, false, true)
            }
            "Forms$InputReferenceSetSelector" => {
                self.render_association_selector(widget, scope, entity, true, true)
            }
            "Forms$ReferenceSetSelector" => {
                self.render_association_selector(widget, scope, entity, true, false)
            }
            "Forms$RadioButtonGroup" => {
                self.render_simple_input(widget, scope, entity, "RadioButtonGroup")
            }
            "Forms$DynamicText" | "Forms$Title" => {
                let template = widget
                    .get_document("Content")
                    .ok()
                    .or_else(|| widget.get_document("CaptionTemplate").ok());
                if template.is_some_and(|template| !array_docs(template, "Parameters").is_empty()) {
                    return (type_name == "Forms$DynamicText")
                        .then(|| self.render_bound_text(widget, template.expect("checked"), scope))
                        .flatten();
                }
                let tag = text_mode(widget.get_str("RenderMode").unwrap_or(
                    if type_name == "Forms$Title" {
                        "Heading1"
                    } else {
                        "Text"
                    },
                ));
                Some(format!(
                    "React.createElement({}, {}, {})",
                    js_string(tag),
                    self.html_props(widget, ""),
                    js_string(&client_template_text(template)),
                ))
            }
            "Forms$Label" => Some(format!(
                "React.createElement(\"label\", {}, {})",
                self.html_props(widget, "mx-label"),
                js_string(&client_template_text(
                    widget
                        .get_document("CaptionTemplate")
                        .ok()
                        .or_else(|| widget.get_document("LabelTemplate").ok()),
                )),
            )),
            "Forms$GroupBox" => {
                let children = self.render_widgets(&array_docs(widget, "Widgets"), scope, entity);
                let caption = client_template_text(widget.get_document("CaptionTemplate").ok());
                Some(format!(
                    "React.createElement(\"fieldset\", {}, [React.createElement(\"legend\", {{ key: \"legend\" }}, {}), ...{}])",
                    self.html_props(widget, "mx-groupbox"),
                    js_string(&caption),
                    children,
                ))
            }
            "Forms$Table" => Some(self.render_table(widget, scope, entity)),
            "Forms$TabControl" | "Forms$TabContainer" => {
                Some(self.render_tabs(widget, scope, entity))
            }
            "Forms$SnippetCall" | "Forms$SnippetCallWidget" => {
                self.render_snippet(widget, scope, entity)
            }
            _ => None,
        }
    }

    fn render_element(
        &self,
        tag: &str,
        widget: &Document,
        children: &[Document],
        scope: Option<&str>,
        entity: &str,
        base_class: &str,
    ) -> String {
        format!(
            "React.createElement({}, {}, {})",
            js_string(tag),
            self.html_props(widget, base_class),
            self.render_widgets(children, scope, entity),
        )
    }

    fn render_action_button(
        &self,
        widget: &Document,
        current_scope: Option<&str>,
        current_entity: &str,
    ) -> String {
        let action = widget.get_document("Action").ok();
        let action_property = action.and_then(|action| {
            self.compiler
                .action_renderer
                .and_then(|renderer| renderer(action))
                .or_else(|| {
                    self.builtin_action_property(widget, action, current_scope, current_entity)
                })
        });
        let caption = client_template_text(widget.get_document("CaptionTemplate").ok());
        let Some(action_property) = action_property else {
            let classes = [
                "btn",
                "mx-button",
                button_style(widget).as_str(),
                css_class(widget).as_str(),
            ]
            .into_iter()
            .filter(|value| !value.is_empty())
            .collect::<Vec<_>>()
            .join(" ");
            return format!(
                "React.createElement(\"button\", {}, {})",
                js_object(&[
                    ("key", js_string(&self.widget_key(widget))),
                    ("type", js_string("button")),
                    ("className", js_string(&classes)),
                    ("disabled", "true".to_string()),
                ]),
                js_string(&caption),
            );
        };

        self.state
            .borrow_mut()
            .used
            .insert(UsedBundle::ActionButton);
        let key = self.widget_key(widget);
        let props = js_object(&[
            ("key", js_string(&key)),
            ("$widgetId", js_string(&key)),
            ("buttonId", js_string(&key)),
            ("class", js_string(&css_class(widget))),
            (
                "renderType",
                js_string(
                    if widget
                        .get_str("RenderType")
                        .unwrap_or_default()
                        .eq_ignore_ascii_case("link")
                    {
                        "link"
                    } else {
                        "button"
                    },
                ),
            ),
            ("buttonClass", js_string(&button_style(widget))),
            (
                "tabIndex",
                widget.get_i32("TabIndex").unwrap_or_default().to_string(),
            ),
            (
                "caption",
                format!("TextProperty({{ value: {} }})", js_string(&caption)),
            ),
            (
                "tooltip",
                format!(
                    "TextProperty({{ value: {} }})",
                    js_string(&translated_text(widget.get_document("Tooltip").ok()))
                ),
            ),
            ("action", action_property),
        ]);
        format!("React.createElement($ActionButton, {props})")
    }

    fn render_bound_text(
        &self,
        widget: &Document,
        template: &Document,
        scope: Option<&str>,
    ) -> Option<String> {
        let scope = scope?;
        let values = array_docs(template, "Parameters")
            .into_iter()
            .enumerate()
            .map(|(index, parameter)| {
                let reference = parameter.get_document("AttributeRef").ok()?;
                let qualified = reference.get_str("Attribute").ok()?;
                let (entity, attribute) = qualified.rsplit_once('.')?;
                if !qualified_name_valid(entity) || !identifier(attribute) {
                    return None;
                }
                let path = reference
                    .get_document("EntityRef")
                    .ok()
                    .map(entity_ref_path)
                    .unwrap_or_default();
                Some((
                    format!("value{}", index + 1),
                    bound_text_attribute_property(scope, entity, attribute, &path),
                ))
            })
            .collect::<Option<Vec<_>>>()?;
        self.state.borrow_mut().used.insert(UsedBundle::BoundText);
        let mut props = vec![
            ("key".to_string(), js_string(&self.widget_key(widget))),
            ("$widgetId".to_string(), js_string(&self.widget_key(widget))),
            ("class".to_string(), js_string(&css_class(widget))),
            (
                "renderMode".to_string(),
                js_string(text_mode(widget.get_str("RenderMode").unwrap_or("Text"))),
            ),
            (
                "template".to_string(),
                js_string(&client_template_text(Some(template))),
            ),
        ];
        props.extend(values);
        Some(format!(
            "React.createElement($MxrbFormattedText, {})",
            js_object_owned(&props)
        ))
    }

    fn render_text_box(
        &self,
        widget: &Document,
        scope: Option<&str>,
        entity: &str,
    ) -> Option<String> {
        let (attribute_entity, attribute) = bound_attribute(widget, scope)?;
        let key = self.widget_key(widget);
        let max_length = widget.get_i32("MaxLengthCode").unwrap_or_default();
        let input = self.render_form_input(
            "TextBox",
            widget,
            scope?,
            &attribute_entity,
            &attribute,
            vec![
                (
                    "isPassword",
                    widget
                        .get_bool("IsPasswordBox")
                        .unwrap_or(false)
                        .to_string(),
                ),
                (
                    "mask",
                    js_string(widget.get_str("InputMask").unwrap_or_default()),
                ),
                ("readOnlyStyle", js_string(&read_only_style(widget))),
                (
                    "maxLength",
                    if max_length > 0 {
                        max_length.to_string()
                    } else {
                        "null".to_string()
                    },
                ),
                ("autocomplete", js_string(&autocomplete_value(widget))),
                (
                    "submitWhileEditing",
                    (widget.get_str("SubmitBehaviour").ok() == Some("OnTyping")).to_string(),
                ),
                (
                    "submitDelay",
                    widget
                        .get_i32("SubmitOnInputDelay")
                        .unwrap_or_default()
                        .to_string(),
                ),
                ("id", js_string(&key)),
                (
                    "ariaRequired",
                    widget.get_bool("AriaRequired").unwrap_or(false).to_string(),
                ),
                (
                    "tabIndex",
                    widget.get_i32("TabIndex").unwrap_or_default().to_string(),
                ),
                (
                    "placeholder",
                    text_property(widget.get_document("PlaceholderTemplate").ok()),
                ),
                (
                    "ariaLabel",
                    text_property(
                        widget
                            .get_document("ScreenReaderLabel")
                            .ok()
                            .and_then(|label| label.get_document("Template").ok())
                            .or_else(|| widget.get_document("ScreenReaderLabel").ok()),
                    ),
                ),
            ],
        );
        Some(self.render_form_group(widget, &key, &input, "mx-textbox", entity))
    }

    fn render_drop_down(
        &self,
        widget: &Document,
        scope: Option<&str>,
        entity: &str,
    ) -> Option<String> {
        let (attribute_entity, attribute) = bound_attribute(widget, scope)?;
        let key = self.widget_key(widget);
        let input = self.render_form_input(
            "EnumSelect",
            widget,
            scope?,
            &attribute_entity,
            &attribute,
            vec![
                ("id", js_string(&key)),
                ("readOnlyStyle", js_string(&read_only_style(widget))),
                (
                    "ariaRequired",
                    widget.get_bool("AriaRequired").unwrap_or(false).to_string(),
                ),
                (
                    "tabIndex",
                    widget.get_i32("TabIndex").unwrap_or_default().to_string(),
                ),
                (
                    "emptyOptionCaption",
                    text_property(widget.get_document("EmptyOptionCaption").ok()),
                ),
                (
                    "ariaLabel",
                    text_property(
                        widget
                            .get_document("ScreenReaderLabel")
                            .ok()
                            .and_then(|label| label.get_document("Template").ok())
                            .or_else(|| widget.get_document("ScreenReaderLabel").ok()),
                    ),
                ),
            ],
        );
        Some(self.render_form_group(widget, &key, &input, "mx-dropdown", entity))
    }

    fn render_list_view(
        &self,
        widget: &Document,
        current_scope: Option<&str>,
        _current_entity: &str,
    ) -> Option<String> {
        let source = crate::WebListDataSource::from_documents(self.compiler.documents, widget);
        if !source.supported() || !qualified_name_valid(&source.entity) {
            return None;
        }
        let key = self.widget_key(widget);
        let list_value = if source.xpath() {
            format!(
                "DatabaseObjectListProperty({})",
                js_object(&[
                    ("dataSourceId", js_string(&key)),
                    (
                        "operationId",
                        js_string(&operation_id(
                            self.qualified_name,
                            widget.get_str("Name").unwrap_or_default(),
                        )),
                    ),
                    ("entity", js_string(&source.entity)),
                    ("sort", "[]".to_string()),
                ])
            )
        } else if source.association() {
            let scope = current_scope?;
            format!(
                "AssociationObjectListProperty({})",
                js_object(&[
                    ("dataSourceId", js_string(&key)),
                    (
                        "operationId",
                        js_string(&operation_id(
                            self.qualified_name,
                            widget.get_str("Name").unwrap_or_default(),
                        )),
                    ),
                    ("scope", js_string(scope)),
                    ("directPath", js_string(&source.association_path)),
                    ("sort", "[]".to_string()),
                ])
            )
        } else if source.microflow() {
            let settings = widget
                .get_document("DataSource")
                .ok()?
                .get_document("MicroflowSettings")
                .unwrap_or(widget.get_document("DataSource").ok()?);
            let arg_map = self.flow_argument_map(
                settings,
                &source.microflow_name,
                current_scope,
                _current_entity,
            )?;
            format!(
                "MicroflowObjectListProperty({})",
                js_object(&[
                    ("dataSourceId", js_string(&key)),
                    (
                        "operationId",
                        js_string(&operation_id(
                            self.qualified_name,
                            widget.get_str("Name").unwrap_or_default(),
                        )),
                    ),
                    ("argMap", arg_map),
                    ("fetchOnlyWithAllParams", "false".to_string()),
                ])
            )
        } else {
            let reference = self
                .compiler
                .nanoflow_renderer
                .and_then(|renderer| renderer(&source.nanoflow_name))?;
            format!(
                "NanoflowObjectListProperty({})",
                js_object(&[
                    ("dataSourceId", js_string(&key)),
                    ("source", format!("{{ nanoflow: {reference} }}")),
                    ("argMap", "{}".to_string()),
                    ("fetchOnlyWithAllParams", "false".to_string()),
                ])
            )
        };
        let content =
            self.render_widgets(&array_docs(widget, "Widgets"), Some(&key), &source.entity);
        self.state.borrow_mut().used.insert(UsedBundle::ListView);
        Some(format!(
            "React.createElement($ListView, {})",
            js_object(&[
                ("key", js_string(&key)),
                ("$widgetId", js_string(&key)),
                ("class", js_string(&css_class(widget))),
                (
                    "pageSize",
                    positive_i32(widget.get_i32("PageSize").unwrap_or_default(), 20).to_string(),
                ),
                ("listValue", list_value),
                (
                    "itemTemplate",
                    format!(
                        "TemplatedWidgetProperty({{ children: () => {content}, dataSourceId: {}, editable: {} }})",
                        js_string(&key),
                        widget.get_bool("Editable").unwrap_or(false),
                    ),
                ),
            ])
        ))
    }

    fn render_native_data_grid(
        &self,
        widget: &Document,
        current_scope: Option<&str>,
        current_entity: &str,
    ) -> Option<String> {
        let source = crate::WebListDataSource::from_documents(self.compiler.documents, widget);
        if !source.supported() || !qualified_name_valid(&source.entity) {
            return None;
        }
        let key = self.widget_key(widget);
        let list_value =
            self.native_list_property(widget, &source, &key, current_scope, current_entity)?;
        let columns = array_docs(widget, "Columns");
        let headers = columns
            .iter()
            .map(|column| {
                format!(
                    "React.createElement(\"div\", {}, {})",
                    js_object(&[
                        ("key", js_string(&self.widget_key(column))),
                        (
                            "className",
                            js_string(&format!("mx-datagrid-head-cell {}", css_class(column))),
                        ),
                    ]),
                    js_string(&translated_text(column.get_document("Caption").ok())),
                )
            })
            .collect::<Vec<_>>()
            .join(", ");
        let cells = columns
            .iter()
            .map(|column| {
                let value = column
                    .get_document("AttributeRef")
                    .ok()
                    .and_then(|reference| {
                        let qualified = reference.get_str("Attribute").ok()?;
                        let (entity, attribute) = qualified.rsplit_once('.')?;
                        let path = reference
                            .get_document("EntityRef")
                            .ok()
                            .map(entity_ref_path)
                            .unwrap_or_default();
                        (qualified_name_valid(entity) && identifier(attribute)).then(|| {
                            format!(
                                "React.createElement($MxrbAttributeValue, {{ value: {} }})",
                                bound_text_attribute_property(&key, entity, attribute, &path)
                            )
                        })
                    })
                    .unwrap_or_else(|| "null".to_string());
                format!(
                    "React.createElement(\"div\", {}, {value})",
                    js_object(&[
                        ("key", js_string(&self.widget_key(column))),
                        (
                            "className",
                            js_string(&format!("mx-datagrid-cell {}", css_class(column))),
                        ),
                    ])
                )
            })
            .collect::<Vec<_>>()
            .join(", ");
        let row = format!(
            "React.createElement(\"div\", {{ className: \"mx-datagrid-row\" }}, [{cells}])"
        );
        let list = format!(
            "React.createElement($ListView, {})",
            js_object(&[
                ("key", js_string(&key)),
                ("$widgetId", js_string(&key)),
                (
                    "pageSize",
                    positive_i32(widget.get_i32("NumberOfRows").unwrap_or_default(), 20)
                        .to_string(),
                ),
                ("listValue", list_value),
                (
                    "itemTemplate",
                    format!(
                        "TemplatedWidgetProperty({{ children: () => {row}, dataSourceId: {}, editable: false }})",
                        js_string(&key)
                    ),
                ),
            ])
        );
        let controls = array_docs(
            widget.get_document("ControlBar").unwrap_or(widget),
            "NewButtons",
        )
        .iter()
        .map(|button| self.render_action_button(button, current_scope, current_entity))
        .collect::<Vec<_>>()
        .join(", ");
        self.state.borrow_mut().used.insert(UsedBundle::BoundText);
        self.state.borrow_mut().used.insert(UsedBundle::ListView);
        Some(format!(
            "React.createElement(\"div\", {}, [React.createElement(\"div\", {{ className: \"mx-grid-controlbar\" }}, [{controls}]), React.createElement(\"div\", {{ className: \"mx-datagrid-head\" }}, [{headers}]), {list}])",
            self.html_props(widget, "mx-datagrid mx-datagrid-legacy")
        ))
    }

    fn native_list_property(
        &self,
        widget: &Document,
        source: &crate::WebListDataSource,
        key: &str,
        current_scope: Option<&str>,
        current_entity: &str,
    ) -> Option<String> {
        if source.xpath() {
            return Some(format!(
                "DatabaseObjectListProperty({})",
                js_object(&[
                    ("dataSourceId", js_string(key)),
                    (
                        "operationId",
                        js_string(&operation_id(
                            self.qualified_name,
                            widget.get_str("Name").unwrap_or_default(),
                        )),
                    ),
                    ("entity", js_string(&source.entity)),
                    ("sort", "[]".to_string()),
                ])
            ));
        }
        if source.association() {
            return Some(format!(
                "AssociationObjectListProperty({})",
                js_object(&[
                    ("dataSourceId", js_string(key)),
                    (
                        "operationId",
                        js_string(&operation_id(
                            self.qualified_name,
                            widget.get_str("Name").unwrap_or_default(),
                        )),
                    ),
                    ("scope", js_string(current_scope?)),
                    ("directPath", js_string(&source.association_path)),
                    ("sort", "[]".to_string()),
                ])
            ));
        }
        if source.microflow() {
            let direct = widget.get_document("DataSource").ok()?;
            let settings = direct.get_document("MicroflowSettings").unwrap_or(direct);
            let arg_map = self.flow_argument_map(
                settings,
                &source.microflow_name,
                current_scope,
                current_entity,
            )?;
            return Some(format!(
                "MicroflowObjectListProperty({})",
                js_object(&[
                    ("dataSourceId", js_string(key)),
                    (
                        "operationId",
                        js_string(&operation_id(
                            self.qualified_name,
                            widget.get_str("Name").unwrap_or_default(),
                        )),
                    ),
                    ("argMap", arg_map),
                    ("fetchOnlyWithAllParams", "false".to_string()),
                ])
            ));
        }
        let reference = self
            .compiler
            .nanoflow_renderer
            .and_then(|renderer| renderer(&source.nanoflow_name))?;
        Some(format!(
            "NanoflowObjectListProperty({})",
            js_object(&[
                ("dataSourceId", js_string(key)),
                ("source", format!("{{ nanoflow: {reference} }}")),
                ("argMap", "{}".to_string()),
                ("fetchOnlyWithAllParams", "false".to_string()),
            ])
        ))
    }

    fn builtin_custom_list_property(
        &self,
        widget: &Document,
        value: &Document,
        current_scope: Option<&str>,
        current_entity: &str,
    ) -> Option<String> {
        let source = crate::WebListDataSource::from_documents(self.compiler.documents, value);
        if !source.supported() || !qualified_name_valid(&source.entity) || source.xpath() {
            return None;
        }
        let key = self.widget_key(widget);
        let operation = operation_id(
            self.qualified_name,
            widget.get_str("Name").unwrap_or_default(),
        );
        if source.association() {
            return Some(format!(
                "AssociationObjectListProperty({})",
                js_object(&[
                    ("dataSourceId", js_string(&key)),
                    ("operationId", js_string(&operation)),
                    ("scope", js_string(current_scope?)),
                    ("directPath", js_string(&source.association_path)),
                    ("sort", "[]".to_string()),
                ])
            ));
        }
        let direct = value.get_document("DataSource").ok()?;
        if source.microflow() {
            let settings = direct.get_document("MicroflowSettings").unwrap_or(direct);
            let arg_map = self.flow_argument_map(
                settings,
                &source.microflow_name,
                current_scope,
                current_entity,
            )?;
            return Some(format!(
                "MicroflowObjectListProperty({})",
                js_object(&[
                    ("dataSourceId", js_string(&key)),
                    ("operationId", js_string(&operation)),
                    ("argMap", arg_map),
                    ("fetchOnlyWithAllParams", "false".to_string()),
                ])
            ));
        }
        let reference = self
            .compiler
            .nanoflow_renderer
            .and_then(|renderer| renderer(&source.nanoflow_name))?;
        let settings = direct.get_document("NanoflowSettings").unwrap_or(direct);
        let arg_map = if array_docs(settings, "ParameterMappings").is_empty() {
            "{}".to_string()
        } else {
            self.explicit_argument_map(settings, current_scope)?
        };
        Some(format!(
            "NanoflowObjectListProperty({})",
            js_object(&[
                ("dataSourceId", js_string(&key)),
                ("source", format!("{{ nanoflow: {reference} }}")),
                ("argMap", arg_map),
                ("fetchOnlyWithAllParams", "false".to_string()),
            ])
        ))
    }

    fn render_static_image(&self, widget: &Document) -> Option<String> {
        let reference = widget.get_str("Image").ok()?;
        let mut parts = reference.splitn(3, '.');
        let module = parts.next()?;
        let collection_name = parts.next()?;
        let image_name = parts.next()?;
        let collection = self
            .compiler
            .documents
            .iter()
            .find_map(|(owner, document)| {
                (owner == module
                    && document.get_str("$Type").ok() == Some("Images$ImageCollection")
                    && document.get_str("Name").ok() == Some(collection_name))
                .then_some(document)
            })?;
        let image = array_docs(collection, "Images")
            .into_iter()
            .find(|image| image.get_str("Name").ok() == Some(image_name))?;
        let format = crate::image_format::image_format(&image).ok()?;
        let uri = format!("img/{module}${collection_name}${image_name}.{format}");
        let key = self.widget_key(widget);
        self.state.borrow_mut().used.insert(UsedBundle::NativeImage);
        Some(format!(
            "React.createElement($NativeImage, {})",
            js_object(&[
                ("key", js_string(&key)),
                ("$widgetId", js_string(&key)),
                ("class", js_string(&css_class(widget))),
                (
                    "responsive",
                    widget.get_bool("Responsive").unwrap_or(false).to_string(),
                ),
                (
                    "tabIndex",
                    widget.get_i32("TabIndex").unwrap_or_default().to_string(),
                ),
                (
                    "source",
                    format!(
                        "WebStaticImageProperty({{ image: {{ uri: {} }} }})",
                        js_string(&uri)
                    ),
                ),
                (
                    "alternativeText",
                    text_property(
                        widget
                            .get_document("AlternativeText")
                            .ok()
                            .and_then(|text| text.get_document("Template").ok())
                            .or_else(|| widget.get_document("AlternativeText").ok()),
                    ),
                ),
            ])
        ))
    }

    fn render_navigation_list(
        &self,
        widget: &Document,
        scope: Option<&str>,
        entity: &str,
    ) -> Option<String> {
        let items = array_docs(widget, "Items")
            .iter()
            .map(|item| {
                let mut values = vec![
                    ("class".to_string(), js_string(&css_class(item))),
                    (
                        "content".to_string(),
                        self.render_widgets(&array_docs(item, "Widgets"), scope, entity),
                    ),
                ];
                if let Some(action) = item.get_document("Action").ok()
                    && let Some(property) = self
                        .compiler
                        .action_renderer
                        .and_then(|renderer| renderer(action))
                        .or_else(|| self.builtin_action_property(widget, action, scope, entity))
                {
                    values.push(("action".to_string(), property));
                }
                js_object_owned(&values)
            })
            .collect::<Vec<_>>()
            .join(", ");
        self.state
            .borrow_mut()
            .used
            .insert(UsedBundle::NavigationList);
        let key = self.widget_key(widget);
        Some(format!(
            "React.createElement($NavigationList, {})",
            js_object(&[
                ("key", js_string(&key)),
                ("$widgetId", js_string(&key)),
                ("class", js_string(&css_class(widget))),
                ("items", format!("[{items}]")),
            ])
        ))
    }

    fn render_scroll_container(
        &self,
        widget: &Document,
        scope: Option<&str>,
        entity: &str,
    ) -> Option<String> {
        let key = self.widget_key(widget);
        self.state
            .borrow_mut()
            .used
            .insert(UsedBundle::ScrollContainer);
        Some(format!(
            "React.createElement($ScrollContainer, {})",
            js_object(&[
                ("key", js_string(&key)),
                ("$widgetId", js_string(&key)),
                ("class", js_string(&css_class(widget))),
                (
                    "scrollPerRegion",
                    (widget.get_str("ScrollBehavior").ok() == Some("PerRegion")).to_string(),
                ),
                (
                    "layoutMode",
                    js_string(
                        &widget
                            .get_str("LayoutMode")
                            .unwrap_or_default()
                            .to_ascii_lowercase(),
                    ),
                ),
                (
                    "top",
                    self.render_scroll_region(widget, "Top", scope, entity)
                ),
                (
                    "bottom",
                    self.render_scroll_region(widget, "Bottom", scope, entity),
                ),
                (
                    "left",
                    self.render_scroll_region(widget, "Left", scope, entity)
                ),
                (
                    "right",
                    self.render_scroll_region(widget, "Right", scope, entity),
                ),
                (
                    "center",
                    self.render_scroll_region(widget, "CenterRegion", scope, entity),
                ),
            ])
        ))
    }

    fn render_scroll_region(
        &self,
        widget: &Document,
        field: &str,
        scope: Option<&str>,
        entity: &str,
    ) -> String {
        let Ok(region) = widget.get_document(field) else {
            return "{ enabled: false }".to_string();
        };
        let toggle = region.get_str("ToggleMode").unwrap_or_default();
        let toggle_mode = if toggle.starts_with("ShrinkContent") {
            "shrink"
        } else if toggle.starts_with("PushContent") {
            "push"
        } else if toggle.starts_with("SlideOverContent") {
            "slide"
        } else {
            "none"
        };
        js_object(&[
            ("enabled", "true".to_string()),
            (
                "content",
                self.render_widgets(&array_docs(region, "Widgets"), scope, entity),
            ),
            (
                "sizeMode",
                js_string(
                    &region
                        .get_str("SizeMode")
                        .unwrap_or_default()
                        .to_ascii_lowercase(),
                ),
            ),
            (
                "sizeValue",
                region.get_i32("Size").unwrap_or_default().to_string(),
            ),
            (
                "class",
                js_string(
                    region
                        .get_document("Appearance")
                        .ok()
                        .and_then(|appearance| appearance.get_str("Class").ok())
                        .unwrap_or_default(),
                ),
            ),
            ("toggleMode", js_string(toggle_mode)),
            (
                "initiallyOpen",
                (!toggle.contains("InitiallyClosed")).to_string(),
            ),
        ])
    }

    fn render_file_manager(&self, widget: &Document, scope: Option<&str>) -> Option<String> {
        let scope = scope?;
        let key = self.widget_key(widget);
        self.state.borrow_mut().used.insert(UsedBundle::FileManager);
        Some(format!(
            "React.createElement($FileManager, {})",
            js_object(&[
                ("key", js_string(&key)),
                ("$widgetId", js_string(&key)),
                ("class", js_string(&css_class(widget))),
                ("id", js_string(&key)),
                (
                    "widgetType",
                    js_string(
                        &widget
                            .get_str("Type")
                            .unwrap_or("both")
                            .to_ascii_lowercase(),
                    ),
                ),
                (
                    "extensions",
                    js_string(widget.get_str("AllowedExtensions").unwrap_or_default()),
                ),
                (
                    "maxFileSize",
                    positive_i32(widget.get_i32("MaxFileSize").unwrap_or_default(), 200)
                        .to_string(),
                ),
                (
                    "content",
                    format!(
                        "DynamicFileProperty({})",
                        js_object(&[
                            ("scope", js_string(scope)),
                            ("path", js_string("")),
                            ("isEditable", "true".to_string()),
                            (
                                "allowUpload",
                                (widget.get_str("Type").ok() != Some("Download")).to_string(),
                            ),
                        ])
                    ),
                ),
            ])
        ))
    }

    fn render_association_selector(
        &self,
        widget: &Document,
        scope: Option<&str>,
        source_entity: &str,
        reference_set: bool,
        form_group: bool,
    ) -> Option<String> {
        let scope = scope?;
        if !qualified_name_valid(source_entity) {
            return None;
        }
        let reference = widget.get_document("AttributeRef").ok()?;
        let steps = reference
            .get_document("EntityRef")
            .ok()
            .map(|reference| array_docs(reference, "Steps"))?;
        let association = steps.last()?;
        let association_name = association
            .get_str("Association")
            .ok()
            .filter(|name| qualified_name_valid(name))?;
        let endpoint = association
            .get_str("DestinationEntity")
            .ok()
            .filter(|name| qualified_name_valid(name))?;
        let caption = reference.get_str("Attribute").ok()?;
        let (caption_entity, caption_attribute) = caption.rsplit_once('.')?;
        if !qualified_name_valid(caption_entity) || !identifier(caption_attribute) {
            return None;
        }
        let parent_path = steps[..steps.len() - 1]
            .iter()
            .flat_map(|step| {
                [
                    step.get_str("Association").ok(),
                    step.get_str("DestinationEntity").ok(),
                ]
            })
            .flatten()
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>()
            .join("/");
        let key = self.widget_key(widget);
        let data_source_id = format!("{key}$options");
        let input = format!(
            "React.createElement({}, {})",
            if reference_set {
                "$MxrbReferenceSetSelector"
            } else {
                "$ReferenceSelector"
            },
            js_object(&[
                ("key", js_string(&key)),
                ("$widgetId", js_string(&key)),
                ("id", js_string(&key)),
                ("class", js_string(&css_class(widget))),
                ("readOnlyStyle", js_string(&read_only_style(widget))),
                (
                    "tabIndex",
                    widget.get_i32("TabIndex").unwrap_or_default().to_string(),
                ),
                (
                    "value",
                    format!(
                        "AssociationProperty({})",
                        js_object(&[
                            (
                                "type",
                                js_string(if reference_set {
                                    "ReferenceSet"
                                } else {
                                    "Reference"
                                }),
                            ),
                            ("entity", js_string(source_entity)),
                            ("path", js_string(&parent_path)),
                            ("attribute", js_string(association_name)),
                            ("endpointEntity", js_string(endpoint)),
                            ("selectableObjectsId", js_string(&data_source_id)),
                            ("scope", js_string(scope)),
                            ("restrictToDataSource", "false".to_string()),
                            ("onChange", simple_client_action("doNothing", "false"),),
                        ])
                    ),
                ),
                (
                    "valueOptions",
                    format!(
                        "DatabaseObjectListProperty({})",
                        js_object(&[
                            ("dataSourceId", js_string(&data_source_id)),
                            ("entity", js_string(endpoint)),
                            (
                                "operationId",
                                js_string(&operation_id(
                                    self.qualified_name,
                                    widget.get_str("Name").unwrap_or_default(),
                                )),
                            ),
                            ("sort", "[]".to_string()),
                        ])
                    ),
                ),
                (
                    "attribute",
                    format!(
                        "ListAttributeProperty({})",
                        js_object(&[
                            ("path", js_string("")),
                            ("entity", js_string(caption_entity)),
                            ("attribute", js_string(caption_attribute)),
                            ("attributeType", js_string("String")),
                            ("sortable", "true".to_string()),
                            ("filterable", "true".to_string()),
                            ("dataSourceId", js_string(&data_source_id)),
                            ("isList", "false".to_string()),
                        ])
                    ),
                ),
                (
                    "emptyCaption",
                    text_property(widget.get_document("EmptyOptionCaption").ok()),
                ),
            ])
        );
        self.state.borrow_mut().used.insert(if reference_set {
            UsedBundle::ReferenceSetSelector
        } else {
            UsedBundle::ReferenceSelector
        });
        if !form_group {
            return Some(input);
        }
        Some(js_form_group(
            widget,
            &key,
            &input,
            if reference_set {
                "mx-referencesetselector"
            } else {
                "mx-referenceselector"
            },
            &client_template_text(widget.get_document("LabelTemplate").ok()),
        ))
    }

    fn render_text_area(
        &self,
        widget: &Document,
        scope: Option<&str>,
        entity: &str,
    ) -> Option<String> {
        let (attribute_entity, attribute) = bound_attribute(widget, scope)?;
        let key = self.widget_key(widget);
        let max_length = widget.get_i32("MaxLengthCode").unwrap_or_default();
        let input = self.render_form_input(
            "TextArea",
            widget,
            scope?,
            &attribute_entity,
            &attribute,
            vec![
                ("readOnlyStyle", js_string(&read_only_style(widget))),
                (
                    "numberOfLines",
                    positive_i32(widget.get_i32("NumberOfLines").unwrap_or_default(), 5)
                        .to_string(),
                ),
                (
                    "autoGrow",
                    widget.get_bool("AutoGrow").unwrap_or(false).to_string(),
                ),
                (
                    "maxLength",
                    if max_length > 0 {
                        max_length.to_string()
                    } else {
                        "null".to_string()
                    },
                ),
                ("autocomplete", js_string(&autocomplete_value(widget))),
                (
                    "submitWhileEditing",
                    matches!(
                        widget.get_str("SubmitBehaviour").ok(),
                        Some("WhileEditing" | "OnTyping")
                    )
                    .to_string(),
                ),
                (
                    "submitDelay",
                    widget
                        .get_i32("SubmitOnInputDelay")
                        .unwrap_or_default()
                        .to_string(),
                ),
                ("id", js_string(&key)),
                (
                    "ariaRequired",
                    widget.get_bool("AriaRequired").unwrap_or(false).to_string(),
                ),
                (
                    "tabIndex",
                    widget.get_i32("TabIndex").unwrap_or_default().to_string(),
                ),
                (
                    "placeholder",
                    text_property(widget.get_document("PlaceholderTemplate").ok()),
                ),
                (
                    "textTooLongMessage",
                    text_property_with_fallback(
                        widget.get_document("TextTooLongMessage").ok(),
                        "Text is too long",
                    ),
                ),
                (
                    "counterMessage",
                    text_property(widget.get_document("CounterMessage").ok()),
                ),
            ],
        );
        Some(self.render_form_group(widget, &key, &input, "mx-textarea", entity))
    }

    fn render_date_picker(
        &self,
        widget: &Document,
        scope: Option<&str>,
        entity: &str,
    ) -> Option<String> {
        let (attribute_entity, attribute) = bound_attribute(widget, scope)?;
        let key = self.widget_key(widget);
        let time = widget
            .get_document("FormattingInfo")
            .ok()
            .and_then(|formatting| formatting.get_str("DateFormat").ok())
            .is_some_and(|format| format.eq_ignore_ascii_case("time"));
        let formatting = if time {
            "{ \"timeFormat\": { \"type\": \"time\" } }"
        } else {
            "{ \"dateFormat\": { \"type\": \"date\" } }"
        };
        let input = self.render_form_input_with_formatting(
            "DatePicker",
            widget,
            scope?,
            &attribute_entity,
            &attribute,
            formatting,
            vec![
                ("mode", js_string(if time { "time" } else { "date" })),
                (
                    "showCalendarButton",
                    widget
                        .get_bool("ShowCalendarButton")
                        .unwrap_or(true)
                        .to_string(),
                ),
                ("readOnlyStyle", js_string(&read_only_style(widget))),
                ("id", js_string(&key)),
                (
                    "placeholder",
                    text_property(widget.get_document("PlaceholderTemplate").ok()),
                ),
                (
                    "buttonLabel",
                    "TextProperty({ value: \"Show date picker\" })".to_string(),
                ),
            ],
        );
        Some(self.render_form_group(widget, &key, &input, "mx-datepicker", entity))
    }

    fn render_simple_input(
        &self,
        widget: &Document,
        scope: Option<&str>,
        entity: &str,
        component: &str,
    ) -> Option<String> {
        let (attribute_entity, attribute) = bound_attribute(widget, scope)?;
        let key = self.widget_key(widget);
        let input = self.render_form_input(
            component,
            widget,
            scope?,
            &attribute_entity,
            &attribute,
            vec![
                ("readOnlyStyle", js_string(&read_only_style(widget))),
                ("id", js_string(&key)),
                (
                    "ariaRequired",
                    widget.get_bool("AriaRequired").unwrap_or(false).to_string(),
                ),
                (
                    "tabIndex",
                    widget.get_i32("TabIndex").unwrap_or_default().to_string(),
                ),
            ],
        );
        Some(self.render_form_group(
            widget,
            &key,
            &input,
            if component == "CheckBox" {
                "mx-checkbox"
            } else {
                "mx-radiogroup"
            },
            entity,
        ))
    }

    fn render_form_input(
        &self,
        component: &str,
        widget: &Document,
        scope: &str,
        entity: &str,
        attribute: &str,
        properties: Vec<(&str, String)>,
    ) -> String {
        self.render_form_input_with_formatting(
            component, widget, scope, entity, attribute, "{}", properties,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn render_form_input_with_formatting(
        &self,
        component: &str,
        widget: &Document,
        scope: &str,
        entity: &str,
        attribute: &str,
        formatting: &str,
        mut properties: Vec<(&str, String)>,
    ) -> String {
        let key = self.widget_key(widget);
        properties.extend([
            ("key", js_string(&key)),
            ("$widgetId", js_string(&key)),
            (
                if matches!(component, "CheckBox" | "RadioButtonGroup" | "EnumSelect") {
                    "value"
                } else {
                    "inputValue"
                },
                attribute_property(widget, scope, entity, attribute, formatting),
            ),
        ]);
        let mut state = self.state.borrow_mut();
        state.used.insert(UsedBundle::FormInput);
        state.form_widgets.insert(component.to_string());
        drop(state);
        format!(
            "React.createElement(${component}, {})",
            js_object(&properties)
        )
    }

    fn render_form_group(
        &self,
        widget: &Document,
        key: &str,
        input: &str,
        widget_class: &str,
        _entity: &str,
    ) -> String {
        let caption = client_template_text(widget.get_document("LabelTemplate").ok());
        js_form_group(widget, key, input, widget_class, &caption)
    }

    fn builtin_action_property(
        &self,
        widget: &Document,
        action: &Document,
        current_scope: Option<&str>,
        current_entity: &str,
    ) -> Option<String> {
        let disabled = action
            .get_bool("DisabledDuringExecution")
            .unwrap_or(true)
            .to_string();
        if action.get_str("$Type").ok() == Some("Forms$CallNanoflowClientAction") {
            let name = action
                .get_str("Nanoflow")
                .ok()
                .filter(|name| qualified_name_valid(name))?;
            let reference = self
                .compiler
                .nanoflow_renderer
                .and_then(|renderer| renderer(name))?;
            let arg_map = self.nanoflow_argument_map(action, current_scope, current_entity)?;
            let payload = js_object(&[
                ("type", js_string("callNanoflow")),
                ("argMap", arg_map),
                ("config", format!("{{ nanoflow: {reference} }}")),
                ("disabledDuringExecution", disabled),
            ]);
            return Some(format!(
                "ActionProperty({})",
                js_object(&[
                    ("action", payload),
                    ("abortOnServerValidation", "false".to_string()),
                    ("skipClientValidation", "false".to_string()),
                ])
            ));
        }
        let action_payload = match action.get_str("$Type").ok()? {
            "Forms$SaveChangesClientAction"
            | "Forms$CancelChangesClientAction"
            | "Forms$DeleteClientAction" => {
                let scope = current_scope?;
                let kind = match action.get_str("$Type").ok()? {
                    "Forms$SaveChangesClientAction" => "saveChanges",
                    "Forms$CancelChangesClientAction" => "cancelChanges",
                    _ => "deleteObject",
                };
                let arg_map = if kind == "cancelChanges" {
                    "{}".to_string()
                } else {
                    js_object(&[(
                        "$object",
                        js_object(&[
                            ("widget", js_string(scope)),
                            ("source", js_string("object")),
                        ]),
                    )])
                };
                js_object(&[
                    ("type", js_string(kind)),
                    ("argMap", arg_map),
                    (
                        "config",
                        js_object(&[
                            (
                                "operationId",
                                js_string(&operation_id(
                                    self.qualified_name,
                                    widget.get_str("Name").unwrap_or_default(),
                                )),
                            ),
                            (
                                "closePage",
                                action.get_bool("ClosePage").unwrap_or(true).to_string(),
                            ),
                        ]),
                    ),
                    ("disabledDuringExecution", disabled),
                ])
            }
            "Forms$ClosePageClientAction" => simple_client_action("closePage", &disabled),
            "Forms$SignOutClientAction" => js_object(&[
                ("type", js_string("signOut")),
                ("argMap", "{}".to_string()),
                ("config", js_object(&[("namedUser", "true".to_string())])),
                ("disabledDuringExecution", disabled),
            ]),
            "Forms$OpenLinkClientAction" => {
                let address = action.get_document("Address").ok()?;
                if address.get_bool("IsDynamic").unwrap_or(false) {
                    return None;
                }
                js_object(&[
                    ("type", js_string("openLink")),
                    ("argMap", "{}".to_string()),
                    (
                        "config",
                        js_object(&[
                            (
                                "schema",
                                js_string(
                                    &action
                                        .get_str("LinkType")
                                        .unwrap_or_default()
                                        .to_ascii_lowercase(),
                                ),
                            ),
                            (
                                "address",
                                js_string(address.get_str("Value").unwrap_or_default()),
                            ),
                        ]),
                    ),
                    ("disabledDuringExecution", disabled),
                ])
            }
            "Forms$FormAction" => {
                let settings = action.get_document("FormSettings").ok()?;
                let form = settings
                    .get_str("Form")
                    .ok()
                    .filter(|name| qualified_name_valid(name))?;
                if !array_docs(settings, "ParameterMappings").is_empty() {
                    return None;
                }
                js_object(&[
                    ("type", js_string("openPage")),
                    ("argMap", "{}".to_string()),
                    (
                        "config",
                        js_object(&[
                            (
                                "name",
                                js_string(&format!("{}.page.xml", form.replace('.', "/"))),
                            ),
                            ("location", js_string("content")),
                            ("allowedRoles", "[]".to_string()),
                        ]),
                    ),
                    ("disabledDuringExecution", disabled),
                ])
            }
            "Forms$MicroflowAction" => {
                let settings = action.get_document("MicroflowSettings").ok()?;
                let name = settings
                    .get_str("Microflow")
                    .ok()
                    .filter(|name| qualified_name_valid(name))?;
                let arg_map =
                    self.flow_argument_map(settings, name, current_scope, current_entity)?;
                js_object(&[
                    ("type", js_string("callMicroflow")),
                    ("argMap", arg_map),
                    (
                        "config",
                        js_object(&[(
                            "operationId",
                            js_string(&operation_id(
                                self.qualified_name,
                                widget.get_str("Name").unwrap_or_default(),
                            )),
                        )]),
                    ),
                    ("disabledDuringExecution", disabled),
                ])
            }
            _ => return None,
        };
        Some(format!(
            "ActionProperty({})",
            js_object(&[
                ("action", action_payload),
                ("abortOnServerValidation", "true".to_string()),
            ])
        ))
    }

    fn nanoflow_argument_map(
        &self,
        action: &Document,
        current_scope: Option<&str>,
        current_entity: &str,
    ) -> Option<String> {
        let entries = array_docs(action, "ParameterMappings")
            .into_iter()
            .map(|mapping| {
                let name = mapping.get_str("Parameter").ok()?.rsplit('.').next()?;
                let variable = mapping
                    .get_str("Expression")
                    .ok()?
                    .strip_prefix('$')?
                    .to_string();
                if !identifier(name) || !identifier(&variable) {
                    return None;
                }
                let entity_variable = current_entity.rsplit('.').next() == Some(&variable);
                let widget = if entity_variable {
                    current_scope.map(str::to_string)?
                } else {
                    format!("${variable}")
                };
                Some((
                    name.to_string(),
                    js_object(&[
                        (
                            "expression",
                            js_object(&[
                                (
                                    "expr",
                                    js_object(&[
                                        ("type", js_string("variable")),
                                        ("variable", js_string(&variable)),
                                    ]),
                                ),
                                (
                                    "args",
                                    js_object(&[(
                                        &variable,
                                        js_object(&[
                                            ("widget", js_string(&widget)),
                                            ("source", js_string("object")),
                                        ]),
                                    )]),
                                ),
                            ]),
                        ),
                        ("kind", js_string("object")),
                    ]),
                ))
            })
            .collect::<Option<Vec<_>>>()?;
        Some(js_object_owned(&entries))
    }

    fn render_data_view(
        &self,
        widget: &Document,
        current_scope: Option<&str>,
        current_entity: &str,
    ) -> Option<String> {
        let scope = self.widget_key(widget);
        let source = widget.get_document("DataSource").ok()?;
        let object =
            self.data_view_object_property(widget, source, &scope, current_scope, current_entity)?;
        let entity = self.data_view_entity(source);
        let body = self.render_widgets(&array_docs(widget, "Widgets"), Some(&scope), &entity);
        let footer =
            self.render_widgets(&array_docs(widget, "FooterWidgets"), Some(&scope), &entity);
        self.state.borrow_mut().used.insert(UsedBundle::DataView);
        let props = js_object(&[
            ("key", js_string(&scope)),
            ("$widgetId", js_string(&scope)),
            ("class", js_string(&css_class(widget))),
            ("body", body),
            ("footer", footer),
            (
                "hideFooter",
                (!widget.get_bool("ShowFooter").unwrap_or(true)).to_string(),
            ),
            ("object", object),
            (
                "emptyMessage",
                format!(
                    "TextProperty({{ value: {} }})",
                    js_string(&translated_text(
                        widget.get_document("NoEntityMessage").ok()
                    ))
                ),
            ),
        ]);
        Some(format!("React.createElement($DataView, {props})"))
    }

    fn data_view_object_property(
        &self,
        widget: &Document,
        source: &Document,
        data_view_scope: &str,
        current_scope: Option<&str>,
        current_entity: &str,
    ) -> Option<String> {
        match source.get_str("$Type").ok()? {
            "Forms$ListenTargetSource" => self.listen_object_property(widget, source),
            "Forms$MicroflowSource" => self.microflow_object_property(
                widget,
                source,
                data_view_scope,
                current_scope,
                current_entity,
            ),
            "Forms$NanoflowSource" => {
                self.nanoflow_object_property(source, data_view_scope, current_scope)
            }
            _ => {
                let path = source
                    .get_document("EntityRef")
                    .ok()
                    .map(entity_ref_path)
                    .unwrap_or_default();
                if let Some(source_scope) = source_variable_scope(
                    source.get_document("SourceVariable").ok(),
                    self.current_document(),
                ) {
                    if path.is_empty() {
                        return Some(association_object_property(&source_scope, "", None));
                    }
                    return Some(association_object_property(
                        &source_scope,
                        &path,
                        Some(operation_id(
                            self.qualified_name,
                            widget.get_str("Name").unwrap_or_default(),
                        )),
                    ));
                }
                if source
                    .get_document("SourceVariable")
                    .ok()
                    .and_then(|variable| variable.get_str("SnippetParameter").ok())
                    .is_some_and(identifier)
                    && let Some(scope) = current_scope
                {
                    return Some(association_object_property(
                        scope,
                        &path,
                        (!path.is_empty()).then(|| {
                            operation_id(
                                self.qualified_name,
                                widget.get_str("Name").unwrap_or_default(),
                            )
                        }),
                    ));
                }
                current_scope.filter(|_| !path.is_empty()).map(|scope| {
                    association_object_property(
                        scope,
                        &path,
                        Some(operation_id(
                            self.qualified_name,
                            widget.get_str("Name").unwrap_or_default(),
                        )),
                    )
                })
            }
        }
    }

    fn listen_object_property(&self, widget: &Document, source: &Document) -> Option<String> {
        let target_name = source
            .get_str("ListenTarget")
            .ok()
            .filter(|name| identifier(name))?;
        let target = self
            .current_document()
            .and_then(|document| find_widget(document, target_name))?;
        Some(format!(
            "ListenObjectProperty({})",
            js_object(&[
                ("listenTo", js_string(&self.widget_key(target))),
                ("editable", "true".to_string()),
                (
                    "operationId",
                    js_string(&operation_id(
                        self.qualified_name,
                        widget.get_str("Name").unwrap_or_default(),
                    )),
                ),
            ])
        ))
    }

    fn microflow_object_property(
        &self,
        widget: &Document,
        source: &Document,
        data_view_scope: &str,
        current_scope: Option<&str>,
        current_entity: &str,
    ) -> Option<String> {
        let settings = source.get_document("MicroflowSettings").unwrap_or(source);
        let name = settings
            .get_str("Microflow")
            .ok()
            .filter(|name| qualified_name_valid(name))?;
        let arg_map = self.flow_argument_map(settings, name, current_scope, current_entity)?;
        Some(format!(
            "MicroflowObjectProperty({})",
            js_object(&[
                ("dataSourceId", js_string(data_view_scope)),
                (
                    "operationId",
                    js_string(&operation_id(
                        self.qualified_name,
                        widget.get_str("Name").unwrap_or_default(),
                    )),
                ),
                ("editable", "true".to_string()),
                ("argMap", arg_map),
            ])
        ))
    }

    fn nanoflow_object_property(
        &self,
        source: &Document,
        data_view_scope: &str,
        current_scope: Option<&str>,
    ) -> Option<String> {
        let name = source
            .get_str("Nanoflow")
            .ok()
            .filter(|name| qualified_name_valid(name))?;
        let reference = self
            .compiler
            .nanoflow_renderer
            .and_then(|renderer| renderer(name))?;
        let arg_map = self.explicit_argument_map(source, current_scope)?;
        Some(format!(
            "NanoflowObjectProperty({})",
            js_object(&[
                ("dataSourceId", js_string(data_view_scope)),
                ("editable", "true".to_string()),
                ("source", format!("{{ nanoflow: {reference} }}"),),
                ("argMap", arg_map),
            ])
        ))
    }

    fn flow_argument_map(
        &self,
        settings: &Document,
        flow_name: &str,
        current_scope: Option<&str>,
        current_entity: &str,
    ) -> Option<String> {
        let mappings = array_docs(settings, "ParameterMappings");
        if !mappings.is_empty() {
            return self.explicit_argument_map(settings, current_scope);
        }
        let Some(scope) = current_scope.filter(|_| identifier(current_entity)) else {
            return Some("{}".to_string());
        };
        let flow = self.qualified_document("Microflows$Microflow", flow_name)?;
        let entries = flow
            .get_document("ObjectCollection")
            .ok()
            .map(|collection| array_docs(collection, "Objects"))
            .unwrap_or_default()
            .into_iter()
            .filter(|object| {
                object.get_str("$Type").ok() == Some("Microflows$MicroflowParameter")
                    && object
                        .get_document("VariableType")
                        .ok()
                        .and_then(|type_| type_.get_str("Entity").ok())
                        == Some(current_entity)
            })
            .filter_map(|parameter| {
                parameter
                    .get_str("Name")
                    .ok()
                    .filter(|name| identifier(name))
                    .map(|name| {
                        (
                            name.to_string(),
                            js_object(&[
                                ("widget", js_string(scope)),
                                ("source", js_string("object")),
                            ]),
                        )
                    })
            })
            .collect::<Vec<_>>();
        Some(js_object_owned(&entries))
    }

    fn explicit_argument_map(
        &self,
        settings: &Document,
        current_scope: Option<&str>,
    ) -> Option<String> {
        let entries = array_docs(settings, "ParameterMappings")
            .into_iter()
            .map(|mapping| {
                let name = mapping.get_str("Parameter").ok()?.rsplit('.').next()?;
                if !identifier(name) {
                    return None;
                }
                let expression = mapping.get_str("Expression").unwrap_or_default();
                let scope = if expression == "$currentObject" {
                    current_scope.map(str::to_string)
                } else if expression.is_empty() {
                    source_variable_scope(
                        mapping.get_document("Variable").ok(),
                        self.current_document(),
                    )
                } else if expression.starts_with('$') {
                    Some(expression.to_string())
                } else {
                    None
                }?;
                Some((
                    name.to_string(),
                    js_object(&[
                        ("widget", js_string(&scope)),
                        ("source", js_string("object")),
                    ]),
                ))
            })
            .collect::<Option<Vec<_>>>()?;
        Some(js_object_owned(&entries))
    }

    fn data_view_entity(&self, source: &Document) -> String {
        if source.get_str("$Type").ok() == Some("Forms$ListenTargetSource") {
            return source
                .get_str("ListenTarget")
                .ok()
                .and_then(|name| {
                    self.current_document()
                        .and_then(|page| find_widget(page, name))
                })
                .map(|target| {
                    crate::WebListDataSource::from_documents(self.compiler.documents, target).entity
                })
                .unwrap_or_default();
        }
        let direct = source
            .get_document("EntityRef")
            .ok()
            .and_then(entity_ref_destination);
        if let Some(entity) = direct.filter(|entity| !entity.is_empty()) {
            return entity;
        }
        if let Some(parameter) = source
            .get_document("SourceVariable")
            .ok()
            .and_then(|variable| variable.get_str("PageParameter").ok())
            .filter(|name| identifier(name))
            && let Some(entity) = self.current_document().and_then(|document| {
                array_docs(document, "Parameters")
                    .into_iter()
                    .find(|candidate| candidate.get_str("Name").ok() == Some(parameter))
                    .and_then(|candidate| {
                        candidate
                            .get_document("ParameterType")
                            .ok()
                            .and_then(|type_| type_.get_str("Entity").ok().map(str::to_string))
                    })
            })
        {
            return entity;
        }
        let flow_name = match source.get_str("$Type").ok() {
            Some("Forms$MicroflowSource") => source
                .get_document("MicroflowSettings")
                .unwrap_or(source)
                .get_str("Microflow")
                .ok(),
            Some("Forms$NanoflowSource") => source.get_str("Nanoflow").ok(),
            _ => None,
        };
        flow_name
            .and_then(|name| {
                self.qualified_document(
                    if source.get_str("$Type").ok() == Some("Forms$NanoflowSource") {
                        "Microflows$Nanoflow"
                    } else {
                        "Microflows$Microflow"
                    },
                    name,
                )
            })
            .and_then(|flow| flow.get_document("MicroflowReturnType").ok())
            .and_then(|return_type| return_type.get_str("Entity").ok())
            .unwrap_or_default()
            .to_string()
    }

    fn current_document(&self) -> Option<&Document> {
        let (module, name) = self.qualified_name.rsplit_once('.')?;
        self.compiler
            .documents
            .iter()
            .find_map(|(owner, document)| {
                (owner == module && document.get_str("Name").ok() == Some(name)).then_some(document)
            })
    }

    fn qualified_document(&self, type_name: &str, qualified_name: &str) -> Option<&Document> {
        self.compiler
            .documents
            .iter()
            .find_map(|(module, document)| {
                (document.get_str("$Type").ok() == Some(type_name)
                    && format!("{module}.{}", document.get_str("Name").unwrap_or_default())
                        == qualified_name)
                    .then_some(document)
            })
    }

    fn html_props(&self, widget: &Document, base_class: &str) -> String {
        let widget_class = css_class(widget);
        let class = [base_class, widget_class.as_str()]
            .into_iter()
            .filter(|value| !value.is_empty())
            .collect::<Vec<_>>()
            .join(" ");
        js_object(&[
            ("key", js_string(&self.widget_key(widget))),
            ("className", js_string(&class)),
        ])
    }

    fn widget_key(&self, widget: &Document) -> String {
        format!(
            "{}.{}.{}",
            self.key_prefix,
            self.qualified_name,
            widget.get_str("Name").unwrap_or_default()
        )
    }

    fn render_table(&self, widget: &Document, scope: Option<&str>, entity: &str) -> String {
        let cells = array_docs(widget, "Cells");
        let rows = array_docs(widget, "Rows")
            .iter()
            .enumerate()
            .map(|(row_index, row)| {
                let mut row_cells = cells
                    .iter()
                    .filter(|cell| {
                        cell.get_i32("TopRowIndex").unwrap_or_default() == row_index as i32
                    })
                    .cloned()
                    .collect::<Vec<_>>();
                row_cells.sort_by_key(|cell| cell.get_i32("LeftColumnIndex").unwrap_or_default());
                let rendered = row_cells
                    .iter()
                    .map(|cell| {
                        let tag = if cell.get_bool("IsHeader").unwrap_or(false) {
                            "th"
                        } else {
                            "td"
                        };
                        let props = js_object(&[
                            ("key", js_string(&self.widget_key(cell))),
                            (
                                "colSpan",
                                cell.get_i32("Width").unwrap_or(1).max(1).to_string(),
                            ),
                            (
                                "rowSpan",
                                cell.get_i32("Height").unwrap_or(1).max(1).to_string(),
                            ),
                        ]);
                        format!(
                            "React.createElement({}, {props}, {})",
                            js_string(tag),
                            self.render_widgets(&array_docs(cell, "Widgets"), scope, entity),
                        )
                    })
                    .collect::<Vec<_>>()
                    .join(", ");
                format!(
                    "React.createElement(\"tr\", {}, [{rendered}])",
                    self.html_props(row, ""),
                )
            })
            .collect::<Vec<_>>()
            .join(", ");
        format!(
            "React.createElement(\"table\", {}, React.createElement(\"tbody\", null, [{rows}]))",
            self.html_props(widget, "mx-table"),
        )
    }

    fn render_tabs(&self, widget: &Document, scope: Option<&str>, entity: &str) -> String {
        let tabs = array_docs(widget, "TabPages")
            .iter()
            .map(|tab| {
                let caption = client_template_text(tab.get_document("Caption").ok());
                format!(
                    "React.createElement(\"section\", {}, [React.createElement(\"h2\", {{ key: \"caption\" }}, {}), ...{}])",
                    self.html_props(tab, "mx-tabcontainer-tab"),
                    js_string(&caption),
                    self.render_widgets(&array_docs(tab, "Widgets"), scope, entity),
                )
            })
            .collect::<Vec<_>>()
            .join(", ");
        format!(
            "React.createElement(\"div\", {}, [{tabs}])",
            self.html_props(widget, "mx-tabcontainer"),
        )
    }

    fn render_snippet(
        &self,
        widget: &Document,
        scope: Option<&str>,
        entity: &str,
    ) -> Option<String> {
        let reference = widget
            .get_document("FormCall")
            .ok()
            .and_then(|call| call.get_str("Form").ok())
            .or_else(|| {
                widget
                    .get_document("SnippetSettings")
                    .ok()
                    .and_then(|settings| settings.get_str("Snippet").ok())
            })?;
        let (module, name) = reference.split_once('.')?;
        let snippet = self
            .compiler
            .documents
            .iter()
            .find_map(|(owner, document)| {
                (owner == module
                    && document.get_str("$Type").ok() == Some("Forms$Snippet")
                    && document.get_str("Name").ok() == Some(name))
                .then_some(document)
            })?;
        Some(format!(
            "React.createElement(React.Fragment, {{ key: {} }}, {})",
            js_string(&self.widget_key(widget)),
            self.render_widgets(&array_docs(snippet, "Widgets"), scope, entity),
        ))
    }

    fn render_custom_widget(&self, widget: &Document, scope: Option<&str>, entity: &str) -> String {
        let nested = |widgets: &[Document], nested_scope: &str, nested_entity: &str| {
            self.render_widgets(widgets, Some(nested_scope), nested_entity)
        };
        let mut grid =
            DataGridBundleCompiler::new(self.compiler.documents, self.qualified_name, widget)
                .with_widget_renderer(&nested);
        if let Some(scope) = scope {
            grid = grid.with_scope(scope);
        }
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
                .or_else(|| self.builtin_action_property(widget, action, scope, entity))
        };
        let data_source =
            |value: &Document| self.builtin_custom_list_property(widget, value, scope, entity);
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
        let probe = GenericWidgetBundleCompiler::new(
            self.compiler.documents,
            self.compiler.project_path,
            self.qualified_name,
            widget,
            scope,
        );
        let module_path = probe.module_path().to_string();
        let cached = self
            .compiler
            .package_modules
            .borrow()
            .get(&module_path)
            .copied();
        let available = if let Some(available) = cached {
            available
        } else {
            let available = GenericWidgetBundleCompiler::module_available(
                self.compiler.project_path,
                &module_path,
            );
            self.compiler
                .package_modules
                .borrow_mut()
                .insert(module_path.clone(), available);
            available
        };
        let generic = probe
            .with_key_prefix(self.key_prefix)
            .with_package_available(available)
            .with_widget_renderer(&generic_nested)
            .with_action_renderer(&action)
            .with_data_source_renderer(&data_source);
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
                    "AssociationObjectListProperty",
                    "ExpressionProperty",
                    "ListAssociationProperty",
                    "ListAttributeProperty",
                    "ListExpressionProperty",
                    "MicroflowObjectListProperty",
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
        if state.used.contains(&UsedBundle::Container) {
            add_property_imports(&mut imports, &["ActionProperty"]);
            imports
                .insert("import { Container } from \"mendix/widgets/web/Container\";".to_string());
            widgets.insert("Container".to_string());
        }
        if state.used.contains(&UsedBundle::ActionButton) {
            add_property_imports(&mut imports, &["ActionProperty", "TextProperty"]);
            imports.insert(
                "import { ActionButton } from \"mendix/widgets/web/ActionButton\";".to_string(),
            );
            widgets.insert("ActionButton".to_string());
        }
        if state.used.contains(&UsedBundle::BoundText) {
            add_property_imports(&mut imports, &["AttributeProperty"]);
            imports.insert(
                "const MxrbFormattedText = ({ template, renderMode, class: className, ...props }) => React.createElement(renderMode, { className }, Object.keys(props).filter(key => /^value\\d+$/.test(key)).sort((left, right) => Number(left.slice(5)) - Number(right.slice(5))).reduce((text, key, index) => text.split(`{${index + 1}}`).join(props[key]?.displayValue ?? \" \"), template));".to_string(),
            );
            imports.insert("MxrbFormattedText.displayName = \"MxrbFormattedText\";".to_string());
            imports.insert(
                "const MxrbAttributeValue = ({ value }) => value?.displayValue ?? \"\";"
                    .to_string(),
            );
            imports.insert("MxrbAttributeValue.displayName = \"MxrbAttributeValue\";".to_string());
            widgets.insert("MxrbAttributeValue".to_string());
            widgets.insert("MxrbFormattedText".to_string());
        }
        if state.used.contains(&UsedBundle::FormInput) {
            add_property_imports(
                &mut imports,
                &["ActionProperty", "AttributeProperty", "TextProperty"],
            );
            imports
                .insert("import { FormGroup } from \"mendix/widgets/web/FormGroup\";".to_string());
            widgets.insert("FormGroup".to_string());
            for component in &state.form_widgets {
                imports.insert(format!(
                    "import {{ {component} }} from \"mendix/widgets/web/{component}\";"
                ));
                widgets.insert(component.clone());
            }
        }
        if state.used.contains(&UsedBundle::ListView) {
            add_property_imports(
                &mut imports,
                &[
                    "AssociationObjectListProperty",
                    "DatabaseObjectListProperty",
                    "MicroflowObjectListProperty",
                    "NanoflowObjectListProperty",
                    "TemplatedWidgetProperty",
                ],
            );
            imports.insert("import { ListView } from \"mendix/widgets/web/ListView\";".to_string());
            widgets.insert("ListView".to_string());
        }
        if state.used.contains(&UsedBundle::NativeImage) {
            add_property_imports(&mut imports, &["TextProperty", "WebStaticImageProperty"]);
            imports.insert(
                "import { Image as NativeImage } from \"mendix/widgets/web/Image\";".to_string(),
            );
            widgets.insert("NativeImage".to_string());
        }
        if state.used.contains(&UsedBundle::ReferenceSelector)
            || state.used.contains(&UsedBundle::ReferenceSetSelector)
        {
            add_property_imports(
                &mut imports,
                &[
                    "AssociationProperty",
                    "DatabaseObjectListProperty",
                    "ListAttributeProperty",
                    "TextProperty",
                ],
            );
            imports
                .insert("import { FormGroup } from \"mendix/widgets/web/FormGroup\";".to_string());
            widgets.insert("FormGroup".to_string());
            if state.used.contains(&UsedBundle::ReferenceSelector) {
                imports.insert(
                    "import { ReferenceSelector } from \"mendix/widgets/web/ReferenceSelector\";"
                        .to_string(),
                );
                widgets.insert("ReferenceSelector".to_string());
            }
            if state.used.contains(&UsedBundle::ReferenceSetSelector) {
                imports.insert(
                    "const MxrbReferenceSetSelector = ({ value, valueOptions, attribute, id, class: className }) => { const options = valueOptions?.items || []; const selected = value?.value || []; return React.createElement(\"select\", { id, multiple: true, className, disabled: value?.readOnly, value: selected.map(item => item.id), onChange: event => value?.setValue(Array.from(event.target.selectedOptions).map(option => options.find(item => item.id === option.value)).filter(Boolean)) }, options.map(item => React.createElement(\"option\", { key: item.id, value: item.id }, attribute.get(item).displayValue))); };".to_string(),
                );
                imports.insert(
                    "MxrbReferenceSetSelector.displayName = \"MxrbReferenceSetSelector\";"
                        .to_string(),
                );
                widgets.insert("MxrbReferenceSetSelector".to_string());
            }
        }
        if state.used.contains(&UsedBundle::NavigationList) {
            add_property_imports(&mut imports, &["ActionProperty"]);
            imports.insert(
                "import { NavigationList } from \"mendix/widgets/web/NavigationList\";".to_string(),
            );
            widgets.insert("NavigationList".to_string());
        }
        if state.used.contains(&UsedBundle::ScrollContainer) {
            imports.insert(
                "import { ScrollContainer } from \"mendix/widgets/web/ScrollContainer\";"
                    .to_string(),
            );
            widgets.insert("ScrollContainer".to_string());
        }
        if state.used.contains(&UsedBundle::FileManager) {
            add_property_imports(&mut imports, &["DynamicFileProperty"]);
            imports.insert(
                "import { FileManager } from \"mendix/widgets/web/FileManager\";".to_string(),
            );
            widgets.insert("FileManager".to_string());
        }
        if state.used.contains(&UsedBundle::DataView) {
            add_property_imports(
                &mut imports,
                &[
                    "AssociationObjectProperty",
                    "ListenObjectProperty",
                    "MicroflowObjectProperty",
                    "NanoflowObjectProperty",
                    "TextProperty",
                ],
            );
            imports.insert("import { DataView } from \"mendix/widgets/web/DataView\";".to_string());
            widgets.insert("DataView".to_string());
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
                    "AssociationObjectListProperty",
                    "AttributeProperty",
                    "DatabaseObjectListProperty",
                    "ExpressionProperty",
                    "ListAttributeProperty",
                    "ListExpressionProperty",
                    "MicroflowObjectListProperty",
                    "NanoflowObjectListProperty",
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

fn owning_module(
    container_id: &str,
    parent_by_id: &BTreeMap<String, String>,
    module_by_id: &BTreeMap<String, String>,
) -> Option<String> {
    let mut current = container_id;
    let mut seen = BTreeSet::new();
    while seen.insert(current.to_string()) {
        if let Some(module) = module_by_id.get(current) {
            return Some(module.clone());
        }
        current = parent_by_id.get(current)?;
    }
    None
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

fn client_template_text(template: Option<&Document>) -> String {
    let text = template
        .and_then(|template| template.get_document("Template").ok())
        .or(template);
    translated_text(text)
}

fn css_class(widget: &Document) -> String {
    let name = widget.get_str("Name").unwrap_or_default();
    let appearance = widget
        .get_document("Appearance")
        .ok()
        .and_then(|appearance| appearance.get_str("Class").ok())
        .unwrap_or_default();
    let direct = widget.get_str("Class").unwrap_or_default();
    [
        format!("mx-name-{name}"),
        direct.to_string(),
        appearance.to_string(),
    ]
    .into_iter()
    .filter(|value| !value.is_empty())
    .collect::<Vec<_>>()
    .join(" ")
}

fn text_mode(value: &str) -> &str {
    match value {
        "Paragraph" => "p",
        "Heading1" | "H1" => "h1",
        "Heading2" | "H2" => "h2",
        "Heading3" | "H3" => "h3",
        "Heading4" | "H4" => "h4",
        "Heading5" | "H5" => "h5",
        "Heading6" | "H6" => "h6",
        _ => "span",
    }
}

fn grid_weight_class(size: &str, weight: i32) -> String {
    if (1..=12).contains(&weight) {
        format!("col-{size}-{weight}")
    } else {
        String::new()
    }
}

fn button_style(widget: &Document) -> String {
    let style = widget
        .get_str("ButtonStyle")
        .unwrap_or("default")
        .to_ascii_lowercase();
    format!("btn-{}", if style.is_empty() { "default" } else { &style })
}

fn bound_attribute(widget: &Document, scope: Option<&str>) -> Option<(String, String)> {
    let attribute = widget
        .get_document("AttributeRef")
        .ok()?
        .get_str("Attribute")
        .ok()?;
    let (entity, name) = attribute.rsplit_once('.')?;
    (scope.is_some() && qualified_name_valid(entity) && identifier(name))
        .then(|| (entity.to_string(), name.to_string()))
}

fn read_only_style(widget: &Document) -> String {
    match widget
        .get_str("ReadOnlyStyle")
        .unwrap_or_default()
        .to_ascii_lowercase()
        .as_str()
    {
        "control" => "control".to_string(),
        _ => "text".to_string(),
    }
}

fn autocomplete_value(widget: &Document) -> String {
    if !widget.get_bool("Autocomplete").unwrap_or(true) {
        return "off".to_string();
    }
    let purpose = widget.get_str("AutocompletePurpose").unwrap_or("On");
    if purpose.eq_ignore_ascii_case("on") || purpose.eq_ignore_ascii_case("off") {
        return purpose.to_ascii_lowercase();
    }
    let mut result = String::new();
    for character in purpose.chars() {
        if character.is_ascii_uppercase() && !result.is_empty() {
            result.push('-');
        }
        result.push(character.to_ascii_lowercase());
    }
    result
}

fn attribute_property(
    widget: &Document,
    scope: &str,
    entity: &str,
    attribute: &str,
    formatting: &str,
) -> String {
    let editable = match widget.get_str("Editable").ok() {
        Some("Never") => js_object(&[
            (
                "expr",
                js_object(&[
                    ("type", js_string("literal")),
                    ("value", "false".to_string()),
                ]),
            ),
            ("args", "{}".to_string()),
        ]),
        _ => "null".to_string(),
    };
    format!(
        "AttributeProperty({})",
        js_object(&[
            ("scope", js_string(scope)),
            ("path", js_string("")),
            ("entity", js_string(entity)),
            ("attribute", js_string(attribute)),
            (
                "onChange",
                js_object(&[
                    ("type", js_string("doNothing")),
                    ("argMap", "{}".to_string()),
                    ("config", "{}".to_string()),
                    ("disabledDuringExecution", "false".to_string()),
                ]),
            ),
            ("isList", "false".to_string()),
            ("validation", "null".to_string()),
            ("formatting", formatting.to_string()),
            ("isEditable", editable),
        ])
    )
}

fn bound_text_attribute_property(scope: &str, entity: &str, attribute: &str, path: &str) -> String {
    format!(
        "AttributeProperty({})",
        js_object(&[
            ("scope", js_string(scope)),
            ("path", js_string(path)),
            ("entity", js_string(entity)),
            ("attribute", js_string(attribute)),
            (
                "onChange",
                js_object(&[
                    ("type", js_string("doNothing")),
                    ("argMap", "{}".to_string()),
                    ("config", "{}".to_string()),
                    ("disabledDuringExecution", "false".to_string()),
                ]),
            ),
            ("isList", "false".to_string()),
            ("validation", "null".to_string()),
            ("formatting", "{}".to_string()),
        ])
    )
}

fn text_property(text: Option<&Document>) -> String {
    text_property_with_fallback(text, "")
}

fn text_property_with_fallback(text: Option<&Document>, fallback: &str) -> String {
    let mut value = text
        .map(|text| client_template_text(Some(text)))
        .unwrap_or_default();
    if value.is_empty() {
        value = fallback.to_string();
    }
    format!("TextProperty({{ value: {} }})", js_string(&value))
}

fn js_form_group(
    widget: &Document,
    key: &str,
    input: &str,
    widget_class: &str,
    caption: &str,
) -> String {
    format!(
        "React.createElement($FormGroup, {})",
        js_object(&[
            ("key", js_string(&format!("{key}$formGroup"))),
            ("$widgetId", js_string(&format!("{key}$formGroup"))),
            (
                "class",
                js_string(&format!(
                    "mx-name-{} {widget_class}",
                    widget.get_str("Name").unwrap_or_default()
                )),
            ),
            ("control", format!("[{input}]")),
            ("width", "3".to_string()),
            ("orientation", js_string("horizontal")),
            ("labelFor", js_string(key)),
            (
                "caption",
                format!("TextProperty({{ value: {} }})", js_string(caption)),
            ),
            ("hasError", "TextProperty({ value: false })".to_string()),
        ])
    )
}

fn positive_i32(value: i32, fallback: i32) -> i32 {
    if value > 0 { value } else { fallback }
}

fn simple_client_action(kind: &str, disabled: &str) -> String {
    js_object(&[
        ("type", js_string(kind)),
        ("argMap", "{}".to_string()),
        ("config", "{}".to_string()),
        ("disabledDuringExecution", disabled.to_string()),
    ])
}

fn association_object_property(scope: &str, path: &str, operation: Option<String>) -> String {
    let mut values = vec![
        ("scope".to_string(), js_string(scope)),
        ("path".to_string(), js_string(path)),
        ("editable".to_string(), "true".to_string()),
    ];
    if let Some(operation) = operation {
        values.push(("operationId".to_string(), js_string(&operation)));
    }
    format!("AssociationObjectProperty({})", js_object_owned(&values))
}

fn source_variable_scope(
    variable: Option<&Document>,
    _current_document: Option<&Document>,
) -> Option<String> {
    let variable = variable?;
    if let Some(parameter) = variable
        .get_str("PageParameter")
        .ok()
        .filter(|name| identifier(name))
    {
        return Some(format!("${parameter}"));
    }
    variable
        .get_str("LocalVariable")
        .ok()
        .filter(|name| identifier(name))
        .map(|name| format!("${name}"))
}

fn entity_ref_destination(reference: &Document) -> Option<String> {
    array_docs(reference, "Steps")
        .last()
        .and_then(|step| step.get_str("DestinationEntity").ok())
        .filter(|entity| !entity.is_empty())
        .or_else(|| {
            reference
                .get_str("Entity")
                .ok()
                .filter(|entity| !entity.is_empty())
        })
        .map(str::to_string)
}

fn entity_ref_path(reference: &Document) -> String {
    array_docs(reference, "Steps")
        .iter()
        .flat_map(|step| {
            [
                step.get_str("Association").ok(),
                step.get_str("DestinationEntity").ok(),
            ]
        })
        .flatten()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("/")
}

fn find_widget<'a>(document: &'a Document, name: &str) -> Option<&'a Document> {
    if document.get_str("$Type").ok().is_some_and(|type_name| {
        type_name.starts_with("Forms$") || type_name.starts_with("CustomWidgets$")
    }) && document.get_str("Name").ok() == Some(name)
    {
        return Some(document);
    }
    document.values().find_map(|value| match value {
        mxrs_bson::Bson::Document(child) => find_widget(child, name),
        mxrs_bson::Bson::Array(children) => children.iter().find_map(|child| match child {
            mxrs_bson::Bson::Document(child) => find_widget(child, name),
            _ => None,
        }),
        _ => None,
    })
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
    fn renders_builtin_actions_inside_generic_widgets() {
        let temp = tempdir().unwrap();
        fs::create_dir_all(temp.path().join("widgets/example")).unwrap();
        fs::write(
            temp.path().join("widgets/example/ActionWidget.mjs"),
            "export default {};",
        )
        .unwrap();
        let project = temp.path().join("App.mpr");
        let widget = doc! {
            "$Type": "CustomWidgets$CustomWidget",
            "Name": "actionWidget",
            "Type": {
                "WidgetId": "example.ActionWidget",
                "ObjectType": {
                    "$ID": "action-object",
                    "PropertyTypes": mxrs_bson::build_array(vec![Bson::Document(doc! {
                        "$ID": "action-property",
                        "PropertyKey": "onClick",
                        "ValueType": { "Type": "Action" },
                    })], 2),
                },
            },
            "Object": {
                "TypePointer": "action-object",
                "Properties": mxrs_bson::build_array(vec![Bson::Document(doc! {
                    "TypePointer": "action-property",
                    "Value": { "Action": { "$Type": "Forms$SignOutClientAction" } },
                })], 2),
            },
        };
        let documents = Vec::new();
        let bundle = PageBundleCompiler::new(&documents, &project)
            .compile_page("Demo", &page(vec![widget]))
            .unwrap();

        assert!(bundle.unsupported_custom_widgets.is_empty());
        assert!(bundle.source.contains("React.createElement($ActionWidget"));
        assert!(bundle.source.contains("ActionProperty"));
        assert!(bundle.source.contains("signOut"));
    }

    #[test]
    fn renders_microflow_sources_inside_generic_widgets() {
        let temp = tempdir().unwrap();
        fs::create_dir_all(temp.path().join("widgets/example")).unwrap();
        fs::write(
            temp.path().join("widgets/example/ListWidget.mjs"),
            "export default {};",
        )
        .unwrap();
        let project = temp.path().join("App.mpr");
        let widget = doc! {
            "$Type": "CustomWidgets$CustomWidget",
            "Name": "listWidget",
            "Type": {
                "WidgetId": "example.ListWidget",
                "ObjectType": {
                    "$ID": "list-object",
                    "PropertyTypes": mxrs_bson::build_array(vec![Bson::Document(doc! {
                        "$ID": "source-property",
                        "PropertyKey": "items",
                        "ValueType": { "Type": "DataSource" },
                    })], 2),
                },
            },
            "Object": {
                "TypePointer": "list-object",
                "Properties": mxrs_bson::build_array(vec![Bson::Document(doc! {
                    "TypePointer": "source-property",
                    "Value": { "DataSource": {
                        "$Type": "Forms$MicroflowSource",
                        "MicroflowSettings": {
                            "Microflow": "Demo.LoadOrders",
                            "ParameterMappings": mxrs_bson::build_array(Vec::new(), 2),
                        },
                    } },
                })], 2),
            },
        };
        let documents = vec![(
            "Demo".to_string(),
            doc! {
                "$Type": "Microflows$Microflow",
                "Name": "LoadOrders",
                "MicroflowReturnType": { "Entity": "Demo.Order" },
            },
        )];
        let bundle = PageBundleCompiler::new(&documents, &project)
            .compile_page("Demo", &page(vec![widget]))
            .unwrap();

        assert!(bundle.unsupported_custom_widgets.is_empty());
        assert!(bundle.source.contains("React.createElement($ListWidget"));
        assert!(bundle.source.contains("MicroflowObjectListProperty"));
        assert!(bundle.source.contains("fetchOnlyWithAllParams"));
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

    #[test]
    fn renders_structural_and_static_text_forms_widgets_without_injection() {
        let temp = tempdir().unwrap();
        let project = temp.path().join("App.mpr");
        let text = doc! {
            "$Type": "Forms$DynamicText",
            "Name": "caption",
            "RenderMode": "Paragraph",
            "Content": {
                "$Type": "Forms$ClientTemplate",
                "Template": { "Items": mxrs_bson::build_array(vec![Bson::Document(doc! {
                    "LanguageCode": "en_US", "Text": "Hello",
                })], 3) },
                "Parameters": mxrs_bson::build_array(Vec::new(), 2),
            },
        };
        let container = doc! {
            "$Type": "Forms$DivContainer",
            "Name": "body",
            "Appearance": { "Class": "card" },
            "Widgets": mxrs_bson::build_array(vec![Bson::Document(text)], 2),
        };
        let column = doc! {
            "$Type": "Forms$LayoutGridColumn",
            "Name": "column",
            "Weight": 8,
            "TabletWeight": 12,
            "PhoneWeight": -1,
            "Widgets": mxrs_bson::build_array(vec![Bson::Document(container)], 2),
        };
        let row = doc! {
            "$Type": "Forms$LayoutGridRow",
            "Name": "row",
            "Columns": mxrs_bson::build_array(vec![Bson::Document(column)], 2),
        };
        let grid = doc! {
            "$Type": "Forms$LayoutGrid",
            "Name": "grid",
            "Rows": mxrs_bson::build_array(vec![Bson::Document(row)], 2),
        };
        let documents = Vec::new();
        let bundle = PageBundleCompiler::new(&documents, &project)
            .compile_page("Demo", &page(vec![grid]))
            .unwrap();
        assert!(bundle.unsupported_widgets.is_empty());
        for expected in [
            "mx-layoutgrid mx-layoutgrid-fluid",
            "col-md-8 col-sm-12",
            "mx-name-body card",
            "React.createElement(\"p\"",
            "Hello",
        ] {
            assert!(bundle.source.contains(expected), "missing {expected}");
        }
    }

    #[test]
    fn renders_data_view_bound_input_and_data_action() {
        let temp = tempdir().unwrap();
        let project = temp.path().join("App.mpr");
        let text_box = doc! {
            "$Type": "Forms$TextBox",
            "Name": "customerName",
            "AttributeRef": { "Attribute": "Demo.Order.CustomerName" },
            "LabelTemplate": { "Template": { "Items": mxrs_bson::build_array(vec![Bson::Document(doc! {
                "LanguageCode": "en_US", "Text": "Customer",
            })], 3) } },
            "PlaceholderTemplate": { "Template": { "Items": mxrs_bson::build_array(vec![Bson::Document(doc! {
                "LanguageCode": "en_US", "Text": "Enter a name",
            })], 3) } },
        };
        let save = doc! {
            "$Type": "Forms$ActionButton",
            "Name": "save",
            "Action": { "$Type": "Forms$SaveChangesClientAction", "ClosePage": true },
            "CaptionTemplate": { "Template": { "Items": mxrs_bson::build_array(vec![Bson::Document(doc! {
                "LanguageCode": "en_US", "Text": "Save",
            })], 3) } },
        };
        let data_view = doc! {
            "$Type": "Forms$DataView",
            "Name": "orderView",
            "DataSource": {
                "$Type": "Forms$DataViewSource",
                "EntityRef": { "Entity": "Demo.Order" },
                "SourceVariable": { "PageParameter": "Order" },
            },
            "Widgets": mxrs_bson::build_array(vec![Bson::Document(text_box), Bson::Document(save)], 2),
            "FooterWidgets": mxrs_bson::build_array(Vec::new(), 2),
        };
        let documents = Vec::new();
        let bundle = PageBundleCompiler::new(&documents, &project)
            .compile_page("Demo", &page(vec![data_view]))
            .unwrap();
        assert!(bundle.unsupported_widgets.is_empty());
        for expected in [
            "React.createElement($DataView",
            "AssociationObjectProperty",
            "React.createElement($TextBox",
            "AttributeProperty",
            "React.createElement($FormGroup",
            "React.createElement($ActionButton",
            "saveChanges",
            "Customer",
            "Enter a name",
        ] {
            assert!(bundle.source.contains(expected), "missing {expected}");
        }
    }
}
