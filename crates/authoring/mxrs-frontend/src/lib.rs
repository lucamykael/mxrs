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

mod literal;
mod navigation;

use std::path::Path;

use mxrs_ir::{NavigationDecl, NavigationProfileDecl, ProjectDecl};

/// What a frontend declares.
#[derive(Debug, Default)]
pub struct FrontendDecl {
    pub navigation: Option<NavigationDecl>,
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
    #[error("{path}: {detail}")]
    Shape { path: String, detail: String },
    #[error("the navigation is declared twice: in Rust with #[navigation] and in {0}; keep one")]
    Duplicate(String),
}

/// Reads what the frontend rooted at `frontend` declares.
pub fn read_frontend(frontend: impl AsRef<Path>) -> Result<FrontendDecl, FrontendError> {
    let source = frontend.as_ref().join("src");
    let mut declared = FrontendDecl::default();
    let navigation = source.join("navigation/index.ts");
    if navigation.is_file() {
        let value = literal::read_default_export(&navigation)?;
        declared.navigation =
            Some(
                navigation::navigation(value).map_err(|detail| FrontendError::Shape {
                    path: navigation.display().to_string(),
                    detail,
                })?,
            );
    }
    Ok(declared)
}

/// Adds what the frontend rooted at `frontend` declares to `project`.
///
/// The navigation the frontend declares is the project's; an item a Rust
/// page adds for itself with `#[navigation_item]` joins its profile there.
/// A navigation declared in both places is refused rather than merged.
pub fn merge_frontend(
    project: &mut ProjectDecl,
    frontend: impl AsRef<Path>,
) -> Result<(), FrontendError> {
    let declared = read_frontend(&frontend)?;
    let Some(mut navigation) = declared.navigation else {
        return Ok(());
    };
    for added in project
        .navigation
        .take()
        .map(|rust| rust.profiles)
        .unwrap_or_default()
    {
        let items_only = NavigationProfileDecl {
            items: Vec::new(),
            ..added.clone()
        } == NavigationProfileDecl::new(&added.name);
        if !items_only {
            return Err(FrontendError::Duplicate(
                frontend
                    .as_ref()
                    .join("src/navigation/index.ts")
                    .display()
                    .to_string(),
            ));
        }
        match navigation
            .profiles
            .iter_mut()
            .find(|profile| profile.name == added.name)
        {
            Some(profile) => profile.items.extend(added.items),
            None => navigation.profiles.push(added),
        }
    }
    project.navigation = Some(navigation);
    Ok(())
}

#[cfg(test)]
mod tests {
    use mxrs_ir::NavigationItemDecl;

    use super::*;

    fn frontend(navigation: &str) -> tempfile::TempDir {
        let directory = tempfile::tempdir().unwrap();
        let folder = directory.path().join("src/navigation");
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(folder.join("index.ts"), navigation).unwrap();
        directory
    }

    fn project() -> ProjectDecl {
        ProjectDecl {
            mendix_version: "11.12.1".into(),
            modules: vec![],
            security: None,
            navigation: None,
            demo_users: vec![],
        }
    }

    fn item(caption: &str, page: &str) -> NavigationItemDecl {
        NavigationItemDecl {
            caption: [("en_US".to_string(), caption.to_string())].into(),
            page: Some(page.to_string()),
            ..Default::default()
        }
    }

    #[test]
    fn the_frontend_declares_the_navigation_and_rust_pages_add_their_items() {
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
  ],
} satisfies Navigation;
"#,
        );
        let mut project = project();
        project.navigation_item("Responsive", item("Invoices", "Sales.Invoices"));
        project.navigation_item("Phone", item("Home", "Main.Home"));
        merge_frontend(&mut project, directory.path()).unwrap();
        let navigation = project.navigation.unwrap();
        let [responsive, phone] = navigation.profiles.as_slice() else {
            panic!("{navigation:?}");
        };
        assert_eq!(responsive.app_title["en_US"], "Shop");
        assert_eq!(responsive.home_page.as_deref(), Some("Main.Home"));
        assert_eq!(responsive.role_homes[0].user_role, "Anonymous");
        let captions: Vec<_> = responsive
            .items
            .iter()
            .map(|item| item.caption["en_US"].as_str())
            .collect();
        assert_eq!(captions, ["Orders", "Admin", "Invoices"]);
        assert_eq!(
            responsive.items[0].icon,
            Some(mxrs_ir::NavigationIconDecl::Glyph("list".into()))
        );
        assert_eq!(responsive.items[1].caption["nl_NL"], "Beheer");
        assert_eq!(
            responsive.items[1].items[0].icon,
            Some(mxrs_ir::NavigationIconDecl::Code(57344))
        );
        assert_eq!(phone.items.len(), 1);
    }

    #[test]
    fn a_navigation_declared_twice_or_misshapen_is_refused() {
        let directory = frontend(
            "export default { profiles: [{ name: \"Responsive\", homePage: \"Main.Home\" }] };\n",
        );
        let mut project = project();
        project.navigation = Some(NavigationDecl {
            profiles: vec![NavigationProfileDecl {
                home_page: Some("Main.Other".into()),
                ..NavigationProfileDecl::new("Responsive")
            }],
        });
        let error = merge_frontend(&mut project, directory.path()).unwrap_err();
        assert!(matches!(error, FrontendError::Duplicate(_)), "{error}");

        for (source, expected) in [
            (
                "export default { profiles: [{ name: \"R\", items: [{ caption: \"A\", page: \"M.P\", microflow: \"M.F\" }] }] };",
                "one of them",
            ),
            (
                "export default { profiles: [{ name: \"R\", homes: [{ role: \"User\" }] }] };",
                "one of them",
            ),
            (
                "export default { profiles: [{ name: \"R\", colour: \"red\" }] };",
                "unknown field `colour`",
            ),
        ] {
            let directory = frontend(source);
            let error = read_frontend(directory.path()).unwrap_err().to_string();
            assert!(error.contains(expected), "{source}: {error}");
        }
    }

    #[test]
    fn a_frontend_without_a_navigation_leaves_the_project_alone() {
        let directory = tempfile::tempdir().unwrap();
        let mut project = project();
        project.navigation_item("Responsive", item("Home", "Main.Home"));
        merge_frontend(&mut project, directory.path()).unwrap();
        assert_eq!(project.navigation.unwrap().profiles[0].items.len(), 1);
    }
}
