//! Theme/design-system tooling — ports `bin/mxrb`'s `design scan` and
//! `design migrate` (`lib/mxrb/model/design_{system,migration}.rb`). Both
//! operate on the stylesheet assets living NEXT to an `.mpr` (the same
//! `ASSET_DIRECTORIES` the compare snapshot already inventories), so they
//! work identically for Studio Pro exports and Cargo-built models. The
//! `design init` scaffold lives in `mxrs-scaffold`; the `design_system`
//! Ruby DSL policy block (`forbid_literal_colors`) has no MXRS equivalent
//! and is not pretended here.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use regex::Regex;
use serde::Serialize;
use sha2::{Digest, Sha256};

/// `Mxrb::Model::DesignSystem::ASSET_DIRECTORIES`.
const ASSET_DIRECTORIES: [&str; 9] = [
    "theme",
    "theme-cache",
    "themesource",
    "resources",
    "widgets",
    "javasource",
    "javascriptsource",
    "userlib",
    "vendorlib",
];

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct DesignToken {
    pub name: String,
    pub value: String,
    pub kind: &'static str,
    pub theme: Option<String>,
    pub path: String,
    pub line: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct UnresolvedReference {
    pub token: String,
    pub reference: String,
}

/// Read-only inventory of Mendix theme assets, rooted at the directory
/// holding the `.mpr`.
pub struct DesignSystem {
    root: PathBuf,
    tokens: Vec<DesignToken>,
    catalogs: Vec<(String, Option<serde_json::Value>)>,
}

impl DesignSystem {
    pub fn scan(mpr: impl AsRef<Path>) -> Result<Self, String> {
        let mpr = open_model_root(mpr.as_ref())?;
        let root = mpr.parent().unwrap_or_else(|| Path::new(".")).to_path_buf();
        let css = Regex::new(r"(--[A-Za-z0-9_-]+)\s*:\s*([^;{}]+);").expect("static pattern");
        let scss = Regex::new(r"(\$[A-Za-z0-9_-]+)\s*:\s*([^;{}]+);").expect("static pattern");
        let mut tokens = Vec::new();
        for path in stylesheet_paths(&root, false)? {
            let relative = relative_path(&root, &path);
            let theme = path
                .file_name()
                .and_then(|name| name.to_str())
                .and_then(|name| name.strip_prefix("_theme-"))
                .and_then(|rest| rest.split('.').next())
                .filter(|theme| !theme.is_empty())
                .map(str::to_string);
            let source = std::fs::read_to_string(&path)
                .map_err(|error| format!("cannot read {relative}: {error}"))?;
            for (index, line) in source.lines().enumerate() {
                if line.trim_start().starts_with("//") {
                    continue;
                }
                for (pattern, kind) in [(&css, "css_custom_property"), (&scss, "scss_variable")] {
                    for capture in pattern.captures_iter(line) {
                        tokens.push(DesignToken {
                            name: capture[1].to_string(),
                            value: capture[2].trim().to_string(),
                            kind,
                            theme: theme.clone(),
                            path: relative.clone(),
                            line: index + 1,
                        });
                    }
                }
            }
        }
        let mut catalog_paths: Vec<PathBuf> = walk_files(&root.join("themesource"))?
            .into_iter()
            .filter(|path| {
                path.file_name()
                    .is_some_and(|n| n == "design-properties.json")
            })
            .collect();
        catalog_paths.sort();
        let catalogs = catalog_paths
            .into_iter()
            .map(|path| {
                let relative = relative_path(&root, &path);
                let parsed = std::fs::read(&path)
                    .ok()
                    .and_then(|bytes| serde_json::from_slice(&bytes).ok());
                (relative, parsed)
            })
            .collect();
        Ok(Self {
            root,
            tokens,
            catalogs,
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn tokens(&self) -> &[DesignToken] {
        &self.tokens
    }

    pub fn themes(&self) -> Vec<String> {
        let mut themes: Vec<String> = self
            .tokens
            .iter()
            .filter_map(|token| token.theme.clone())
            .collect();
        themes.sort();
        themes.dedup();
        themes
    }

    pub fn catalogs(&self) -> &[(String, Option<serde_json::Value>)] {
        &self.catalogs
    }

    pub fn unresolved_references(&self) -> Vec<UnresolvedReference> {
        let names: HashSet<&str> = self
            .tokens
            .iter()
            .map(|token| token.name.as_str())
            .collect();
        let reference = Regex::new(r"var\((--[A-Za-z0-9_-]+)").expect("static pattern");
        self.tokens
            .iter()
            .flat_map(|token| {
                reference
                    .captures_iter(&token.value)
                    .map(|capture| capture[1].to_string())
                    .filter(|name| !names.contains(name.as_str()))
                    .map(|name| UnresolvedReference {
                        token: token.name.clone(),
                        reference: name,
                    })
                    .collect::<Vec<_>>()
            })
            .collect()
    }

    pub fn literal_colors(&self) -> Vec<&DesignToken> {
        let color = Regex::new(r"(?i)#[0-9a-f]{3,8}\b").expect("static pattern");
        self.tokens
            .iter()
            .filter(|token| color.is_match(&token.value))
            .collect()
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct MigrationChange {
    pub path: String,
    pub occurrences: usize,
    #[serde(skip)]
    before: Vec<u8>,
    #[serde(skip)]
    after: Vec<u8>,
}

/// Immutable preview of literal-to-token stylesheet replacements, ports
/// `Mxrb::Model::DesignMigrationPlan` for the CLI's single literal/token
/// pair. Staged temporary files and digest verification keep `--apply`
/// honest when assets changed between preview and application.
pub struct MigrationPlan {
    root: PathBuf,
    changes: Vec<MigrationChange>,
}

impl MigrationPlan {
    pub fn build(mpr: impl AsRef<Path>, literal: &str, token: &str) -> Result<Self, String> {
        let mpr = open_model_root(mpr.as_ref())?;
        let root = mpr.parent().unwrap_or_else(|| Path::new(".")).to_path_buf();
        let mut changes = Vec::new();
        for path in stylesheet_paths(&root, true)? {
            let relative = relative_path(&root, &path);
            let before =
                std::fs::read(&path).map_err(|error| format!("cannot read {relative}: {error}"))?;
            let occurrences = count_occurrences(&before, literal.as_bytes());
            if occurrences == 0 {
                continue;
            }
            let after = replace_all(&before, literal.as_bytes(), token.as_bytes());
            if after == before {
                continue;
            }
            changes.push(MigrationChange {
                path: relative,
                occurrences,
                before,
                after,
            });
        }
        Ok(Self { root, changes })
    }

    pub fn changes(&self) -> &[MigrationChange] {
        &self.changes
    }

    pub fn occurrences(&self) -> usize {
        self.changes.iter().map(|change| change.occurrences).sum()
    }

    pub fn apply(&self) -> Result<(), String> {
        for change in &self.changes {
            let target = self.root.join(&change.path);
            let current = std::fs::read(&target)
                .map_err(|_| format!("design asset changed after preview: {}", change.path))?;
            if Sha256::digest(&current) != Sha256::digest(&change.before) {
                return Err(format!(
                    "design asset changed after preview: {}",
                    change.path
                ));
            }
        }
        // Stage every replacement next to its target, then rename each into
        // place; on failure the original bytes are restored in reverse order.
        let mut staged = Vec::new();
        for change in &self.changes {
            let target = self.root.join(&change.path);
            let temporary = target.with_file_name(format!(
                "{}.mxrs-{}",
                target
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or("asset"),
                std::process::id()
            ));
            if let Err(error) = std::fs::write(&temporary, &change.after) {
                for path in &staged {
                    let _ = std::fs::remove_file(path);
                }
                return Err(format!("cannot stage {}: {error}", change.path));
            }
            staged.push(temporary);
        }
        let mut committed: Vec<&MigrationChange> = Vec::new();
        for (change, temporary) in self.changes.iter().zip(&staged) {
            let target = self.root.join(&change.path);
            if let Err(error) = std::fs::rename(temporary, &target) {
                for change in committed.into_iter().rev() {
                    let _ = std::fs::write(self.root.join(&change.path), &change.before);
                }
                for path in &staged {
                    let _ = std::fs::remove_file(path);
                }
                return Err(format!("cannot apply {}: {error}", change.path));
            }
            committed.push(change);
        }
        Ok(())
    }
}

/// MXRB's `design scan`/`migrate` open the project before touching assets,
/// so a missing or unreadable model store is refused rather than treated as
/// an empty theme.
fn open_model_root(mpr: &Path) -> Result<PathBuf, String> {
    let mpr = std::path::absolute(mpr).map_err(|error| error.to_string())?;
    mxrs_mpr::MprFile::open(&mpr, true).map_err(|error| error.to_string())?;
    Ok(mpr)
}

fn count_occurrences(haystack: &[u8], needle: &[u8]) -> usize {
    if needle.is_empty() {
        return 0;
    }
    let mut count = 0;
    let mut offset = 0;
    while let Some(position) = find(haystack, needle, offset) {
        count += 1;
        offset = position + needle.len();
    }
    count
}

fn replace_all(haystack: &[u8], needle: &[u8], replacement: &[u8]) -> Vec<u8> {
    if needle.is_empty() {
        return haystack.to_vec();
    }
    let mut result = Vec::with_capacity(haystack.len());
    let mut offset = 0;
    while let Some(position) = find(haystack, needle, offset) {
        result.extend_from_slice(&haystack[offset..position]);
        result.extend_from_slice(replacement);
        offset = position + needle.len();
    }
    result.extend_from_slice(&haystack[offset..]);
    result
}

fn find(haystack: &[u8], needle: &[u8], offset: usize) -> Option<usize> {
    haystack
        .get(offset..)?
        .windows(needle.len())
        .position(|window| window == needle)
        .map(|position| offset + position)
}

/// Every `*.css`/`*.scss` under the asset directories, sorted by full path
/// exactly as MXRB globs them. `skip_symlinks` mirrors the migration plan's
/// extra exclusion.
fn stylesheet_paths(root: &Path, skip_symlinks: bool) -> Result<Vec<PathBuf>, String> {
    let mut paths = Vec::new();
    for directory in ASSET_DIRECTORIES {
        for path in walk_files(&root.join(directory))? {
            let stylesheet = path
                .extension()
                .and_then(|extension| extension.to_str())
                .is_some_and(|extension| extension == "css" || extension == "scss");
            if !stylesheet {
                continue;
            }
            if skip_symlinks && path.is_symlink() {
                continue;
            }
            paths.push(path);
        }
    }
    paths.sort();
    Ok(paths)
}

fn walk_files(directory: &Path) -> Result<Vec<PathBuf>, String> {
    let mut files = Vec::new();
    let entries = match std::fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(files),
        Err(error) => return Err(format!("cannot read {}: {error}", directory.display())),
    };
    for entry in entries {
        let path = entry
            .map_err(|error| format!("cannot read {}: {error}", directory.display()))?
            .path();
        if path.is_dir() {
            files.extend(walk_files(&path)?);
        } else {
            files.push(path);
        }
    }
    Ok(files)
}

fn relative_path(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(root: &Path) {
        std::fs::create_dir_all(root.join("theme/web")).unwrap();
        std::fs::create_dir_all(root.join("themesource/atlas/web")).unwrap();
        std::fs::write(
            root.join("theme/web/custom-variables.scss"),
            "// $ignored: #123456;\n$brand-primary: #264ae5;\n$gap: var(--spacing-medium) 0;\n",
        )
        .unwrap();
        std::fs::write(
            root.join("theme/web/_theme-dark.scss"),
            "--surface: #101820;\n",
        )
        .unwrap();
        std::fs::write(
            root.join("themesource/atlas/web/design-properties.json"),
            "{\"pages\": []}",
        )
        .unwrap();
        let project = mxrs_dsl::ProjectBuilder::new("11.12.1");
        mxrs_writer::write_project(root.join("Project.mpr"), &project.build()).unwrap();
    }

    #[test]
    fn scan_inventories_tokens_themes_catalogs_and_quality_signals() {
        let directory = tempfile::tempdir().unwrap();
        fixture(directory.path());
        let design = DesignSystem::scan(directory.path().join("Project.mpr")).unwrap();
        let names: Vec<&str> = design
            .tokens()
            .iter()
            .map(|token| token.name.as_str())
            .collect();
        assert_eq!(names, ["--surface", "$brand-primary", "$gap"]);
        assert_eq!(design.tokens()[0].theme.as_deref(), Some("dark"));
        assert_eq!(design.tokens()[0].kind, "css_custom_property");
        assert_eq!(design.tokens()[1].line, 2);
        assert_eq!(design.themes(), ["dark"]);
        assert_eq!(design.catalogs().len(), 1);
        assert!(design.catalogs()[0].1.is_some());
        let unresolved = design.unresolved_references();
        assert_eq!(unresolved.len(), 1);
        assert_eq!(unresolved[0].reference, "--spacing-medium");
        let literals: Vec<&str> = design
            .literal_colors()
            .iter()
            .map(|token| token.name.as_str())
            .collect();
        assert_eq!(literals, ["--surface", "$brand-primary"]);
    }

    #[test]
    fn migrate_previews_then_applies_with_digest_verification() {
        let directory = tempfile::tempdir().unwrap();
        fixture(directory.path());
        let mpr = directory.path().join("Project.mpr");
        let plan = MigrationPlan::build(&mpr, "#264ae5", "$brand-primary").unwrap();
        assert_eq!(plan.occurrences(), 1);
        assert_eq!(plan.changes().len(), 1);
        // Preview did not write.
        let stylesheet = directory.path().join("theme/web/custom-variables.scss");
        assert!(
            std::fs::read_to_string(&stylesheet)
                .unwrap()
                .contains("#264ae5")
        );
        plan.apply().unwrap();
        let migrated = std::fs::read_to_string(&stylesheet).unwrap();
        assert!(migrated.contains("$brand-primary: $brand-primary;"));
        assert!(!migrated.contains("#264ae5"));
        // A stale plan refuses to double-apply over changed contents.
        assert!(plan.apply().unwrap_err().contains("changed after preview"));
    }
}
