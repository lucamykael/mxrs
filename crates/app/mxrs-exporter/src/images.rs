//! The image collections of the modules a project made: each image a file
//! of `assets/images/<module>/<collection>/`, editable with any image
//! editor, and the collection the Rust that declares it —
//! `src/domain/documents/<module>/<collection>.rs` — naming each file
//! through `mxrs::asset!`.
//!
//! A collection is declared only when it holds nothing a declaration cannot
//! say: images of a known format, nothing beyond their name and bytes.

use std::fmt::Write as _;

use mxrs_ir::ImageFormat;

/// One collection the importer declares: its module, its name, the file
/// that states it, and the image files it names, by their paths under
/// `assets/`.
pub(crate) struct DeclaredCollection {
    pub(crate) module: String,
    pub(crate) name: String,
    pub(crate) stem: String,
    pub(crate) source: String,
    pub(crate) files: Vec<(String, Vec<u8>)>,
}

/// The image collections of `modules` a declaration restates, of the
/// modules `authored` says the project made.
pub(crate) fn declare(
    modules: &[mxrs_model::Module],
    authored: impl Fn(&str) -> bool,
) -> Vec<DeclaredCollection> {
    let mut declared = Vec::new();
    for module in modules {
        let Some(module_name) = module.name.as_deref() else {
            continue;
        };
        if !authored(module_name) {
            continue;
        }
        for document in &module.artifact_units {
            if document.get_str("$Type").ok() != Some("Images$ImageCollection") {
                continue;
            }
            if let Some(collection) = collection(module_name, document) {
                declared.push(collection);
            }
        }
    }
    declared.sort_by(|left, right| (&left.module, &left.name).cmp(&(&right.module, &right.name)));
    declared
}

fn collection(module: &str, document: &mxrs_bson::Document) -> Option<DeclaredCollection> {
    let keys: Vec<&str> = document.keys().map(String::as_str).collect();
    if keys
        != [
            "$ID",
            "$Type",
            "Documentation",
            "Excluded",
            "ExportLevel",
            "Images",
            "Name",
        ]
    {
        return None;
    }
    let name = document.get_str("Name").ok()?;
    let documentation = document.get_str("Documentation").ok()?;
    let excluded = document.get_bool("Excluded").ok()?;
    let export_level = document.get_str("ExportLevel").ok()?;
    let items = document.get_array("Images").ok()?;
    if items.first() != Some(&mxrs_bson::Bson::Int32(3)) || name.is_empty() {
        return None;
    }
    let folder = format!(
        "images/{}/{}",
        crate::module_stem(module),
        crate::inner_file_stem(name)
    );
    let mut images = Vec::new();
    let mut files = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for item in &items[1..] {
        let image = item.as_document()?;
        let keys: Vec<&str> = image.keys().map(String::as_str).collect();
        if keys != ["$ID", "$Type", "Image", "ImageFormat", "Name"]
            || image.get_str("$Type").ok()? != "Images$Image"
        {
            return None;
        }
        let image_name = image.get_str("Name").ok()?;
        let format = ImageFormat::parse(image.get_str("ImageFormat").ok()?)?;
        let mxrs_bson::Bson::Binary(data) = image.get("Image")? else {
            return None;
        };
        let file = format!("{folder}/{image_name}.{}", format.extension());
        // Two images a file system would not tell apart: the collection
        // stays in the imported model.
        if image_name.is_empty() || !seen.insert(file.to_lowercase()) {
            return None;
        }
        images.push((image_name.to_string(), format, file.clone()));
        files.push((file, data.bytes.clone()));
    }
    let stem = crate::inner_file_stem(name);
    let mut source = format!(
        "//! Image collection `{module}.{name}`: the images its pages show, each a\n//! file of `assets/{folder}/`.\n\nuse mxrs::prelude::*;\n\n#[declaration(module = {})]\npub fn {stem}(module: &mut ModuleBuilder) {{\n",
        crate::rust_string(module),
    );
    let mut calls = Vec::new();
    for (image_name, format, file) in &images {
        calls.push(format!(
            "images.image({}, ImageFormat::{}, mxrs::asset!({}));",
            crate::rust_string(image_name),
            format.as_str(),
            crate::rust_string(file)
        ));
    }
    if !documentation.is_empty() {
        calls.push(format!(
            "images.documentation({});",
            crate::rust_string(documentation)
        ));
    }
    if excluded {
        calls.push("images.excluded(true);".to_string());
    }
    match export_level {
        "Hidden" => {}
        "Published" => calls.push("images.export_level(ExportLevel::Published);".to_string()),
        _ => return None,
    }
    let name_literal = crate::rust_string(name);
    if calls.is_empty() {
        let _ = writeln!(
            source,
            "    module.image_collection({name_literal}, |_images| {{}});"
        );
    } else {
        let _ = writeln!(
            source,
            "    module.image_collection({name_literal}, |images| {{"
        );
        for call in &calls {
            let _ = writeln!(source, "        {call}");
        }
        let _ = writeln!(source, "    }});");
    }
    source.push_str("}\n");
    Some(DeclaredCollection {
        module: module.to_string(),
        name: name.to_string(),
        stem,
        source,
        files,
    })
}
