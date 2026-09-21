//! Compiles `mxrs_ir::page::PageDecl`/`WidgetDecl` (the small,
//! storage-independent page IR — see that module's doc comment for the
//! exact widget vocabulary and every deferred gap) into a `Forms$Page`
//! document, via `mxrs-forms`'s already-lossless, schema-driven
//! `Node`/`MprCodec` rather than hand-rolled BSON (reuses the mature Forms
//! metamodel codec instead of re-deriving it — the same reasoning
//! `domain.rs`/`flow_compiler.rs` apply to `mxrs-model`/`mxrs-bson`).
//!
//! Mirrors `flow_compiler`'s role: an author-level IR is compiled into a
//! *low-level* structure the storage layer understands, the same
//! relationship `flow_compiler::build_microflow_graph` has to `Activity`.
//! Nested element `$ID`s are freshly randomized on every compile
//! (`mxrs-forms::MprCodec::encode` always mints one via
//! `uuid::Uuid::new_v4()` — it has no way to accept a caller-supplied id),
//! exactly like `flow_compiler`'s activity graph: only the page's own
//! top-level identity needs to survive across rebuilds, because the whole
//! document is fully re-derived from the declared `PageDecl` on every
//! write, never diffed widget-by-widget. The caller
//! (`documents::synchronize_pages_with_identity`) overwrites the returned
//! document's `$ID` with the page's stable identity after this returns —
//! mirroring how `documents::enumeration_document` and
//! `module::write_module`'s microflow loop assign their own top-level `$ID`
//! after building the rest of the document.
//!
//! **Widgets require a `layout`, loudly**: a `Forms$Page` document has no
//! `Widgets` property of its own — every real Mendix page attaches its
//! widget tree to a `LayoutCallArgument` inside `LayoutCall.arguments`,
//! keyed to one of the referenced Layout's own parameter names (confirmed
//! against the embedded `forms-11.12.1.json` schema, and matches how
//! `mxrs-model::page::form_call_widgets` reads widgets back out of exactly
//! this shape). A page that declares widgets but no `layout` is therefore
//! a loud [`crate::WriterError::PageWidgetsRequireLayout`], never a
//! silently empty page — rule #1's "loud, not silent" gap policy applied
//! to a structural impossibility rather than an unimplemented feature.
//!
//! **Verified against a real Studio-Pro-produced document, not just this
//! crate's own round trip**: `mxrs-forms/tests/fixtures/native_page.bson`
//! (decoded in that crate's own test suite) is a real exported
//! `Forms$Page` that itself omits `Widgets`/`LayoutCall`/`Appearance`
//! entirely — confirming `MprCodec` doesn't require every schema-"required"
//! property to be present to produce a well-formed document; this compiler
//! likewise only ever sets the properties this IR captures. Native fixture
//! round trips do not certify the whole authoring surface: official MxBuild
//! validation is a separate gate. It exposed the requirement for a nonempty
//! language on newly authored text even though our display reader could show
//! the unlocalized value. Every supported widget/version combination still
//! needs its own official and browser acceptance evidence.
//!
//! **Pluggable widgets (`WidgetDecl::DataGrid2`/`Gallery`/`ComboBox`) via
//! `mxrs-pluggable`, name/class plus write-time package-schema
//! synchronization.** `mxrs_pluggable::encode_widget` can build a
//! from-scratch `CustomWidgets$CustomWidget` two ways: with an inline
//! property *schema* (`WidgetType::object_type`), or without one — Data
//! Grid 2/Gallery/ComboBox's *real* schemas (dozens of properties each,
//! e.g. Data Grid 2's `columns`/`datasource`/`itemSelection`) only exist
//! inside the actual widget package Studio Pro installs into a project.
//! In the oracle, `lib/mxrb/writer.rb#pluggable_widget_doc` calls
//! `WidgetPackage.find(...)` first: when the widget's `.mpk` is installed
//! next to the target, the embedded type carries that package's full
//! schema and the object carries default-valued properties
//! (`WidgetPackage.template`). This compiler now ports that path through
//! `mxrs-widget-package` (see `pluggable_widget_value`/
//! `default_object_node` below and the `packages_root` parameter). When
//! the lookup misses, mxrb's own fallback
//! (`configure_fallback_data_grid!`/`configure_fallback_combo_box!`) emits
//! an **empty** inline schema (`ObjectType` with zero `PropertyTypes`) so
//! Studio Pro can recognize the widget by its real `WidgetId` and
//! "hydrate" the schema itself on next open. This compiler ports exactly
//! that empty-schema shape (see `pluggable_widget_type` below) — real,
//! oracle-confirmed `WidgetId`s
//! (`com.mendix.widget.web.datagrid.Datagrid`/`.gallery.Gallery`/
//! `.combobox.Combobox`, also used verifiably elsewhere in this workspace
//! by `mxrs-compiler-widgets::{DATA_GRID_WIDGET_ID,GALLERY_WIDGET_ID,COMBO_BOX_WIDGET_ID}`),
//! real `Name`/`Appearance` (reusing `widget_name`/`appearance_node` below,
//! not duplicated). **Deliberately not ported**: mxrb's fallback additionally
//! writes a handful of native-Forms-shaped convenience keys directly onto
//! the wrapper document outside the pluggable schema entirely (e.g. a
//! Data Grid 2's fallback `DataSource`/`Columns`/`ToolBar`, a Combo Box's
//! `AttributePath`/`SelectorType`) — inert metadata `mxrb`'s own comment
//! says exists only "until Studio Pro can hydrate the widget", consumed by
//! no decoder anywhere in this workspace (`mxrs-model::page::pluggable_widget`
//! only ever reads a `CustomWidget`'s `Name`). Reproducing them exactly
//! would mean guessing at Mendix-version-specific raw type names mxrb's own
//! fallback code doesn't consistently match against this workspace's own
//! `forms-11.12.1.json` schema (e.g. it writes `Forms$DataGridColumn`,
//! `Forms$GridDeleteButton` — the schema's real names, per
//! `mxrs-forms::storage_naming`'s `TYPE_ALIASES`, are `GridColumn`/
//! `DataGridRemoveButton`) — exactly the "risky BSON shape" this
//! project's own rules say not to guess at. With the package-schema source
//! now in place, entity/attribute/column/data-source *configuration* (an
//! IR richer than name/class assigning real values into that schema)
//! remains the separately trackable follow-up.

use std::collections::BTreeMap;
use std::path::Path;
use std::rc::Rc;

use mxrs_bson::Document;
use mxrs_forms::catalog::{Catalog, ReferenceKind};
use mxrs_forms::node::{Node, Value};
use mxrs_forms::values::{AttributeReference, Reference, Text, Translation};
use mxrs_ir::page::{
    ButtonAction, DataSourceDecl, LayoutGridColumnDecl, LayoutGridRowDecl, PageDecl, WidgetDecl,
};
use mxrs_pluggable::{Assignment, ObjectNode, ObjectType, PropertyType, WidgetNode, WidgetType};

use crate::error::{Result, WriterError};

/// Compiles a page into a `Forms$Page` document. The returned document's
/// `$ID` is a fresh random UUID (from `MprCodec::encode`) — callers that
/// need a stable identity must overwrite it, see this module's doc comment.
///
/// `packages_root` is the directory holding the target project's
/// `widgets/*.mpk` packages (the `.mpr`'s own directory in practice) —
/// mxrb's write-time `WidgetPackage.find(File.dirname(@path), id)` schema
/// synchronization. `None` compiles pluggable widgets with the empty-schema
/// fallback only.
pub fn compile_page(
    catalog: &Rc<Catalog>,
    decl: &PageDecl,
    packages_root: Option<&Path>,
) -> Result<Document> {
    if decl.layout.is_none() && !decl.widgets.is_empty() {
        return Err(WriterError::PageWidgetsRequireLayout(decl.name.clone()));
    }
    validate_page_parameters(decl)?;

    let mut page = Node::new("Page", catalog.clone())?;
    page.set("name", Value::String(decl.name.clone()))?;
    let title = decl.title.clone().unwrap_or_else(|| decl.name.clone());
    page.set("title", Value::Text(default_language_text(&title)))?;
    if !decl.documentation.is_empty() {
        page.set("documentation", Value::String(decl.documentation.clone()))?;
    }
    if !decl.url.is_empty() {
        page.set("url", Value::String(decl.url.clone()))?;
    }
    page.set("excluded", Value::Boolean(decl.excluded))?;
    page.set("exportLevel", Value::String(decl.export_level.clone()))?;
    page.set("popupWidth", Value::Integer(i64::from(decl.popup_width)))?;
    page.set("popupHeight", Value::Integer(i64::from(decl.popup_height)))?;
    page.set("popupResizable", Value::Boolean(decl.popup_resizable))?;
    page.set(
        "allowedRoles",
        Value::List(
            decl.allowed_module_roles
                .iter()
                .map(|role| {
                    Value::Reference(Reference {
                        target: role.clone(),
                        kind: ReferenceKind::ByName,
                    })
                })
                .collect(),
        ),
    )?;
    if let Some(appearance) =
        appearance_node(catalog, decl.class.as_deref(), decl.style.as_deref())?
    {
        page.set("appearance", Value::Node(appearance))?;
    }
    let parameters = decl
        .parameters
        .iter()
        .map(|parameter| {
            let mut node = Node::new("PageParameter", catalog.clone())?;
            node.set("name", Value::String(parameter.name.clone()))?;
            node.set(
                "parameterType",
                Value::DataType(mxrs_forms::values::DataType::object(
                    parameter.entity.clone(),
                )),
            )?;
            node.set("isRequired", Value::Boolean(parameter.required))?;
            if let Some(default_value) = &parameter.default_value {
                node.set(
                    "defaultValue",
                    Value::Expression(mxrs_forms::values::Expression::new(default_value)),
                )?;
            }
            Ok(Value::Node(node))
        })
        .collect::<Result<Vec<_>>>()?;
    page.set("parameters", Value::List(parameters))?;

    if let Some(layout) = &decl.layout {
        let mut layout_call = Node::new("LayoutCall", catalog.clone())?;
        layout_call.set(
            "layout",
            Value::Reference(Reference {
                target: layout.qualified_name.clone(),
                kind: ReferenceKind::ByName,
            }),
        )?;
        let mut argument = Node::new("LayoutCallArgument", catalog.clone())?;
        argument.set(
            "parameter",
            Value::Reference(Reference {
                target: qualified_layout_parameter(layout)?,
                kind: ReferenceKind::ByName,
            }),
        )?;
        let mut counter = 0u32;
        let widgets = decl
            .widgets
            .iter()
            .map(|widget| compile_widget(catalog, widget, &mut counter, packages_root))
            .collect::<Result<Vec<_>>>()?;
        argument.set("widgets", Value::List(widgets))?;
        layout_call.set("arguments", Value::List(vec![Value::Node(argument)]))?;
        page.set("layoutCall", Value::Node(layout_call))?;
    }

    let codec = mxrs_forms::MprCodec::new(catalog.clone());
    Ok(codec.encode(&page)?)
}

/// By-name references point at `Module.Layout.Placeholder`, not the local
/// placeholder name (`mxrb/writer.rb`, `page_doc`, FormCallArgument.Parameter).
fn qualified_layout_parameter(layout: &mxrs_ir::LayoutRef) -> Result<String> {
    let prefix = format!("{}.", layout.qualified_name);
    let local = if layout.parameter.contains('.') {
        layout.parameter.strip_prefix(&prefix)
    } else {
        Some(layout.parameter.as_str())
    };
    let Some(local) = local.filter(|name| !name.is_empty() && !name.contains('.')) else {
        return Err(WriterError::InvalidLayoutParameterReference {
            layout: layout.qualified_name.clone(),
            parameter: layout.parameter.clone(),
        });
    };
    Ok(format!("{prefix}{local}"))
}

fn validate_page_parameters(decl: &PageDecl) -> Result<()> {
    let mut parameters = BTreeMap::new();
    for parameter in &decl.parameters {
        if parameters
            .insert(parameter.name.as_str(), parameter.entity.as_str())
            .is_some()
        {
            return Err(WriterError::DuplicatePageParameter {
                page: decl.name.clone(),
                parameter: parameter.name.clone(),
            });
        }
    }
    fn visit(page: &str, widget: &WidgetDecl, parameters: &BTreeMap<&str, &str>) -> Result<()> {
        match widget {
            WidgetDecl::ApplicationShell { .. } => {
                return Err(WriterError::PlaceholderOutsideLayout {
                    page: page.into(),
                    placeholder: "Main".into(),
                });
            }
            WidgetDecl::LayoutPlaceholder { name } => {
                return Err(WriterError::PlaceholderOutsideLayout {
                    page: page.to_string(),
                    placeholder: name.clone(),
                });
            }
            WidgetDecl::DataView {
                source: DataSourceDecl::Context { parameter, entity },
                children,
                ..
            } => {
                let Some(actual) = parameters.get(parameter.as_str()) else {
                    return Err(WriterError::UnknownPageParameter {
                        page: page.to_string(),
                        parameter: parameter.clone(),
                    });
                };
                if *actual != entity {
                    return Err(WriterError::PageParameterEntityMismatch {
                        page: page.to_string(),
                        parameter: parameter.clone(),
                        expected: entity.clone(),
                        actual: (*actual).to_string(),
                    });
                }
                for child in children {
                    visit(page, child, parameters)?;
                }
            }
            WidgetDecl::DataView { children, .. } | WidgetDecl::Container { children, .. } => {
                for child in children {
                    visit(page, child, parameters)?;
                }
            }
            WidgetDecl::LayoutGrid { rows, .. } => {
                for child in rows
                    .iter()
                    .flat_map(|row| &row.columns)
                    .flat_map(|column| &column.children)
                {
                    visit(page, child, parameters)?;
                }
            }
            _ => {}
        }
        Ok(())
    }
    for widget in &decl.widgets {
        visit(&decl.name, widget, &parameters)?;
    }
    Ok(())
}

pub(crate) fn compile_widget(
    catalog: &Rc<Catalog>,
    widget: &WidgetDecl,
    counter: &mut u32,
    packages_root: Option<&Path>,
) -> Result<Value> {
    match widget {
        WidgetDecl::ApplicationShell { title, navigation } => {
            application_shell(catalog, title, navigation.as_deref())
        }
        WidgetDecl::LayoutPlaceholder { name } => {
            let mut node = Node::new("Placeholder", catalog.clone())?;
            node.set("name", Value::String(name.clone()))?;
            node.set("tabIndex", Value::Integer(0))?;
            node.set(
                "appearance",
                Value::Node(Node::new("Appearance", catalog.clone())?),
            )?;
            Ok(Value::Node(node))
        }
        WidgetDecl::Container {
            name,
            class,
            style,
            children,
        } => {
            let mut node = Node::new("DivContainer", catalog.clone())?;
            node.set(
                "name",
                Value::String(widget_name(name, "container", counter)),
            )?;
            if let Some(appearance) = appearance_node(catalog, class.as_deref(), style.as_deref())?
            {
                node.set("appearance", Value::Node(appearance))?;
            }
            let compiled = children
                .iter()
                .map(|child| compile_widget(catalog, child, counter, packages_root))
                .collect::<Result<Vec<_>>>()?;
            node.set("widgets", Value::List(compiled))?;
            Ok(Value::Node(node))
        }
        WidgetDecl::LayoutGrid { name, rows } => {
            let mut node = Node::new("LayoutGrid", catalog.clone())?;
            node.set(
                "name",
                Value::String(widget_name(name, "layoutGrid", counter)),
            )?;
            let compiled_rows = rows
                .iter()
                .map(|row| compile_layout_grid_row(catalog, row, counter, packages_root))
                .collect::<Result<Vec<_>>>()?;
            node.set("rows", Value::List(compiled_rows))?;
            Ok(Value::Node(node))
        }
        WidgetDecl::Text {
            name,
            caption,
            class,
        } => {
            let mut node = Node::new("DynamicText", catalog.clone())?;
            node.set("name", Value::String(widget_name(name, "text", counter)))?;
            node.set("content", Value::Node(client_template(catalog, caption)?))?;
            if let Some(appearance) = appearance_node(catalog, class.as_deref(), None)? {
                node.set("appearance", Value::Node(appearance))?;
            }
            Ok(Value::Node(node))
        }
        WidgetDecl::Button {
            name,
            caption,
            class,
            action,
        } => {
            let mut node = Node::new("ActionButton", catalog.clone())?;
            node.set("name", Value::String(widget_name(name, "button", counter)))?;
            node.set("caption", Value::Node(client_template(catalog, caption)?))?;
            node.set("action", Value::Node(button_action_node(catalog, action)?))?;
            if let Some(appearance) = appearance_node(catalog, class.as_deref(), None)? {
                node.set("appearance", Value::Node(appearance))?;
            }
            Ok(Value::Node(node))
        }
        WidgetDecl::DataView {
            name,
            source,
            children,
        } => {
            let mut node = Node::new("DataView", catalog.clone())?;
            node.set(
                "name",
                Value::String(widget_name(name, "dataView", counter)),
            )?;
            node.set(
                "dataSource",
                Value::Node(data_source_node(catalog, source)?),
            )?;
            let compiled = children
                .iter()
                .map(|child| compile_widget(catalog, child, counter, packages_root))
                .collect::<Result<Vec<_>>>()?;
            node.set("widgets", Value::List(compiled))?;
            Ok(Value::Node(node))
        }
        WidgetDecl::TextBox {
            name,
            attribute,
            class,
        } => attribute_widget_node(
            catalog, "TextBox", "textBox", name, attribute, class, counter,
        ),
        WidgetDecl::CheckBox {
            name,
            attribute,
            class,
        } => attribute_widget_node(
            catalog, "CheckBox", "checkBox", name, attribute, class, counter,
        ),
        WidgetDecl::DatePicker {
            name,
            attribute,
            class,
        } => attribute_widget_node(
            catalog,
            "DatePicker",
            "datePicker",
            name,
            attribute,
            class,
            counter,
        ),
        WidgetDecl::DropDown {
            name,
            attribute,
            class,
        } => attribute_widget_node(
            catalog, "DropDown", "dropDown", name, attribute, class, counter,
        ),
        WidgetDecl::DataGrid2 { name, class } => pluggable_widget_value(
            catalog,
            packages_root,
            data_grid_2_widget_type(),
            "dataGrid2",
            name,
            class,
            counter,
        ),
        WidgetDecl::Gallery { name, class } => pluggable_widget_value(
            catalog,
            packages_root,
            gallery_widget_type(),
            "gallery",
            name,
            class,
            counter,
        ),
        WidgetDecl::ComboBox { name, class } => pluggable_widget_value(
            catalog,
            packages_root,
            combo_box_widget_type(),
            "comboBox",
            name,
            class,
            counter,
        ),
    }
}

/// Real, oracle-confirmed `WidgetId`s — see this module's doc comment.
/// `studio_category`/`studio_pro_category` for Data Grid 2 and Combo Box
/// are also taken directly from `lib/mxrb/writer.rb`'s
/// `data_grid2_descriptor`/`combo_box_descriptor`; Gallery has no
/// equivalent descriptor in the oracle (mxrb's own DSL has no
/// Gallery-specific writer support either — only the fully generic
/// `:pluggable_widget` path), so its category reuses Data Grid 2's as a
/// reasonable placeholder, not an oracle-derived value.
fn data_grid_2_widget_type() -> WidgetType {
    empty_pluggable_widget_type(
        "com.mendix.widget.web.datagrid.Datagrid",
        "Data grid 2",
        "Data containers",
        "Data Containers",
    )
}

fn gallery_widget_type() -> WidgetType {
    empty_pluggable_widget_type(
        "com.mendix.widget.web.gallery.Gallery",
        "Gallery",
        "Data containers",
        "Data Containers",
    )
}

fn combo_box_widget_type() -> WidgetType {
    empty_pluggable_widget_type(
        "com.mendix.widget.web.combobox.Combobox",
        "Combo box",
        "Input widgets",
        "Input Widgets",
    )
}

/// Empty-schema shell matching `mxrb`'s own fallback shape — see this
/// module's doc comment for why. `offline: true`/`needs_context: false`/
/// `plugin: true`/`help_url: ""`/`description: ""` mirror
/// `Mxrb::Writer#pluggable_widget_doc`'s fallback literal exactly
/// (`"OfflineCapable" => true`, `"WidgetNeedsEntityContext" => false`,
/// `"WidgetPluginWidget" => true`).
fn empty_pluggable_widget_type(
    id: &str,
    name: &str,
    studio_category: &str,
    studio_pro_category: &str,
) -> WidgetType {
    WidgetType {
        id: id.to_string(),
        name: name.to_string(),
        description: String::new(),
        prompt: String::new(),
        studio_pro_category: studio_pro_category.to_string(),
        studio_category: studio_category.to_string(),
        platform: "Web".to_string(),
        offline: true,
        needs_context: false,
        plugin: true,
        help_url: String::new(),
        object_type: ObjectType { properties: vec![] },
    }
}

/// Builds a `Value::Pluggable` for a custom widget — the same mechanism
/// `mxrs-forms::node::Value::Pluggable` uses for any pluggable widget
/// embedded in a page's widget list (verified round-trip coverage in
/// `mxrs-forms/tests/native_page.rs`). `Name`/`Appearance` are set via
/// `extra` (see `mxrs_pluggable::WidgetNode::extra`'s doc comment) rather
/// than through the pluggable property schema, reusing this file's own
/// `widget_name`/`appearance_node` — not duplicated — encoded through the
/// same schema-driven `MprCodec` the rest of this compiler uses, rather
/// than hand-rolled BSON.
///
/// When the widget's real package is installed next to the target
/// (`packages_root`, mxrb's `WidgetPackage.find`), the widget carries that
/// package's full property schema plus a default-valued object — mxrb's
/// `WidgetPackage.template`. Otherwise it falls back to the empty-schema
/// shell described in this module's doc comment.
fn pluggable_widget_value(
    catalog: &Rc<Catalog>,
    packages_root: Option<&Path>,
    fallback: WidgetType,
    default_prefix: &str,
    name: &Option<String>,
    class: &Option<String>,
    counter: &mut u32,
) -> Result<Value> {
    let found = match packages_root {
        Some(root) => mxrs_widget_package::find(root, &fallback.id)
            .map_err(|error| WriterError::WidgetPackage(error.to_string()))?,
        None => None,
    };
    let (widget_type, object) = match found {
        Some(definition) => {
            let object = default_object_node(catalog, &definition.object_type)?;
            (definition, object)
        }
        None => (fallback, ObjectNode::new()),
    };
    let mut widget = WidgetNode::new(widget_type, object);
    widget
        .extra
        .insert("Name", widget_name(name, default_prefix, counter));
    if let Some(appearance) = appearance_node(catalog, class.as_deref(), None)? {
        let codec = mxrs_forms::MprCodec::new(catalog.clone());
        widget
            .extra
            .insert("Appearance", codec.encode(&appearance)?);
    }
    Ok(Value::Pluggable(Box::new(widget)))
}

/// Ports the value side of `Mxrb::WidgetPackage.template`
/// (`property_pair`/`widget_value`/`default_widget_value`): every
/// non-system property gets an assignment carrying its schema default.
/// Kinds mxrb leaves at the `WidgetValue` baseline (references, data
/// sources, nested objects/widget lists) are `Value::Null` here — the
/// pluggable codec's `empty_widget_value` IS that baseline — except
/// `Action`, whose baseline `Forms$NoAction` must be stated explicitly
/// because `Null` means "explicitly no action" for that kind.
fn default_object_node(
    catalog: &Rc<Catalog>,
    object_type: &ObjectType,
) -> Result<ObjectNode<Node>> {
    let mut object = ObjectNode::new();
    for property in &object_type.properties {
        if property.is_system() {
            continue;
        }
        object.push(Assignment {
            property: property.clone(),
            value: default_property_value(catalog, property)?,
            source_variable: None,
        });
    }
    Ok(object)
}

fn default_property_value(
    catalog: &Rc<Catalog>,
    property: &PropertyType,
) -> Result<mxrs_pluggable::Value<Node>> {
    let value_type = &property.value_type;
    let default = value_type.default_value.as_str();
    Ok(match value_type.kind.as_str() {
        "Boolean" => mxrs_pluggable::Value::Boolean(default == "true"),
        "Integer" => mxrs_pluggable::Value::Integer(if default.is_empty() {
            0
        } else {
            default
                .parse()
                .map_err(|_| WriterError::InvalidWidgetPropertyDefault {
                    property: property.key.clone(),
                    default: default.to_string(),
                })?
        }),
        "String" | "Decimal" | "Enumeration" => {
            mxrs_pluggable::Value::Primitive(default.to_string())
        }
        "Expression" => mxrs_pluggable::Value::Expression(default.to_string()),
        "Selection" => mxrs_pluggable::Value::Selection(if default.is_empty() {
            value_type
                .selection_types
                .first()
                .cloned()
                .unwrap_or_else(|| "None".to_string())
        } else {
            default.to_string()
        }),
        "TextTemplate"
            if value_type.required
                || !default.is_empty()
                || !value_type.translations.is_empty() =>
        {
            let mut template = Node::new("ClientTemplate", catalog.clone())?;
            template.set(
                "template",
                Value::Text(Text::from_translations(
                    value_type
                        .translations
                        .iter()
                        .map(|translation| Translation {
                            language: Some(translation.language.clone()),
                            text: translation.text.clone(),
                        })
                        .collect(),
                )),
            )?;
            mxrs_pluggable::Value::TextTemplate(template)
        }
        "Action" => {
            let mut action = Node::new("NoClientAction", catalog.clone())?;
            action.set("disabledDuringExecution", Value::Boolean(true))?;
            mxrs_pluggable::Value::Action(action)
        }
        _ => mxrs_pluggable::Value::Null,
    })
}

/// Shared shape behind `TextBox`/`CheckBox`/`DatePicker`/`DropDown`: all
/// four extend Mendix's `MemberWidget` (directly, or via `AttributeWidget`)
/// and expose exactly one property this compiler needs beyond
/// name/appearance — `attributeRef` — so one function builds all of them,
/// keyed by schema type name. Not folded into `compile_widget`'s own
/// `match` to keep that `match` one arm per `WidgetDecl` variant, matching
/// this file's existing style for `Container`/`Text`/`Button`.
fn attribute_widget_node(
    catalog: &Rc<Catalog>,
    schema_type: &str,
    default_prefix: &str,
    name: &Option<String>,
    attribute: &str,
    class: &Option<String>,
    counter: &mut u32,
) -> Result<Value> {
    let mut node = Node::new(schema_type, catalog.clone())?;
    node.set(
        "name",
        Value::String(widget_name(name, default_prefix, counter)),
    )?;
    node.set(
        "attributeRef",
        Value::AttributeReference(AttributeReference {
            attribute: attribute.to_string(),
            entity_reference: None,
        }),
    )?;
    if let Some(appearance) = appearance_node(catalog, class.as_deref(), None)? {
        node.set("appearance", Value::Node(appearance))?;
    }
    Ok(Value::Node(node))
}

/// Builds the `MicroflowSource`/`NanoflowSource` node behind a
/// [`WidgetDecl::DataView`] — see `DataSourceDecl`'s doc comment for why
/// only these two variants exist yet.
fn data_source_node(catalog: &Rc<Catalog>, source: &DataSourceDecl) -> Result<Node> {
    match source {
        DataSourceDecl::Microflow(qualified_name) => {
            let mut settings = Node::new("MicroflowSettings", catalog.clone())?;
            settings.set(
                "microflow",
                Value::Reference(Reference {
                    target: qualified_name.clone(),
                    kind: ReferenceKind::ByName,
                }),
            )?;
            let mut node = Node::new("MicroflowSource", catalog.clone())?;
            node.set("microflowSettings", Value::Node(settings))?;
            Ok(node)
        }
        DataSourceDecl::Nanoflow(qualified_name) => {
            let mut node = Node::new("NanoflowSource", catalog.clone())?;
            node.set(
                "nanoflow",
                Value::Reference(Reference {
                    target: qualified_name.clone(),
                    kind: ReferenceKind::ByName,
                }),
            )?;
            Ok(node)
        }
        DataSourceDecl::Context { parameter, .. } => {
            let mut variable = Node::new("PageVariable", catalog.clone())?;
            variable.set(
                "pageParameter",
                Value::Reference(Reference {
                    target: parameter.clone(),
                    kind: ReferenceKind::ByName,
                }),
            )?;
            let mut node = Node::new("DataViewSource", catalog.clone())?;
            node.set("sourceVariable", Value::Node(variable))?;
            Ok(node)
        }
    }
}

fn compile_layout_grid_row(
    catalog: &Rc<Catalog>,
    row: &LayoutGridRowDecl,
    counter: &mut u32,
    packages_root: Option<&Path>,
) -> Result<Value> {
    let mut node = Node::new("LayoutGridRow", catalog.clone())?;
    let columns = row
        .columns
        .iter()
        .map(|column| compile_layout_grid_column(catalog, column, counter, packages_root))
        .collect::<Result<Vec<_>>>()?;
    node.set("columns", Value::List(columns))?;
    Ok(Value::Node(node))
}

fn compile_layout_grid_column(
    catalog: &Rc<Catalog>,
    column: &LayoutGridColumnDecl,
    counter: &mut u32,
    packages_root: Option<&Path>,
) -> Result<Value> {
    let mut node = Node::new("LayoutGridColumn", catalog.clone())?;
    node.set("weight", Value::Integer(i64::from(column.weight)))?;
    let children = column
        .children
        .iter()
        .map(|child| compile_widget(catalog, child, counter, packages_root))
        .collect::<Result<Vec<_>>>()?;
    node.set("widgets", Value::List(children))?;
    Ok(Value::Node(node))
}

fn client_template(catalog: &Rc<Catalog>, text: &str) -> Result<Node> {
    let mut node = Node::new("ClientTemplate", catalog.clone())?;
    node.set("template", Value::Text(default_language_text(text)))?;
    Ok(node)
}

/// Plain authoring strings use `en_US`, as `mxrb/writer.rb#text_doc` does.
/// An empty language is not a fallback: MxBuild 11.12.1 reports CW0263 even
/// when the stored text is nonempty. Explicit multilingual page authoring
/// remains outside this string-only IR; imported translations stay lossless
/// through the snapshot fallback, without changing the generic Forms codec.
fn default_language_text(text: &str) -> Text {
    Text::from_translations(vec![Translation {
        language: Some("en_US".into()),
        text: text.into(),
    }])
}

fn button_action_node(catalog: &Rc<Catalog>, action: &ButtonAction) -> Result<Node> {
    match action {
        ButtonAction::None => Ok(Node::new("NoClientAction", catalog.clone())?),
        ButtonAction::ClosePage => Ok(Node::new("ClosePageClientAction", catalog.clone())?),
        // Both carry no settings node: the catalog declares their properties
        // (`syncAutomatically`/`closePage`) with defaults, so constructing the
        // node is the whole action.
        ButtonAction::SaveChanges => Ok(Node::new("SaveChangesClientAction", catalog.clone())?),
        ButtonAction::CancelChanges => Ok(Node::new("CancelChangesClientAction", catalog.clone())?),
        ButtonAction::CallMicroflow(qualified_name) => {
            let mut settings = Node::new("MicroflowSettings", catalog.clone())?;
            settings.set(
                "microflow",
                Value::Reference(Reference {
                    target: qualified_name.clone(),
                    kind: ReferenceKind::ByName,
                }),
            )?;
            let mut node = Node::new("MicroflowClientAction", catalog.clone())?;
            node.set("microflowSettings", Value::Node(settings))?;
            Ok(node)
        }
        ButtonAction::CallNanoflow(qualified_name) => {
            let mut node = Node::new("CallNanoflowClientAction", catalog.clone())?;
            node.set(
                "nanoflow",
                Value::Reference(Reference {
                    target: qualified_name.clone(),
                    kind: ReferenceKind::ByName,
                }),
            )?;
            Ok(node)
        }
    }
}

pub(crate) fn appearance_node(
    catalog: &Rc<Catalog>,
    class: Option<&str>,
    style: Option<&str>,
) -> Result<Option<Node>> {
    if class.is_none() && style.is_none() {
        return Ok(None);
    }
    let mut node = Node::new("Appearance", catalog.clone())?;
    if let Some(class) = class {
        node.set("class", Value::String(class.to_string()))?;
    }
    if let Some(style) = style {
        node.set("style", Value::String(style.to_string()))?;
    }
    Ok(Some(node))
}

/// Falls back to a sequential default (`"container1"`, `"text2"`, ...) so
/// every widget still gets a unique-within-the-page name even when the
/// author didn't call `.name(...)` — Mendix widget names must be unique per
/// page, though nothing in this compiler enforces that for
/// author-supplied names (a duplicate author-chosen name round-trips
/// through `MprCodec` today; whether Studio Pro accepts it is unverified,
/// same caveat as this module's doc comment).
fn widget_name(name: &Option<String>, default_prefix: &str, counter: &mut u32) -> String {
    if let Some(name) = name {
        return name.clone();
    }
    *counter += 1;
    format!("{default_prefix}{counter}")
}

// The composition used by MXRB's layout helper, expressed through the checked
// Forms schema. IDs and version-specific storage names belong to the codec.
fn application_shell(
    catalog: &Rc<Catalog>,
    title: &str,
    navigation: Option<&str>,
) -> Result<Value> {
    fn widget(catalog: &Rc<Catalog>, kind: &str, name: &str, class: &str) -> Result<Node> {
        let mut node = Node::new(kind, catalog.clone())?;
        node.set("name", Value::String(name.into()))?;
        node.set("tabIndex", Value::Integer(0))?;
        node.set(
            "appearance",
            Value::Node(shell_appearance(catalog, Some(class), None)?),
        )?;
        Ok(node)
    }
    fn region(
        catalog: &Rc<Catalog>,
        class: &str,
        size: i64,
        mode: &str,
        toggle: &str,
        children: Vec<Value>,
    ) -> Result<Node> {
        let mut node = Node::new("ScrollContainerRegion", catalog.clone())?;
        node.set(
            "appearance",
            Value::Node(shell_appearance(catalog, Some(class), None)?),
        )?;
        node.set("size", Value::Integer(size))?;
        node.set("sizeMode", Value::String(mode.into()))?;
        node.set("toggleMode", Value::String(toggle.into()))?;
        node.set("widgets", Value::List(children))?;
        Ok(node)
    }
    let mut container = widget(catalog, "ScrollContainer", "scrollContainer1", "")?;
    for (property, value) in [
        ("alignment", "Center"),
        ("layoutMode", "Headline"),
        ("widthMode", "Auto"),
        ("scrollBehavior", "PerRegion"),
    ] {
        container.set(property, Value::String(value.into()))?;
    }
    container.set("width", Value::Integer(960))?;
    container.set("nativeHideScrollbars", Value::Boolean(false))?;
    let mut header = Vec::new();
    if let Some(profile) = navigation {
        let mut source = Node::new("NavigationSource", catalog.clone())?;
        source.set("navigationProfile", Value::String(profile.into()))?;
        let mut tree = widget(
            catalog,
            "NavigationTree",
            "navigationTree1",
            "mxrb-navigation",
        )?;
        tree.set("menuSource", Value::Node(source))?;
        container.set(
            "left",
            Value::Node(region(
                catalog,
                "region-sidebar",
                264,
                "Pixels",
                "ShrinkContentInitiallyClosed",
                vec![Value::Node(tree)],
            )?),
        )?;
        let mut toggle = widget(
            catalog,
            "SidebarToggleButton",
            "sidebarToggle1",
            "mxrb-sidebar-toggle",
        )?;
        toggle.set("buttonStyle", Value::String("Primary".into()))?;
        toggle.set("caption", Value::Node(shell_template(catalog, "Menu")?))?;
        toggle.set(
            "tooltip",
            Value::Text(default_language_text("Toggle navigation")),
        )?;
        toggle.set("renderType", Value::String("Button".into()))?;
        header.push(Value::Node(toggle));
    }
    let mut brand = widget(catalog, "DynamicText", "applicationTitle", "mxrb-brand")?;
    brand.set("content", Value::Node(shell_template(catalog, title)?))?;
    brand.set("nativeTextStyle", Value::String("Text".into()))?;
    brand.set("renderMode", Value::String("Text".into()))?;
    header.push(Value::Node(brand));
    let mut topbar = widget(catalog, "DivContainer", "topbarContent", "mxrb-topbar")?;
    let mut action = Node::new("NoClientAction", catalog.clone())?;
    action.set("disabledDuringExecution", Value::Boolean(true))?;
    topbar.set("onClickAction", Value::Node(action))?;
    topbar.set("renderMode", Value::String("Div".into()))?;
    topbar.set("screenReaderHidden", Value::Boolean(false))?;
    topbar.set("widgets", Value::List(header))?;
    container.set(
        "top",
        Value::Node(region(
            catalog,
            "region-topbar",
            72,
            "Pixels",
            "None",
            vec![Value::Node(topbar)],
        )?),
    )?;
    let content = widget(catalog, "Placeholder", "Main", "")?;
    container.set(
        "center",
        Value::Node(region(
            catalog,
            "region-content",
            200,
            "Auto",
            "None",
            vec![Value::Node(content)],
        )?),
    )?;
    Ok(Value::Node(container))
}

pub(crate) fn shell_appearance(
    catalog: &Rc<Catalog>,
    class: Option<&str>,
    style: Option<&str>,
) -> Result<Node> {
    let mut node = Node::new("Appearance", catalog.clone())?;
    node.set("class", Value::String(class.unwrap_or_default().into()))?;
    node.set("style", Value::String(style.unwrap_or_default().into()))?;
    node.set(
        "dynamicClasses",
        Value::Expression(mxrs_forms::values::Expression::new("")),
    )?;
    node.set("designProperties", Value::List(vec![]))?;
    Ok(node)
}
fn shell_template(catalog: &Rc<Catalog>, text: &str) -> Result<Node> {
    let mut node = client_template(catalog, text)?;
    node.set("fallback", Value::Text(default_language_text("")))?;
    node.set("parameters", Value::List(vec![]))?;
    Ok(node)
}

#[cfg(test)]
mod tests {
    use super::*;
    use mxrs_ir::page::LayoutRef;

    fn catalog() -> Rc<Catalog> {
        Rc::new(Catalog::for_version("11.12.1").unwrap())
    }

    #[test]
    fn a_page_with_widgets_but_no_layout_is_a_loud_error() {
        let mut decl = PageDecl::new("OrderOverview");
        decl.widgets.push(WidgetDecl::Text {
            name: None,
            caption: "hi".into(),
            class: None,
        });
        let error = compile_page(&catalog(), &decl, None).unwrap_err();
        assert!(
            matches!(error, WriterError::PageWidgetsRequireLayout(name) if name == "OrderOverview")
        );
    }

    #[test]
    fn a_page_with_no_widgets_compiles_without_a_layout() {
        let decl = PageDecl::new("Empty");
        let document = compile_page(&catalog(), &decl, None).unwrap();
        assert_eq!(document.get_str("$Type").unwrap(), "Forms$Page");
        assert_eq!(document.get_str("Name").unwrap(), "Empty");
    }

    #[test]
    fn plain_page_titles_text_and_buttons_have_the_native_default_language() {
        let mut decl = PageDecl::new("Home");
        decl.layout = Some(LayoutRef::new("Main.ApplicationLayout", "Main"));
        decl.widgets = vec![
            WidgetDecl::Text {
                name: None,
                caption: "Welcome".into(),
                class: None,
            },
            WidgetDecl::Button {
                name: None,
                caption: "Close".into(),
                class: None,
                action: ButtonAction::ClosePage,
            },
        ];
        fn translations(value: &mxrs_bson::Bson, text: &mut Vec<String>) {
            match value {
                mxrs_bson::Bson::Document(document) => {
                    if document.get_str("$Type").ok() == Some("Texts$Translation") {
                        assert_eq!(document.get_str("LanguageCode").unwrap(), "en_US");
                        text.push(document.get_str("Text").unwrap().to_string());
                    }
                    for (_, value) in document {
                        translations(value, text);
                    }
                }
                mxrs_bson::Bson::Array(values) => {
                    for value in values {
                        translations(value, text);
                    }
                }
                _ => {}
            }
        }
        let document = compile_page(&catalog(), &decl, None).unwrap();
        let mut actual = Vec::new();
        translations(&mxrs_bson::Bson::Document(document), &mut actual);
        actual.sort();
        assert_eq!(actual, ["Close", "Home", "Welcome"]);
    }

    #[test]
    fn compiled_page_round_trips_through_the_forms_codec() {
        let catalog = catalog();
        let mut decl = PageDecl::new("OrderOverview");
        decl.title = Some("Orders".into());
        decl.url = "orderoverview".into();
        decl.layout = Some(LayoutRef::new("Atlas_Core.ApplicationLayout", "Main"));
        decl.widgets.push(WidgetDecl::Container {
            name: Some("row".into()),
            class: Some("row".into()),
            style: None,
            children: vec![
                WidgetDecl::Text {
                    name: None,
                    caption: "Manage your orders".into(),
                    class: None,
                },
                WidgetDecl::Button {
                    name: None,
                    caption: "Close".into(),
                    class: None,
                    action: ButtonAction::ClosePage,
                },
            ],
        });
        decl.widgets.push(WidgetDecl::LayoutGrid {
            name: None,
            rows: vec![LayoutGridRowDecl {
                columns: vec![LayoutGridColumnDecl {
                    weight: 1,
                    children: vec![],
                }],
            }],
        });

        let document = compile_page(&catalog, &decl, None).unwrap();
        assert_eq!(document.get_str("$Type").unwrap(), "Forms$Page");

        let codec = mxrs_forms::MprCodec::new(catalog);
        let node = codec.decode(&document).unwrap();
        let re_encoded = codec.encode(&node).unwrap();
        let re_decoded = codec.decode(&re_encoded).unwrap();
        assert_eq!(node, re_decoded);
    }

    #[test]
    fn page_level_metadata_is_written_with_schema_checked_types() {
        let mut decl = PageDecl::new("OrderEdit");
        decl.class = Some("page-order".into());
        decl.style = Some("max-width: 80rem".into());
        decl.allowed_module_roles = vec!["Sales.Editor".into(), "Sales.Manager".into()];
        decl.popup_width = 720;
        decl.popup_height = 480;
        decl.popup_resizable = true;
        decl.excluded = true;
        decl.export_level = "API".into();

        let document = compile_page(&catalog(), &decl, None).unwrap();
        let page = mxrs_model::page::Page::from_bson(&document);
        assert_eq!(page.appearance_class, "page-order");
        assert_eq!(page.appearance_style, "max-width: 80rem");
        assert_eq!(page.allowed_module_roles, ["Sales.Editor", "Sales.Manager"]);
        assert_eq!((page.popup_width, page.popup_height), (720, 480));
        assert!(page.popup_resizable);
        assert!(page.excluded);
        assert_eq!(page.export_level, "API");
    }

    #[test]
    fn a_data_view_with_attribute_bound_widgets_and_flow_calling_buttons_round_trips() {
        let catalog = catalog();
        let mut decl = PageDecl::new("OrderDetail");
        decl.layout = Some(LayoutRef::new("Atlas_Core.ApplicationLayout", "Main"));
        decl.widgets.push(WidgetDecl::DataView {
            name: Some("orderView".into()),
            source: DataSourceDecl::Microflow("Sales.ACT_GetOrder".into()),
            children: vec![
                WidgetDecl::TextBox {
                    name: None,
                    attribute: "Number".into(),
                    class: None,
                },
                WidgetDecl::CheckBox {
                    name: None,
                    attribute: "IsPaid".into(),
                    class: None,
                },
                WidgetDecl::DatePicker {
                    name: None,
                    attribute: "SubmittedAt".into(),
                    class: None,
                },
                WidgetDecl::DropDown {
                    name: None,
                    attribute: "Status".into(),
                    class: None,
                },
                WidgetDecl::Button {
                    name: None,
                    caption: "Submit".into(),
                    class: None,
                    action: ButtonAction::CallMicroflow("Sales.ACT_SubmitOrder".into()),
                },
                WidgetDecl::Button {
                    name: None,
                    caption: "Validate".into(),
                    class: None,
                    action: ButtonAction::CallNanoflow("Sales.NF_ValidateOrder".into()),
                },
            ],
        });

        let document = compile_page(&catalog, &decl, None).unwrap();
        let codec = mxrs_forms::MprCodec::new(catalog);
        let node = codec.decode(&document).unwrap();
        let re_encoded = codec.encode(&node).unwrap();
        let re_decoded = codec.decode(&re_encoded).unwrap();
        assert_eq!(node, re_decoded);
    }

    #[test]
    fn save_and_cancel_buttons_compile_to_the_native_client_actions_and_round_trip() {
        let catalog = catalog();
        let mut decl = PageDecl::new("OrderForm");
        decl.layout = Some(LayoutRef::new("Atlas_Core.ApplicationLayout", "Main"));
        decl.widgets.push(WidgetDecl::DataView {
            name: None,
            source: DataSourceDecl::Microflow("Sales.ACT_LoadOrder".into()),
            children: vec![
                WidgetDecl::Button {
                    name: None,
                    caption: "Save".into(),
                    class: None,
                    action: ButtonAction::SaveChanges,
                },
                WidgetDecl::Button {
                    name: None,
                    caption: "Cancel".into(),
                    class: None,
                    action: ButtonAction::CancelChanges,
                },
            ],
        });

        let document = compile_page(&catalog, &decl, None).unwrap();
        let mut action_types = Vec::new();
        fn collect(value: &mxrs_bson::Bson, out: &mut Vec<String>) {
            if let mxrs_bson::Bson::Document(document) = value {
                if let Ok(kind) = document.get_str("$Type")
                    && kind.ends_with("ChangesClientAction")
                {
                    out.push(kind.to_string());
                }
                for (_, value) in document {
                    collect(value, out);
                }
            } else if let mxrs_bson::Bson::Array(items) = value {
                for item in items {
                    collect(item, out);
                }
            }
        }
        collect(
            &mxrs_bson::Bson::Document(document.clone()),
            &mut action_types,
        );
        action_types.sort();
        assert_eq!(
            action_types,
            [
                "Forms$CancelChangesClientAction",
                "Forms$SaveChangesClientAction"
            ]
        );

        let codec = mxrs_forms::MprCodec::new(catalog);
        let node = codec.decode(&document).unwrap();
        let re_encoded = codec.encode(&node).unwrap();
        let re_decoded = codec.decode(&re_encoded).unwrap();
        assert_eq!(node, re_decoded);
    }

    #[test]
    fn a_data_view_sourced_from_a_nanoflow_round_trips() {
        let catalog = catalog();
        let mut decl = PageDecl::new("OrderDetail");
        decl.layout = Some(LayoutRef::new("Atlas_Core.ApplicationLayout", "Main"));
        decl.widgets.push(WidgetDecl::DataView {
            name: None,
            source: DataSourceDecl::Nanoflow("Sales.NF_GetOrder".into()),
            children: vec![],
        });

        let document = compile_page(&catalog, &decl, None).unwrap();
        let codec = mxrs_forms::MprCodec::new(catalog);
        let node = codec.decode(&document).unwrap();
        let re_encoded = codec.encode(&node).unwrap();
        let re_decoded = codec.decode(&re_encoded).unwrap();
        assert_eq!(node, re_decoded);
    }

    #[test]
    fn object_page_parameter_and_context_data_view_round_trip() {
        let catalog = catalog();
        let mut decl = PageDecl::new("OrderEdit");
        decl.layout = Some(LayoutRef::new("Atlas_Core.ApplicationLayout", "Main"));
        decl.parameters.push(mxrs_ir::page::PageParameterDecl {
            name: "Order".into(),
            entity: "Sales.Order".into(),
            required: true,
            default_value: None,
        });
        decl.widgets.push(WidgetDecl::DataView {
            name: Some("orderView".into()),
            source: DataSourceDecl::Context {
                parameter: "Order".into(),
                entity: "Sales.Order".into(),
            },
            children: vec![WidgetDecl::TextBox {
                name: Some("number".into()),
                attribute: "Number".into(),
                class: None,
            }],
        });

        let document = compile_page(&catalog, &decl, None).unwrap();
        let model = mxrs_model::page::Page::from_bson(&document);
        assert_eq!(model.parameters[0].get_str("Name").unwrap(), "Order");
        assert_eq!(
            model.parameters[0]
                .get_document("ParameterType")
                .unwrap()
                .get_str("Entity")
                .unwrap(),
            "Sales.Order"
        );
        assert_eq!(
            model.widgets[0]
                .options
                .get_document("source")
                .unwrap()
                .get_str("parameter")
                .unwrap(),
            "Order"
        );
    }

    #[test]
    fn context_data_views_reject_unknown_and_mismatched_parameters() {
        let mut decl = PageDecl::new("OrderEdit");
        decl.layout = Some(LayoutRef::new("Atlas_Core.ApplicationLayout", "Main"));
        decl.widgets.push(WidgetDecl::DataView {
            name: None,
            source: DataSourceDecl::Context {
                parameter: "Order".into(),
                entity: "Sales.Order".into(),
            },
            children: vec![],
        });
        assert!(matches!(
            compile_page(&catalog(), &decl, None),
            Err(WriterError::UnknownPageParameter { parameter, .. }) if parameter == "Order"
        ));

        decl.parameters.push(mxrs_ir::page::PageParameterDecl {
            name: "Order".into(),
            entity: "Sales.Customer".into(),
            required: true,
            default_value: None,
        });
        assert!(matches!(
            compile_page(&catalog(), &decl, None),
            Err(WriterError::PageParameterEntityMismatch { actual, expected, .. })
                if actual == "Sales.Customer" && expected == "Sales.Order"
        ));
    }

    #[test]
    fn data_grid_2_gallery_and_combo_box_compile_to_real_pluggable_widget_ids_and_round_trip() {
        let catalog = catalog();
        let mut decl = PageDecl::new("Dashboard");
        decl.layout = Some(LayoutRef::new("Atlas_Core.ApplicationLayout", "Main"));
        decl.widgets.push(WidgetDecl::DataGrid2 {
            name: Some("ordersGrid".into()),
            class: Some("orders-grid".into()),
        });
        decl.widgets.push(WidgetDecl::Gallery {
            name: None,
            class: None,
        });
        decl.widgets.push(WidgetDecl::ComboBox {
            name: None,
            class: None,
        });

        let document = compile_page(&catalog, &decl, None).unwrap();
        let codec = mxrs_forms::MprCodec::new(catalog);
        let node = codec.decode(&document).unwrap();
        let re_encoded = codec.encode(&node).unwrap();
        let re_decoded = codec.decode(&re_encoded).unwrap();
        assert_eq!(node, re_decoded);

        // `mxrs_bson::build_array` prepends a marker int at index 0 (see
        // that function) — real elements start at index 1.
        let layout_call = document.get_document("FormCall").unwrap();
        let arguments = layout_call.get_array("Arguments").unwrap();
        let argument = arguments[1].as_document().unwrap();
        let widgets = argument.get_array("Widgets").unwrap();
        let grid = widgets[1].as_document().unwrap();
        assert_eq!(grid.get_str("Name").unwrap(), "ordersGrid");
        assert_eq!(
            grid.get_document("Type")
                .unwrap()
                .get_str("WidgetId")
                .unwrap(),
            "com.mendix.widget.web.datagrid.Datagrid"
        );
        assert_eq!(
            widgets[2]
                .as_document()
                .unwrap()
                .get_document("Type")
                .unwrap()
                .get_str("WidgetId")
                .unwrap(),
            "com.mendix.widget.web.gallery.Gallery"
        );
        assert_eq!(
            widgets[3]
                .as_document()
                .unwrap()
                .get_document("Type")
                .unwrap()
                .get_str("WidgetId")
                .unwrap(),
            "com.mendix.widget.web.combobox.Combobox"
        );
    }

    #[test]
    fn an_installed_widget_package_supplies_the_real_schema_and_default_values() {
        use std::io::Write as _;

        let directory = tempfile::tempdir().unwrap();
        let widgets_directory = directory.path().join("widgets");
        std::fs::create_dir_all(&widgets_directory).unwrap();
        let file = std::fs::File::create(widgets_directory.join("DataGrid.mpk")).unwrap();
        let mut package = zip::ZipWriter::new(file);
        package
            .start_file("DataGrid.xml", zip::write::SimpleFileOptions::default())
            .unwrap();
        package
            .write_all(
                br#"<widget id="com.mendix.widget.web.datagrid.Datagrid" pluginWidget="true">
                    <name>Data grid 2</name><description/>
                    <properties><propertyGroup caption="General">
                        <property key="showPaging" type="boolean" defaultValue="true">
                            <caption>Show paging</caption><description/></property>
                        <property key="pageSize" type="integer" defaultValue="10">
                            <caption>Page size</caption><description/></property>
                        <property key="onClick" type="action" required="false">
                            <caption>On click</caption><description/></property>
                        <systemProperty key="Visibility"/>
                    </propertyGroup></properties></widget>"#,
            )
            .unwrap();
        package.finish().unwrap();

        let catalog = catalog();
        let mut decl = PageDecl::new("Dashboard");
        decl.layout = Some(LayoutRef::new("Atlas_Core.ApplicationLayout", "Main"));
        decl.widgets.push(WidgetDecl::DataGrid2 {
            name: Some("ordersGrid".into()),
            class: None,
        });
        let document = compile_page(&catalog, &decl, Some(directory.path())).unwrap();

        // The full round trip through the typed codec still holds with a
        // real (non-empty) schema and default-valued object.
        let codec = mxrs_forms::MprCodec::new(catalog);
        let node = codec.decode(&document).unwrap();
        let re_decoded = codec.decode(&codec.encode(&node).unwrap()).unwrap();
        assert_eq!(node, re_decoded);

        let layout_call = document.get_document("FormCall").unwrap();
        let arguments = layout_call.get_array("Arguments").unwrap();
        let widgets = arguments[1]
            .as_document()
            .unwrap()
            .get_array("Widgets")
            .unwrap();
        let grid = widgets[1].as_document().unwrap();
        assert_eq!(grid.get_str("Name").unwrap(), "ordersGrid");
        let widget_type = grid.get_document("Type").unwrap();
        assert_eq!(widget_type.get_str("WidgetName").unwrap(), "Data grid 2");
        let property_types = widget_type
            .get_document("ObjectType")
            .unwrap()
            .get_array("PropertyTypes")
            .unwrap();
        // Marker at index 0; three real properties plus the system slot.
        assert_eq!(property_types.len(), 5);

        // The object assigns package defaults to every non-system property.
        let properties = grid
            .get_document("Object")
            .unwrap()
            .get_array("Properties")
            .unwrap();
        assert_eq!(properties.len(), 4);
        let value_of = |index: usize| {
            properties[index]
                .as_document()
                .unwrap()
                .get_document("Value")
                .unwrap()
        };
        assert_eq!(value_of(1).get_str("PrimitiveValue").unwrap(), "true");
        assert_eq!(value_of(2).get_str("PrimitiveValue").unwrap(), "10");
        assert_eq!(
            value_of(3)
                .get_document("Action")
                .unwrap()
                .get_str("$Type")
                .unwrap(),
            "Forms$NoAction"
        );
    }
}
