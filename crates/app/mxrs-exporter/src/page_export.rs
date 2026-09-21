//! Detects pages built entirely from the widget vocabulary
//! `mxrs-dsl`/`mxrs-writer` can author (see `mxrs_ir::page`'s doc comment)
//! and renders them as real, compiled `mxrs-dsl` source: a standalone
//! `pub fn <page>() -> ::mxrs_ir::page::PageDecl` per detected page, plus
//! the `src/presentation/mod.rs` wiring (see `lib.rs::render`) that pushes each
//! one into its module and returns it from `build()`.
//!
//! **Now wired into the live `build()`/`.mpr` output**, same as domain
//! entities. Two things had to be true first, both now closed:
//!
//! 1. **`layout` had to be reliably recoverable.** `mxrs_model::page::Page`
//!    now exposes `layout_id` *and* `layout_parameter` (the
//!    `LayoutCallArgument.Parameter` by-name reference), read together by
//!    `mxrs_model::page::extract_layout`/`extract_layout_parameter` off the
//!    real `LayoutCall`-based storage shape (`doc.FormCall.Form` /
//!    `doc.FormCall.Arguments[0].Parameter` — see those functions' doc
//!    comments for the storage-naming evidence). A page whose widgets exist
//!    but whose layout isn't resolvable is still treated as unconvertible
//!    (opaque), rather than emitting a `PageDecl` `compile_page` would
//!    reject at write time (`WriterError::PageWidgetsRequireLayout`).
//! 2. **`LayoutGrid` decode had to be lossless.** `mxrs_model::page::
//!    layout_grid_widget` now nests each column's actual widgets (via
//!    synthetic `"layout_grid_row"`/`"layout_grid_column"` wrapper
//!    `Widget`s) instead of a `"widget_count"` summary, so this module can
//!    walk back to `WidgetDecl::LayoutGrid`.
//!
//! **Still narrower than a full page**: no conditional visibility/dynamic
//! classes or non-object page parameters. A page using any of those stays exactly
//! as opaque as before — served from the generated snapshot, contributing
//! nothing to `build()`. Static page appearance, security roles, popup
//! dimensions, exclusion, and export level are preserved by the typed path.
//!
//! Data views, attribute-bound widgets, flow-calling buttons and the three
//! built-in pluggable shells are also detected, but only at a strict
//! lossless boundary. Flow targets must resolve to generated markers;
//! data-view flows must return a known entity; attributes must exist on that
//! entity; call arguments and data-view footers remain opaque because their
//! authoring IR has no representation yet; and a pluggable widget converts
//! only when its schema/object configuration is the empty shell emitted by
//! `mxrs-writer`. A configured Studio Pro/marketplace widget therefore stays
//! snapshot-backed instead of being flattened to name/class.
//!
//! Detection is only a candidate: the existing writer recompiles it against
//! the source metamodel and every field present in the original BSON must
//! survive. This catches concepts the display-oriented widget summary erased
//! already (translations, editor metadata, source settings, extra layout
//! arguments, unknown containers). Generated element identities alone may
//! differ; the only rebinding recognized is a pluggable object's internal
//! schema pointer, verified against its original structural target.

use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;
use std::rc::Rc;

use mxrs_bson::{Bson, Document};
use mxrs_ir::page::{
    ButtonAction, DataSourceDecl, LayoutGridColumnDecl, LayoutGridRowDecl, LayoutRef, PageDecl,
    PageParameterDecl, WidgetDecl,
};
use mxrs_model::Module;
use mxrs_model::page::{Page, Widget};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PageExportReport {
    /// Pages built entirely from the detected widget vocabulary — wired
    /// directly into `build()` via `src/presentation/pages/mod.rs`.
    pub typed_candidates: usize,
    /// Every other page (pluggable widgets, data binding, conditional
    /// visibility, security roles, unresolvable layout, ...) — stays
    /// exactly as opaque as before this pass, served from the generated
    /// snapshot.
    pub opaque: usize,
}

/// One page detected as buildable from `mxrs-dsl`'s native/structural
/// widget vocabulary, ready to be rendered both as a standalone function
/// (`pages.rs`) and as a `src/domain/mod.rs` wiring statement that pushes
/// it into `module_name`'s `ModuleDecl.pages`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConvertedPage {
    pub module_name: String,
    pub function_name: String,
    pub decl: PageDecl,
    source_type: String,
    flow_return_entities: HashMap<String, String>,
}

impl ConvertedPage {
    pub(crate) fn source_type(&self) -> &str {
        &self.source_type
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FlowKind {
    Microflow,
    Nanoflow,
}

#[derive(Debug, Clone)]
struct FlowInfo {
    kind: FlowKind,
    return_entity: Option<String>,
}

#[derive(Debug, Default)]
struct ConversionContext {
    flows: HashMap<String, FlowInfo>,
    entity_attributes: HashMap<String, HashSet<String>>,
}

impl ConversionContext {
    fn from_modules(modules: &[Module]) -> Self {
        let mut context = Self::default();
        for module in modules {
            let module_name = module.name.as_deref().unwrap_or("Unnamed");
            for entity in module.entities() {
                let Some(entity_name) = entity.name.as_deref() else {
                    continue;
                };
                context.entity_attributes.insert(
                    format!("{module_name}.{entity_name}"),
                    entity
                        .attributes
                        .iter()
                        .filter_map(|attribute| attribute.name.clone())
                        .collect(),
                );
            }
            for (kind, flows) in [
                (FlowKind::Microflow, module.microflows.as_slice()),
                (FlowKind::Nanoflow, module.nanoflows.as_slice()),
            ] {
                for flow in flows {
                    let Some(name) = flow.name.as_deref() else {
                        continue;
                    };
                    let return_entity = flow
                        .return_type_document
                        .as_ref()
                        .and_then(|document| document.get_str("Entity").ok())
                        .map(str::to_string);
                    context.flows.insert(
                        format!("{module_name}.{name}"),
                        FlowInfo {
                            kind,
                            return_entity,
                        },
                    );
                }
            }
        }
        context
    }

    fn resolve_flow<'a>(
        &'a self,
        current_module: &str,
        name: &str,
        expected: FlowKind,
    ) -> Option<(String, &'a FlowInfo)> {
        let qualified = if name.contains('.') {
            name.to_string()
        } else {
            format!("{current_module}.{name}")
        };
        let info = self.flows.get(&qualified)?;
        (info.kind == expected).then_some((qualified, info))
    }
}

#[cfg(test)]
fn convert_pages(modules: &[Module]) -> (Vec<ConvertedPage>, PageExportReport) {
    convert_pages_for_version(modules, "11.12.1")
}

/// The same verified candidates drive both the Rust page functions and live
/// build wiring. Imports additionally check references from opaque units via
/// `protect_referenced_page_elements` before rendering either source file.
pub fn convert_pages_for_version(
    modules: &[Module],
    mendix_version: &str,
) -> (Vec<ConvertedPage>, PageExportReport) {
    let context = ConversionContext::from_modules(modules);
    let catalog = mxrs_forms::Catalog::for_version(mendix_version)
        .ok()
        .map(Rc::new);
    let mut report = PageExportReport::default();
    let mut pages = Vec::new();
    for module in modules {
        let module_name = module.name.clone().unwrap_or_else(|| "Unnamed".into());
        for page in &module.pages {
            match try_convert_page(page, &context, &module_name) {
                Some(decl)
                    if catalog
                        .as_ref()
                        .is_some_and(|catalog| reproduces_source(page, &decl, catalog)) =>
                {
                    report.typed_candidates += 1;
                    pages.push(ConvertedPage {
                        module_name: module_name.clone(),
                        function_name: to_snake_case(&decl.name),
                        source_type: page
                            .raw_document()
                            .get_str("$Type")
                            .unwrap_or("Forms$Page")
                            .to_string(),
                        flow_return_entities: context
                            .flows
                            .iter()
                            .filter_map(|(name, info)| {
                                info.return_entity
                                    .as_ref()
                                    .map(|entity| (name.clone(), entity.clone()))
                            })
                            .collect(),
                        decl,
                    });
                }
                _ => report.opaque += 1,
            }
        }
    }
    assign_page_function_names(&mut pages);
    (pages, report)
}

fn assign_page_function_names(pages: &mut [ConvertedPage]) {
    let mut order: Vec<_> = (0..pages.len()).collect();
    order.sort_by(|&left, &right| {
        (&pages[left].module_name, &pages[left].decl.name)
            .cmp(&(&pages[right].module_name, &pages[right].decl.name))
    });
    let mut used = HashSet::new();
    for index in order {
        let page = &mut pages[index];
        let mut base = to_snake_case(&page.decl.name);
        if !mxrs_typegen::is_rust_identifier(&base) {
            base.insert_str(0, "page_");
        }
        let mut name = base.clone();
        let mut suffix = 2;
        while !used.insert(name.clone()) {
            name = format!("{base}_{suffix}");
            suffix += 1;
        }
        page.function_name = name;
    }
}

fn reproduces_source(page: &Page, decl: &PageDecl, catalog: &Rc<mxrs_forms::Catalog>) -> bool {
    if page.raw_document().get_str("$Type").ok() != Some("Forms$Page")
        || !page.raw_document().contains_key("$ID")
    {
        return false;
    }
    // `None`: reproduction is judged against the fallback empty-schema
    // shape this exporter's own `PageDecl`s compile to; a widget carrying a
    // real package schema will simply stay opaque (lossless), same as today.
    mxrs_writer::page_compiler::compile_page(catalog, decl, None).is_ok_and(|compiled| {
        let mut identities = HashMap::new();
        collect_identities(page.raw_document(), &compiled, &mut identities)
            && preserves_document(page.raw_document(), &compiled, &identities)
    })
}

type IdentityMap = HashMap<String, (String, String)>;

fn collect_identities(
    original: &Document,
    compiled: &Document,
    identities: &mut IdentityMap,
) -> bool {
    if let Some(id) = original.get("$ID") {
        let Some(original_id) = mxrs_bson::extract_id(id) else {
            return false;
        };
        let Some(compiled_id) = compiled.get("$ID").and_then(mxrs_bson::extract_id) else {
            return false;
        };
        let Ok(kind) = original.get_str("$Type") else {
            return false;
        };
        if mxrs_bson::uuid_to_blob(&original_id).is_err()
            || compiled.get_str("$Type").ok() != Some(kind)
            || identities
                .insert(original_id, (compiled_id, kind.to_string()))
                .is_some()
        {
            return false;
        }
    }
    fn nested(original: &Bson, compiled: &Bson, identities: &mut IdentityMap) -> bool {
        match (original, compiled) {
            (Bson::Document(a), Bson::Document(b)) => collect_identities(a, b, identities),
            (Bson::Array(a), Bson::Array(b)) => {
                a.len() == b.len() && a.iter().zip(b).all(|(a, b)| nested(a, b, identities))
            }
            _ => true,
        }
    }
    original.iter().all(|(key, value)| match compiled.get(key) {
        Some(other) => nested(value, other, identities),
        _ => true,
    })
}

/// Extra compiler fields are metamodel defaults. Original fields, including
/// unknown or apparently inert ones, are never inferred to be dispensable.
fn preserves_document(original: &Document, compiled: &Document, identities: &IdentityMap) -> bool {
    original
        .keys()
        .eq(compiled.keys().filter(|key| original.contains_key(*key)))
        && original.iter().all(|(key, value)| {
            if key == "$ID" {
                return true;
            }
            if key == "TypePointer"
                && original.get_str("$Type").ok() == Some("CustomWidgets$WidgetObject")
            {
                return mxrs_bson::extract_id(value)
                    .and_then(|id| identities.get(&id))
                    .is_some_and(|(target, kind)| {
                        kind == "CustomWidgets$WidgetObjectType"
                            && compiled.get(key).and_then(mxrs_bson::extract_id).as_ref()
                                == Some(target)
                    });
            }
            compiled
                .get(key)
                .is_some_and(|other| preserves_value(value, other, identities))
        })
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct PageOwner {
    module: String,
    page: String,
    root_id: Option<String>,
}

type ElementOwners = HashMap<String, Vec<PageOwner>>;

/// Synchronizing a typed page regenerates child identities. An otherwise
/// reproducible page must stay snapshot-backed when another unit points at
/// those identities; the writer currently preserves only its root identity.
pub(crate) fn protect_referenced_page_elements(
    project: &mxrs_model::Project,
    modules: &[Module],
    pages: &mut Vec<ConvertedPage>,
    report: &mut PageExportReport,
) -> mxrs_model::Result<()> {
    let candidates: HashSet<_> = pages
        .iter()
        .map(|page| (page.module_name.as_str(), page.decl.name.as_str()))
        .collect();
    let mut owners = ElementOwners::new();
    for module in modules {
        for page in &module.pages {
            let Some(module_name) = &module.name else {
                continue;
            };
            let Some(page_name) = &page.name else {
                continue;
            };
            if candidates.contains(&(module_name.as_str(), page_name.as_str())) {
                let owner = PageOwner {
                    module: module_name.clone(),
                    page: page_name.clone(),
                    root_id: page.id.clone(),
                };
                visit_document_values(page.raw_document(), &mut |key, value, _| {
                    if key == "$ID"
                        && let Some(id) = mxrs_bson::extract_id(value)
                        && Some(&id) != owner.root_id.as_ref()
                    {
                        owners.entry(id).or_default().push(owner.clone());
                    }
                });
            }
        }
    }
    drop(candidates);
    if owners.is_empty() {
        return Ok(());
    }
    let mut protected = HashSet::new();
    for unit in project.all_units()? {
        let document = project.mpr().parse_contents(&unit)?;
        referenced_page_elements(&document, &owners, &mut protected);
    }
    let before = pages.len();
    let protected_names: HashSet<_> = protected
        .iter()
        .map(|owner| (owner.module.as_str(), owner.page.as_str()))
        .collect();
    pages.retain(|page| {
        !protected_names.contains(&(page.module_name.as_str(), page.decl.name.as_str()))
    });
    let removed = before - pages.len();
    report.typed_candidates -= removed;
    report.opaque += removed;
    Ok(())
}

fn referenced_page_elements(
    document: &Document,
    owners: &ElementOwners,
    protected: &mut HashSet<PageOwner>,
) {
    let root_id = document.get("$ID").and_then(mxrs_bson::extract_id);
    visit_document_values(document, &mut |key, value, parent| {
        if key != "$ID"
            && let Some(id) = mxrs_bson::extract_id(value)
            && let Some(targets) = owners.get(&id)
        {
            for owner in targets {
                let internal_schema_pointer = owner.root_id == root_id
                    && key == "TypePointer"
                    && parent.get_str("$Type").ok() == Some("CustomWidgets$WidgetObject");
                if !internal_schema_pointer {
                    protected.insert(owner.clone());
                }
            }
        }
    });
}

fn visit_document_values(document: &Document, visit: &mut impl FnMut(&str, &Bson, &Document)) {
    fn descend(
        key: &str,
        value: &Bson,
        parent: &Document,
        visit: &mut impl FnMut(&str, &Bson, &Document),
    ) {
        visit(key, value, parent);
        match value {
            Bson::Document(document) => visit_document_values(document, visit),
            Bson::Array(values) => {
                for value in values {
                    descend(key, value, parent, visit);
                }
            }
            _ => {}
        }
    }
    for (key, value) in document {
        descend(key, value, document, visit);
    }
}

fn preserves_value(original: &Bson, compiled: &Bson, identities: &IdentityMap) -> bool {
    match (original, compiled) {
        (Bson::Document(original), Bson::Document(compiled)) => {
            preserves_document(original, compiled, identities)
        }
        (Bson::Array(original), Bson::Array(compiled)) => {
            original.len() == compiled.len()
                && original
                    .iter()
                    .zip(compiled)
                    .all(|(a, b)| preserves_value(a, b, identities))
        }
        _ => original == compiled,
    }
}

/// Renders every converted page into one `src/presentation/pages/mod.rs` source file
/// of real, compiled `pub fn` page-builders. Returns `None` when there is
/// nothing to show (no point writing an empty file).
pub fn render_pages_module(pages: &[ConvertedPage]) -> Option<String> {
    if pages.is_empty() {
        return None;
    }

    let mut out = String::new();
    let _ = writeln!(
        out,
        "// Generated by `mxrs-exporter`. Each function below reproduces one page"
    );
    let _ = writeln!(
        out,
        "// detected as buildable from mxrs-dsl's native/structural widget"
    );
    let _ = writeln!(
        out,
        "// vocabulary (see mxrs_ir::page's doc comment) and is wired into"
    );
    let _ = writeln!(
        out,
        "// `build()` by src/presentation/mod.rs — edit freely, same as the rest of"
    );
    let _ = writeln!(out, "// this crate's generated source.");
    out.push('\n');
    for page in pages {
        out.push_str(&render_page_function(page));
        out.push('\n');
    }
    Some(out)
}

fn try_convert_page(
    page: &Page,
    context: &ConversionContext,
    current_module: &str,
) -> Option<PageDecl> {
    let name = page.name.clone()?;
    let parameters = convert_page_parameters(&page.parameters, context)?;
    let page_parameters = parameters
        .iter()
        .map(|parameter| (parameter.name.clone(), parameter.entity.clone()))
        .collect::<HashMap<_, _>>();
    let widgets = page
        .widgets
        .iter()
        .map(|widget| try_convert_widget(widget, context, current_module, &page_parameters, None))
        .collect::<Option<Vec<_>>>()?;

    let layout = match (&page.layout_id, &page.layout_parameter) {
        (Some(qualified_name), Some(parameter)) => {
            Some(LayoutRef::new(qualified_name.clone(), parameter.clone()))
        }
        _ => None,
    };
    if layout.is_none() && !widgets.is_empty() {
        // `compile_page` requires a layout whenever a page has widgets
        // (`WriterError::PageWidgetsRequireLayout`) — a page whose layout
        // isn't resolvable can never round-trip through this IR, so treat
        // it as opaque rather than emit a `PageDecl` that would fail at
        // write time.
        return None;
    }

    let mut decl = PageDecl::new(name);
    decl.documentation = page.documentation.clone();
    decl.url = page.url.clone();
    // Imported pages always make the decoded title explicit. `None` means
    // "default to page name" on the authoring side, which is not equivalent
    // to an explicitly empty title in an existing model.
    decl.title = Some(page.title.clone());
    decl.class = (!page.appearance_class.is_empty()).then(|| page.appearance_class.clone());
    decl.style = (!page.appearance_style.is_empty()).then(|| page.appearance_style.clone());
    decl.allowed_module_roles = page.allowed_module_roles.clone();
    decl.popup_width = page.popup_width;
    decl.popup_height = page.popup_height;
    decl.popup_resizable = page.popup_resizable;
    decl.excluded = page.excluded;
    decl.export_level = page.export_level.clone();
    decl.parameters = parameters;
    decl.layout = layout;
    decl.widgets = widgets;
    Some(decl)
}

fn try_convert_widget(
    widget: &Widget,
    context: &ConversionContext,
    current_module: &str,
    page_parameters: &HashMap<String, String>,
    data_view_entity: Option<&str>,
) -> Option<WidgetDecl> {
    match widget.widget_type.as_str() {
        "container" => {
            if !widget.events.is_empty() || !only_keys(&widget.options, &["class", "style"]) {
                return None;
            }
            let children = widget
                .children
                .iter()
                .map(|child| {
                    try_convert_widget(
                        child,
                        context,
                        current_module,
                        page_parameters,
                        data_view_entity,
                    )
                })
                .collect::<Option<Vec<_>>>()?;
            Some(WidgetDecl::Container {
                name: widget.name.clone(),
                class: string_option(&widget.options, "class"),
                style: string_option(&widget.options, "style"),
                children,
            })
        }
        "text" => {
            if !widget.events.is_empty() || !only_keys(&widget.options, &["class", "caption"]) {
                return None;
            }
            let caption = string_option(&widget.options, "caption")?;
            Some(WidgetDecl::Text {
                name: widget.name.clone(),
                caption,
                class: string_option(&widget.options, "class"),
            })
        }
        "button" => {
            if !only_keys(&widget.options, &["class", "caption"]) {
                return None;
            }
            let action = match widget.events.as_slice() {
                [] => ButtonAction::None,
                // `close_page`, `save_changes` and `cancel_changes` share a
                // shape: a targetless `kind: "action"` event. Recognizing all
                // three keeps a page that merely commits its data view on the
                // typed path instead of falling through to the lossless
                // snapshot below.
                [event]
                    if event.get_str("kind").ok() == Some("action")
                        && event.get_str("event").ok() == Some("on_click")
                        && only_keys(event, &["kind", "handler", "event"])
                        && matches!(
                            event.get_str("handler").ok(),
                            Some("close_page" | "save_changes" | "cancel_changes")
                        ) =>
                {
                    match event.get_str("handler").ok()? {
                        "close_page" => ButtonAction::ClosePage,
                        "save_changes" => ButtonAction::SaveChanges,
                        _ => ButtonAction::CancelChanges,
                    }
                }
                [event]
                    if event.get_str("event").ok() == Some("on_click")
                        && event
                            .get_document("arguments")
                            .is_ok_and(Document::is_empty)
                        && matches!(event.get_str("kind").ok(), Some("microflow" | "nanoflow"))
                        && only_keys(event, &["kind", "handler", "arguments", "event"]) =>
                {
                    let kind = match event.get_str("kind").ok()? {
                        "microflow" => FlowKind::Microflow,
                        "nanoflow" => FlowKind::Nanoflow,
                        _ => return None,
                    };
                    let handler = event.get_str("handler").ok()?;
                    let (qualified, _) = context.resolve_flow(current_module, handler, kind)?;
                    match kind {
                        FlowKind::Microflow => ButtonAction::CallMicroflow(qualified),
                        FlowKind::Nanoflow => ButtonAction::CallNanoflow(qualified),
                    }
                }
                _ => return None,
            };
            Some(WidgetDecl::Button {
                name: widget.name.clone(),
                caption: string_option(&widget.options, "caption").unwrap_or_default(),
                class: string_option(&widget.options, "class"),
                action,
            })
        }
        "layout_grid" => try_convert_layout_grid(
            widget,
            context,
            current_module,
            page_parameters,
            data_view_entity,
        ),
        "data_view" => try_convert_data_view(widget, context, current_module, page_parameters),
        "text_box" | "check_box" | "date_picker" | "drop_down" => {
            try_convert_attribute_widget(widget, context, data_view_entity)
        }
        "pluggable" => try_convert_pluggable_widget(widget),
        _ => None,
    }
}

fn try_convert_data_view(
    widget: &Widget,
    context: &ConversionContext,
    current_module: &str,
    page_parameters: &HashMap<String, String>,
) -> Option<WidgetDecl> {
    if !widget.events.is_empty()
        || !only_keys(
            &widget.options,
            &[
                "source",
                "show_footer",
                "body_widget_count",
                "footer_widget_count",
            ],
        )
        || widget.options.get_bool("show_footer").ok() != Some(true)
        || widget.options.get_i32("footer_widget_count").ok() != Some(0)
        || widget.options.get_i32("body_widget_count").ok()? as usize != widget.children.len()
    {
        return None;
    }
    let source = widget.options.get_document("source").ok()?;
    let (source_decl, entity) = match source.get_str("kind").ok()? {
        "microflow" | "nanoflow" => {
            if !only_keys(source, &["kind", "name"]) {
                return None;
            }
            let kind = if source.get_str("kind").ok()? == "microflow" {
                FlowKind::Microflow
            } else {
                FlowKind::Nanoflow
            };
            let (qualified, flow) =
                context.resolve_flow(current_module, source.get_str("name").ok()?, kind)?;
            let entity = flow.return_entity.as_deref()?;
            let source = match kind {
                FlowKind::Microflow => DataSourceDecl::Microflow(qualified),
                FlowKind::Nanoflow => DataSourceDecl::Nanoflow(qualified),
            };
            (source, entity)
        }
        "context" => {
            if !only_keys(source, &["kind", "parameter"]) {
                return None;
            }
            let parameter = source.get_str("parameter").ok()?;
            let entity = page_parameters.get(parameter)?;
            (
                DataSourceDecl::Context {
                    parameter: parameter.to_string(),
                    entity: entity.clone(),
                },
                entity.as_str(),
            )
        }
        _ => return None,
    };
    if !context.entity_attributes.contains_key(entity) {
        return None;
    }
    let children = widget
        .children
        .iter()
        .map(|child| {
            try_convert_widget(
                child,
                context,
                current_module,
                page_parameters,
                Some(entity),
            )
        })
        .collect::<Option<Vec<_>>>()?;
    Some(WidgetDecl::DataView {
        name: widget.name.clone(),
        source: source_decl,
        children,
    })
}

fn convert_page_parameters(
    parameters: &[Document],
    context: &ConversionContext,
) -> Option<Vec<PageParameterDecl>> {
    let mut names = HashSet::new();
    parameters
        .iter()
        .map(|parameter| {
            if !only_keys(
                parameter,
                &[
                    "$ID",
                    "$Type",
                    "Name",
                    "ParameterType",
                    "IsRequired",
                    "DefaultValue",
                ],
            ) {
                return None;
            }
            if parameter.get_str("$Type").ok() != Some("Forms$PageParameter") {
                return None;
            }
            let name = parameter.get_str("Name").ok()?;
            if !valid_mendix_name(name) || !names.insert(name) {
                return None;
            }
            let parameter_type = parameter.get_document("ParameterType").ok()?;
            if !only_keys(parameter_type, &["$ID", "$Type", "Entity"])
                || parameter_type.get_str("$Type").ok()? != "DataTypes$ObjectType"
            {
                return None;
            }
            let entity = parameter_type.get_str("Entity").ok()?;
            let (module, entity_name) = entity.split_once('.')?;
            if !valid_mendix_name(module)
                || !valid_mendix_name(entity_name)
                || !context.entity_attributes.contains_key(entity)
            {
                return None;
            }
            let default_value = match parameter.get("DefaultValue") {
                None => None,
                Some(mxrs_bson::Bson::String(value)) => Some(value.clone()),
                _ => return None,
            };
            Some(PageParameterDecl {
                name: name.to_string(),
                entity: entity.to_string(),
                required: match parameter.get("IsRequired") {
                    None => true,
                    Some(Bson::Boolean(value)) => *value,
                    _ => return None,
                },
                default_value,
            })
        })
        .collect()
}

fn valid_mendix_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

fn try_convert_attribute_widget(
    widget: &Widget,
    context: &ConversionContext,
    data_view_entity: Option<&str>,
) -> Option<WidgetDecl> {
    if !widget.events.is_empty()
        || !widget.children.is_empty()
        || !only_keys(&widget.options, &["attribute", "class"])
    {
        return None;
    }
    let entity = data_view_entity?;
    let raw_attribute = widget.options.get_str("attribute").ok()?;
    let attribute = validated_attribute_name(raw_attribute, entity)?;
    if !context.entity_attributes.get(entity)?.contains(attribute) {
        return None;
    }
    let fields = (
        widget.name.clone(),
        attribute.to_string(),
        string_option(&widget.options, "class"),
    );
    Some(match widget.widget_type.as_str() {
        "text_box" => WidgetDecl::TextBox {
            name: fields.0,
            attribute: fields.1,
            class: fields.2,
        },
        "check_box" => WidgetDecl::CheckBox {
            name: fields.0,
            attribute: fields.1,
            class: fields.2,
        },
        "date_picker" => WidgetDecl::DatePicker {
            name: fields.0,
            attribute: fields.1,
            class: fields.2,
        },
        "drop_down" => WidgetDecl::DropDown {
            name: fields.0,
            attribute: fields.1,
            class: fields.2,
        },
        _ => return None,
    })
}

fn validated_attribute_name<'a>(raw: &'a str, entity: &str) -> Option<&'a str> {
    let Some((prefix, attribute)) = raw.rsplit_once(['/', '.']) else {
        return (!raw.is_empty()).then_some(raw);
    };
    let entity_name = entity.rsplit('.').next()?;
    (prefix == entity || prefix == entity_name)
        .then_some(attribute)
        .filter(|name| !name.is_empty())
}

fn try_convert_pluggable_widget(widget: &Widget) -> Option<WidgetDecl> {
    if !widget.events.is_empty()
        || !widget.children.is_empty()
        || !only_keys(
            &widget.options,
            &["native_type", "widget_id", "configuration_empty", "class"],
        )
        || widget.options.get_str("native_type").ok() != Some("CustomWidgets$CustomWidget")
        || widget.options.get_bool("configuration_empty").ok() != Some(true)
    {
        return None;
    }
    let name = widget.name.clone();
    let class = string_option(&widget.options, "class");
    Some(match widget.options.get_str("widget_id").ok()? {
        "com.mendix.widget.web.datagrid.Datagrid" => WidgetDecl::DataGrid2 { name, class },
        "com.mendix.widget.web.gallery.Gallery" => WidgetDecl::Gallery { name, class },
        "com.mendix.widget.web.combobox.Combobox" => WidgetDecl::ComboBox { name, class },
        _ => return None,
    })
}

/// Walks the `"layout_grid_row"`/`"layout_grid_column"` wrapper tree
/// `mxrs_model::page::layout_grid_widget` now nests real children under
/// (see that function's doc comment) back into `WidgetDecl::LayoutGrid`.
/// Only desktop-weight columns with no tablet/phone override convert —
/// `WidgetDecl::LayoutGrid` has no per-breakpoint weight concept yet (see
/// `mxrs_ir::page`), so a column that actually uses one stays opaque rather
/// than silently dropping it.
fn try_convert_layout_grid(
    widget: &Widget,
    context: &ConversionContext,
    current_module: &str,
    page_parameters: &HashMap<String, String>,
    data_view_entity: Option<&str>,
) -> Option<WidgetDecl> {
    if !widget.events.is_empty() || !only_keys(&widget.options, &[]) {
        return None;
    }
    let rows = widget
        .children
        .iter()
        .map(|row| {
            try_convert_layout_grid_row(
                row,
                context,
                current_module,
                page_parameters,
                data_view_entity,
            )
        })
        .collect::<Option<Vec<_>>>()?;
    Some(WidgetDecl::LayoutGrid {
        name: widget.name.clone(),
        rows,
    })
}

fn try_convert_layout_grid_row(
    row: &Widget,
    context: &ConversionContext,
    current_module: &str,
    page_parameters: &HashMap<String, String>,
    data_view_entity: Option<&str>,
) -> Option<LayoutGridRowDecl> {
    if row.widget_type != "layout_grid_row" || !row.events.is_empty() {
        return None;
    }
    let columns = row
        .children
        .iter()
        .map(|column| {
            try_convert_layout_grid_column(
                column,
                context,
                current_module,
                page_parameters,
                data_view_entity,
            )
        })
        .collect::<Option<Vec<_>>>()?;
    Some(LayoutGridRowDecl { columns })
}

fn try_convert_layout_grid_column(
    column: &Widget,
    context: &ConversionContext,
    current_module: &str,
    page_parameters: &HashMap<String, String>,
    data_view_entity: Option<&str>,
) -> Option<LayoutGridColumnDecl> {
    if column.widget_type != "layout_grid_column" || !column.events.is_empty() {
        return None;
    }
    let weight = column.options.get_i32("desktop").ok()?;
    let is_uniform_breakpoint = |key: &str| {
        column
            .options
            .get_i32(key)
            .ok()
            .is_some_and(|value| value == weight)
    };
    if !is_uniform_breakpoint("tablet") || !is_uniform_breakpoint("phone") {
        return None;
    }
    let children = column
        .children
        .iter()
        .map(|child| {
            try_convert_widget(
                child,
                context,
                current_module,
                page_parameters,
                data_view_entity,
            )
        })
        .collect::<Option<Vec<_>>>()?;
    Some(LayoutGridColumnDecl { weight, children })
}

fn only_keys(document: &Document, allowed: &[&str]) -> bool {
    document.keys().all(|key| allowed.contains(&key.as_str()))
}

fn string_option(document: &Document, key: &str) -> Option<String> {
    document.get_str(key).ok().map(str::to_string)
}

fn render_page_function(page: &ConvertedPage) -> String {
    let decl = &page.decl;
    let mut out = String::new();
    let _ = writeln!(
        out,
        "/// {:?}",
        format!("{}.{}", page.module_name, decl.name)
    );
    let _ = writeln!(
        out,
        "pub fn {}() -> ::mxrs_ir::page::PageDecl {{",
        page.function_name
    );
    let _ = writeln!(
        out,
        "    let mut p = ::mxrs_dsl::PageBuilder::new({:?});",
        decl.name
    );
    if !decl.documentation.is_empty() {
        let _ = writeln!(out, "    p.documentation({:?});", decl.documentation);
    }
    if !decl.url.is_empty() {
        let _ = writeln!(out, "    p.url({:?});", decl.url);
    }
    if let Some(title) = &decl.title {
        let _ = writeln!(out, "    p.title({title:?});");
    }
    if let Some(class) = &decl.class {
        let _ = writeln!(out, "    p.class({class:?});");
    }
    if let Some(style) = &decl.style {
        let _ = writeln!(out, "    p.style({style:?});");
    }
    for role in &decl.allowed_module_roles {
        let _ = writeln!(out, "    p.allow_role({role:?});");
    }
    if decl.popup_width != 0 || decl.popup_height != 0 || decl.popup_resizable {
        let _ = writeln!(
            out,
            "    p.popup({}, {}, {});",
            decl.popup_width, decl.popup_height, decl.popup_resizable
        );
    }
    if decl.excluded {
        let _ = writeln!(out, "    p.excluded(true);");
    }
    if decl.export_level != "Hidden" {
        let _ = writeln!(out, "    p.export_level({:?});", decl.export_level);
    }
    for parameter in &decl.parameters {
        let marker = entity_marker_path(&parameter.entity);
        if let Some(default_value) = &parameter.default_value {
            let _ = writeln!(
                out,
                "    p.object_parameter_with_default::<{marker}>({:?}, {}, {default_value:?});",
                parameter.name, parameter.required
            );
        } else {
            let _ = writeln!(
                out,
                "    p.object_parameter::<{marker}>({:?}, {});",
                parameter.name, parameter.required
            );
        }
    }
    if let Some(layout) = &decl.layout {
        let _ = writeln!(
            out,
            "    p.layout({:?}, {:?});",
            layout.qualified_name, layout.parameter
        );
    }
    for widget in &decl.widgets {
        out.push_str(&render_widget(
            widget,
            1,
            "p",
            None,
            &page.flow_return_entities,
        ));
    }
    let _ = writeln!(out, "    p.into_decl()");
    let _ = writeln!(out, "}}");
    out
}

fn render_widget(
    widget: &WidgetDecl,
    indent: usize,
    receiver: &str,
    data_view_entity: Option<&str>,
    flow_return_entities: &HashMap<String, String>,
) -> String {
    let pad = "    ".repeat(indent);
    let mut out = String::new();
    match widget {
        WidgetDecl::ApplicationShell { title, navigation } => {
            let profile = navigation
                .as_ref()
                .map_or_else(|| "None".into(), |profile| format!("Some({profile:?})"));
            let _ = writeln!(
                out,
                "{pad}{receiver}.application_shell({title:?}, {profile});"
            );
        }
        WidgetDecl::LayoutPlaceholder { name } => {
            let _ = writeln!(out, "{pad}{receiver}.placeholder({name:?});");
        }
        WidgetDecl::Container {
            name,
            class,
            style,
            children,
        } => {
            let _ = writeln!(out, "{pad}{receiver}.container(|w| {{");
            if let Some(name) = name {
                let _ = writeln!(out, "{pad}    w.name({name:?});");
            }
            if let Some(class) = class {
                let _ = writeln!(out, "{pad}    w.class({class:?});");
            }
            if let Some(style) = style {
                let _ = writeln!(out, "{pad}    w.style({style:?});");
            }
            for child in children {
                out.push_str(&render_widget(
                    child,
                    indent + 1,
                    "w",
                    data_view_entity,
                    flow_return_entities,
                ));
            }
            let _ = writeln!(out, "{pad}}});");
        }
        WidgetDecl::LayoutGrid { name, rows } => {
            let _ = writeln!(out, "{pad}{receiver}.layout_grid(|grid| {{");
            if let Some(name) = name {
                let _ = writeln!(out, "{pad}    grid.name({name:?});");
            }
            for row in rows {
                let _ = writeln!(out, "{pad}    grid.row(|row| {{");
                for column in &row.columns {
                    let _ = writeln!(out, "{pad}        row.column({}, |col| {{", column.weight);
                    for child in &column.children {
                        out.push_str(&render_widget(
                            child,
                            indent + 3,
                            "col",
                            data_view_entity,
                            flow_return_entities,
                        ));
                    }
                    let _ = writeln!(out, "{pad}        }});");
                }
                let _ = writeln!(out, "{pad}    }});");
            }
            let _ = writeln!(out, "{pad}}});");
        }
        WidgetDecl::Text {
            name,
            caption,
            class,
        } => {
            if name.is_none() && class.is_none() {
                let _ = writeln!(out, "{pad}{receiver}.text({caption:?});");
            } else {
                let _ = writeln!(out, "{pad}{receiver}.text_with({caption:?}, |w| {{");
                render_name_and_class(&mut out, indent + 1, "w", name, class);
                let _ = writeln!(out, "{pad}}});");
            }
        }
        WidgetDecl::Button {
            name,
            caption,
            class,
            action,
        } => {
            let _ = writeln!(out, "{pad}{receiver}.button({caption:?}, |b| {{");
            render_name_and_class(&mut out, indent + 1, "b", name, class);
            match action {
                ButtonAction::ClosePage => {
                    let _ = writeln!(out, "{pad}    b.close_page();");
                }
                ButtonAction::None => {}
                ButtonAction::CallMicroflow(target) => {
                    let marker = flow_marker_path(target);
                    let _ = writeln!(
                        out,
                        "{pad}    b.call_microflow(::mxrs_ir::markers::MicroflowRef::<{marker}>::new());"
                    );
                }
                ButtonAction::CallNanoflow(target) => {
                    let marker = flow_marker_path(target);
                    let _ = writeln!(
                        out,
                        "{pad}    b.call_nanoflow(::mxrs_ir::markers::NanoflowRef::<{marker}>::new());"
                    );
                }
                ButtonAction::SaveChanges => {
                    let _ = writeln!(out, "{pad}    b.save_changes();");
                }
                ButtonAction::CancelChanges => {
                    let _ = writeln!(out, "{pad}    b.cancel_changes();");
                }
            }
            let _ = writeln!(out, "{pad}}});");
        }
        WidgetDecl::DataView {
            name,
            source,
            children,
        } => {
            let (method, reference, target) = match source {
                DataSourceDecl::Microflow(target) => {
                    ("data_view_from_microflow", "MicroflowRef", target.as_str())
                }
                DataSourceDecl::Nanoflow(target) => {
                    ("data_view_from_nanoflow", "NanoflowRef", target.as_str())
                }
                DataSourceDecl::Context { parameter, entity } => {
                    let marker = entity_marker_path(entity);
                    let _ = writeln!(
                        out,
                        "{pad}{receiver}.data_view_from_context::<{marker}>({parameter:?}, |w| {{"
                    );
                    if let Some(name) = name {
                        let _ = writeln!(out, "{pad}    w.name({name:?});");
                    }
                    for child in children {
                        out.push_str(&render_widget(
                            child,
                            indent + 1,
                            "w",
                            Some(entity),
                            flow_return_entities,
                        ));
                    }
                    let _ = writeln!(out, "{pad}}});");
                    return out;
                }
            };
            let marker = flow_marker_path(target);
            let entity = flow_return_entities
                .get(target)
                .expect("converted data-view flow has a validated return entity");
            let _ = writeln!(
                out,
                "{pad}{receiver}.{method}(::mxrs_ir::markers::{reference}::<{marker}>::new(), |w| {{"
            );
            if let Some(name) = name {
                let _ = writeln!(out, "{pad}    w.name({name:?});");
            }
            for child in children {
                out.push_str(&render_widget(
                    child,
                    indent + 1,
                    "w",
                    Some(entity),
                    flow_return_entities,
                ));
            }
            let _ = writeln!(out, "{pad}}});");
        }
        WidgetDecl::TextBox {
            name,
            attribute,
            class,
        }
        | WidgetDecl::CheckBox {
            name,
            attribute,
            class,
        }
        | WidgetDecl::DatePicker {
            name,
            attribute,
            class,
        }
        | WidgetDecl::DropDown {
            name,
            attribute,
            class,
        } => {
            let method = match widget {
                WidgetDecl::TextBox { .. } => "text_box_with",
                WidgetDecl::CheckBox { .. } => "check_box_with",
                WidgetDecl::DatePicker { .. } => "date_picker_with",
                WidgetDecl::DropDown { .. } => "drop_down_with",
                _ => unreachable!(),
            };
            let marker = attribute_marker_path(
                data_view_entity.expect("converted attribute widget is inside a data view"),
                attribute,
            );
            let _ = writeln!(out, "{pad}{receiver}.{method}::<{marker}>(|w| {{");
            render_name_and_class(&mut out, indent + 1, "w", name, class);
            let _ = writeln!(out, "{pad}}});");
        }
        WidgetDecl::DataGrid2 { name, class }
        | WidgetDecl::Gallery { name, class }
        | WidgetDecl::ComboBox { name, class } => {
            let method = match widget {
                WidgetDecl::DataGrid2 { .. } => "data_grid_2",
                WidgetDecl::Gallery { .. } => "gallery",
                WidgetDecl::ComboBox { .. } => "combo_box",
                _ => unreachable!(),
            };
            let _ = writeln!(out, "{pad}{receiver}.{method}(|w| {{");
            render_name_and_class(&mut out, indent + 1, "w", name, class);
            let _ = writeln!(out, "{pad}}});");
        }
    }
    out
}

fn render_name_and_class(
    out: &mut String,
    indent: usize,
    receiver: &str,
    name: &Option<String>,
    class: &Option<String>,
) {
    let pad = "    ".repeat(indent);
    if let Some(name) = name {
        let _ = writeln!(out, "{pad}{receiver}.name({name:?});");
    }
    if let Some(class) = class {
        let _ = writeln!(out, "{pad}{receiver}.class({class:?});");
    }
}

fn flow_marker_path(qualified_name: &str) -> String {
    let (module, name) = qualified_name
        .split_once('.')
        .expect("converted flow reference is qualified");
    format!("crate::infrastructure::markers::{module}::{name}")
}

fn attribute_marker_path(entity: &str, attribute: &str) -> String {
    let (module, entity) = entity
        .split_once('.')
        .expect("converted entity reference is qualified");
    format!("crate::infrastructure::markers::{module}::{entity}_{attribute}")
}

fn entity_marker_path(entity: &str) -> String {
    let (module, entity) = entity
        .split_once('.')
        .expect("converted entity reference is qualified");
    format!("crate::infrastructure::markers::{module}::{entity}")
}

/// `OrderOverview` -> `order_overview`. A defensive, not exhaustive, name
/// sanitizer mirroring `mxrs-exporter`'s own `sanitize_ident` for the
/// domain-model renderer: non-alphanumeric characters become `_`.
fn to_snake_case(name: &str) -> String {
    let mut out = String::new();
    for (index, ch) in name.chars().enumerate() {
        if ch.is_ascii_uppercase() {
            if index != 0 {
                out.push('_');
            }
            out.extend(ch.to_lowercase());
        } else if ch.is_ascii_alphanumeric() || ch == '_' {
            out.push(ch);
        } else {
            out.push('_');
        }
    }
    if out.is_empty() || out.chars().next().is_some_and(|c| c.is_ascii_digit()) {
        out.insert(0, '_');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use mxrs_bson::Bson;

    fn compiled_page(name: &str) -> Page {
        let mut builder = mxrs_dsl::PageBuilder::new(name);
        builder.layout("Atlas_Core.ApplicationLayout", "Main");
        builder.container(|container| {
            container.text("Hello");
        });
        let catalog = Rc::new(mxrs_forms::Catalog::for_version("11.12.1").unwrap());
        Page::from_bson(
            &mxrs_writer::page_compiler::compile_page(&catalog, &builder.into_decl(), None)
                .unwrap(),
        )
    }

    fn module_with_pages(pages: Vec<Page>) -> Module {
        Module {
            id: "m".into(),
            name: Some("Sales".into()),
            sort_index: None,
            from_app_store: false,
            app_store_guid: None,
            app_store_version: None,
            export_level: "Hidden".into(),
            domain_model: None,
            pages,
            microflows: vec![],
            nanoflows: vec![],
            rules: vec![],
            menus: vec![],
            module_roles: vec![],
            artifact_units: vec![],
        }
    }

    fn first_widget(document: &mut Document) -> &mut Document {
        document
            .get_document_mut("FormCall")
            .unwrap()
            .get_array_mut("Arguments")
            .unwrap()[1]
            .as_document_mut()
            .unwrap()
            .get_array_mut("Widgets")
            .unwrap()[1]
            .as_document_mut()
            .unwrap()
    }

    #[test]
    fn native_fields_erased_by_the_display_summary_keep_the_entire_page_opaque() {
        let original = compiled_page("Home");
        let mut variants = Vec::new();
        for (key, value) in [
            ("MarkAsUsed", Bson::Boolean(true)),
            ("CanvasWidth", Bson::Int32(1024)),
            (
                "PopupCloseAction",
                Bson::Document(mxrs_bson::doc! { "$Type": "Forms$ClosePageClientAction" }),
            ),
            (
                "Variables",
                Bson::Array(vec![Bson::Int32(3), Bson::String("editor state".into())]),
            ),
        ] {
            let mut document = original.raw_document().clone();
            document.insert(key, value);
            variants.push(document);
        }
        let mut document = original.raw_document().clone();
        first_widget(&mut document).insert(
            "ConditionalVisibilitySettings",
            mxrs_bson::doc! { "Condition": "$Order/Active" },
        );
        variants.push(document);
        let mut document = original.raw_document().clone();
        first_widget(&mut document).insert("$Type", "Forms$UnknownContainer");
        variants.push(document);
        let mut document = original.raw_document().clone();
        document
            .get_document_mut("FormCall")
            .unwrap()
            .get_array_mut("Arguments")
            .unwrap()
            .push(Bson::Document(
                mxrs_bson::doc! { "Parameter": "Sidebar", "Widgets": [3] },
            ));
        variants.push(document);
        let mut document = original.raw_document().clone();
        document.get_document_mut("Title").unwrap().get_array_mut("Items").unwrap()
            .push(Bson::Document(mxrs_bson::doc! { "$Type": "Texts$Translation", "LanguageCode": "pt_BR", "Text": "Início" }));
        variants.push(document);
        let mut document = original.raw_document().clone();
        document.insert("PopupResizable", "false");
        variants.push(document);
        for document in variants {
            let page = Page::from_bson(&document);
            let (pages, report) = convert_pages(&[module_with_pages(vec![page])]);
            assert!(
                pages.is_empty(),
                "unsupported native data was projected: {document:?}"
            );
            assert_eq!(report.opaque, 1);
        }
        let (_, report) = convert_pages(&[module_with_pages(vec![original])]);
        assert_eq!(report.typed_candidates, 1);
    }

    #[test]
    fn data_source_settings_and_context_subpaths_are_not_silently_discarded() {
        let mut decl = PageDecl::new("OrderEdit");
        decl.layout = Some(LayoutRef::new("Atlas_Core.ApplicationLayout", "Main"));
        decl.parameters.push(PageParameterDecl {
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
            children: vec![],
        });
        let catalog = Rc::new(mxrs_forms::Catalog::for_version("11.12.1").unwrap());
        let original = mxrs_writer::page_compiler::compile_page(&catalog, &decl, None).unwrap();
        assert!(reproduces_source(
            &Page::from_bson(&original),
            &decl,
            &catalog
        ));
        let mut modified = original.clone();
        first_widget(&mut modified)
            .get_document_mut("DataSource")
            .unwrap()
            .get_document_mut("SourceVariable")
            .unwrap()
            .insert("SubKey", "Sales.Order_Customer");
        let page = Page::from_bson(&modified);
        let candidate = try_convert_page(&page, &bound_context(), "Sales").unwrap();
        assert!(!reproduces_source(&page, &candidate, &catalog));
        let mut modified = original;
        first_widget(&mut modified)
            .get_document_mut("DataSource")
            .unwrap()
            .insert("ApplyEntityAccess", true);
        let page = Page::from_bson(&modified);
        let candidate = try_convert_page(&page, &bound_context(), "Sales").unwrap();
        assert!(!reproduces_source(&page, &candidate, &catalog));
    }

    #[test]
    fn malformed_missing_and_duplicate_object_parameters_never_reach_marker_rendering() {
        let valid = mxrs_bson::doc! { "$Type": "Forms$PageParameter", "Name": "Order", "ParameterType": { "$Type": "DataTypes$ObjectType", "Entity": "Sales.Order" }, "IsRequired": true };
        let context = bound_context();
        assert!(convert_page_parameters(std::slice::from_ref(&valid), &context).is_some());
        for entity in [
            "Order",
            "",
            "Sales.",
            ".Order",
            "Sales.Order.Extra",
            "Sales.Missing",
            "Sales.bad-name",
        ] {
            let mut parameter = valid.clone();
            parameter
                .get_document_mut("ParameterType")
                .unwrap()
                .insert("Entity", entity);
            assert!(
                convert_page_parameters(&[parameter], &context).is_none(),
                "{entity}"
            );
        }
        for name in ["", "bad name", "1Order"] {
            let mut parameter = valid.clone();
            parameter.insert("Name", name);
            assert!(convert_page_parameters(&[parameter], &context).is_none());
        }
        for (key, value) in [
            ("$Type", Bson::String("Forms$UnknownParameter".into())),
            ("IsRequired", Bson::String("true".into())),
            ("DefaultValue", Bson::Boolean(false)),
        ] {
            let mut parameter = valid.clone();
            parameter.insert(key, value);
            assert!(convert_page_parameters(&[parameter], &context).is_none());
        }
        assert!(convert_page_parameters(&[valid.clone(), valid.clone()], &context).is_none());
        let mut parameter = valid;
        parameter.remove("IsRequired");
        assert!(convert_page_parameters(&[parameter], &context).unwrap()[0].required);
    }

    #[test]
    fn internal_schema_pointers_must_rebind_to_the_same_element_and_unknown_pointers_do_not() {
        let schema_id = uuid::Uuid::new_v4().to_string();
        let root_id = uuid::Uuid::new_v4().to_string();
        let target_id = uuid::Uuid::new_v4().to_string();
        let original =
            mxrs_bson::doc! { "$Type": "CustomWidgets$WidgetObject", "TypePointer": &schema_id };
        let compiled =
            mxrs_bson::doc! { "$Type": "CustomWidgets$WidgetObject", "TypePointer": &target_id };
        let identities = HashMap::from([(
            schema_id.clone(),
            (target_id.clone(), "CustomWidgets$WidgetObjectType".into()),
        )]);
        assert!(preserves_document(&original, &compiled, &identities));
        assert!(!preserves_document(&original, &compiled, &HashMap::new()));
        let identities = HashMap::from([(schema_id.clone(), (target_id, "Forms$Text".into()))]);
        assert!(!preserves_document(&original, &compiled, &identities));
        let owner = PageOwner {
            module: "Sales".into(),
            page: "Home".into(),
            root_id: Some(root_id.clone()),
        };
        let owners = HashMap::from([(schema_id.clone(), vec![owner.clone()])]);
        let own_document = mxrs_bson::doc! { "$ID": root_id, "Object": original.clone() };
        let mut protected = HashSet::new();
        referenced_page_elements(&own_document, &owners, &mut protected);
        assert!(protected.is_empty());
        referenced_page_elements(
            &mxrs_bson::doc! { "OtherReference": [schema_id] },
            &owners,
            &mut protected,
        );
        assert!(protected.contains(&owner));
    }

    #[test]
    fn unsupported_metamodel_versions_and_invalid_or_duplicated_element_ids_stay_opaque() {
        let page = compiled_page("Home");
        let (_, report) =
            convert_pages_for_version(&[module_with_pages(vec![page.clone()])], "9.0.0");
        assert_eq!(report.opaque, 1);
        for id in [
            Bson::Boolean(false),
            Bson::String("not-a-uuid".into()),
            page.raw_document().get("$ID").unwrap().clone(),
        ] {
            let mut document = page.raw_document().clone();
            first_widget(&mut document).insert("$ID", id);
            assert_eq!(
                convert_pages(&[module_with_pages(vec![Page::from_bson(&document)])])
                    .1
                    .opaque,
                1
            );
        }
    }

    fn container(children: Vec<Widget>) -> Widget {
        Widget {
            widget_type: "container".into(),
            name: Some("row".into()),
            options: mxrs_bson::doc! { "class": "row" },
            events: vec![],
            children,
        }
    }

    fn text(caption: &str) -> Widget {
        Widget {
            widget_type: "text".into(),
            name: Some("hint".into()),
            options: mxrs_bson::doc! { "caption": caption },
            events: vec![],
            children: vec![],
        }
    }

    fn close_button(caption: &str) -> Widget {
        Widget {
            widget_type: "button".into(),
            name: Some("close".into()),
            options: mxrs_bson::doc! { "caption": caption },
            events: vec![
                mxrs_bson::doc! { "kind": "action", "handler": "close_page", "event": "on_click" },
            ],
            children: vec![],
        }
    }

    fn layout_grid_column(weight: i32, children: Vec<Widget>) -> Widget {
        Widget {
            widget_type: "layout_grid_column".into(),
            name: None,
            options: mxrs_bson::doc! { "desktop": weight, "tablet": weight, "phone": weight },
            events: vec![],
            children,
        }
    }

    fn layout_grid_row(columns: Vec<Widget>) -> Widget {
        Widget {
            widget_type: "layout_grid_row".into(),
            name: None,
            options: Document::new(),
            events: vec![],
            children: columns,
        }
    }

    fn layout_grid(rows: Vec<Widget>) -> Widget {
        Widget {
            widget_type: "layout_grid".into(),
            name: Some("grid1".into()),
            options: Document::new(),
            events: vec![],
            children: rows,
        }
    }

    fn bare_page(name: &str, widgets: Vec<Widget>) -> Page {
        page_with_layout(
            name,
            widgets,
            Some(("Atlas_Core.ApplicationLayout", "Main")),
        )
    }

    fn convert(page: &Page) -> Option<PageDecl> {
        try_convert_page(page, &ConversionContext::default(), "Sales")
    }

    fn page_with_layout(name: &str, widgets: Vec<Widget>, layout: Option<(&str, &str)>) -> Page {
        let mut doc = mxrs_bson::doc! {
            "$ID": uuid::Uuid::new_v4().to_string(),
            "$Type": "Forms$Page",
            "Name": name,
        };
        if let Some((qualified_name, parameter)) = layout {
            doc.insert(
                "FormCall",
                mxrs_bson::doc! {
                    "Form": qualified_name,
                    "Arguments": mxrs_bson::build_array(
                        vec![mxrs_bson::Bson::Document(mxrs_bson::doc! {
                            "Parameter": parameter,
                            "Widgets": mxrs_bson::build_array(
                                widgets.into_iter().map(widget_to_bson).collect(),
                                3,
                            ),
                        })],
                        3,
                    ),
                },
            );
        } else {
            doc.insert(
                "Widgets",
                mxrs_bson::build_array(widgets.into_iter().map(widget_to_bson).collect(), 3),
            );
        }
        Page::from_bson(&doc)
    }

    fn page_from_documents(name: &str, widgets: Vec<Document>) -> Page {
        Page::from_bson(&mxrs_bson::doc! {
            "$Type": "Forms$Page",
            "Name": name,
            "FormCall": {
                "Form": "Atlas_Core.ApplicationLayout",
                "Arguments": mxrs_bson::build_array(vec![Bson::Document(mxrs_bson::doc! {
                    "Parameter": "Main",
                    "Widgets": mxrs_bson::build_array(
                        widgets.into_iter().map(Bson::Document).collect(),
                        3,
                    ),
                })], 3),
            },
        })
    }

    fn bound_context() -> ConversionContext {
        ConversionContext {
            flows: HashMap::from([
                (
                    "Sales.ACT_GetOrder".into(),
                    FlowInfo {
                        kind: FlowKind::Microflow,
                        return_entity: Some("Sales.Order".into()),
                    },
                ),
                (
                    "Sales.NF_Validate".into(),
                    FlowInfo {
                        kind: FlowKind::Nanoflow,
                        return_entity: None,
                    },
                ),
            ]),
            entity_attributes: HashMap::from([(
                "Sales.Order".into(),
                HashSet::from(["Number".into()]),
            )]),
        }
    }

    fn widget_to_bson(widget: Widget) -> mxrs_bson::Bson {
        match widget.widget_type.as_str() {
            "container" => mxrs_bson::Bson::Document(mxrs_bson::doc! {
                "$ID": uuid::Uuid::new_v4().to_string(),
                "$Type": "Forms$DivContainer",
                "Name": widget.name.unwrap_or_default(),
                "Class": widget.options.get_str("class").unwrap_or("").to_string(),
                "Widgets": mxrs_bson::build_array(
                    widget.children.into_iter().map(widget_to_bson).collect(),
                    3,
                ),
            }),
            "text" => {
                let mut doc = mxrs_bson::doc! {
                    "$ID": uuid::Uuid::new_v4().to_string(),
                    "$Type": "Forms$DynamicText",
                    "Name": widget.name.unwrap_or_default(),
                    "Content": widget.options.get_str("caption").unwrap_or("").to_string(),
                };
                if let Ok(expression) = widget.options.get_str("visible") {
                    doc.insert(
                        "ConditionalVisibilitySettings",
                        mxrs_bson::doc! {
                            "$ID": uuid::Uuid::new_v4().to_string(),
                            "$Type": "Forms$ConditionalVisibilitySettings",
                            "Expression": expression.to_string(),
                        },
                    );
                }
                mxrs_bson::Bson::Document(doc)
            }
            "button" => mxrs_bson::Bson::Document(mxrs_bson::doc! {
                "$ID": uuid::Uuid::new_v4().to_string(),
                "$Type": "Forms$ActionButton",
                "Name": widget.name.unwrap_or_default(),
                "Caption": widget.options.get_str("caption").unwrap_or("").to_string(),
                "Action": { "$Type": "Forms$ClosePageClientAction" },
            }),
            "layout_grid" => mxrs_bson::Bson::Document(mxrs_bson::doc! {
                "$ID": uuid::Uuid::new_v4().to_string(),
                "$Type": "Forms$LayoutGrid",
                "Name": widget.name.unwrap_or_default(),
                "Rows": mxrs_bson::build_array(
                    widget.children.into_iter().map(row_to_bson).collect(),
                    3,
                ),
            }),
            other => panic!("unsupported test widget type {other}"),
        }
    }

    fn row_to_bson(row: Widget) -> mxrs_bson::Bson {
        mxrs_bson::Bson::Document(mxrs_bson::doc! {
            "Columns": mxrs_bson::build_array(
                row.children.into_iter().map(column_to_bson).collect(),
                3,
            ),
        })
    }

    fn column_to_bson(column: Widget) -> mxrs_bson::Bson {
        mxrs_bson::Bson::Document(mxrs_bson::doc! {
            "Weight": column.options.get_i32("desktop").unwrap_or(-1),
            "TabletWeight": column.options.get_i32("tablet").unwrap_or(-1),
            "PhoneWeight": column.options.get_i32("phone").unwrap_or(-1),
            "Widgets": mxrs_bson::build_array(
                column.children.into_iter().map(widget_to_bson).collect(),
                3,
            ),
        })
    }

    #[test]
    fn a_page_built_from_container_text_and_close_button_converts() {
        let page = bare_page(
            "OrderOverview",
            vec![container(vec![
                text("Manage your orders"),
                close_button("Close"),
            ])],
        );
        let decl = convert(&page).expect("should convert");
        assert_eq!(decl.name, "OrderOverview");
        assert_eq!(decl.widgets.len(), 1);
        let layout = decl.layout.expect("layout recovered from FormCall");
        assert_eq!(layout.qualified_name, "Atlas_Core.ApplicationLayout");
        assert_eq!(layout.parameter, "Main");
    }

    #[test]
    fn a_page_with_widgets_but_no_resolvable_layout_is_left_opaque() {
        let page = page_with_layout("Broken", vec![text("hi")], None);
        assert!(convert(&page).is_none());
    }

    #[test]
    fn a_layout_grid_with_uniform_weighted_columns_converts_losslessly() {
        let page = bare_page(
            "Dashboard",
            vec![layout_grid(vec![layout_grid_row(vec![
                layout_grid_column(1, vec![text("Left")]),
                layout_grid_column(2, vec![text("Right")]),
            ])])],
        );
        let decl = convert(&page).expect("should convert");
        let WidgetDecl::LayoutGrid { rows, .. } = &decl.widgets[0] else {
            panic!("expected a layout grid");
        };
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].columns.len(), 2);
        assert_eq!(rows[0].columns[0].weight, 1);
        assert!(matches!(
            &rows[0].columns[0].children[0],
            WidgetDecl::Text { caption, .. } if caption == "Left"
        ));
        assert_eq!(rows[0].columns[1].weight, 2);
    }

    #[test]
    fn a_layout_grid_column_with_a_breakpoint_override_is_left_opaque() {
        let mut column = layout_grid_column(1, vec![text("Left")]);
        column.options.insert("tablet", -2);
        let page = bare_page(
            "Dashboard",
            vec![layout_grid(vec![layout_grid_row(vec![column])])],
        );
        assert!(convert(&page).is_none());
    }

    #[test]
    fn page_metadata_converts_and_renders_instead_of_forcing_opaque_fallback() {
        let mut page = bare_page("OrderEdit", vec![text("Edit")]);
        page.appearance_class = "page-order".into();
        page.appearance_style = "max-width: 80rem".into();
        page.allowed_module_roles = vec!["Sales.Editor".into()];
        page.popup_width = 720;
        page.popup_height = 480;
        page.popup_resizable = true;
        page.excluded = true;
        page.export_level = "API".into();

        let decl = convert(&page).expect("page metadata is typed");
        let source = render_page_function(&ConvertedPage {
            module_name: "Sales".into(),
            function_name: "order_edit".into(),
            source_type: "Forms$Page".into(),
            decl,
            flow_return_entities: HashMap::new(),
        });
        assert!(source.contains("p.class(\"page-order\")"));
        assert!(source.contains("p.style(\"max-width: 80rem\")"));
        assert!(source.contains("p.allow_role(\"Sales.Editor\")"));
        assert!(source.contains("p.popup(720, 480, true)"));
        assert!(source.contains("p.excluded(true)"));
        assert!(source.contains("p.export_level(\"API\")"));
    }

    #[test]
    fn save_and_cancel_buttons_stay_typed_instead_of_falling_back_to_the_snapshot() {
        // Built from raw documents rather than `bare_page`: that helper's
        // `widget_to_bson` hardcodes `ClosePageClientAction` for every button,
        // so it cannot express the actions under test.
        fn action_button(name: &str, caption: &str, action_type: &str) -> Document {
            mxrs_bson::doc! {
                "$ID": uuid::Uuid::new_v4().to_string(),
                "$Type": "Forms$ActionButton",
                "Name": name,
                "Caption": caption,
                "Action": { "$Type": action_type },
            }
        }
        let page = page_from_documents(
            "OrderForm",
            vec![
                action_button("save", "Save", "Forms$SaveChangesClientAction"),
                action_button("cancel", "Cancel", "Forms$CancelChangesClientAction"),
            ],
        );

        let decl = convert(&page).expect("targetless client actions are typed");
        let source = render_page_function(&ConvertedPage {
            module_name: "Sales".into(),
            function_name: "order_form".into(),
            source_type: "Forms$Page".into(),
            decl,
            flow_return_entities: HashMap::new(),
        });
        assert!(source.contains("b.save_changes();"), "{source}");
        assert!(source.contains("b.cancel_changes();"), "{source}");

        // An action this IR has no variant for must still go opaque rather
        // than being quietly rendered as one of the two above.
        let unsupported = page_from_documents(
            "OrderList",
            vec![action_button("del", "Delete", "Forms$DeleteClientAction")],
        );
        assert!(convert(&unsupported).is_none());
    }

    #[test]
    fn convert_pages_counts_typed_and_opaque_pages_separately() {
        let convertible = compiled_page("Simple");
        let unsupported = bare_page(
            "WithVisibility",
            vec![Widget {
                widget_type: "text".into(),
                name: Some("hint".into()),
                options: mxrs_bson::doc! { "caption": "hi", "visible": "$currentObject/Active" },
                events: vec![],
                children: vec![],
            }],
        );
        let module = Module {
            id: "m".into(),
            name: Some("Sales".into()),
            sort_index: None,
            from_app_store: false,
            app_store_guid: None,
            app_store_version: None,
            export_level: "Hidden".into(),
            domain_model: None,
            pages: vec![convertible, unsupported],
            microflows: vec![],
            nanoflows: vec![],
            rules: vec![],
            menus: vec![],
            module_roles: vec![],
            artifact_units: vec![],
        };

        let (pages, report) = convert_pages(std::slice::from_ref(&module));
        assert_eq!(report.typed_candidates, 1);
        assert_eq!(report.opaque, 1);
        assert_eq!(pages.len(), 1);
        assert_eq!(pages[0].module_name, "Sales");
        assert_eq!(pages[0].function_name, "simple");

        let source = render_pages_module(&pages).expect("one convertible page should render");
        assert!(source.contains("pub fn simple"));
        assert!(!source.contains("with_visibility"));
    }

    #[test]
    fn a_page_with_a_data_bound_text_widget_is_left_opaque() {
        let page = bare_page(
            "Bound",
            vec![Widget {
                widget_type: "text".into(),
                name: Some("hint".into()),
                options: mxrs_bson::doc! { "attribute": "Order/Number" },
                events: vec![],
                children: vec![],
            }],
        );
        assert!(convert(&page).is_none());
    }

    #[test]
    fn data_view_and_attribute_widget_convert_with_validated_markers() {
        let page = page_from_documents(
            "OrderDetail",
            vec![mxrs_bson::doc! {
                "$Type": "Forms$DataView",
                "Name": "orderView",
                "DataSource": { "$Type": "Forms$MicroflowSource", "Microflow": "Sales.ACT_GetOrder" },
                "Widgets": mxrs_bson::build_array(vec![Bson::Document(mxrs_bson::doc! {
                    "$Type": "Forms$TextBox",
                    "Name": "numberInput",
                    "AttributeRef": { "Attribute": "Number" },
                })], 3),
            }],
        );
        let context = bound_context();
        let decl = try_convert_page(&page, &context, "Sales").expect("validated page converts");
        let converted = ConvertedPage {
            module_name: "Sales".into(),
            function_name: "order_detail".into(),
            source_type: "Forms$Page".into(),
            flow_return_entities: HashMap::from([(
                "Sales.ACT_GetOrder".into(),
                "Sales.Order".into(),
            )]),
            decl,
        };
        let source = render_page_function(&converted);
        assert!(source.contains("data_view_from_microflow"));
        assert!(source.contains("markers::Sales::ACT_GetOrder"));
        assert!(
            source.contains("text_box_with::<crate::infrastructure::markers::Sales::Order_Number>")
        );
        assert!(source.contains("w.name(\"numberInput\")"));
    }

    #[test]
    fn object_parameter_and_context_data_view_convert_with_entity_markers() {
        let page = Page::from_bson(&mxrs_bson::doc! {
            "$Type": "Forms$Page",
            "Name": "OrderEdit",
            "Parameters": mxrs_bson::build_array(vec![Bson::Document(mxrs_bson::doc! {
                "$Type": "Forms$PageParameter",
                "Name": "Order",
                "ParameterType": { "$Type": "DataTypes$ObjectType", "Entity": "Sales.Order" },
                "IsRequired": true,
            })], 3),
            "FormCall": {
                "Form": "Atlas_Core.ApplicationLayout",
                "Arguments": mxrs_bson::build_array(vec![Bson::Document(mxrs_bson::doc! {
                    "Parameter": "Main",
                    "Widgets": mxrs_bson::build_array(vec![Bson::Document(mxrs_bson::doc! {
                        "$Type": "Forms$DataView",
                        "Name": "orderView",
                        "DataSource": {
                            "$Type": "Forms$DataViewSource",
                            "SourceVariable": { "PageParameter": "Order" },
                        },
                        "Widgets": mxrs_bson::build_array(vec![Bson::Document(mxrs_bson::doc! {
                            "$Type": "Forms$TextBox",
                            "Name": "numberInput",
                            "AttributeRef": { "Attribute": "Number" },
                        })], 3),
                    })], 3),
                })], 3),
            },
        });
        let decl = try_convert_page(&page, &bound_context(), "Sales")
            .expect("typed object parameter should make context resolvable");
        let source = render_page_function(&ConvertedPage {
            module_name: "Sales".into(),
            function_name: "order_edit".into(),
            source_type: "Forms$Page".into(),
            flow_return_entities: HashMap::new(),
            decl,
        });
        assert!(source.contains(
            "p.object_parameter::<crate::infrastructure::markers::Sales::Order>(\"Order\", true)"
        ));
        assert!(source.contains(
            "p.data_view_from_context::<crate::infrastructure::markers::Sales::Order>(\"Order\""
        ));
        assert!(
            source.contains("text_box_with::<crate::infrastructure::markers::Sales::Order_Number>")
        );
    }

    #[test]
    fn flow_button_converts_only_without_argument_mappings() {
        let page = page_from_documents(
            "Actions",
            vec![mxrs_bson::doc! {
                "$Type": "Forms$ActionButton",
                "Name": "validate",
                "Caption": "Validate",
                "Action": {
                    "$Type": "Forms$CallNanoflowClientAction",
                    "Nanoflow": "Sales.NF_Validate",
                    "NanoflowSettings": {
                        "ParameterMappings": mxrs_bson::build_array(vec![], 3),
                    },
                },
            }],
        );
        let decl = try_convert_page(&page, &bound_context(), "Sales").expect("button converts");
        assert!(matches!(
            &decl.widgets[0],
            WidgetDecl::Button { action: ButtonAction::CallNanoflow(name), .. }
                if name == "Sales.NF_Validate"
        ));
    }

    #[test]
    fn empty_official_pluggable_shell_converts_but_configured_one_does_not() {
        let shell = || {
            mxrs_bson::doc! {
                "$Type": "CustomWidgets$CustomWidget",
                "Name": "ordersGrid",
                "Type": {
                    "WidgetId": "com.mendix.widget.web.datagrid.Datagrid",
                    "ObjectType": { "PropertyTypes": mxrs_bson::build_array(vec![], 3) },
                },
                "Object": { "Properties": mxrs_bson::build_array(vec![], 3) },
            }
        };
        let page = page_from_documents("Grid", vec![shell()]);
        assert!(try_convert_page(&page, &bound_context(), "Sales").is_some());

        let mut configured = shell();
        configured.insert("Columns", mxrs_bson::build_array(vec![], 3));
        let page = page_from_documents("ConfiguredGrid", vec![configured]);
        assert!(try_convert_page(&page, &bound_context(), "Sales").is_none());
    }
}
