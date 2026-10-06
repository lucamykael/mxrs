//! Pages from the page templates a project installed from the Marketplace
//! — Atlas Web Content's, the ones Studio Pro offers — opt-in only: `mxrs
//! page new Module.Page --template <Template>` names one. Without
//! `--template`, nothing of them is applied; the layout a page lands in is
//! decided apart, and is not a template.
//!
//! A template is copied the way Studio Pro copies it: its widgets become
//! the page's, in the placeholder of the layout the page is shown in. The
//! page is then stated as TSX like any scaffolded page.

use std::path::{Path, PathBuf};

use mxrs_bson::{Bson, Document, build_array, parse_array};
use mxrs_ir::page::{LayoutRef, PageDecl};

use crate::templates::humanize;
use crate::{Result, ScaffoldError};

/// A page template the project's imported model holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstalledTemplate {
    /// The module it belongs to, `Atlas_Web_Content` for Atlas's.
    pub module: String,
    pub name: String,
    /// The category the module files it under, `Grids` for Atlas's `Grid`.
    pub category: String,
    /// Its stored document, under the imported model.
    pub file: PathBuf,
}

impl InstalledTemplate {
    pub fn qualified_name(&self) -> String {
        format!("{}.{}", self.module, self.name)
    }
}

const PAGE_TEMPLATE: &str = "Forms$PageTemplate";
const MODULE_TYPES: [&str; 2] = ["Projects$Module", "Projects$ModuleImpl"];

fn invalid(path: &Path, reason: impl ToString) -> ScaffoldError {
    ScaffoldError::InvalidProjectSource {
        path: path.display().to_string(),
        reason: reason.to_string(),
    }
}

/// Where the project keeps its imported model: what `mxrs.toml` says, or
/// `model/imported`, where an import writes it.
fn snapshot(root: &Path) -> PathBuf {
    let manifest = root.join("mxrs.toml");
    let declared = std::fs::read_to_string(&manifest)
        .ok()
        .and_then(|text| {
            text.lines().find_map(|line| {
                let (key, value) = line.split_once('=')?;
                (key.trim() == "imported_snapshot")
                    .then(|| value.trim().trim_matches('"').to_string())
            })
        })
        .unwrap_or_else(|| "model/imported".to_string());
    root.join(declared)
}

/// Every page template of the project's imported model, by module and
/// name — none when the project imported nothing.
pub fn installed(root: &Path) -> Result<Vec<InstalledTemplate>> {
    let snapshot = snapshot(root);
    let manifest = snapshot.join("manifest.json");
    let Ok(text) = std::fs::read_to_string(&manifest) else {
        return Ok(Vec::new());
    };
    let parsed: serde_json::Value =
        serde_json::from_str(&text).map_err(|error| invalid(&manifest, error))?;
    let units = parsed["units"]
        .as_array()
        .ok_or_else(|| invalid(&manifest, "it lists no units"))?;
    let field = |unit: &serde_json::Value, key: &str| -> String {
        unit[key].as_str().unwrap_or_default().to_string()
    };
    let by_id: std::collections::HashMap<String, (String, String, String)> = units
        .iter()
        .map(|unit| {
            (
                field(unit, "unit_id"),
                (
                    field(unit, "native_type"),
                    field(unit, "name"),
                    field(unit, "container_id"),
                ),
            )
        })
        .collect();
    // The module a unit is in: the first module up its containers.
    let module_of = |unit: &serde_json::Value| -> Option<String> {
        let mut container = field(unit, "container_id");
        for _ in 0..64 {
            let (ty, name, parent) = by_id.get(&container)?;
            if MODULE_TYPES.contains(&ty.as_str()) {
                return Some(name.clone());
            }
            container = parent.clone();
        }
        None
    };
    let mut templates = Vec::new();
    for unit in units {
        if field(unit, "native_type") != PAGE_TEMPLATE {
            continue;
        }
        let Some(module) = module_of(unit) else {
            continue;
        };
        let file = snapshot.join(field(unit, "file"));
        let document = read(&file)?;
        templates.push(InstalledTemplate {
            module,
            name: field(unit, "name"),
            category: document
                .get_str("TemplateCategory")
                .unwrap_or_default()
                .to_string(),
            file,
        });
    }
    templates.sort_by(|left, right| {
        (&left.module, &left.category, &left.name).cmp(&(
            &right.module,
            &right.category,
            &right.name,
        ))
    });
    Ok(templates)
}

/// The installed template `name` names: its own name (`Grid`), or its
/// qualified one (`Atlas_Web_Content.Grid`) when two modules share it.
pub fn find(root: &Path, name: &str) -> Result<Option<InstalledTemplate>> {
    let templates = installed(root)?;
    let matching: Vec<&InstalledTemplate> = templates
        .iter()
        .filter(|template| template.name == name || template.qualified_name() == name)
        .collect();
    match matching.as_slice() {
        [] => Ok(None),
        [one] => Ok(Some((*one).clone())),
        many => Err(ScaffoldError::InvalidProjectSource {
            path: name.to_string(),
            reason: format!(
                "names a page template of more than one installed module; name it as one of {}",
                many.iter()
                    .map(|template| template.qualified_name())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        }),
    }
}

/// The installed templates by module, then by category, as names.
pub fn grouped(templates: &[InstalledTemplate]) -> Vec<(String, String, Vec<String>)> {
    let mut groups: Vec<(String, String, Vec<String>)> = Vec::new();
    for template in templates {
        match groups.iter_mut().find(|(module, category, _)| {
            module == &template.module && category == &template.category
        }) {
            Some((_, _, names)) => names.push(template.name.clone()),
            None => groups.push((
                template.module.clone(),
                template.category.clone(),
                vec![template.name.clone()],
            )),
        }
    }
    groups
}

/// The same box-drawing tree `mxrs page templates` draws for mxrs's own:
/// one per module, its categories, their templates.
pub fn tree(templates: &[InstalledTemplate]) -> String {
    let mut lines = Vec::new();
    let groups = grouped(templates);
    let modules: Vec<&str> = {
        let mut seen = Vec::new();
        for (module, _, _) in &groups {
            if !seen.contains(&module.as_str()) {
                seen.push(module.as_str());
            }
        }
        seen
    };
    for module in modules {
        lines.push(format!("Page templates of {module}"));
        let categories: Vec<&(String, String, Vec<String>)> =
            groups.iter().filter(|(of, _, _)| of == module).collect();
        for (index, (_, category, names)) in categories.iter().enumerate() {
            let last_category = index == categories.len() - 1;
            lines.push(format!(
                "{} {category}",
                if last_category {
                    "└──"
                } else {
                    "├──"
                }
            ));
            let indent = if last_category { "    " } else { "│   " };
            for (index, name) in names.iter().enumerate() {
                let last = index == names.len() - 1;
                lines.push(format!(
                    "{indent}{} {name}",
                    if last { "└──" } else { "├──" }
                ));
            }
        }
    }
    let mut text = lines.join("\n");
    text.push('\n');
    text
}

fn read(file: &Path) -> Result<Document> {
    let bytes = std::fs::read(file).map_err(|source| ScaffoldError::Io {
        path: file.display().to_string(),
        source,
    })?;
    mxrs_bson::parse(&bytes).map_err(|error| invalid(file, error))
}

/// The document of page `name`, shown in `layout`, holding what `template`
/// holds: the page Studio Pro makes from it. With it, what the template
/// holds that the page cannot: widgets for a placeholder the layout does
/// not have.
pub(crate) fn page_document(
    template: &InstalledTemplate,
    version: &str,
    layout: &LayoutRef,
    name: &str,
    roles: &[String],
) -> Result<(Document, Vec<String>)> {
    let stored = read(&template.file)?;
    let mut decl = PageDecl::new(name);
    let display_name = stored.get_str("DisplayName").unwrap_or_default();
    decl.title = Some(if display_name.is_empty() {
        humanize(name)
    } else {
        display_name.to_string()
    });
    decl.layout = Some(layout.clone());
    decl.allowed_module_roles = roles.to_vec();
    let mut page = crate::forms::page_document(version, &decl)?;

    // The template's widgets, by the placeholder they are for.
    let mut arguments: Vec<(String, Bson)> = Vec::new();
    if let Ok(call) = stored.get_document("LayoutCall") {
        let items = parse_array(call.get_array("Arguments").ok().map(Vec::as_slice)).items;
        for item in items {
            let Some(argument) = item.as_document() else {
                continue;
            };
            let parameter = argument
                .get_str("Parameter")
                .unwrap_or_default()
                .to_string();
            let widgets = argument
                .get("Widgets")
                .cloned()
                .unwrap_or_else(|| Bson::Array(build_array(Vec::new(), 2)));
            arguments.push((parameter, widgets));
        }
    }
    let mut notes = Vec::new();
    // Studio Pro's templates fill one placeholder, `Main`; a template for a
    // split layout fills more, and only the one the page's layout has — the
    // one named like the page's — is copied.
    let local = |parameter: &str| {
        parameter
            .rsplit('.')
            .next()
            .unwrap_or(parameter)
            .to_string()
    };
    let wanted = local(&layout.parameter);
    let chosen = arguments
        .iter()
        .position(|(parameter, _)| local(parameter) == wanted)
        .or_else(|| (!arguments.is_empty()).then_some(0));
    for (index, (parameter, widgets)) in arguments.iter().enumerate() {
        if Some(index) == chosen {
            continue;
        }
        let count = parse_array(widgets.as_array().map(Vec::as_slice))
            .items
            .len();
        notes.push(format!(
            "{}: {count} widget(s) for {parameter} are not on the page: {} has no such placeholder",
            template.qualified_name(),
            layout.qualified_name
        ));
    }
    if let Some(index) = chosen {
        let widgets = arguments[index].1.clone();
        let call = page
            .get_document_mut("FormCall")
            .map_err(|error| invalid(&template.file, format!("the page's layout call: {error}")))?;
        let mut items = parse_array(call.get_array("Arguments").ok().map(Vec::as_slice));
        let Some(Bson::Document(argument)) = items.items.first_mut() else {
            return Err(invalid(
                &template.file,
                "the page's layout call has no argument to hold the template's widgets",
            ));
        };
        argument.insert("Widgets", widgets);
        let marker = items.marker;
        call.insert("Arguments", build_array(items.items, marker));
    }
    // What the template says of the page itself.
    if let Ok(appearance) = stored.get_document("Appearance") {
        let styled = ["Class", "Style", "DynamicClasses"]
            .iter()
            .any(|key| !appearance.get_str(key).unwrap_or_default().is_empty())
            || !parse_array(
                appearance
                    .get_array("DesignProperties")
                    .ok()
                    .map(Vec::as_slice),
            )
            .items
            .is_empty();
        if styled {
            page.insert("Appearance", Bson::Document(appearance.clone()));
        }
    }
    Ok((page, notes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::forms::PageLayout;

    /// A page template whose widgets are those of a scaffolded page: the
    /// shape Studio Pro stores, with a module's own content.
    fn template_document(name: &str, category: &str, arguments: Vec<(&str, Bson)>) -> Document {
        let mut call = Document::new();
        call.insert("$Type", "Forms$LayoutCall");
        let arguments: Vec<Bson> = arguments
            .into_iter()
            .map(|(parameter, widgets)| {
                let mut argument = Document::new();
                argument.insert("$Type", "Forms$FormCallArgument");
                argument.insert("Parameter", parameter);
                argument.insert("Widgets", widgets);
                Bson::Document(argument)
            })
            .collect();
        call.insert("Arguments", build_array(arguments, 2));
        let mut template = Document::new();
        template.insert("$Type", PAGE_TEMPLATE);
        template.insert("Name", name);
        template.insert("DisplayName", "");
        template.insert("TemplateCategory", category);
        template.insert("LayoutCall", call);
        template
    }

    /// The widgets of a page scaffolded with `title`.
    fn seed_widgets(title: &str) -> Bson {
        let page = crate::forms::page_document(
            "11.12.1",
            &crate::forms::plain_page(&PageLayout::of_module("Seeds", "Main"), title, &[]),
        )
        .unwrap();
        let call = page.get_document("FormCall").unwrap();
        let argument = parse_array(call.get_array("Arguments").ok().map(Vec::as_slice)).items;
        argument[0]
            .as_document()
            .unwrap()
            .get("Widgets")
            .cloned()
            .unwrap()
    }

    fn write_snapshot(root: &Path, templates: &[(&str, &Document)]) {
        let units = root.join("model/imported/units");
        std::fs::create_dir_all(&units).unwrap();
        let mut listed = vec![
            serde_json::json!({"unit_id": "root", "container_id": "", "containment_name": "", "native_type": "Projects$Project", "name": null, "file": "units/root.mxdoc"}),
            serde_json::json!({"unit_id": "m1", "container_id": "root", "containment_name": "Modules", "native_type": "Projects$ModuleImpl", "name": "Atlas_Web_Content", "file": "units/m1.mxdoc"}),
            serde_json::json!({"unit_id": "m2", "container_id": "root", "containment_name": "Modules", "native_type": "Projects$Module", "name": "Other", "file": "units/m2.mxdoc"}),
            serde_json::json!({"unit_id": "f1", "container_id": "m1", "containment_name": "Folders", "native_type": "Projects$Folder", "name": "Grids", "file": "units/f1.mxdoc"}),
            serde_json::json!({"unit_id": "p1", "container_id": "f1", "containment_name": "Documents", "native_type": "Forms$Page", "name": "NotATemplate", "file": "units/p1.mxdoc"}),
        ];
        for (index, (module, template)) in templates.iter().enumerate() {
            let id = format!("t{index}");
            let container = if *module == "Other" { "m2" } else { "f1" };
            std::fs::write(
                units.join(format!("{id}.mxdoc")),
                mxrs_bson::serialize(template).unwrap(),
            )
            .unwrap();
            listed.push(serde_json::json!({
                "unit_id": id, "container_id": container, "containment_name": "Documents",
                "native_type": PAGE_TEMPLATE, "name": template.get_str("Name").unwrap(),
                "file": format!("units/{id}.mxdoc"),
            }));
        }
        std::fs::write(
            root.join("model/imported/manifest.json"),
            serde_json::to_string_pretty(
                &serde_json::json!({"snapshot_version": 2, "units": listed}),
            )
            .unwrap(),
        )
        .unwrap();
    }

    #[test]
    fn the_installed_templates_are_listed_by_module_and_category_and_found_by_name() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        assert!(installed(root).unwrap().is_empty());
        assert!(find(root, "Grid").unwrap().is_none());
        let grid = template_document(
            "Grid",
            "Grids",
            vec![("Atlas_Core.Atlas_TopBar.Main", seed_widgets("Grid"))],
        );
        let blank = template_document("Blank", "Blank", vec![]);
        write_snapshot(
            root,
            &[
                ("Atlas_Web_Content", &grid),
                ("Atlas_Web_Content", &blank),
                ("Other", &grid),
            ],
        );
        let listed = installed(root).unwrap();
        assert_eq!(
            listed
                .iter()
                .map(|template| (
                    template.module.as_str(),
                    template.category.as_str(),
                    template.name.as_str()
                ))
                .collect::<Vec<_>>(),
            vec![
                ("Atlas_Web_Content", "Blank", "Blank"),
                ("Atlas_Web_Content", "Grids", "Grid"),
                ("Other", "Grids", "Grid"),
            ]
        );
        assert_eq!(
            find(root, "Blank").unwrap().unwrap().qualified_name(),
            "Atlas_Web_Content.Blank"
        );
        assert_eq!(find(root, "Other.Grid").unwrap().unwrap().module, "Other");
        let ambiguous = find(root, "Grid").unwrap_err().to_string();
        assert!(
            ambiguous.contains("Atlas_Web_Content.Grid, Other.Grid"),
            "{ambiguous}"
        );
        assert!(find(root, "Nope").unwrap().is_none());
        let tree = tree(&listed);
        assert!(
            tree.starts_with("Page templates of Atlas_Web_Content\n"),
            "{tree}"
        );
        assert!(tree.contains("Page templates of Other\n"), "{tree}");
    }

    #[test]
    fn a_page_from_a_template_holds_its_widgets_in_the_layouts_placeholder() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let widgets = seed_widgets("Seeded");
        let split = template_document(
            "Split",
            "Forms",
            vec![
                ("Atlas_Core.Tablet_Split_Left.Left", seed_widgets("Aside")),
                ("Atlas_Core.Tablet_Split_Left.Main", widgets.clone()),
            ],
        );
        write_snapshot(root, &[("Atlas_Web_Content", &split)]);
        let template = find(root, "Split").unwrap().unwrap();
        let layout = LayoutRef::new("Sales.ApplicationLayout", "Main");
        let (page, notes) = page_document(
            &template,
            "11.12.1",
            &layout,
            "Orders",
            &["Sales.Clerk".to_string()],
        )
        .unwrap();
        assert_eq!(page.get_str("$Type").unwrap(), "Forms$Page");
        assert_eq!(page.get_str("Name").unwrap(), "Orders");
        let call = page.get_document("FormCall").unwrap();
        assert_eq!(call.get_str("Form").unwrap(), "Sales.ApplicationLayout");
        let arguments = parse_array(call.get_array("Arguments").ok().map(Vec::as_slice)).items;
        assert_eq!(arguments.len(), 1);
        let argument = arguments[0].as_document().unwrap();
        assert_eq!(
            argument.get_str("Parameter").unwrap(),
            "Sales.ApplicationLayout.Main"
        );
        assert_eq!(argument.get("Widgets").unwrap(), &widgets);
        assert!(page.get("Appearance").is_none());
        assert_eq!(notes.len(), 1);
        assert!(
            notes[0].contains("Atlas_Core.Tablet_Split_Left.Left")
                && notes[0].contains("Sales.ApplicationLayout"),
            "{}",
            notes[0]
        );
        // What the template says of the page itself is kept when it says something.
        let mut styled = template_document(
            "Styled",
            "Forms",
            vec![("Atlas_Core.Atlas_TopBar.Main", widgets.clone())],
        );
        let mut appearance = Document::new();
        appearance.insert("$Type", "Forms$Appearance");
        appearance.insert("Class", "login-page");
        styled.insert("Appearance", appearance.clone());
        write_snapshot(root, &[("Atlas_Web_Content", &styled)]);
        let template = find(root, "Styled").unwrap().unwrap();
        let (page, notes) = page_document(&template, "11.12.1", &layout, "Login", &[]).unwrap();
        assert!(notes.is_empty());
        assert_eq!(page.get_document("Appearance").unwrap(), &appearance);
    }
}
