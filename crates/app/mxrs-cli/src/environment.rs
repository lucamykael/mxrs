//! Safe, read-only environment profile loading for runtime commands.
//!
//! Values are parsed as data: shell interpolation and command substitution are
//! never evaluated. The CLI exposes only source paths and key names, so this
//! module must not include values in errors or debug output.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

const PROFILE_KEYS: &[&str] = &["MXRS_ENV", "MXRB_ENV", "RACK_ENV", "RAILS_ENV"];

#[derive(Debug, thiserror::Error)]
pub enum EnvironmentError {
    #[error("invalid environment name {0:?}")]
    InvalidName(String),
    #[error("invalid environment file {path}:{line}: {reason}")]
    Parse {
        path: String,
        line: usize,
        reason: &'static str,
    },
    #[error("cannot read environment file {path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
}

#[derive(Clone)]
pub struct EnvironmentProfile {
    pub environment: String,
    pub requested: String,
    pub root: PathBuf,
    pub sources: Vec<PathBuf>,
    values: BTreeMap<String, String>,
}

impl std::fmt::Debug for EnvironmentProfile {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("EnvironmentProfile")
            .field("environment", &self.environment)
            .field("requested", &self.requested)
            .field("root", &self.root)
            .field("sources", &self.sources)
            .field("keys", &self.values.keys().collect::<Vec<_>>())
            .finish()
    }
}

impl EnvironmentProfile {
    pub fn load(root: impl AsRef<Path>, requested: Option<&str>) -> Result<Self, EnvironmentError> {
        Self::load_from(root, requested, std::env::vars())
    }

    fn load_from(
        root: impl AsRef<Path>,
        requested: Option<&str>,
        process: impl IntoIterator<Item = (String, String)>,
    ) -> Result<Self, EnvironmentError> {
        let process = process.into_iter().collect::<BTreeMap<_, _>>();
        let requested = requested
            .map(str::to_string)
            .or_else(|| {
                PROFILE_KEYS.iter().find_map(|key| {
                    process
                        .get(*key)
                        .filter(|value| !value.trim().is_empty())
                        .cloned()
                })
            })
            .unwrap_or_else(|| "development".to_string());
        let requested = normalize_name(&requested)?;
        let environment = canonical_name(&requested).to_string();
        let root = std::path::absolute(root.as_ref()).map_err(|source| EnvironmentError::Io {
            path: root.as_ref().display().to_string(),
            source,
        })?;
        let candidates = [
            root.join(".env"),
            root.join(format!(".env.{environment}")),
            root.join("config/environments")
                .join(format!("{environment}.env")),
        ];
        let sources = candidates
            .into_iter()
            .filter(|path| path.is_file())
            .collect::<Vec<_>>();
        let mut values = BTreeMap::new();
        for source in &sources {
            values.extend(parse_file(source)?);
        }
        values.extend(process);
        Ok(Self {
            environment,
            requested,
            root,
            sources,
            values,
        })
    }

    pub fn keys(&self) -> impl Iterator<Item = &str> {
        self.values.keys().map(String::as_str)
    }

    #[cfg(test)]
    fn get(&self, key: &str) -> Option<&str> {
        self.values.get(key).map(String::as_str)
    }
}

fn normalize_name(raw: &str) -> Result<String, EnvironmentError> {
    let name = raw.trim().to_ascii_lowercase();
    let mut characters = name.chars();
    let valid = characters
        .next()
        .is_some_and(|character| character.is_ascii_lowercase())
        && characters.all(|character| {
            character.is_ascii_lowercase()
                || character.is_ascii_digit()
                || matches!(character, '_' | '-')
        });
    if valid {
        Ok(name)
    } else {
        Err(EnvironmentError::InvalidName(raw.to_string()))
    }
}

fn canonical_name(name: &str) -> &str {
    match name {
        "dev" | "development" => "development",
        "homolog" | "homologation" | "staging" => "staging",
        "prod" | "production" => "production",
        name => name,
    }
}

fn parse_file(path: &Path) -> Result<BTreeMap<String, String>, EnvironmentError> {
    let bytes = std::fs::read(path).map_err(|source| EnvironmentError::Io {
        path: path.display().to_string(),
        source,
    })?;
    let text = std::str::from_utf8(&bytes).map_err(|_| EnvironmentError::Parse {
        path: path.display().to_string(),
        line: 1,
        reason: "file is not valid UTF-8",
    })?;
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut values = BTreeMap::new();
    for (offset, line) in text.lines().enumerate() {
        parse_line(path, offset + 1, line, &mut values)?;
    }
    Ok(values)
}

fn parse_line(
    path: &Path,
    line_number: usize,
    line: &str,
    values: &mut BTreeMap<String, String>,
) -> Result<(), EnvironmentError> {
    let mut source = line.trim();
    if source.is_empty() || source.starts_with('#') {
        return Ok(());
    }
    if let Some(rest) = source.strip_prefix("export ") {
        source = rest.trim_start();
    }
    let Some((key, raw)) = source.split_once('=') else {
        return parse_error(path, line_number, "expected KEY=VALUE");
    };
    let key = key.trim();
    if !valid_key(key) {
        return parse_error(path, line_number, "expected KEY=VALUE");
    }
    let value = parse_value(path, line_number, raw.trim_start())?;
    values.insert(key.to_string(), value);
    Ok(())
}

fn valid_key(key: &str) -> bool {
    let mut characters = key.chars();
    characters
        .next()
        .is_some_and(|character| character == '_' || character.is_ascii_alphabetic())
        && characters.all(|character| character == '_' || character.is_ascii_alphanumeric())
}

fn parse_value(path: &Path, line: usize, source: &str) -> Result<String, EnvironmentError> {
    let Some(quote) = source
        .chars()
        .next()
        .filter(|character| matches!(character, '\'' | '"'))
    else {
        return Ok(source
            .find(" #")
            .map_or(source, |index| &source[..index])
            .trim_end()
            .to_string());
    };
    let mut value = String::new();
    let mut escaped = false;
    let mut closing = None;
    for (index, character) in source.char_indices().skip(1) {
        if quote == '"' && escaped {
            match character {
                'n' => value.push('\n'),
                'r' => value.push('\r'),
                't' => value.push('\t'),
                '"' => value.push('"'),
                '\\' => value.push('\\'),
                other => {
                    value.push('\\');
                    value.push(other);
                }
            }
            escaped = false;
        } else if quote == '"' && character == '\\' {
            escaped = true;
        } else if character == quote {
            closing = Some(index + character.len_utf8());
            break;
        } else {
            value.push(character);
        }
    }
    let Some(closing) = closing else {
        return parse_error(path, line, "unterminated quoted value");
    };
    let remainder = source[closing..].trim();
    if !remainder.is_empty() && !remainder.starts_with('#') {
        return parse_error(path, line, "unexpected content after quoted value");
    }
    Ok(value)
}

fn parse_error<T>(path: &Path, line: usize, reason: &'static str) -> Result<T, EnvironmentError> {
    Err(EnvironmentError::Parse {
        path: path.display().to_string(),
        line,
        reason,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(root: &Path, relative: &str, content: &str) -> PathBuf {
        let path = root.join(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, content).unwrap();
        path
    }

    #[test]
    fn layers_profiles_below_process_values_and_canonicalizes_aliases() {
        let directory = tempfile::tempdir().unwrap();
        write(directory.path(), ".env", "SHARED=base\nBASE=yes\n");
        write(
            directory.path(),
            ".env.staging",
            "SHARED=profile\nDOT=yes\n",
        );
        let last = write(
            directory.path(),
            "config/environments/staging.env",
            "SHARED=config\nCONFIG=yes\n",
        );
        let profile = EnvironmentProfile::load_from(
            directory.path(),
            Some("homolog"),
            [
                ("SHARED".into(), "process".into()),
                ("ONLY".into(), "yes".into()),
            ],
        )
        .unwrap();
        assert_eq!(profile.environment, "staging");
        assert_eq!(profile.requested, "homolog");
        assert_eq!(profile.sources.last(), Some(&last));
        assert_eq!(profile.get("SHARED"), Some("process"));
        assert_eq!(profile.get("BASE"), Some("yes"));
        assert_eq!(profile.get("DOT"), Some("yes"));
        assert_eq!(profile.get("CONFIG"), Some("yes"));
    }

    #[test]
    fn parses_values_as_data_without_expansion() {
        let directory = tempfile::tempdir().unwrap();
        write(
            directory.path(),
            ".env",
            "# comment\nexport SIMPLE = value # ignored\nHASH=kept#inside\nSINGLE='literal $SIMPLE # value'\nDOUBLE=\"line\\nnext\\t\\\"quoted\\\"\" # ignored\nCOMMAND=$(uname)\nEMPTY=\n",
        );
        let profile =
            EnvironmentProfile::load_from(directory.path(), None, std::iter::empty()).unwrap();
        assert_eq!(profile.get("SIMPLE"), Some("value"));
        assert_eq!(profile.get("HASH"), Some("kept#inside"));
        assert_eq!(profile.get("SINGLE"), Some("literal $SIMPLE # value"));
        assert_eq!(profile.get("DOUBLE"), Some("line\nnext\t\"quoted\""));
        assert_eq!(profile.get("COMMAND"), Some("$(uname)"));
        assert_eq!(profile.get("EMPTY"), Some(""));
    }

    #[test]
    fn rejects_bad_names_and_syntax_without_disclosing_values() {
        let directory = tempfile::tempdir().unwrap();
        assert!(matches!(
            EnvironmentProfile::load_from(directory.path(), Some("../prod"), std::iter::empty()),
            Err(EnvironmentError::InvalidName(_))
        ));
        for (contents, reason) in [
            ("NOT A KEY=secret", "expected KEY=VALUE"),
            ("TOKEN='never-show-this-secret", "unterminated quoted value"),
            (
                "TOKEN='secret' trailing",
                "unexpected content after quoted value",
            ),
        ] {
            write(directory.path(), ".env", contents);
            let error = EnvironmentProfile::load_from(directory.path(), None, std::iter::empty())
                .unwrap_err();
            let rendered = error.to_string();
            assert!(rendered.contains(reason));
            assert!(!rendered.contains("secret"));
        }
    }

    #[test]
    fn detects_profile_from_process_and_debug_never_contains_values() {
        let directory = tempfile::tempdir().unwrap();
        let profile = EnvironmentProfile::load_from(
            directory.path(),
            None,
            [
                ("MXRS_ENV".into(), "prod".into()),
                ("TOKEN".into(), "super-secret".into()),
            ],
        )
        .unwrap();
        assert_eq!(profile.environment, "production");
        assert!(!format!("{profile:?}").contains("super-secret"));
        assert_eq!(profile.keys().collect::<Vec<_>>(), ["MXRS_ENV", "TOKEN"]);
    }
}
