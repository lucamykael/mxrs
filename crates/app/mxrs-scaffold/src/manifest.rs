//! What `mxrs.toml` says of the project: a setting of its `[project]`
//! table (or of no table, as older manifests wrote it), read by a TOML
//! parser — so a key of another table, a comment or an `=` inside a value
//! is not taken for it — and changed where the parser found it, the rest of
//! the file left as it was written.

use std::ops::Range;
use std::path::Path;

use toml::de::{DeTable, DeValue};

/// The string `key` holds in the `[project]` table of `source`.
pub(crate) fn project_setting(source: &str, key: &str) -> Option<String> {
    setting(source, key).map(|(_, value)| value)
}

/// The setting of `key` in the project's `mxrs.toml`, when it has one.
pub(crate) fn read_project_setting(root: &Path, key: &str) -> Option<String> {
    std::fs::read_to_string(root.join("mxrs.toml"))
        .ok()
        .and_then(|source| project_setting(&source, key))
}

/// `source` with the string `key` holds in the `[project]` table set to
/// `value`, written as a basic string; `None` when it holds no string.
pub(crate) fn with_project_setting(source: &str, key: &str, value: &str) -> Option<String> {
    let (span, _) = setting(source, key)?;
    let mut written = String::with_capacity(value.len() + 2);
    written.push('"');
    for character in value.chars() {
        match character {
            '"' => written.push_str("\\\""),
            '\\' => written.push_str("\\\\"),
            '\n' => written.push_str("\\n"),
            '\t' => written.push_str("\\t"),
            '\r' => written.push_str("\\r"),
            other => written.push(other),
        }
    }
    written.push('"');
    Some(format!(
        "{}{written}{}",
        &source[..span.start],
        &source[span.end..]
    ))
}

/// Where the string `key` holds is written in `source`, and what it holds:
/// the `[project]` table's, else the one an older manifest wrote before any
/// table.
fn setting(source: &str, key: &str) -> Option<(Range<usize>, String)> {
    let document = DeTable::parse(source).ok()?;
    let root = document.get_ref();
    let string = |table: &DeTable<'_>| {
        table
            .iter()
            .find(|(name, _)| name.get_ref() == key)
            .and_then(|(_, value)| match value.get_ref() {
                DeValue::String(text) => Some((value.span(), text.to_string())),
                _ => None,
            })
    };
    let project = root
        .iter()
        .find(|(name, _)| name.get_ref() == "project")
        .and_then(|(_, value)| match value.get_ref() {
            DeValue::Table(table) => string(table),
            _ => None,
        });
    project.or_else(|| string(root))
}

#[cfg(test)]
mod tests {
    use super::{project_setting, with_project_setting};

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
        for (manifest, name) in [
            ("[project]\nname = \"a\\\"b\"\n", "a\"b"),
            ("[project]\nname = \"\\u00e9\"\n", "\u{e9}"),
            ("[\"project\"]\nname = \"quoted\"\n", "quoted"),
            ("[project]\nname = \"\"\"multi\"\"\"\n", "multi"),
            ("name = \"before any table\"\n", "before any table"),
        ] {
            assert_eq!(project_setting(manifest, "name").as_deref(), Some(name));
        }
        // What a TOML parser refuses says nothing.
        assert_eq!(project_setting("[project\nname = \"x\"\n", "name"), None);
    }

    /// A setting is changed where it is written, however it is quoted; the
    /// rest of the file stays as it was.
    #[test]
    fn a_setting_is_changed_where_it_is_written() {
        let source =
            "[tooling]\nmendix_version = \"9\"\n[project]\nmendix_version = '11.12.1' # pinned\n";
        assert_eq!(
            with_project_setting(source, "mendix_version", "11.13.0").as_deref(),
            Some(
                "[tooling]\nmendix_version = \"9\"\n[project]\nmendix_version = \"11.13.0\" # pinned\n"
            )
        );
        assert_eq!(with_project_setting(source, "missing", "x"), None);
    }
}
