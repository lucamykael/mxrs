//! The project's theme, compiled the way Studio Pro compiles it: the Sass
//! every module publishes under `assets/themesource/<module>/web/` — Atlas
//! Core's first, which the others build on — and then the project's own
//! `assets/theme/web/main.scss`, into the one stylesheet a page loads,
//! `assets/theme-cache/web/theme.compiled.css` — and published, with the
//! fonts and images beside it and what every module makes public, into
//! the web root a deployment serves.

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

/// What a deployment merges into the site's root beside the shell, the
/// first folder winning: the compiled theme and what the build wrote with
/// it, the project's own `theme/web`, and what every module publishes.
fn roots(assets: &Path) -> Vec<PathBuf> {
    let mut roots = vec![assets.join("theme-cache/web"), assets.join("theme/web")];
    let mut modules: Vec<PathBuf> = std::fs::read_dir(assets.join("themesource"))
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path().join("public"))
        .collect();
    modules.sort();
    roots.extend(modules);
    roots.into_iter().filter(|root| root.is_dir()).collect()
}

/// What a theme is written in, not what a browser asks for.
fn source(path: &Path) -> bool {
    path.extension()
        .is_some_and(|extension| extension == "scss" || extension == "map")
        || path.file_name().is_some_and(|name| name == "settings.json")
}

fn copy_tree(
    from: &Path,
    to: &Path,
    relative: &Path,
    written: &mut Vec<PathBuf>,
    seen: &mut std::collections::BTreeSet<PathBuf>,
) -> Result<(), String> {
    for entry in std::fs::read_dir(from).map_err(|error| format!("{}: {error}", from.display()))? {
        let entry = entry.map_err(|error| format!("{}: {error}", from.display()))?;
        let path = entry.path();
        let inner = relative.join(entry.file_name());
        if path.is_dir() {
            copy_tree(&path, to, &inner, written, seen)?;
        } else if !source(&path) && seen.insert(inner.clone()) {
            let bytes =
                std::fs::read(&path).map_err(|error| format!("{}: {error}", path.display()))?;
            let target = to.join(&inner);
            if std::fs::read(&target).is_ok_and(|current| current == bytes) {
                continue;
            }
            if let Some(folder) = target.parent() {
                std::fs::create_dir_all(folder)
                    .map_err(|error| format!("{}: {error}", folder.display()))?;
            }
            std::fs::write(&target, bytes)
                .map_err(|error| format!("{}: {error}", target.display()))?;
            written.push(target);
        }
    }
    Ok(())
}

/// The stylesheets a page loads, in order, when the build wrote them.
const STYLESHEETS: [&str; 2] = ["theme.compiled.css", "collections.css"];

/// Publishes the theme of the project at `root` into the web root `web`:
/// the files a deployment keeps beside the shell, and the shell's page
/// linking the stylesheets it has. Answers with what it wrote.
pub fn publish(root: &Path, web: &Path) -> Result<Vec<PathBuf>, String> {
    let assets = root.join("assets");
    let mut written = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    for folder in roots(&assets) {
        copy_tree(&folder, web, Path::new(""), &mut written, &mut seen)?;
    }
    let index = web.join("index.html");
    if let Ok(page) = std::fs::read_to_string(&index) {
        let links: String = STYLESHEETS
            .iter()
            .filter(|name| web.join(name).is_file())
            .map(|name| format!("    <link rel=\"stylesheet\" href=\"./{name}\">\n"))
            .filter(|link| !page.contains(link.trim()))
            .collect();
        if !links.is_empty()
            && let Some(at) = page.find("</head>")
        {
            let patched = format!("{}{links}{}", &page[..at], &page[at..]);
            std::fs::write(&index, patched)
                .map_err(|error| format!("{}: {error}", index.display()))?;
            written.push(index);
        }
    }
    Ok(written)
}

/// Publishes the theme and says so when something changed; a theme that
/// cannot be published is a warning — the build stands.
pub fn publish_and_report(root: &Path, web: &Path) {
    match publish(root, web) {
        Ok(written) if written.is_empty() => {}
        Ok(written) => println!(
            "[mxrs] published the theme: {} file(s) into {}",
            written.len(),
            web.display()
        ),
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

    #[test]
    fn publishing_merges_the_theme_roots_first_wins_and_links_the_page_once() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let write = |relative: &str, text: &str| {
            let path = root.join(relative);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        };
        write("assets/theme-cache/web/theme.compiled.css", ".a{}");
        write("assets/theme-cache/web/collections.css", ".b{}");
        write("assets/theme-cache/web/fonts/Atlas_Core$Atlas.ttf", "font");
        write("assets/theme-cache/web/theme.compiled.css.map", "{}");
        write("assets/theme/web/main.scss", "// source");
        write("assets/theme/web/settings.json", "{}");
        write("assets/theme/web/logo.png", "theme's logo");
        write(
            "assets/themesource/atlas_core/public/logo.png",
            "module's logo",
        );
        write(
            "assets/themesource/atlas_core/public/resources/font.woff",
            "woff",
        );
        write(
            "build/web/index.html",
            "<html><head><title>x</title></head><body></body></html>",
        );
        let web = root.join("build/web");
        let written = publish(root, &web).unwrap();
        let read = |relative: &str| std::fs::read_to_string(web.join(relative)).unwrap();
        assert_eq!(read("theme.compiled.css"), ".a{}");
        assert_eq!(read("fonts/Atlas_Core$Atlas.ttf"), "font");
        assert_eq!(read("resources/font.woff"), "woff");
        // The first root's file is the one that stays.
        assert_eq!(read("logo.png"), "theme's logo");
        // What a theme is written in is not served.
        for absent in ["main.scss", "settings.json", "theme.compiled.css.map"] {
            assert!(!web.join(absent).exists(), "{absent}");
        }
        let page = read("index.html");
        assert!(
            page.contains(
                "    <link rel=\"stylesheet\" href=\"./theme.compiled.css\">\n    <link rel=\"stylesheet\" href=\"./collections.css\">\n</head>"
            ),
            "{page}"
        );
        assert_eq!(written.len(), 6, "{written:?}");
        // Again: nothing to write, and the page linked once.
        assert!(publish(root, &web).unwrap().is_empty());
        assert_eq!(read("index.html").matches("theme.compiled.css").count(), 1);
    }
}
