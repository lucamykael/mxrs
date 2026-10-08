//! Declares the model's pages, layouts, snippets, page templates and
//! building blocks in the frontend: each as the TSX that states its
//! document (`mxrs_frontend::forms`), a thumbnail it holds an image beside
//! it.
//!
//! Nothing is taken on trust. The elements, the widgets and every form are
//! read back from the text written for them, and a form is declared in the
//! frontend only when what is read is the document the model stores; any
//! other stays in the imported model, and `MXRS_EXPLAIN_FLOWS=1` says why.

use std::collections::{BTreeMap, BTreeSet};

use mxrs_frontend::forms::{self, Vocabulary};
use mxrs_ir::NativeDocument;

type Outcome<T> = std::result::Result<T, String>;

/// The forms the importer declares in the frontend.
#[derive(Default)]
pub(crate) struct FrontendForms {
    /// Each file, by its path under `frontend/src/`.
    pub(crate) files: Vec<(String, String)>,
    /// Each image a form holds, by its path under `frontend/src/`, beside
    /// the form's file.
    pub(crate) assets: Vec<(String, Vec<u8>)>,
    /// The forms those files declare: module, type and name.
    pub(crate) declared: BTreeSet<(String, String, String)>,
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
                document: mxrs_writer::stated_document(document),
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
        let (source, assets) = match forms::render_form_files(&form.module, document, &vocabulary) {
            Ok(rendered) => rendered,
            Err(reason) => {
                stays(form, &reason);
                continue;
            }
        };
        let folder_of_form = path.rsplit_once('/').map_or("", |(folder, _)| folder);
        let beside = |specifier: &str| {
            let name = specifier.strip_prefix("./")?;
            assets
                .iter()
                .find(|(file, _)| file == name)
                .map(|(_, bytes)| bytes.clone())
        };
        match forms::read_form_with(&source, &path, &vocabulary, &beside) {
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
        // A form's images could share their names with another form's
        // files only if the forms did.
        for (file, bytes) in assets {
            declared
                .assets
                .push((format!("{folder_of_form}/{file}"), bytes));
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
