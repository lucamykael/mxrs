//! What the pluggable widgets a project holds bring for the browser: the
//! stylesheet each package ships (`com/mendix/widget/web/<w>/<W>.css`),
//! which the Mendix client loads beside the theme. Written as one
//! `widgets.css` beside the compiled theme, package by package in name
//! order, only when it changes.

use std::io::Read as _;
use std::path::{Path, PathBuf};

/// Every stylesheet the packages under `widgets` ship, each after a line
/// naming the package it is from, in the packages' name order.
pub fn stylesheet(widgets: &Path) -> Result<String, String> {
    let mut packages: Vec<PathBuf> = std::fs::read_dir(widgets)
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension.eq_ignore_ascii_case("mpk"))
        })
        .collect();
    packages.sort();
    let mut css = String::new();
    for package in packages {
        let file = std::fs::File::open(&package)
            .map_err(|error| format!("{}: {error}", package.display()))?;
        let mut archive = zip::ZipArchive::new(file)
            .map_err(|error| format!("{}: {error}", package.display()))?;
        let mut names: Vec<String> = (0..archive.len())
            .filter_map(|index| {
                archive
                    .by_index(index)
                    .ok()
                    .map(|entry| entry.name().to_string())
            })
            .filter(|name| name.ends_with(".css") && !name.contains("editorPreview"))
            .collect();
        names.sort();
        for name in names {
            let mut text = String::new();
            archive
                .by_name(&name)
                .map_err(|error| format!("{}: {error}", package.display()))?
                .read_to_string(&mut text)
                .map_err(|error| format!("{}: {name}: {error}", package.display()))?;
            let package_name = package
                .file_name()
                .map(|name| name.to_string_lossy().to_string())
                .unwrap_or_default();
            css.push_str(&format!(
                "/* {package_name}: {name} */\n{}\n",
                text.trim_end()
            ));
        }
    }
    Ok(css)
}

/// Writes `widgets.css` beside the compiled theme of the project at `root`
/// from the packages under `assets/widgets`, when it is not already what it
/// would be; removes it when the project has no widget stylesheet. Answers
/// whether the file changed.
pub fn write(root: &Path) -> Result<bool, String> {
    let widgets = root.join("assets/widgets");
    let target = root.join("assets/theme-cache/web/widgets.css");
    let css = if widgets.is_dir() {
        stylesheet(&widgets)?
    } else {
        String::new()
    };
    if css.is_empty() {
        if target.is_file() {
            std::fs::remove_file(&target)
                .map_err(|error| format!("{}: {error}", target.display()))?;
            return Ok(true);
        }
        return Ok(false);
    }
    if std::fs::read_to_string(&target).is_ok_and(|current| current == css) {
        return Ok(false);
    }
    if let Some(folder) = target.parent() {
        std::fs::create_dir_all(folder)
            .map_err(|error| format!("{}: {error}", folder.display()))?;
    }
    std::fs::write(&target, css).map_err(|error| format!("{}: {error}", target.display()))?;
    Ok(true)
}

/// Writes the widgets' stylesheet and says so when it changed; a package
/// that cannot be read is a warning — the build stands.
pub fn write_and_report(root: &Path) {
    match write(root) {
        Ok(true) => {
            println!("[mxrs] wrote the widgets' stylesheet to assets/theme-cache/web/widgets.css")
        }
        Ok(false) => {}
        Err(error) => eprintln!("[mxrs] warning: {error}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write as _;

    fn package(path: &Path, css: Option<&str>) {
        let file = std::fs::File::create(path).unwrap();
        let mut archive = zip::ZipWriter::new(file);
        let options = zip::write::SimpleFileOptions::default();
        archive.start_file("package.xml", options).unwrap();
        archive.write_all(b"<package/>").unwrap();
        archive
            .start_file("Thing.editorPreview.css", options)
            .unwrap();
        archive.write_all(b".preview{}").unwrap();
        if let Some(css) = css {
            archive
                .start_file("com/mendix/widget/web/thing/Thing.css", options)
                .unwrap();
            archive.write_all(css.as_bytes()).unwrap();
        }
        archive.finish().unwrap();
    }

    #[test]
    fn the_packages_stylesheets_are_joined_in_name_order_and_written_once() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let widgets = root.join("assets/widgets");
        std::fs::create_dir_all(&widgets).unwrap();
        package(&widgets.join("Zebra.mpk"), Some(".zebra{}"));
        package(&widgets.join("Apple.mpk"), Some(".apple{}"));
        package(&widgets.join("Plain.mpk"), None);
        assert!(write(root).unwrap());
        let css = std::fs::read_to_string(root.join("assets/theme-cache/web/widgets.css")).unwrap();
        assert_eq!(
            css,
            "/* Apple.mpk: com/mendix/widget/web/thing/Thing.css */\n.apple{}\n/* Zebra.mpk: com/mendix/widget/web/thing/Thing.css */\n.zebra{}\n"
        );
        assert!(!css.contains("preview"));
        assert!(!write(root).unwrap());
        std::fs::remove_dir_all(&widgets).unwrap();
        assert!(write(root).unwrap());
        assert!(!root.join("assets/theme-cache/web/widgets.css").exists());
    }
}
