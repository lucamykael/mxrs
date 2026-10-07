//! What `mxrs.toml` says of the project: a setting of its `[project]`
//! table (or of no table, as older manifests wrote it), read the way TOML reads it — only in that table, a basic or
//! literal string, past comments — so a key of another table, a comment
//! or an `=` inside a value is not taken for it.

use std::path::Path;

/// The string `key` holds in the `[project]` table of `source`.
pub(crate) fn project_setting(source: &str, key: &str) -> Option<String> {
    let mut table = String::new();
    for line in source.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(header) = line.strip_prefix('[') {
            table = header
                .split(']')
                .next()
                .unwrap_or_default()
                .trim()
                .to_string();
            continue;
        }
        // Before any table, a key is the manifest's own, as older ones wrote it.
        if !table.is_empty() && table != "project" {
            continue;
        }
        let Some((name, value)) = line.split_once('=') else {
            continue;
        };
        let name = name.trim().trim_matches('"');
        if name != key {
            continue;
        }
        return string(value.trim());
    }
    None
}

/// The setting of `key` in the project's `mxrs.toml`, when it has one.
pub(crate) fn read_project_setting(root: &Path, key: &str) -> Option<String> {
    std::fs::read_to_string(root.join("mxrs.toml"))
        .ok()
        .and_then(|source| project_setting(&source, key))
}

/// A TOML string value: `"basic"` with its escapes, or `'literal'`.
fn string(value: &str) -> Option<String> {
    if let Some(rest) = value.strip_prefix('\'') {
        return Some(rest[..rest.find('\'')?].to_string());
    }
    let rest = value.strip_prefix('"')?;
    let mut text = String::new();
    let mut characters = rest.chars();
    while let Some(character) = characters.next() {
        match character {
            '"' => return Some(text),
            '\\' => match characters.next()? {
                'n' => text.push('\n'),
                't' => text.push('\t'),
                'r' => text.push('\r'),
                other => text.push(other),
            },
            other => text.push(other),
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::project_setting;

    /// A setting is the `[project]` table's, past comments, whatever its
    /// value holds; another table's key of the same name is not it.
    #[test]
    fn a_setting_is_read_from_the_project_table_as_toml_reads_it() {
        let source = r#"
# imported_snapshot = "commented/out"
[tooling]
imported_snapshot = "other/table"

[project]
name = "shop" # the name
imported_snapshot = "model/a=b"   # where the import wrote it
mendix_version = '11.12.1'
"#;
        assert_eq!(
            project_setting(source, "imported_snapshot").as_deref(),
            Some("model/a=b")
        );
        assert_eq!(project_setting(source, "name").as_deref(), Some("shop"));
        assert_eq!(
            project_setting(source, "mendix_version").as_deref(),
            Some("11.12.1")
        );
        assert_eq!(project_setting(source, "missing"), None);
        assert_eq!(
            project_setting("[project]\nname = \"a\\\"b\"\n", "name").as_deref(),
            Some("a\"b")
        );
    }
}
