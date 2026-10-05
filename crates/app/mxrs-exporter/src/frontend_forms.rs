//! Declares the model's pages, layouts and snippets in the frontend: each
//! as the TSX that states its document (`mxrs_frontend::forms`).
//!
//! Nothing is taken on trust. The elements, the widgets and every form are
//! read back from the text written for them, and a form is declared in the
//! frontend only when what is read is the document the model stores; any
//! other stays in the imported model, and `MXRS_EXPLAIN_FLOWS=1` says why.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use mxrs_bson::{Bson, Document};
use mxrs_frontend::forms::{self, Vocabulary};
use mxrs_ir::{NativeDocument, NativeValue};

type Outcome<T> = std::result::Result<T, String>;

/// The forms the importer declares in the frontend.
#[derive(Default)]
pub(crate) struct FrontendForms {
    /// Each file, by its path under `frontend/src/`.
    pub(crate) files: Vec<(String, String)>,
    /// The forms those files declare: module, type and name.
    pub(crate) declared: BTreeSet<(String, String, String)>,
}

/// Where each document of `root` is, by its identity.
fn places(root: &Document, path: &str, found: &mut HashMap<String, String>) {
    if let Some(id) = root.get("$ID").and_then(mxrs_bson::extract_id) {
        found.insert(id, path.to_string());
    }
    for (key, value) in root {
        let nested = if path.is_empty() {
            key.clone()
        } else {
            format!("{path}.{key}")
        };
        match value {
            Bson::Document(document) => places(document, &nested, found),
            Bson::Array(items) => {
                for (index, item) in items.iter().skip(1).enumerate() {
                    if let Bson::Document(document) = item {
                        places(document, &format!("{nested}[{index}]"), found);
                    }
                }
            }
            _ => {}
        }
    }
}

fn native(document: &Document, found: &HashMap<String, String>) -> Outcome<NativeDocument> {
    let ty = document
        .get_str("$Type")
        .map_err(|_| "a document without a type".to_string())?;
    let mut out = NativeDocument::new(ty);
    for (key, value) in document {
        if matches!(key.as_str(), "$ID" | "$Type") {
            continue;
        }
        out.fields
            .push((key.clone(), native_value(value, ty, key, found)?));
    }
    Ok(out)
}

fn native_value(
    value: &Bson,
    ty: &str,
    key: &str,
    found: &HashMap<String, String>,
) -> Outcome<NativeValue> {
    Ok(match value {
        Bson::Null => NativeValue::Null,
        Bson::Boolean(value) => NativeValue::Bool(*value),
        Bson::Int32(value) => NativeValue::Int32(*value),
        Bson::Int64(value) => NativeValue::Int64(*value),
        Bson::String(value) => NativeValue::Text(value.clone()),
        Bson::Document(value) => NativeValue::Document(native(value, found)?),
        Bson::Array(values) => {
            let Some(Bson::Int32(marker)) = values.first() else {
                return Err(format!("{ty}.{key} is a list without a marker"));
            };
            NativeValue::List(
                *marker,
                values[1..]
                    .iter()
                    .map(|value| native_value(value, ty, key, found))
                    .collect::<Outcome<_>>()?,
            )
        }
        // The identity of another part of the same document.
        Bson::Binary(_) => match mxrs_bson::extract_id(value) {
            Some(id) => match found.get(&id) {
                Some(path) => NativeValue::Pointer(path.clone()),
                None => NativeValue::Identity(id),
            },
            None => return Err(format!("{ty}.{key} holds data")),
        },
        other => {
            return Err(format!(
                "{ty}.{key} holds a value nothing here can state: {other:?}"
            ));
        }
    })
}

/// The documents among a stored list's items.
fn items(value: Option<&Bson>) -> Vec<&Document> {
    match value {
        Some(Bson::Array(items)) => items.iter().filter_map(Bson::as_document).collect(),
        _ => Vec::new(),
    }
}

/// Puts the properties of a widget's object in the order its type defines
/// them, here and in the objects its properties hold. The order they are
/// stored in says nothing — each names its type — and a build keeps it.
fn order_properties(object: &mut Document, object_type: &Document) {
    let types = items(object_type.get("PropertyTypes"));
    let position = |property: &Document| {
        let pointer = property
            .get("TypePointer")
            .and_then(mxrs_bson::extract_id)?;
        types
            .iter()
            .position(|ty| ty.get("$ID").and_then(mxrs_bson::extract_id).as_ref() == Some(&pointer))
    };
    let Some(Bson::Array(stored)) = object.get_mut("Properties") else {
        return;
    };
    let mut properties: Vec<(usize, Document)> = Vec::new();
    for item in stored.iter().skip(1) {
        let Some(found) = item
            .as_document()
            .and_then(|property| Some((position(property)?, property.clone())))
        else {
            return;
        };
        properties.push(found);
    }
    properties.sort_by_key(|(index, _)| *index);
    for (index, property) in &mut properties {
        let nested = types[*index]
            .get_document("ValueType")
            .ok()
            .and_then(|value_type| value_type.get_document("ObjectType").ok());
        let Some(nested) = nested else {
            continue;
        };
        let Some(Bson::Array(objects)) = property
            .get_document_mut("Value")
            .ok()
            .and_then(|value| value.get_mut("Objects"))
        else {
            continue;
        };
        for object in objects.iter_mut().skip(1) {
            if let Bson::Document(object) = object {
                order_properties(object, nested);
            }
        }
    }
    stored.truncate(1);
    stored.extend(
        properties
            .into_iter()
            .map(|(_, property)| Bson::Document(property)),
    );
}

/// `value` with every pluggable widget's properties in the order of its
/// definition.
fn ordered(value: &mut Bson) {
    match value {
        Bson::Document(document) => {
            if document.get_str("$Type").ok() == Some("CustomWidgets$CustomWidget")
                && let Some(object_type) = document
                    .get_document("Type")
                    .ok()
                    .and_then(|ty| ty.get_document("ObjectType").ok())
                    .cloned()
                && let Ok(object) = document.get_document_mut("Object")
            {
                order_properties(object, &object_type);
            }
            for (_, nested) in document.iter_mut() {
                ordered(nested);
            }
        }
        Bson::Array(items) => {
            for item in items {
                ordered(item);
            }
        }
        _ => {}
    }
}

/// A stored page, layout or snippet as the document a declaration states:
/// without identities, each pointer naming where its target is, and each
/// widget's properties in the order of its definition.
fn native_form(document: &Document) -> Outcome<NativeDocument> {
    let mut canonical = Bson::Document(document.clone());
    ordered(&mut canonical);
    let Bson::Document(canonical) = canonical else {
        unreachable!("a document stays one");
    };
    let mut found = HashMap::new();
    places(&canonical, "", &mut found);
    native(&canonical, &found)
}

/// One stored form: its module, and its document or why it has none.
struct StoredForm {
    module: String,
    document: Outcome<NativeDocument>,
    name: String,
    ty: String,
}

/// Every page, layout and snippet of `modules`.
fn stored_forms(modules: &[mxrs_model::Module]) -> Vec<StoredForm> {
    let mut stored = Vec::new();
    for module in modules {
        let Some(module_name) = &module.name else {
            continue;
        };
        let documents = module
            .pages
            .iter()
            .map(mxrs_model::page::Page::raw_document)
            .chain(module.artifact_units.iter());
        for document in documents {
            let ty = document.get_str("$Type").unwrap_or_default();
            if forms::folder(ty).is_none() {
                continue;
            }
            stored.push(StoredForm {
                module: module_name.clone(),
                document: native_form(document),
                name: document.get_str("Name").unwrap_or_default().to_string(),
                ty: ty.to_string(),
            });
        }
    }
    stored.sort_by(|left, right| {
        (&left.module, &left.ty, &left.name).cmp(&(&right.module, &right.ty, &right.name))
    });
    stored
}

/// Declares in the frontend every page, layout and snippet of an authored
/// module that `keeps` does not hold back and whose TSX reads back as the
/// document the model stores.
pub(crate) fn declare_in_frontend(
    modules: &[mxrs_model::Module],
    authored: impl Fn(&str) -> bool,
    keeps: impl Fn(&str, &str, &str) -> bool,
) -> FrontendForms {
    let explain = std::env::var_os("MXRS_EXPLAIN_FLOWS").is_some();
    let stays = |form: &StoredForm, reason: &str| {
        if explain {
            eprintln!(
                "[mxrs] {} {}.{} stays in the imported model: {reason}",
                forms::folder(&form.ty).unwrap_or("form"),
                form.module,
                form.name
            );
        }
    };
    let stored = stored_forms(modules);
    let documents: Vec<&NativeDocument> = stored
        .iter()
        .filter_map(|form| form.document.as_ref().ok())
        .collect();
    if documents.is_empty() {
        return FrontendForms::default();
    }
    let mined = forms::mine(&documents);
    // What a build will read is the text, so the text is what is checked.
    let elements = forms::render_elements(&mined.shapes);
    let shapes = match forms::read_elements(&elements, "src/mxrs/elements.ts") {
        Ok(shapes) if shapes == mined.shapes => shapes,
        Ok(_) => {
            eprintln!(
                "[mxrs] warning: no page is declared in the frontend: its elements read back differently"
            );
            return FrontendForms::default();
        }
        Err(error) => {
            eprintln!("[mxrs] warning: no page is declared in the frontend: {error}");
            return FrontendForms::default();
        }
    };
    let mut widget_files: BTreeMap<String, String> = BTreeMap::new();
    let mut widgets = Vec::new();
    for definition in &mined.widgets {
        let path = format!("src/{}/{}.tsx", forms::WIDGETS_FOLDER, definition.name);
        let read = forms::render_widget(definition, &mined)
            .map_err(|reason| reason.to_string())
            .and_then(|source| match forms::read_widget(&source, &path, &shapes) {
                Ok(read) if &read == definition => Ok(source),
                Ok(_) => Err("its definition reads back differently".to_string()),
                Err(error) => Err(error.to_string()),
            });
        match read {
            Ok(source) => {
                widget_files.insert(definition.name.clone(), source);
                widgets.push(definition.clone());
            }
            Err(reason) if explain => {
                eprintln!(
                    "[mxrs] widget {} is not declared in the frontend: {reason}",
                    definition.name
                );
            }
            Err(_) => {}
        }
    }
    let vocabulary = Vocabulary { shapes, widgets };
    let mut declared = FrontendForms::default();
    let mut used: BTreeSet<String> = BTreeSet::new();
    let mut paths: BTreeSet<String> = BTreeSet::new();
    for form in &stored {
        if !authored(&form.module) || keeps(&form.module, &form.ty, &form.name) {
            continue;
        }
        let document = match &form.document {
            Ok(document) => document,
            Err(reason) => {
                stays(form, reason);
                continue;
            }
        };
        let folder = forms::folder(&form.ty).expect("a stored form has a folder");
        let path = format!(
            "{folder}/{}/{}.tsx",
            crate::module_stem(&form.module),
            form.name
        );
        // Two forms whose files a file system would not tell apart: the
        // second stays where it is.
        if !paths.insert(path.to_lowercase()) {
            stays(form, "another form already has its file's name");
            continue;
        }
        let source = match forms::render_form(&form.module, document, &vocabulary) {
            Ok(source) => source,
            Err(reason) => {
                stays(form, &reason);
                continue;
            }
        };
        match forms::read_form(&source, &path, &vocabulary) {
            Ok((module, read)) if module == form.module && &read.document == document => {}
            Ok(_) => {
                stays(form, "its TSX reads back differently");
                continue;
            }
            Err(error) => {
                stays(form, &error.to_string());
                continue;
            }
        }
        for definition in &vocabulary.widgets {
            if source.contains(&format!(
                "from \"@/{}/{}\"",
                forms::WIDGETS_FOLDER,
                definition.name
            )) {
                used.insert(definition.name.clone());
            }
        }
        declared.files.push((path, source));
        declared
            .declared
            .insert((form.module.clone(), form.ty.clone(), form.name.clone()));
    }
    if declared.files.is_empty() {
        return declared;
    }
    declared
        .files
        .push(("mxrs/elements.ts".to_string(), elements));
    for name in used {
        let source = widget_files
            .remove(&name)
            .expect("a used widget was written");
        declared
            .files
            .push((format!("{}/{name}.tsx", forms::WIDGETS_FOLDER), source));
    }
    declared.files.sort();
    declared
}
