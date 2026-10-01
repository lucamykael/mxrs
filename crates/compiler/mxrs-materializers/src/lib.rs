//! Deterministic React-shell materialization for an MXRS application.
//!
//! The production bundle is built once from the pinned `frontend/` project
//! and embedded in this crate. [`materialize_mpr`] therefore needs no Node,
//! npm, network, or writable package cache at application-build time. The
//! emitted `model.json` is a stable projection of the current `.mpr`; APIs
//! that execute actions are deliberately a runtime (§5) boundary.

use std::collections::{BTreeMap, HashMap};
use std::path::{Component, Path, PathBuf};

use mxrs_bson::{Bson, Document};
use mxrs_model::Project;
use mxrs_model::navigation::NavigationItem;
use mxrs_model::page::Widget;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

const INDEX: EmbeddedAsset = EmbeddedAsset::new(
    "index.html",
    include_bytes!("../assets/index.html"),
    "851856e6e79f477997ef9c8ca99d295392944597e974fb5599c16459a96c3db1",
);
const SCRIPT: EmbeddedAsset = EmbeddedAsset::new(
    "app-C3Of9HqT.js",
    include_bytes!("../assets/app-C3Of9HqT.js"),
    "e972f793e3a56409bac200a5475077dfc6f0f29f20bfd52009a79c37d6dbdf8a",
);
const STYLES: EmbeddedAsset = EmbeddedAsset::new(
    "app-BbpcvhG2.css",
    include_bytes!("../assets/app-BbpcvhG2.css"),
    "4588a89594a270c8b670605195129d72de79a2a1887be046584b76f8a93c6139",
);
const VITE_MANIFEST: EmbeddedAsset = EmbeddedAsset::new(
    ".vite/manifest.json",
    include_bytes!("../assets/.vite/manifest.json"),
    "f0be01d66cce18a5eae18f55e61f83274a189f72592cdfac4a7d25a512770f63",
);
const BUNDLE_MANIFEST: EmbeddedAsset = EmbeddedAsset::new(
    "bundle-manifest.json",
    include_bytes!("../assets/bundle-manifest.json"),
    "e2c3c2478e65cc4c3d90ca7483395ded75d367bb905cfd526c0e51baffeb29ac",
);
const LICENSES: EmbeddedAsset = EmbeddedAsset::new(
    "THIRD_PARTY_LICENSES.md",
    include_bytes!("../assets/THIRD_PARTY_LICENSES.md"),
    "9b5ecd0c4d8c0883b9044e6f040a35536a8f29c5c9cfd1e4b12868dc9908d058",
);
const ASSETS: &[EmbeddedAsset] = &[
    INDEX,
    SCRIPT,
    STYLES,
    VITE_MANIFEST,
    BUNDLE_MANIFEST,
    LICENSES,
];

const SOURCE_FILES: &[(&str, &[u8])] = &[
    (".gitignore", include_bytes!("../frontend/.gitignore")),
    ("package.json", include_bytes!("../frontend/package.json")),
    (
        "package-lock.json",
        include_bytes!("../frontend/package-lock.json"),
    ),
    ("tsconfig.json", include_bytes!("../frontend/tsconfig.json")),
    (
        "vite.config.ts",
        include_bytes!("../frontend/vite.config.ts"),
    ),
    ("index.html", include_bytes!("../frontend/index.html")),
    ("src/main.tsx", include_bytes!("../frontend/src/main.tsx")),
    ("src/style.css", include_bytes!("../frontend/src/style.css")),
    (
        "src/vite-env.d.ts",
        include_bytes!("../frontend/src/vite-env.d.ts"),
    ),
    (
        "scripts/sync-assets.mjs",
        include_bytes!("../frontend/scripts/sync-assets.mjs"),
    ),
];

#[derive(Debug, Clone, Copy)]
struct EmbeddedAsset {
    path: &'static str,
    bytes: &'static [u8],
    sha256: &'static str,
}

impl EmbeddedAsset {
    const fn new(path: &'static str, bytes: &'static [u8], sha256: &'static str) -> Self {
        Self {
            path,
            bytes,
            sha256,
        }
    }
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct MaterializeReport {
    pub changed_files: usize,
    pub unchanged_files: usize,
    pub removed_stale_files: usize,
    pub output: PathBuf,
}

#[derive(Debug, thiserror::Error)]
pub enum MaterializeError {
    #[error(transparent)]
    Model(#[from] mxrs_model::ModelError),
    #[error("cannot access {path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("cannot serialize application model: {0}")]
    Json(#[from] serde_json::Error),
    #[error("embedded frontend asset {path} failed SHA-256 verification")]
    AssetHash { path: &'static str },
    #[error("generated asset inventory contains an unsafe path: {0}")]
    UnsafeInventoryPath(String),
}

pub type Result<T> = std::result::Result<T, MaterializeError>;

/// Opens an MPR read-only and materializes its standalone web application.
pub fn materialize_mpr(
    mpr: impl AsRef<Path>,
    output: impl AsRef<Path>,
) -> Result<MaterializeReport> {
    let mpr = mpr.as_ref();
    let project = Project::open(mpr, true)?;
    let mut manifest = application_manifest(&project)?;
    if project.name()?.as_deref().is_none_or(str::is_empty)
        && let Some(stem) = mpr.file_stem().and_then(|stem| stem.to_str())
    {
        manifest["project"]["name"] = Value::String(stem.to_string());
    }
    materialize_manifest(&manifest, output)
}

/// Materializes the embedded production bundle and one stable `model.json`.
pub fn materialize_manifest(
    manifest: &Value,
    output: impl AsRef<Path>,
) -> Result<MaterializeReport> {
    let output = output.as_ref();
    std::fs::create_dir_all(output).map_err(|source| io_error(output, source))?;
    verify_embedded_assets()?;
    let inventory_path = output.join(".mxrs-assets.json");
    let previous = read_inventory(&inventory_path)?;
    let mut report = MaterializeReport {
        output: output.to_path_buf(),
        ..Default::default()
    };
    let current = ASSETS
        .iter()
        .map(|asset| asset.path.to_string())
        .chain(["model.json".to_string(), ".mxrs-assets.json".to_string()])
        .collect::<Vec<_>>();
    for stale in previous.iter().filter(|path| !current.contains(path)) {
        let relative = safe_relative_path(stale)?;
        let path = output.join(relative);
        if path.is_file() {
            std::fs::remove_file(&path).map_err(|source| io_error(&path, source))?;
            report.removed_stale_files += 1;
        }
    }
    for asset in ASSETS {
        write_if_changed(output.join(asset.path), asset.bytes, &mut report)?;
    }
    let mut model = serde_json::to_vec_pretty(manifest)?;
    model.push(b'\n');
    write_if_changed(output.join("model.json"), &model, &mut report)?;
    let mut inventory = serde_json::to_vec_pretty(&current)?;
    inventory.push(b'\n');
    write_if_changed(inventory_path, &inventory, &mut report)?;
    Ok(report)
}

/// Emits the pinned Vite/TypeScript sources for deliberate frontend work.
/// Running npm is intentionally left to the explicit caller.
pub fn materialize_frontend_sources(output: impl AsRef<Path>) -> Result<MaterializeReport> {
    let output = output.as_ref();
    std::fs::create_dir_all(output).map_err(|source| io_error(output, source))?;
    let mut report = MaterializeReport {
        output: output.to_path_buf(),
        ..Default::default()
    };
    for (path, bytes) in SOURCE_FILES {
        write_if_changed(output.join(path), bytes, &mut report)?;
    }
    Ok(report)
}

/// Builds the frontend's stable, runtime-neutral application contract.
pub fn application_manifest(project: &Project) -> Result<Value> {
    let project_name = project.name()?.unwrap_or_else(|| "Application".into());
    let mendix_version = project.mendix_version()?.unwrap_or_default();
    let mut modules = project.modules()?;
    modules.sort_by(|left, right| left.name.cmp(&right.name));
    let page_names = modules
        .iter()
        .flat_map(|module| {
            let module_name = module.name.as_deref().unwrap_or("Unnamed");
            module.pages.iter().filter_map(move |page| {
                Some((
                    page.id.clone()?,
                    format!("{module_name}.{}", page.name.as_deref()?),
                ))
            })
        })
        .collect::<HashMap<_, _>>();
    let module_values = modules
        .iter()
        .map(|module| -> Result<Value> {
            let module_name = module.name.as_deref().unwrap_or("Unnamed");
            let mut pages = module.pages.iter().collect::<Vec<_>>();
            pages.sort_by(|left, right| left.name.cmp(&right.name));
            let mut entities = module.entities().iter().collect::<Vec<_>>();
            entities.sort_by(|left, right| left.name.cmp(&right.name));
            let page_values = pages
                .into_iter()
                .filter_map(|page| page.name.as_deref().map(|name| (page, name)))
                .map(|(page, name)| -> Result<Value> {
                    Ok(json!({
                        "name": name,
                        "qualified_name": format!("{module_name}.{name}"),
                        "title": page.title,
                        "url": page.url,
                        "widgets": page.widgets.iter().map(widget_value).collect::<Vec<_>>(),
                    }))
                })
                .collect::<Result<Vec<_>>>()?;
            Ok(json!({
                "name": module_name,
                "entities": entities.into_iter().map(|entity| json!({
                    "name": entity.qualified_name.as_deref().or(entity.name.as_deref()).unwrap_or(""),
                    "persistable": entity.persistable,
                    "attributes": entity.attributes.iter().map(|attribute| json!({
                        "name": attribute.name,
                        "type": format!("{:?}", attribute.attribute_type).to_lowercase(),
                        "required": attribute.required,
                        "unique": attribute.unique,
                        "default": attribute.default_value,
                        "enumeration": attribute.enumeration,
                    })).collect::<Vec<_>>(),
                })).collect::<Vec<_>>(),
                "associations": module.associations().into_iter().map(|association| json!({
                    "name": association.name,
                    "from_entity": association.from_entity_id,
                    "to_entity": association.to_entity_id,
                    "type": format!("{:?}", association.association_type),
                })).collect::<Vec<_>>(),
                "microflows": module.microflows.iter().filter_map(|flow| flow.name.as_ref()).collect::<Vec<_>>(),
                "nanoflows": module.nanoflows.iter().filter_map(|flow| flow.name.as_ref()).collect::<Vec<_>>(),
                "pages": page_values,
            }))
        })
        .collect::<Result<Vec<_>>>()?;
    let navigation = project.navigation()?;
    let profiles = navigation
        .profiles
        .iter()
        .map(|profile| json!({
            "name": profile.name,
            "kind": profile.kind,
            "home_page": resolve_page(profile.home_page.as_deref(), &page_names),
            "sign_in_page": resolve_page(profile.sign_in_page.as_deref(), &page_names),
            "items": profile.menu_items.iter().map(|item| navigation_value(item, &page_names)).collect::<Vec<_>>(),
        }))
        .collect::<Vec<_>>();
    Ok(json!({
        "format": 1,
        "project": { "name": project_name, "mendix_version": mendix_version },
        "modules": module_values,
        "navigation": { "profiles": profiles },
    }))
}

fn widget_value(widget: &Widget) -> Value {
    json!({
        "type": widget.widget_type,
        "name": widget.name,
        "options": document_value(&widget.options),
        "events": widget.events.iter().map(document_value).collect::<Vec<_>>(),
        "children": widget.children.iter().map(widget_value).collect::<Vec<_>>(),
    })
}

fn document_value(document: &Document) -> Value {
    let sorted = document
        .iter()
        .map(|(key, value)| (key.clone(), bson_value(value)))
        .collect::<BTreeMap<_, _>>();
    Value::Object(sorted.into_iter().collect())
}

fn bson_value(value: &Bson) -> Value {
    match value {
        Bson::Double(value) => serde_json::Number::from_f64(*value)
            .map(Value::Number)
            .unwrap_or(Value::Null),
        Bson::String(value) | Bson::JavaScriptCode(value) | Bson::Symbol(value) => {
            Value::String(value.clone())
        }
        Bson::Array(values) => {
            let values = values
                .strip_prefix(&[Bson::Int32(1)])
                .or_else(|| values.strip_prefix(&[Bson::Int32(2)]))
                .or_else(|| values.strip_prefix(&[Bson::Int32(3)]))
                .unwrap_or(values);
            Value::Array(values.iter().map(bson_value).collect())
        }
        Bson::Document(document) => document_value(document),
        Bson::Boolean(value) => Value::Bool(*value),
        Bson::Null | Bson::Undefined | Bson::MinKey | Bson::MaxKey => Value::Null,
        Bson::Int32(value) => Value::Number((*value).into()),
        Bson::Int64(value) => Value::Number((*value).into()),
        Bson::DateTime(value) => Value::Number(value.timestamp_millis().into()),
        Bson::Binary(value) => json!({ "$binary_bytes": value.bytes.len() }),
        Bson::ObjectId(value) => Value::String(value.to_hex()),
        Bson::Decimal128(value) => Value::String(value.to_string()),
        Bson::RegularExpression(value) => {
            json!({ "$regex": value.pattern.to_string(), "$options": value.options.to_string() })
        }
        Bson::Timestamp(value) => {
            json!({ "$timestamp": value.time, "$increment": value.increment })
        }
        Bson::JavaScriptCodeWithScope(value) => json!({
            "$code": value.code,
            "$scope": document_value(&value.scope),
        }),
        Bson::DbPointer(value) => Value::String(format!("{value:?}")),
    }
}

fn navigation_value(item: &NavigationItem, pages: &HashMap<String, String>) -> Value {
    let icon = item.icon.as_ref().map(|icon| match icon {
        mxrs_model::navigation::NavigationIcon::Glyph(value) => Value::String(value.clone()),
        mxrs_model::navigation::NavigationIcon::Code(value) => Value::Number((*value).into()),
    });
    json!({
        "caption": item.caption.values().next().cloned().unwrap_or_default(),
        "page": resolve_page(item.page.as_deref(), pages),
        "microflow": item.microflow,
        "icon": icon,
        "items": item.items.iter().map(|child| navigation_value(child, pages)).collect::<Vec<_>>(),
    })
}

fn resolve_page(reference: Option<&str>, pages: &HashMap<String, String>) -> Option<String> {
    let reference = reference?;
    Some(
        pages
            .get(reference)
            .cloned()
            .unwrap_or_else(|| reference.to_string()),
    )
}

fn verify_embedded_assets() -> Result<()> {
    for asset in ASSETS.iter().filter(|asset| !asset.sha256.is_empty()) {
        let actual = format!("{:x}", Sha256::digest(asset.bytes));
        if actual != asset.sha256 {
            return Err(MaterializeError::AssetHash { path: asset.path });
        }
    }
    Ok(())
}

fn read_inventory(path: &Path) -> Result<Vec<String>> {
    if !path.is_file() {
        return Ok(vec![]);
    }
    let bytes = std::fs::read(path).map_err(|source| io_error(path, source))?;
    Ok(serde_json::from_slice(&bytes)?)
}

fn safe_relative_path(path: &str) -> Result<PathBuf> {
    let value = Path::new(path);
    if value.is_absolute()
        || value
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(MaterializeError::UnsafeInventoryPath(path.to_string()));
    }
    Ok(value.to_path_buf())
}

fn write_if_changed(path: PathBuf, bytes: &[u8], report: &mut MaterializeReport) -> Result<()> {
    if std::fs::read(&path).is_ok_and(|current| current == bytes) {
        report.unchanged_files += 1;
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|source| io_error(parent, source))?;
    }
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("asset");
    let temporary = path.with_file_name(format!(".{file_name}.mxrs-{}.tmp", std::process::id()));
    std::fs::write(&temporary, bytes).map_err(|source| io_error(&temporary, source))?;
    std::fs::rename(&temporary, &path).map_err(|source| io_error(&path, source))?;
    report.changed_files += 1;
    Ok(())
}

fn io_error(path: &Path, source: std::io::Error) -> MaterializeError {
    MaterializeError::Io {
        path: path.display().to_string(),
        source,
    }
}

/// Returns the hashes asserted by the embedded release manifest.
pub fn embedded_asset_hashes() -> BTreeMap<&'static str, &'static str> {
    ASSETS
        .iter()
        .filter(|asset| !asset.sha256.is_empty())
        .map(|asset| (asset.path, asset.sha256))
        .collect()
}

pub fn embedded_frontend_source_hash() -> String {
    let mut digest = Sha256::new();
    for (path, bytes) in SOURCE_FILES {
        digest.update(path.as_bytes());
        digest.update([0]);
        digest.update(bytes);
    }
    format!("{:x}", digest.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_embedded_release_asset_matches_its_pinned_hash() {
        verify_embedded_assets().unwrap();
        assert_eq!(embedded_asset_hashes().len(), ASSETS.len());
        let manifest: Value = serde_json::from_slice(BUNDLE_MANIFEST.bytes).unwrap();
        assert_eq!(manifest["source_sha256"], embedded_frontend_source_hash());
    }

    #[test]
    fn production_materialization_is_idempotent_and_removes_only_tracked_stale_assets() {
        let directory = tempfile::tempdir().unwrap();
        let output = directory.path().join("web");
        std::fs::create_dir_all(&output).unwrap();
        std::fs::write(output.join("user.txt"), "keep").unwrap();
        std::fs::write(output.join("old-generated.js"), "old").unwrap();
        std::fs::write(
            output.join(".mxrs-assets.json"),
            "[\"old-generated.js\",\".mxrs-assets.json\"]\n",
        )
        .unwrap();
        let manifest = json!({ "format": 1, "project": { "name": "Demo" } });

        let first = materialize_manifest(&manifest, &output).unwrap();
        assert_eq!(first.removed_stale_files, 1);
        assert!(first.changed_files > ASSETS.len());
        assert_eq!(
            std::fs::read_to_string(output.join("user.txt")).unwrap(),
            "keep"
        );
        assert!(!output.join("old-generated.js").exists());

        let second = materialize_manifest(&manifest, &output).unwrap();
        assert_eq!(second.changed_files, 0);
        assert_eq!(second.unchanged_files, ASSETS.len() + 2);
    }

    #[test]
    fn frontend_sources_include_the_exact_lockfile_and_are_idempotent() {
        let directory = tempfile::tempdir().unwrap();
        let first = materialize_frontend_sources(directory.path()).unwrap();
        assert_eq!(first.changed_files, SOURCE_FILES.len());
        assert_eq!(
            std::fs::read(directory.path().join("package-lock.json")).unwrap(),
            include_bytes!("../frontend/package-lock.json")
        );
        let second = materialize_frontend_sources(directory.path()).unwrap();
        assert_eq!(second.changed_files, 0);
        assert_eq!(second.unchanged_files, SOURCE_FILES.len());
    }

    #[test]
    fn mpr_manifest_and_shell_include_domain_page_widget_and_flow_contracts() {
        let directory = tempfile::tempdir().unwrap();
        let mpr = directory.path().join("Demo.mpr");
        let web = directory.path().join("web");
        let mut builder = mxrs_dsl::ProjectBuilder::new("11.12.1");
        builder.module("Sales", |module| {
            module.entity("Order", |entity| {
                entity.string("Number");
            });
            module.microflow("ACT_Load", |_flow| {});
            module.page("Home", |page| {
                page.layout("Atlas_Core.ApplicationLayout", "Main");
                page.text_with("Welcome", |text| {
                    text.name("welcomeText");
                });
            });
        });
        mxrs_writer::write_project(&mpr, &builder.build()).unwrap();

        let report = materialize_mpr(&mpr, &web).unwrap();
        assert!(report.changed_files > ASSETS.len());
        let model: Value =
            serde_json::from_slice(&std::fs::read(web.join("model.json")).unwrap()).unwrap();
        assert_eq!(model["project"]["name"], "Demo");
        assert_eq!(model["modules"][0]["entities"][0]["name"], "Sales.Order");
        assert_eq!(model["modules"][0]["microflows"][0], "ACT_Load");
        assert_eq!(
            model["modules"][0]["pages"][0]["qualified_name"],
            "Sales.Home"
        );
        assert_eq!(
            model["modules"][0]["pages"][0]["widgets"][0]["type"],
            "text"
        );
        assert!(web.join("index.html").is_file());
        assert!(web.join("app-C3Of9HqT.js").is_file());
    }
}
