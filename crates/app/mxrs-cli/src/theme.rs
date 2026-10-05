//! The project's theme, compiled the way Studio Pro compiles it: the Sass
//! every module publishes under `assets/themesource/<module>/web/` — Atlas
//! Core's first, which the others build on — and then the project's own
//! `assets/theme/web/main.scss`, into the one stylesheet a page loads,
//! `assets/theme-cache/web/theme.compiled.css`.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// What compiling a project's theme did.
#[derive(Debug, PartialEq, Eq)]
pub enum Theme {
    /// The project has no Sass to compile.
    Absent,
    /// The stylesheet is newer than every source it is compiled from.
    Current(PathBuf),
    Compiled {
        stylesheet: PathBuf,
        bytes: usize,
    },
}

/// The module whose theme every other one is written against.
const BASE_MODULE: &str = "atlas_core";

/// The Sass files a theme is compiled from, in the order they are read:
/// each as it is named from the stylesheet's own folder.
fn entries(assets: &Path) -> Vec<String> {
    let mut entries = Vec::new();
    let mut modules: Vec<String> = std::fs::read_dir(assets.join("themesource"))
        .into_iter()
        .flatten()
        .flatten()
        .filter(|entry| entry.path().join("web/main.scss").is_file())
        .filter_map(|entry| entry.file_name().into_string().ok())
        .collect();
    modules.sort_by_key(|module| (module != BASE_MODULE, module.clone()));
    for module in modules {
        entries.push(format!("../../themesource/{module}/web/main"));
    }
    if assets.join("theme/web/main.scss").is_file() {
        entries.push("../../theme/web/main".to_string());
    }
    entries
}

/// When the newest Sass file under `folder` was written.
fn newest(folder: &Path, latest: &mut Option<SystemTime>) {
    for entry in std::fs::read_dir(folder).into_iter().flatten().flatten() {
        let path = entry.path();
        if path.is_dir() {
            newest(&path, latest);
        } else if path
            .extension()
            .is_some_and(|extension| extension == "scss")
            && let Ok(modified) = entry.metadata().and_then(|metadata| metadata.modified())
            && latest.is_none_or(|known| modified > known)
        {
            *latest = Some(modified);
        }
    }
}

/// Compiles the theme of the project at `root`, when it has one and its
/// stylesheet is older than what it is compiled from.
pub fn compile(root: &Path) -> Result<Theme, String> {
    let assets = root.join("assets");
    let entries = entries(&assets);
    if entries.is_empty() {
        return Ok(Theme::Absent);
    }
    let folder = assets.join("theme-cache/web");
    let stylesheet = folder.join("theme.compiled.css");
    let mut latest = None;
    newest(&assets.join("themesource"), &mut latest);
    newest(&assets.join("theme"), &mut latest);
    let compiled = std::fs::metadata(&stylesheet)
        .and_then(|metadata| metadata.modified())
        .ok();
    if let (Some(compiled), Some(latest)) = (compiled, latest)
        && compiled >= latest
    {
        return Ok(Theme::Current(stylesheet));
    }
    let source: String = entries
        .iter()
        .map(|entry| format!("@import \"{entry}\";\n"))
        .collect();
    std::fs::create_dir_all(&folder).map_err(|error| format!("{}: {error}", folder.display()))?;
    // Read from the stylesheet's own folder, so a `url(...)` and an
    // `@import` mean what they mean to Studio Pro.
    let options = grass::Options::default()
        .style(grass::OutputStyle::Expanded)
        .load_path(&folder);
    let css = grass::from_string(source, &options)
        .map_err(|error| format!("the theme does not compile: {error}"))?;
    std::fs::write(&stylesheet, &css)
        .map_err(|error| format!("{}: {error}", stylesheet.display()))?;
    Ok(Theme::Compiled {
        stylesheet,
        bytes: css.len(),
    })
}

/// Compiles the theme and says so; a theme that does not compile is a
/// warning — the model is built all the same.
pub fn compile_and_report(root: &Path) {
    match compile(root) {
        Ok(Theme::Compiled { stylesheet, bytes }) => {
            println!(
                "[mxrs] compiled theme {} ({bytes} bytes)",
                stylesheet.display()
            );
        }
        Ok(Theme::Absent | Theme::Current(_)) => {}
        Err(error) => eprintln!("[mxrs] warning: {error}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_theme_is_compiled_base_first_and_only_when_its_sources_are_newer() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        assert_eq!(compile(root).unwrap(), Theme::Absent);
        let write = |relative: &str, text: &str| {
            let path = root.join("assets").join(relative);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        };
        write(
            "themesource/atlas_core/web/main.scss",
            "$brand: #087f8c;\n.btn { color: $brand; }\n",
        );
        write("themesource/aaa/web/main.scss", ".aaa { color: $brand; }\n");
        write("theme/web/custom-variables.scss", "$gap: 4px;\n");
        write(
            "theme/web/main.scss",
            "@import \"custom-variables\";\n.mine { margin: $gap * 2; }\n",
        );
        let Theme::Compiled { stylesheet, .. } = compile(root).unwrap() else {
            panic!("the theme was not compiled");
        };
        let css = std::fs::read_to_string(&stylesheet).unwrap();
        // The base module first, though another sorts before it; the
        // project's own last.
        let at = |needle: &str| {
            css.find(needle)
                .unwrap_or_else(|| panic!("{needle}\n{css}"))
        };
        assert!(at(".btn") < at(".aaa") && at(".aaa") < at(".mine"), "{css}");
        assert!(
            css.contains("color: #087f8c") && css.contains("margin: 8px"),
            "{css}"
        );
        assert_eq!(compile(root).unwrap(), Theme::Current(stylesheet));
        write("themesource/aaa/web/main.scss", ".aaa { color: $nope; }\n");
        std::fs::remove_file(root.join("assets/theme-cache/web/theme.compiled.css")).unwrap();
        assert!(compile(root).unwrap_err().contains("does not compile"));
    }
}
