//! The user interface a project declares in its frontend.
//!
//! A project's user interface belongs to its frontend: the React +
//! TypeScript application in `frontend/`. Its declaration files are
//! TypeScript that the frontend's own build type-checks, and that this
//! crate reads — without running them — into the same declarations the
//! Rust authoring surface produces, so a build writes them into the model.
//!
//! | file | declares |
//! |---|---|
//! | `src/navigation/index.ts` | the navigation profiles |
//! | `src/services/**/*.ts` | nanoflows, as methods of `nanoflowService(...)` |
//! | `src/mxrs/elements.ts`, `src/widgets/*.tsx` | what pages are written with ([`forms`]) |
//! | `src/pages/<module>/*.tsx` | pages |
//! | `src/components/layout/<module>/*.tsx` | layouts |
//! | `src/components/snippets/<module>/*.tsx` | snippets |

pub mod forms;
mod literal;
pub mod naming;
mod nanoflows;
mod navigation;

use std::collections::HashMap;
use std::path::Path;

/// The forms a frontend declares, each with its module.
type Forms = Vec<(String, mxrs_ir::FormDecl)>;

use mxrs_ir::NavigationDecl;

/// What a frontend declares.
#[derive(Debug, Default)]
pub struct FrontendDecl {
    pub navigation: Option<NavigationDecl>,
    /// Each nanoflow its services declare, with its module.
    pub nanoflows: Vec<(String, mxrs_ir::flow::MicroflowDecl)>,
    /// Where each of those nanoflows is declared — `file:line` — by its
    /// qualified name.
    pub nanoflow_origins: HashMap<String, String>,
    /// Each page, layout and snippet its TSX declares, with its module.
    pub forms: Vec<(String, mxrs_ir::FormDecl)>,
    /// The file that declares each of those forms, by its type and
    /// qualified name: `Forms$Page Sales.Orders`.
    pub form_origins: HashMap<String, String>,
    /// The folder of its module each form and nanoflow declared in one
    /// lives in — `Orders/Admin` — by its qualified name. One this does not
    /// name is at its module's root.
    pub folders: HashMap<String, String>,
}

#[derive(Debug, thiserror::Error)]
pub enum FrontendError {
    #[error("{path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("{path}: {detail}")]
    Syntax { path: String, detail: String },
    #[error("{path}:{line}: expected {expected}, found `{found}`")]
    Unsupported {
        path: String,
        line: usize,
        expected: String,
        found: String,
    },
    /// Data that is not what the file declares, at a line of it.
    #[error("{path}:{line}: {detail}")]
    Shape {
        path: String,
        line: usize,
        detail: String,
    },
    #[error("the navigation is declared twice: in Rust with #[navigation] and in {0}; keep one")]
    Duplicate(String),
}

/// A file TypeScript itself cannot read, at the line that says why.
pub(crate) fn syntax_error(
    path: &str,
    source: &str,
    error: &oxc_diagnostics::OxcDiagnostic,
) -> FrontendError {
    let offset = error
        .labels
        .first()
        .map(|label| (label.offset() as usize).min(source.len()));
    match offset {
        Some(offset) => FrontendError::Shape {
            path: path.to_string(),
            line: source[..offset].matches('\n').count() + 1,
            detail: error.to_string(),
        },
        None => FrontendError::Syntax {
            path: path.to_string(),
            detail: error.to_string(),
        },
    }
}

/// Reads what the frontend rooted at `frontend` declares.
pub fn read_frontend(frontend: impl AsRef<Path>) -> Result<FrontendDecl, FrontendError> {
    let source = frontend.as_ref().join("src");
    let mut declared = FrontendDecl::default();
    let navigation = source.join("navigation/index.ts");
    if navigation.is_file() {
        let file = literal::read_default_export(&navigation)?;
        let declaration =
            navigation::navigation(&file.value).map_err(|(at, detail)| FrontendError::Shape {
                path: navigation.display().to_string(),
                line: file.line(&at).unwrap_or(1),
                detail,
            })?;
        declared.navigation = Some(declaration);
    }
    let services = source.join("services");
    if services.is_dir() {
        let read = nanoflows::read_with_origins(&services)?;
        (declared.nanoflows, declared.nanoflow_origins) = (read.nanoflows, read.origins);
        declared.folders.extend(read.folders);
    }
    let read = read_forms(&source)?;
    (declared.forms, declared.form_origins) = (read.forms, read.origins);
    declared.folders.extend(read.folders);
    Ok(declared)
}

/// The `.tsx` files directly in `folder`, in the order of their names.
fn tsx_files(folder: &Path) -> Result<Vec<std::path::PathBuf>, FrontendError> {
    let io = |source| FrontendError::Io {
        path: folder.display().to_string(),
        source,
    };
    let mut files = Vec::new();
    for entry in std::fs::read_dir(folder).map_err(io)? {
        let path = entry.map_err(io)?.path();
        if path.is_file() && path.extension().is_some_and(|extension| extension == "tsx") {
            files.push(path);
        }
    }
    files.sort();
    Ok(files)
}

fn read_source(path: &Path) -> Result<String, FrontendError> {
    std::fs::read_to_string(path).map_err(|source| FrontendError::Io {
        path: path.display().to_string(),
        source,
    })
}

/// The forms a frontend's TSX declares, the file declaring each and the
/// folder of its module it lives in.
#[derive(Default)]
struct ReadForms {
    forms: Forms,
    origins: HashMap<String, String>,
    folders: HashMap<String, String>,
}

/// The `.tsx` files of a module's folder and of the folders inside it, each
/// with the path of the folder it is in: the module's own folder of the
/// same path (`Orders/Admin`), empty at its root.
fn module_tsx_files(module: &Path) -> Result<Vec<(String, std::path::PathBuf)>, FrontendError> {
    let mut found = Vec::new();
    let mut pending = vec![(String::new(), module.to_path_buf())];
    while let Some((path, folder)) = pending.pop() {
        found.extend(
            tsx_files(&folder)?
                .into_iter()
                .map(|file| (path.clone(), file)),
        );
        let io = |source| FrontendError::Io {
            path: folder.display().to_string(),
            source,
        };
        for entry in std::fs::read_dir(&folder).map_err(io)? {
            let nested = entry.map_err(io)?.path();
            // A link is not a folder of the module's: following one could
            // read a form twice, or forever.
            if !nested.is_dir() || nested.is_symlink() {
                continue;
            }
            let name = nested
                .file_name()
                .map(|name| name.to_string_lossy().to_string())
                .unwrap_or_default();
            pending.push((
                if path.is_empty() {
                    name
                } else {
                    format!("{path}/{name}")
                },
                nested,
            ));
        }
    }
    found.sort_by(|left, right| left.1.cmp(&right.1));
    Ok(found)
}

/// The pages, layouts, snippets, page templates and building blocks the
/// frontend's TSX declares: each file of a module's folder under `pages/`,
/// `components/layout/`, `components/snippets/`, `templates/pages/` and
/// `templates/blocks/` — a folder inside it being the module's folder of
/// that name — read with the elements `mxrs/elements.ts` declares and the
/// widgets `widgets/` defines.
fn read_forms(source: &Path) -> Result<ReadForms, FrontendError> {
    let mut origins = HashMap::new();
    let mut folders = HashMap::new();
    let mut files = Vec::new();
    for ty in [
        "Forms$Layout",
        "Forms$Snippet",
        "Forms$Page",
        "Forms$PageTemplate",
        "Forms$BuildingBlock",
    ] {
        let folder = source.join(forms::folder(ty).expect("a form has a folder"));
        if !folder.is_dir() {
            continue;
        }
        let io = |source| FrontendError::Io {
            path: folder.display().to_string(),
            source,
        };
        let mut modules = Vec::new();
        for entry in std::fs::read_dir(&folder).map_err(io)? {
            let path = entry.map_err(io)?.path();
            if path.is_dir() {
                modules.push(path);
            }
        }
        modules.sort();
        for module in modules {
            let module_folder = module
                .file_name()
                .map(|folder| folder.to_string_lossy().to_string())
                .unwrap_or_default();
            files.extend(
                module_tsx_files(&module)?
                    .into_iter()
                    .map(|(folder, file)| (ty, module_folder.clone(), folder, file)),
            );
        }
    }
    let elements = source.join("mxrs/elements.ts");
    if files.is_empty() {
        return Ok(ReadForms::default());
    }
    if !elements.is_file() {
        return Err(FrontendError::Syntax {
            path: elements.display().to_string(),
            detail: "it is missing, and the pages are written with the elements it declares"
                .to_string(),
        });
    }
    let shapes = forms::read_elements(&read_source(&elements)?, &elements.display().to_string())?;
    let mut widgets = Vec::new();
    let folder = source.join(forms::WIDGETS_FOLDER);
    if folder.is_dir() {
        for file in tsx_files(&folder)? {
            let path = file.display().to_string();
            let widget = forms::read_widget(&read_source(&file)?, &path, &shapes)?;
            if file.file_stem().and_then(|stem| stem.to_str()) != Some(widget.name.as_str()) {
                return Err(FrontendError::Syntax {
                    path,
                    detail: format!("it declares {}, which is not its own name", widget.name),
                });
            }
            widgets.push(widget);
        }
    }
    let vocabulary = forms::Vocabulary { shapes, widgets };
    let mut declared: Vec<(String, mxrs_ir::FormDecl)> = Vec::new();
    for (ty, folder, inner, file) in files {
        let path = file.display().to_string();
        let (module, form) = forms::read_form(&read_source(&file)?, &path, &vocabulary)?;
        if form.kind() != ty {
            return Err(FrontendError::Syntax {
                path,
                detail: format!(
                    "it declares a {}, whose place is src/{}",
                    form.kind(),
                    forms::folder(form.kind()).unwrap_or_default()
                ),
            });
        }
        if declared.iter().any(|(other, known)| {
            other == &module && known.kind() == form.kind() && known.name() == form.name()
        }) {
            return Err(FrontendError::Syntax {
                path,
                detail: format!("{module}.{} is declared twice", form.name()),
            });
        }
        // A module named otherwise than its folder is a typo, not a module.
        if !naming::is_module_folder(&folder, &module) {
            return Err(FrontendError::Syntax {
                path,
                detail: format!(
                    "it declares a form of {module:?} in the folder `{folder}`, which is another module's"
                ),
            });
        }
        origins.insert(format!("{} {module}.{}", form.kind(), form.name()), path);
        if !inner.is_empty() {
            folders.insert(format!("{module}.{}", form.name()), inner);
        }
        declared.push((module, form));
    }
    Ok(ReadForms {
        forms: declared,
        origins,
        folders,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frontend(navigation: &str) -> tempfile::TempDir {
        let directory = tempfile::tempdir().unwrap();
        let folder = directory.path().join("src/navigation");
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(folder.join("index.ts"), navigation).unwrap();
        directory
    }

    #[test]
    fn the_frontend_declares_the_navigation() {
        let directory = frontend(
            r#"import type { Navigation } from "@/types/navigation";

export default {
  profiles: [
    {
      name: "Responsive",
      title: { en_US: "Shop" },
      homePage: "Main.Home",
      homes: [{ role: "Anonymous", page: "Main.Login" }],
      items: [
        { caption: "Orders", page: "Sales.Orders", icon: { glyph: "list" } },
        { caption: { en_US: "Admin", nl_NL: "Beheer" }, items: [{ caption: "Users", microflow: "Admin.ACT_Users", icon: { code: 57344 } }] },
      ],
    },
    { name: "Phone", kind: "Phone" },
  ],
} satisfies Navigation;
"#,
        );
        let navigation = read_frontend(directory.path()).unwrap().navigation.unwrap();
        let [responsive, phone] = navigation.profiles.as_slice() else {
            panic!("{navigation:?}");
        };
        assert_eq!(responsive.app_title["en_US"], "Shop");
        assert_eq!(responsive.home_page.as_deref(), Some("Main.Home"));
        assert_eq!(responsive.role_homes[0].user_role, "Anonymous");
        assert_eq!(responsive.items[0].caption["en_US"], "Orders");
        assert_eq!(
            responsive.items[0].icon,
            Some(mxrs_ir::NavigationIconDecl::Glyph("list".into()))
        );
        assert_eq!(responsive.items[1].caption["nl_NL"], "Beheer");
        assert_eq!(
            responsive.items[1].items[0].icon,
            Some(mxrs_ir::NavigationIconDecl::Code(57344))
        );
        assert_eq!(phone.kind, "Phone");
    }

    #[test]
    fn a_misshapen_navigation_is_refused_at_its_line() {
        for (source, line, expected) in [
            (
                "export default {\n  profiles: [{ name: \"R\",\n    items: [{ caption: \"A\", page: \"M.P\", microflow: \"M.F\" }] }],\n};",
                3,
                "one of them",
            ),
            (
                "export default {\n  profiles: [{ name: \"R\",\n    homes: [{ role: \"User\" }] }],\n};",
                3,
                "one of them",
            ),
            (
                "export default {\n  profiles: [\n    { name: \"R\", colour: \"red\" }],\n};",
                3,
                "unknown field `colour`",
            ),
            (
                "export default {\n  profiles: [{ name: \"R\", items: [\n    { caption: 5 }] }],\n};",
                3,
                "a text, or texts by language code",
            ),
            (
                "export default {\n  profiles: [{ name: \"R\", items: [{ caption: \"A\",\n    icon: { code: 1.5 } }] }],\n};",
                3,
                "floating point",
            ),
            (
                "export default {\n  profiles: [{ name: \"R\" },\n    { name: \"R\" }],\n};",
                3,
                "a second profile",
            ),
        ] {
            let directory = frontend(source);
            let error = read_frontend(directory.path()).unwrap_err().to_string();
            assert!(
                error.contains(&format!("index.ts:{line}:")) && error.contains(expected),
                "{source}: {error}"
            );
        }
    }

    #[test]
    fn a_frontend_without_a_navigation_declares_none() {
        let directory = tempfile::tempdir().unwrap();
        assert!(
            read_frontend(directory.path())
                .unwrap()
                .navigation
                .is_none()
        );
    }
}
