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

use mxrs_ir::NavigationDecl;

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
    Ok(declared)
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
