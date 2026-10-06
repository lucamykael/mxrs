//! What a model gives the browser besides its pages: its icon collections
//! — each a font, and the stylesheet the Mendix client makes of it, one
//! rule per icon — and its image collections, as files. Written beside the
//! compiled theme, where a deployment keeps them: `fonts/<Module>$<Name>.ttf`,
//! `img/<Module>$<Collection>$<Image>.<ext>`, `collections.css` with the
//! font faces and the icon rules, and `collections.json`, which names
//! each collection's classes and each image's file for the frontend.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use mxrs_bson::{Document, parse_array};

/// An icon collection: the font its icons are glyphs of, and the classes
/// the Mendix client gives an icon of it (`<CollectionClass> <Prefix>-<icon>`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IconCollection {
    pub module: String,
    pub name: String,
    pub class: String,
    pub prefix: String,
    pub font: Vec<u8>,
    /// Each icon's name and the character it is in the font.
    pub icons: Vec<(String, u32)>,
}

impl IconCollection {
    /// `Atlas_Core$Atlas`: the font's family, and its file's stem.
    pub fn font_name(&self) -> String {
        format!("{}${}", self.module, self.name)
    }
}

/// An image of a collection, as the model stores it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Image {
    pub module: String,
    pub collection: String,
    pub name: String,
    /// The file's extension, from the format the model names.
    pub extension: String,
    pub bytes: Vec<u8>,
}

impl Image {
    /// `Atlas_Core.Layout.logo`: how a page names it.
    pub fn qualified_name(&self) -> String {
        format!("{}.{}.{}", self.module, self.collection, self.name)
    }

    /// `img/Atlas_Core$Layout$logo.svg`: where the site serves it.
    pub fn file(&self) -> String {
        format!(
            "img/{}${}${}.{}",
            self.module, self.collection, self.name, self.extension
        )
    }
}

/// What a model holds of both, and what of it is not written: a
/// collection or an image whose name is not one a file may be named with,
/// or an image in a format a browser does not show.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Collections {
    pub icons: Vec<IconCollection>,
    pub images: Vec<Image>,
    pub skipped: Vec<String>,
}

/// Whether a name of the model's may name a file: a Mendix identifier.
fn file_safe(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '_')
}

const ICON_COLLECTION: &str = "CustomIcons$CustomIconCollection";
const IMAGE_COLLECTION: &str = "Images$ImageCollection";

fn extension(format: &str) -> Option<&'static str> {
    match format {
        "Svg" => Some("svg"),
        "Png" => Some("png"),
        "Jpeg" => Some("jpg"),
        "Gif" => Some("gif"),
        "Bmp" => Some("bmp"),
        _ => None,
    }
}

fn items(document: &Document, key: &str) -> Vec<Document> {
    parse_array(document.get_array(key).ok().map(Vec::as_slice))
        .items
        .into_iter()
        .filter_map(|item| item.as_document().cloned())
        .collect()
}

fn binary(document: &Document, key: &str) -> Vec<u8> {
    document
        .get_binary_generic(key)
        .map(|bytes| bytes.to_vec())
        .unwrap_or_default()
}

/// The collections of `module`'s documents, each as the model stores it.
pub fn of_documents<'a>(
    module: &str,
    documents: impl IntoIterator<Item = &'a Document>,
) -> Collections {
    let mut collections = Collections::default();
    if !file_safe(module) {
        collections.skipped.push(format!(
            "module {module:?}: not a name a file may be named with"
        ));
        return collections;
    }
    for document in documents {
        let name = document.get_str("Name").unwrap_or_default().to_string();
        let ty = document.get_str("$Type").unwrap_or_default();
        if matches!(ty, ICON_COLLECTION | IMAGE_COLLECTION) && !file_safe(&name) {
            collections.skipped.push(format!(
                "{module}.{name:?}: not a name a file may be named with"
            ));
            continue;
        }
        match ty {
            ICON_COLLECTION => collections.icons.push(IconCollection {
                module: module.to_string(),
                name,
                class: document
                    .get_str("CollectionClass")
                    .unwrap_or_default()
                    .to_string(),
                prefix: document.get_str("Prefix").unwrap_or_default().to_string(),
                font: binary(document, "FontData"),
                icons: items(document, "Icons")
                    .iter()
                    .filter_map(|icon| {
                        let code = icon
                            .get_i32("CharacterCode")
                            .map(|code| code as u32)
                            .or_else(|_| icon.get_i64("CharacterCode").map(|code| code as u32))
                            .ok()?;
                        Some((icon.get_str("Name").ok()?.to_string(), code))
                    })
                    .collect(),
            }),
            IMAGE_COLLECTION => {
                for image in items(document, "Images") {
                    let Ok(image_name) = image.get_str("Name") else {
                        continue;
                    };
                    if !file_safe(image_name) {
                        collections.skipped.push(format!(
                            "{module}.{name}.{image_name:?}: not a name a file may be named with"
                        ));
                        continue;
                    }
                    let format = image.get_str("ImageFormat").unwrap_or_default();
                    let Some(extension) = extension(format) else {
                        collections.skipped.push(format!(
                            "{module}.{name}.{image_name}: {format:?} is not a format a browser shows"
                        ));
                        continue;
                    };
                    collections.images.push(Image {
                        module: module.to_string(),
                        collection: name.clone(),
                        name: image_name.to_string(),
                        extension: extension.to_string(),
                        bytes: binary(&image, "Image"),
                    });
                }
            }
            _ => {}
        }
    }
    collections
}

/// The collections of every module of the model at `mpr`, in a stable
/// order: by module, then by name.
pub fn of_model(mpr: &Path) -> Result<Collections, String> {
    let project = mxrs_model::Project::open(mpr, true)
        .map_err(|error| format!("{}: {error}", mpr.display()))?;
    let mut collections = Collections::default();
    for module in project
        .modules()
        .map_err(|error| format!("{}: {error}", mpr.display()))?
    {
        let Some(name) = module.name.as_deref() else {
            continue;
        };
        let found = of_documents(name, &module.artifact_units);
        collections.icons.extend(found.icons);
        collections.images.extend(found.images);
        collections.skipped.extend(found.skipped);
    }
    collections
        .icons
        .sort_by(|left, right| (&left.module, &left.name).cmp(&(&right.module, &right.name)));
    collections.images.sort_by(|left, right| {
        (&left.module, &left.collection, &left.name).cmp(&(
            &right.module,
            &right.collection,
            &right.name,
        ))
    });
    Ok(collections)
}

/// The stylesheet the Mendix client makes of icon collections: a font face
/// per collection, its class, and a rule per icon.
pub fn stylesheet(collections: &Collections) -> String {
    let mut css = String::new();
    for collection in &collections.icons {
        let font = collection.font_name();
        let _ = writeln!(
            css,
            "@font-face {{\n  font-family: \"{font}\";\n  src: url(\"./fonts/{font}.ttf\") format(\"truetype\");\n}}"
        );
        let _ = writeln!(
            css,
            ".{} {{\n  display: inline-block;\n  font-style: normal;\n  font-weight: normal;\n  line-height: 1;\n  font-family: \"{font}\";\n}}\n",
            collection.class
        );
        for (icon, code) in &collection.icons {
            let _ = writeln!(
                css,
                ".{}.{}-{icon}::before {{\n  content: \"\\{code:x}\";\n}}\n",
                collection.class, collection.prefix
            );
        }
    }
    css
}

/// What the frontend needs of the collections: each icon collection's
/// classes by its qualified name, and each image's file by its own.
pub fn manifest(collections: &Collections) -> serde_json::Value {
    let icons: BTreeMap<String, serde_json::Value> = collections
        .icons
        .iter()
        .map(|collection| {
            (
                format!("{}.{}", collection.module, collection.name),
                serde_json::json!({ "class": collection.class, "prefix": collection.prefix }),
            )
        })
        .collect();
    let images: BTreeMap<String, String> = collections
        .images
        .iter()
        .map(|image| (image.qualified_name(), image.file()))
        .collect();
    serde_json::json!({ "icons": icons, "images": images })
}

/// What writing the collections did.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Written {
    pub fonts: usize,
    pub icons: usize,
    pub images: usize,
    /// The files written anew or changed.
    pub changed: Vec<PathBuf>,
    /// The files of collections the model no longer has, removed.
    pub removed: Vec<PathBuf>,
    /// What the model holds that was not written, and why.
    pub skipped: Vec<String>,
}

fn keep(path: &Path, bytes: &[u8], written: &mut Written) -> Result<(), String> {
    if std::fs::read(path).is_ok_and(|current| current == bytes) {
        return Ok(());
    }
    if let Some(folder) = path.parent() {
        std::fs::create_dir_all(folder)
            .map_err(|error| format!("{}: {error}", folder.display()))?;
    }
    std::fs::write(path, bytes).map_err(|error| format!("{}: {error}", path.display()))?;
    written.changed.push(path.to_path_buf());
    Ok(())
}

/// Removes, under `folder`, every file not in `kept`: what a collection the
/// model no longer has left behind.
fn prune(
    folder: &Path,
    kept: &std::collections::BTreeSet<PathBuf>,
    written: &mut Written,
) -> Result<(), String> {
    for entry in std::fs::read_dir(folder).into_iter().flatten().flatten() {
        let path = entry.path();
        if path.is_file() && !kept.contains(&path) {
            std::fs::remove_file(&path).map_err(|error| format!("{}: {error}", path.display()))?;
            written.removed.push(path);
        }
    }
    Ok(())
}

/// Writes the collections of the model at `mpr` beside the compiled theme
/// of the project at `root`, each file only when it is not already what it
/// would be, and removes what earlier collections left that the model no
/// longer has.
pub fn write(root: &Path, mpr: &Path) -> Result<Written, String> {
    let collections = of_model(mpr)?;
    let folder = root.join("assets/theme-cache/web");
    let mut written = Written {
        skipped: collections.skipped.clone(),
        ..Written::default()
    };
    let mut kept = std::collections::BTreeSet::new();
    for collection in &collections.icons {
        let font = folder.join(format!("fonts/{}.ttf", collection.font_name()));
        keep(&font, &collection.font, &mut written)?;
        kept.insert(font);
        written.fonts += 1;
        written.icons += collection.icons.len();
    }
    for image in &collections.images {
        let file = folder.join(image.file());
        keep(&file, &image.bytes, &mut written)?;
        kept.insert(file);
        written.images += 1;
    }
    prune(&folder.join("fonts"), &kept, &mut written)?;
    prune(&folder.join("img"), &kept, &mut written)?;
    let stylesheet_file = folder.join("collections.css");
    let manifest_file = folder.join("collections.json");
    if collections.icons.is_empty() && collections.images.is_empty() {
        for stale in [stylesheet_file, manifest_file] {
            if stale.is_file() {
                std::fs::remove_file(&stale)
                    .map_err(|error| format!("{}: {error}", stale.display()))?;
                written.removed.push(stale);
            }
        }
        return Ok(written);
    }
    keep(
        &stylesheet_file,
        stylesheet(&collections).as_bytes(),
        &mut written,
    )?;
    let manifest = serde_json::to_string_pretty(&manifest(&collections))
        .expect("the collections manifest is serializable");
    keep(
        &manifest_file,
        format!("{manifest}\n").as_bytes(),
        &mut written,
    )?;
    Ok(written)
}

/// Writes the collections and says so when something changed; a model
/// whose collections cannot be read is a warning — the build stands.
pub fn write_and_report(root: &Path, mpr: &Path) {
    match write(root, mpr) {
        Ok(written) => {
            for skipped in &written.skipped {
                eprintln!("[mxrs] warning: not written to assets/theme-cache/web: {skipped}");
            }
            if !written.changed.is_empty() || !written.removed.is_empty() {
                println!(
                    "[mxrs] wrote {} icon font(s) ({} icons) and {} image(s) to assets/theme-cache/web{}",
                    written.fonts,
                    written.icons,
                    written.images,
                    if written.removed.is_empty() {
                        String::new()
                    } else {
                        format!(", removed {} file(s)", written.removed.len())
                    }
                );
            }
        }
        Err(error) => eprintln!("[mxrs] warning: {error}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mxrs_bson::{Bson, build_array, doc};

    fn icon_collection() -> Document {
        doc! {
            "$Type": ICON_COLLECTION,
            "Name": "Atlas",
            "CollectionClass": "mx-icon-lined",
            "Prefix": "mx-icon",
            "FontData": Bson::Binary(mxrs_bson::Binary { subtype: mxrs_bson::BinarySubtype::Generic, bytes: b"font bytes".to_vec() }),
            "Icons": build_array(vec![
                Bson::Document(doc! { "$Type": "CustomIcons$CustomIcon", "Name": "add", "CharacterCode": 0xe900_i32 }),
                Bson::Document(doc! { "$Type": "CustomIcons$CustomIcon", "Name": "trash-can", "CharacterCode": 0xe9ff_i32 }),
            ], 2),
        }
    }

    fn image_collection() -> Document {
        doc! {
            "$Type": IMAGE_COLLECTION,
            "Name": "Layout",
            "Images": build_array(vec![
                Bson::Document(doc! { "$Type": "Images$Image", "Name": "logo", "ImageFormat": "Svg", "Image": Bson::Binary(mxrs_bson::Binary { subtype: mxrs_bson::BinarySubtype::Generic, bytes: b"<svg/>".to_vec() }) }),
            ], 2),
        }
    }

    #[test]
    fn the_collections_become_the_fonts_rules_and_files_the_mendix_client_makes() {
        let documents = [
            icon_collection(),
            image_collection(),
            doc! { "$Type": "Forms$Snippet", "Name": "Other" },
        ];
        let collections = of_documents("Atlas_Core", &documents);
        assert_eq!(collections.icons.len(), 1);
        assert_eq!(collections.icons[0].font_name(), "Atlas_Core$Atlas");
        assert_eq!(
            collections.icons[0].icons,
            vec![
                ("add".to_string(), 0xe900),
                ("trash-can".to_string(), 0xe9ff)
            ]
        );
        assert_eq!(
            collections.images[0].qualified_name(),
            "Atlas_Core.Layout.logo"
        );
        assert_eq!(
            collections.images[0].file(),
            "img/Atlas_Core$Layout$logo.svg"
        );
        let css = stylesheet(&collections);
        assert!(css.contains("@font-face {\n  font-family: \"Atlas_Core$Atlas\";\n  src: url(\"./fonts/Atlas_Core$Atlas.ttf\") format(\"truetype\");\n}"), "{css}");
        assert!(
            css.contains(".mx-icon-lined {\n  display: inline-block;"),
            "{css}"
        );
        assert!(
            css.contains(".mx-icon-lined.mx-icon-add::before {\n  content: \"\\e900\";\n}"),
            "{css}"
        );
        assert!(
            css.contains(".mx-icon-lined.mx-icon-trash-can::before {\n  content: \"\\e9ff\";\n}"),
            "{css}"
        );
        let manifest = manifest(&collections);
        assert_eq!(
            manifest["icons"]["Atlas_Core.Atlas"]["class"],
            "mx-icon-lined"
        );
        assert_eq!(manifest["icons"]["Atlas_Core.Atlas"]["prefix"], "mx-icon");
        assert_eq!(
            manifest["images"]["Atlas_Core.Layout.logo"],
            "img/Atlas_Core$Layout$logo.svg"
        );
        assert_eq!(extension("Jpeg"), Some("jpg"));
        assert_eq!(extension("Tiff"), None);
        // A name no file may be named with, or a format no browser shows, is said, not written.
        let mut odd = image_collection();
        odd.insert("Name", "../Layout");
        let mut odd_format = image_collection();
        odd_format.insert("Images", build_array(vec![
            Bson::Document(doc! { "$Type": "Images$Image", "Name": "photo", "ImageFormat": "Tiff", "Image": Bson::Binary(mxrs_bson::Binary { subtype: mxrs_bson::BinarySubtype::Generic, bytes: b"x".to_vec() }) }),
        ], 2));
        let skipped = of_documents("Atlas_Core", &[odd, odd_format]);
        assert!(skipped.images.is_empty());
        assert_eq!(skipped.skipped.len(), 2, "{:?}", skipped.skipped);
        assert!(
            of_documents("../core", &[image_collection()])
                .images
                .is_empty()
        );
    }

    #[test]
    fn files_are_written_once_and_only_when_they_change() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let folder = root.join("assets/theme-cache/web");
        let collections = of_documents("Atlas_Core", &[icon_collection(), image_collection()]);
        let mut written = Written::default();
        keep(
            &folder.join("collections.css"),
            stylesheet(&collections).as_bytes(),
            &mut written,
        )
        .unwrap();
        assert_eq!(written.changed.len(), 1);
        let mut again = Written::default();
        keep(
            &folder.join("collections.css"),
            stylesheet(&collections).as_bytes(),
            &mut again,
        )
        .unwrap();
        assert!(again.changed.is_empty());
        assert_eq!(
            std::fs::read_to_string(folder.join("collections.css")).unwrap(),
            stylesheet(&collections)
        );
        // What an earlier model left under the folders is removed.
        std::fs::create_dir_all(folder.join("fonts")).unwrap();
        std::fs::write(folder.join("fonts/Gone$Icons.ttf"), b"old").unwrap();
        let kept = std::collections::BTreeSet::from([folder.join("fonts/Kept$Icons.ttf")]);
        std::fs::write(folder.join("fonts/Kept$Icons.ttf"), b"kept").unwrap();
        let mut pruned = Written::default();
        prune(&folder.join("fonts"), &kept, &mut pruned).unwrap();
        assert_eq!(pruned.removed, vec![folder.join("fonts/Gone$Icons.ttf")]);
        assert!(folder.join("fonts/Kept$Icons.ttf").is_file());
    }
}
