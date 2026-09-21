//! Native web layouts are real Forms documents, not an implicit dependency on
//! Atlas Core. Shape evidence: `mxrb/dsl/builder.rb#layout` (Forms$Layout ->
//! WebLayoutContent.Widgets -> Placeholder) and the embedded 11.12.1 metamodel.
//! Nested layout calls and native-mobile layouts remain outside this IR.

use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use mxrs_bson::{Bson, Document, extract_id};
use mxrs_forms::node::{Node, Value};
use mxrs_forms::{Catalog, MprCodec};
use mxrs_identity::{ArtifactKind, ProjectIdentity};
use mxrs_ir::{LayoutDecl, LayoutKind, WidgetDecl};
use mxrs_mpr::MprFile;

use crate::{Result, WriterError, page_compiler};

pub fn compile_layout(
    catalog: &Rc<Catalog>,
    decl: &LayoutDecl,
    packages_root: Option<&std::path::Path>,
) -> Result<Document> {
    if decl.canvas_width <= 0 || decl.canvas_height <= 0 {
        return Err(WriterError::InvalidLayoutCanvas(decl.name.clone()));
    }
    let placeholders = placeholder_names(decl)?;
    let mut layout = Node::new("Layout", catalog.clone())?;
    layout.set("name", Value::String(decl.name.clone()))?;
    layout.set("documentation", Value::String(decl.documentation.clone()))?;
    layout.set("excluded", Value::Boolean(decl.excluded))?;
    layout.set("exportLevel", Value::String(decl.export_level.clone()))?;
    layout.set("canvasWidth", Value::Integer(i64::from(decl.canvas_width)))?;
    layout.set(
        "canvasHeight",
        Value::Integer(i64::from(decl.canvas_height)),
    )?;
    if decl
        .widgets
        .iter()
        .any(|widget| matches!(widget, WidgetDecl::ApplicationShell { .. }))
    {
        layout.set(
            "appearance",
            Value::Node(page_compiler::shell_appearance(
                catalog,
                decl.class.as_deref(),
                decl.style.as_deref(),
            )?),
        )?;
    } else if let Some(appearance) =
        page_compiler::appearance_node(catalog, decl.class.as_deref(), decl.style.as_deref())?
    {
        layout.set("appearance", Value::Node(appearance))?;
    } else {
        layout.set(
            "appearance",
            Value::Node(Node::new("Appearance", catalog.clone())?),
        )?;
    }
    let parameters = placeholders
        .into_iter()
        .map(|name| {
            let mut parameter = Node::new("LayoutParameter", catalog.clone())?;
            parameter.set("name", Value::String(name))?;
            Ok(Value::Node(parameter))
        })
        .collect::<Result<Vec<_>>>()?;
    layout.set("parameters", Value::List(parameters))?;
    let mut content = Node::new("WebLayoutContent", catalog.clone())?;
    let kind = match decl.kind {
        LayoutKind::Responsive => "Responsive",
        LayoutKind::Tablet => "Tablet",
        LayoutKind::Phone => "Phone",
        LayoutKind::ModalPopup => "ModalPopup",
        LayoutKind::Popup => "Popup",
        LayoutKind::Legacy => "Legacy",
    };
    content.set("layoutType", Value::String(kind.into()))?;
    content.set("layoutCall", Value::Null)?;
    let mut counter = 0;
    content.set(
        "widgets",
        Value::List(
            decl.widgets
                .iter()
                .map(|widget| {
                    page_compiler::compile_widget(catalog, widget, &mut counter, packages_root)
                })
                .collect::<Result<Vec<_>>>()?,
        ),
    )?;
    layout.set("content", Value::Node(content))?;
    Ok(MprCodec::new(catalog.clone()).encode(&layout)?)
}

fn placeholder_names(decl: &LayoutDecl) -> Result<Vec<String>> {
    fn visit(widget: &WidgetDecl, names: &mut Vec<String>) {
        match widget {
            WidgetDecl::LayoutPlaceholder { name } => names.push(name.clone()),
            WidgetDecl::ApplicationShell { .. } => names.push("Main".into()),
            WidgetDecl::Container { children, .. } | WidgetDecl::DataView { children, .. } => {
                for child in children {
                    visit(child, names);
                }
            }
            WidgetDecl::LayoutGrid { rows, .. } => {
                for child in rows
                    .iter()
                    .flat_map(|row| &row.columns)
                    .flat_map(|column| &column.children)
                {
                    visit(child, names);
                }
            }
            _ => {}
        }
    }
    let mut names = Vec::new();
    for widget in &decl.widgets {
        visit(widget, &mut names);
    }
    let mut seen = HashSet::new();
    for name in &names {
        let mut chars = name.chars();
        if !chars
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
            || !chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
            || !seen.insert(name)
        {
            return Err(WriterError::InvalidLayoutPlaceholder {
                layout: decl.name.clone(),
                placeholder: name.clone(),
            });
        }
    }
    Ok(names)
}

pub(crate) fn synchronize_layouts_with_identity(
    mpr: &mut MprFile,
    module_id: &str,
    module_name: &str,
    mendix_version: &str,
    layouts: &[LayoutDecl],
    identity: ProjectIdentity,
) -> Result<()> {
    if layouts.is_empty() {
        return Ok(());
    }
    let catalog = Rc::new(Catalog::for_version(mendix_version)?);
    let packages_root = mpr.path().parent().map(std::path::Path::to_path_buf);
    let mut seen = HashSet::new();
    let compiled = layouts
        .iter()
        .map(|layout| {
            if !seen.insert(&layout.name) {
                return Err(WriterError::DuplicateLayout(layout.name.clone()));
            }
            Ok((
                layout,
                compile_layout(&catalog, layout, packages_root.as_deref())?,
            ))
        })
        .collect::<Result<Vec<_>>>()?;
    let mut existing = HashMap::new();
    let mut pending = vec![module_id.to_string()];
    let mut visited = HashSet::new();
    while let Some(container) = pending.pop() {
        if !visited.insert(container.clone()) {
            continue;
        }
        for unit in mpr.children_of(&container)? {
            if unit.containment_name == "Folders" {
                pending.push(unit.unit_id);
                continue;
            }
            if unit.containment_name != "Documents" {
                continue;
            }
            let document = mpr.parse_contents(&unit)?;
            if document.get_str("$Type").ok() == Some("Forms$Layout")
                && let Ok(name) = document.get_str("Name")
                && existing
                    .insert(name.to_string(), (unit.unit_id, document.clone()))
                    .is_some()
            {
                return Err(WriterError::DuplicateLayout(name.to_string()));
            }
        }
    }
    for (decl, mut document) in compiled {
        let previous = existing.get(&decl.name);
        let key = format!("{module_name}.{}", decl.name);
        let mut remapping = HashMap::new();
        stabilize_ids(
            &mut document,
            previous.map(|(_, document)| document),
            identity,
            &key,
            &mut remapping,
        );
        rewrite_schema_pointers(&mut document, &remapping);
        if let Some((id, _)) = previous {
            document.insert("$ID", id.clone());
            mpr.update_unit(id, document)?;
        } else {
            let id = identity.artifact_id(ArtifactKind::Layout, &key);
            document.insert("$ID", id.clone());
            mpr.insert_unit(module_id, "Documents", document, Some(&id))?;
        }
    }
    Ok(())
}

/// Named widgets/parameters retain identities even when siblings are reordered;
/// unnamed structural nodes retain them by position. New nodes get deterministic
/// path-scoped IDs; pluggable schema pointers are rebound after the complete walk.
fn stabilize_ids(
    document: &mut Document,
    previous: Option<&Document>,
    identity: ProjectIdentity,
    key: &str,
    remapping: &mut HashMap<String, String>,
) {
    let previous = previous
        .filter(|previous| previous.get_str("$Type").ok() == document.get_str("$Type").ok());
    if let Some(generated) = document.get("$ID").and_then(extract_id) {
        let id = previous
            .and_then(|previous| previous.get("$ID"))
            .and_then(extract_id)
            .unwrap_or_else(|| identity.artifact_id(ArtifactKind::Layout, key));
        remapping.insert(generated, id.clone());
        document.insert("$ID", id);
    }
    for (property, value) in document.iter_mut() {
        match value {
            Bson::Document(child) => stabilize_ids(
                child,
                previous.and_then(|p| p.get_document(property).ok()),
                identity,
                &format!("{key}/{property}"),
                remapping,
            ),
            Bson::Array(children) => {
                let prior = previous.and_then(|p| p.get_array(property).ok());
                for (index, value) in children.iter_mut().enumerate() {
                    if let Bson::Document(child) = value {
                        let name = child
                            .get_str("Name")
                            .ok()
                            .or_else(|| child.get_str("LanguageCode").ok());
                        let prior = if let Some(name) = name {
                            prior.and_then(|items| {
                                items.iter().filter_map(Bson::as_document).find(|item| {
                                    item.get_str("$Type").ok() == child.get_str("$Type").ok()
                                        && item
                                            .get_str("Name")
                                            .ok()
                                            .or_else(|| item.get_str("LanguageCode").ok())
                                            == Some(name)
                                })
                            })
                        } else {
                            prior
                                .and_then(|items| items.get(index))
                                .and_then(Bson::as_document)
                        };
                        let suffix = name.map_or_else(
                            || format!("index:{index}"),
                            |name| format!("name:{}:{name}", name.len()),
                        );
                        stabilize_ids(
                            child,
                            prior,
                            identity,
                            &format!("{key}/{property}/{suffix}"),
                            remapping,
                        );
                    }
                }
            }
            _ => {}
        }
    }
}

fn rewrite_schema_pointers(document: &mut Document, remapping: &HashMap<String, String>) {
    for (key, value) in document.iter_mut() {
        if key == "TypePointer"
            && let Some(id) = extract_id(value)
            && let Some(replacement) = remapping.get(&id)
        {
            *value = Bson::String(replacement.clone());
        } else {
            match value {
                Bson::Document(child) => rewrite_schema_pointers(child, remapping),
                Bson::Array(values) => {
                    for child in values.iter_mut().filter_map(Bson::as_document_mut) {
                        rewrite_schema_pointers(child, remapping);
                    }
                }
                _ => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mxrs_dsl::{LayoutBuilder, PageBuilder, ProjectBuilder};

    fn catalog() -> Rc<Catalog> {
        Rc::new(Catalog::for_version("11.12.1").unwrap())
    }

    fn layout() -> LayoutDecl {
        let mut layout = LayoutBuilder::new("ApplicationLayout");
        layout
            .documentation("Native application shell")
            .class("shell")
            .style("min-height: 100vh")
            .canvas(1000, 700)
            .excluded(false)
            .export_level("API");
        layout.text_with("Application", |text| {
            text.name("brand");
        });
        layout.container(|container| {
            container.name("content");
            container.placeholder("Main");
        });
        layout.layout_grid(|grid| {
            grid.row(|row| {
                row.column(12, |column| {
                    column.placeholder("Footer");
                });
            });
        });
        layout.into_decl()
    }

    fn document(mpr: &MprFile, kind: &str, name: &str) -> (String, Document) {
        mpr.all_units()
            .unwrap()
            .into_iter()
            .find_map(|unit| {
                let document = mpr.parse_contents(&unit).unwrap();
                (document.get_str("$Type").ok() == Some(kind)
                    && document.get_str("Name").ok() == Some(name))
                .then_some((unit.unit_id, document))
            })
            .unwrap()
    }

    fn named_ids(document: &Document) -> HashMap<String, String> {
        fn walk(document: &Document, target: &mut HashMap<String, String>) {
            if let Ok(name) = document.get_str("Name") {
                target.insert(
                    format!("{}:{name}", document.get_str("$Type").unwrap()),
                    document.get("$ID").and_then(extract_id).unwrap(),
                );
            }
            for (_, value) in document {
                match value {
                    Bson::Document(document) => walk(document, target),
                    Bson::Array(values) => {
                        for document in values.iter().filter_map(Bson::as_document) {
                            walk(document, target);
                        }
                    }
                    _ => {}
                }
            }
        }
        let mut target = HashMap::new();
        walk(document, &mut target);
        target
    }

    #[test]
    fn native_layouts_encode_metadata_widgets_and_named_parameters_through_the_real_forms_codec() {
        let catalog = catalog();
        let document = compile_layout(&catalog, &layout(), None).unwrap();
        assert_eq!(document.get_str("$Type").unwrap(), "Forms$Layout");
        assert_eq!(document.get_i64("CanvasWidth").unwrap(), 1000);
        assert_eq!(document.get_i64("CanvasHeight").unwrap(), 700);
        assert_eq!(
            document
                .get_document("Appearance")
                .unwrap()
                .get_str("Class")
                .unwrap(),
            "shell"
        );
        let content = document.get_document("Content").unwrap();
        assert_eq!(content.get_str("$Type").unwrap(), "Forms$WebLayoutContent");
        assert_eq!(content.get_str("LayoutType").unwrap(), "Responsive");
        let parameters: Vec<_> = document
            .get_array("Parameters")
            .unwrap()
            .iter()
            .filter_map(Bson::as_document)
            .map(|parameter| parameter.get_str("Name").unwrap())
            .collect();
        assert_eq!(parameters, ["Main", "Footer"]);
        let codec = MprCodec::new(catalog);
        assert!(codec.decode(&document).is_ok());
        for (kind, expected) in [
            (LayoutKind::Tablet, "Tablet"),
            (LayoutKind::Phone, "Phone"),
            (LayoutKind::ModalPopup, "ModalPopup"),
            (LayoutKind::Popup, "Popup"),
            (LayoutKind::Legacy, "Legacy"),
        ] {
            let mut decl = layout();
            decl.kind = kind;
            assert_eq!(
                compile_layout(&self::catalog(), &decl, None)
                    .unwrap()
                    .get_document("Content")
                    .unwrap()
                    .get_str("LayoutType")
                    .unwrap(),
                expected
            );
        }
        let mut shell = LayoutBuilder::new("Shell");
        shell
            .kind(LayoutKind::Responsive)
            .text("Welcome")
            .button("Close", |button| {
                button.close_page();
            });
        assert!(compile_layout(&self::catalog(), &shell.into_decl(), None).is_ok());
    }

    #[test]
    fn invalid_layouts_and_page_placeholder_usage_are_rejected_instead_of_dropping_widgets() {
        for name in ["", "1Main", "Main.Content", "Main content"] {
            let mut layout = LayoutBuilder::new("Invalid");
            layout.placeholder(name);
            assert!(matches!(
                compile_layout(&catalog(), &layout.into_decl(), None),
                Err(WriterError::InvalidLayoutPlaceholder { .. })
            ));
        }
        let mut duplicate = LayoutBuilder::new("Duplicate");
        duplicate.placeholder("Main").container(|container| {
            container.placeholder("Main");
        });
        assert!(matches!(
            compile_layout(&catalog(), &duplicate.into_decl(), None),
            Err(WriterError::InvalidLayoutPlaceholder { .. })
        ));
        for (width, height) in [(0, 600), (800, -1)] {
            let mut invalid = layout();
            invalid.canvas_width = width;
            invalid.canvas_height = height;
            assert!(matches!(
                compile_layout(&catalog(), &invalid, None),
                Err(WriterError::InvalidLayoutCanvas(_))
            ));
        }
        let mut page = PageBuilder::new("InvalidPage");
        page.layout("Main.ApplicationLayout", "Main");
        page.container(|container| {
            container.placeholder("Main");
        });
        assert!(matches!(
            page_compiler::compile_page(&catalog(), &page.into_decl(), None),
            Err(WriterError::PlaceholderOutsideLayout { .. })
        ));
    }

    #[test]
    fn page_layout_argument_references_are_qualified_and_foreign_parameter_prefixes_fail() {
        for parameter in ["Main", "Main.ApplicationLayout.Main"] {
            let mut page = PageBuilder::new("Home");
            page.layout("Main.ApplicationLayout", parameter)
                .text("Hello");
            let document =
                page_compiler::compile_page(&catalog(), &page.into_decl(), None).unwrap();
            let argument = document
                .get_document("FormCall")
                .unwrap()
                .get_array("Arguments")
                .unwrap()[1]
                .as_document()
                .unwrap();
            assert_eq!(
                argument.get_str("Parameter").unwrap(),
                "Main.ApplicationLayout.Main"
            );
        }
        for parameter in ["", "Other.Layout.Main", "Main.ApplicationLayout.Main.Extra"] {
            let mut page = PageBuilder::new("Invalid");
            page.layout("Main.ApplicationLayout", parameter);
            assert!(matches!(
                page_compiler::compile_page(&catalog(), &page.into_decl(), None),
                Err(WriterError::InvalidLayoutParameterReference { .. })
            ));
        }
    }

    #[test]
    fn layout_writes_are_deterministic_and_sync_preserves_named_ids_across_reordering_and_folders()
    {
        let first = tempfile::tempdir().unwrap();
        let second = tempfile::tempdir().unwrap();
        let first_path = first.path().join("Project.mpr");
        let second_path = second.path().join("Project.mpr");
        let mut builder = ProjectBuilder::new("11.12.1");
        builder.module("Main", |module| {
            module.layout("Opaque", |layout| {
                layout.placeholder("Body");
            });
        });
        let mut project = builder.build();
        project.modules[0].layouts.push(layout());
        crate::write_project(&first_path, &project).unwrap();
        crate::write_project(&second_path, &project).unwrap();
        let mut mpr = MprFile::open(&first_path, false).unwrap();
        let other = MprFile::open(&second_path, true).unwrap();
        let (id, original) = document(&mpr, "Forms$Layout", "ApplicationLayout");
        assert_eq!(
            original,
            document(&other, "Forms$Layout", "ApplicationLayout").1
        );
        let (opaque_id, opaque) = document(&mpr, "Forms$Layout", "Opaque");
        let module_id = document(&mpr, "Projects$Module", "Main").0;
        let folder_id = mpr
            .insert_unit(
                &module_id,
                "Folders",
                mxrs_bson::doc! { "$Type": "Projects$Folder", "Name": "Layouts" },
                None,
            )
            .unwrap();
        mpr.relocate_unit(&id, &folder_id, "Documents").unwrap();
        let original_ids = named_ids(&original);
        let mut changed = layout();
        changed.widgets.reverse();
        changed.documentation = "Updated".into();
        crate::documents::synchronize_layouts(
            &mut mpr,
            &module_id,
            "11.12.1",
            std::slice::from_ref(&changed),
        )
        .unwrap();
        let (updated_id, updated) = document(&mpr, "Forms$Layout", "ApplicationLayout");
        assert_eq!(updated_id, id);
        assert_eq!(named_ids(&updated), original_ids);
        assert_eq!(mpr.unit(&id).unwrap().unwrap().container_id, folder_id);
        let bytes = mpr
            .content_bytes(&mpr.unit(&id).unwrap().unwrap())
            .unwrap()
            .unwrap();
        crate::documents::synchronize_layouts(
            &mut mpr,
            &module_id,
            "11.12.1",
            std::slice::from_ref(&changed),
        )
        .unwrap();
        assert_eq!(
            mpr.content_bytes(&mpr.unit(&id).unwrap().unwrap())
                .unwrap()
                .unwrap(),
            bytes
        );
        assert_eq!(
            mpr.parse_contents(&mpr.unit(&opaque_id).unwrap().unwrap())
                .unwrap(),
            opaque
        );
        assert!(matches!(
            crate::documents::synchronize_layouts(
                &mut mpr,
                &module_id,
                "11.12.1",
                &[changed.clone(), changed]
            ),
            Err(WriterError::DuplicateLayout(_))
        ));
        crate::documents::synchronize_layouts(&mut mpr, &module_id, "unsupported", &[]).unwrap();
    }

    #[test]
    fn stable_layout_identities_rebind_pluggable_schema_pointers_to_their_preserved_targets() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("Project.mpr");
        let mut project = ProjectBuilder::new("11.12.1");
        project.module("Main", |module| {
            module.layout("ApplicationLayout", |layout| {
                layout.container(|container| {
                    container.data_grid_2(|widget| {
                        widget.name("grid");
                    });
                    container.placeholder("Main");
                });
            });
        });
        let project = project.build();
        crate::write_project(&path, &project).unwrap();
        let mpr = MprFile::open(&path, true).unwrap();
        let (_, original) = document(&mpr, "Forms$Layout", "ApplicationLayout");
        let widget = original
            .get_document("Content")
            .unwrap()
            .get_array("Widgets")
            .unwrap()[1]
            .as_document()
            .unwrap()
            .get_array("Widgets")
            .unwrap()[1]
            .as_document()
            .unwrap();
        let schema_id = widget
            .get_document("Type")
            .unwrap()
            .get_document("ObjectType")
            .unwrap()
            .get("$ID")
            .and_then(extract_id)
            .unwrap();
        let pointer = widget
            .get_document("Object")
            .unwrap()
            .get("TypePointer")
            .and_then(extract_id)
            .unwrap();
        assert_eq!(schema_id, pointer);
        drop(mpr);
        crate::synchronize_project(&path, &project).unwrap();
        let mpr = MprFile::open(&path, true).unwrap();
        assert_eq!(
            document(&mpr, "Forms$Layout", "ApplicationLayout").1,
            original
        );
    }
}

#[cfg(test)]
mod application_shell_tests {
    use super::*;
    #[test]
    fn stock_shell_compiles_both_navigation_modes_and_reserves_main() {
        fn types(value: &Bson, result: &mut Vec<String>) {
            match value {
                Bson::Document(doc) => {
                    if let Ok(ty) = doc.get_str("$Type") {
                        result.push(ty.into());
                    }
                    for value in doc.values() {
                        types(value, result);
                    }
                }
                Bson::Array(items) => {
                    for item in items {
                        types(item, result);
                    }
                }
                _ => {}
            }
        }
        let catalog = Rc::new(Catalog::for_version("11.12.1").unwrap());
        for navigation in [None, Some("Responsive")] {
            let mut layout = mxrs_dsl::LayoutBuilder::new("ApplicationLayout");
            layout.application_shell("My application", navigation);
            let mut decl = layout.into_decl();
            assert_eq!(placeholder_names(&decl).unwrap(), ["Main"]);
            let doc = compile_layout(&catalog, &decl, None).unwrap();
            let mut actual = Vec::new();
            types(&Bson::Document(doc), &mut actual);
            for kind in ["ScrollContainer", "DynamicText", "Placeholder"] {
                assert!(
                    actual.iter().any(|ty| ty.ends_with(&format!("${kind}"))),
                    "{actual:?}"
                );
            }
            for kind in ["NavigationTree", "SidebarToggleButton"] {
                assert_eq!(
                    actual.iter().any(|ty| ty.ends_with(&format!("${kind}"))),
                    navigation.is_some()
                );
            }
            decl.widgets.push(WidgetDecl::LayoutPlaceholder {
                name: "Main".into(),
            });
            assert!(compile_layout(&catalog, &decl, None).is_err());
            let mut page = mxrs_ir::PageDecl::new("Page");
            page.widgets.push(WidgetDecl::ApplicationShell {
                title: "x".into(),
                navigation: None,
            });
            page.layout = Some(mxrs_ir::LayoutRef {
                qualified_name: "Main.ApplicationLayout".into(),
                parameter: "Main".into(),
            });
            assert!(page_compiler::compile_page(&catalog, &page, None).is_err());
        }
    }
}
