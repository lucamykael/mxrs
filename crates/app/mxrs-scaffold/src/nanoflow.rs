//! Where a scaffolded nanoflow goes: a method of a TypeScript service of the
//! frontend, `frontend/src/services/<module>/<subject>Service.ts`, placed the
//! way the importer places an imported one. Rust pages call it by the name
//! `src/ui/nanoflows/<module>/in_frontend.rs` gives it.

use std::collections::BTreeSet;
use std::path::Path;

use mxrs_frontend::naming::{camel_from_snake, service_file};

use crate::service::{declared_entities, method_name, plan_subject, service_names, split_prefix};
use crate::templates::snake_case;
use crate::transaction::Transaction;
use crate::{Result, ScaffoldError};

/// A nanoflow to scaffold as a service method.
pub(crate) struct NanoflowMethod {
    /// The flow's Mendix name.
    pub(crate) name: String,
    /// Its documentation, line by line.
    pub(crate) docs: Vec<String>,
    /// Its body's statements; none for an empty flow.
    pub(crate) body: Vec<String>,
    /// What the body uses of `@/mxrs/flows`.
    pub(crate) vocabulary: Vec<&'static str>,
}

/// The widest an import is written on one line.
const WIDTH: usize = 100;

/// Adds `method` to the frontend service of its subject in `module_name`:
/// creating the service's file, or adding the method to it. `entities` are
/// the module's entities this scaffold creates besides the ones its project
/// already declares.
pub(crate) fn add_nanoflow(
    transaction: &mut Transaction,
    root: &Path,
    module_name: &str,
    method: &NanoflowMethod,
    entities: &[String],
) -> Result<()> {
    let module_stem = snake_case(module_name);
    let folder = root.join("frontend/src/services").join(&module_stem);
    let mut names: Vec<String> = declared_entities(root, &module_stem)
        .into_iter()
        .map(|(name, _)| name)
        .collect();
    names.extend(entities.iter().cloned());
    let (prefix, core) = split_prefix(&method.name);
    let (subject, action) = plan_subject(core, &names, |word| {
        let (_, service) = service_names(
            Some(&crate::service::Subject::Word(word.to_string())),
            &module_stem,
        );
        folder.join(service_file(&service)).is_file()
    });
    let (_, service) = service_names(subject.as_ref(), &module_stem);
    let path = folder.join(service_file(&service));
    let existing = transaction.content(&path)?;
    let mut function = camel_from_snake(&method_name(&action, &method.name));
    // The module's services, wherever the subject rule now places the flow:
    // a service that already declares it is the one that keeps it.
    let mut services: Vec<std::path::PathBuf> = match std::fs::read_dir(&folder) {
        Ok(entries) => entries
            .flatten()
            .map(|entry| entry.path())
            .filter(|file| file.extension().is_some_and(|extension| extension == "ts"))
            .collect(),
        Err(_) => Vec::new(),
    };
    services.sort();
    for service in services {
        let declares = transaction.content(&service)?.is_some_and(|source| {
            source.lines().any(|line| {
                let mut words = line.trim().trim_start_matches('*').split_whitespace();
                words.next() == Some("@nanoflow") && words.next() == Some(method.name.as_str())
            })
        });
        if declares {
            return Err(ScaffoldError::FileExists(format!(
                "{} already declares {module_name}.{}",
                service.display(),
                method.name
            )));
        }
    }
    if let Some(existing) = &existing {
        let taken = |candidate: &str| existing.contains(&format!("  async {candidate}("));
        if taken(&function) {
            // Two flows of one subject doing the same: the kind tells them
            // apart, and failing that a number — the importer's rule.
            let kinded = prefix.map(|prefix| {
                format!(
                    "{}{}",
                    prefix.to_ascii_lowercase(),
                    mxrs_frontend::naming::pascal(&function)
                )
            });
            function = match kinded {
                Some(kinded) if !taken(&kinded) => kinded,
                _ => (2..)
                    .map(|suffix| format!("{function}{suffix}"))
                    .find(|candidate| !taken(candidate))
                    .expect("an unbounded suffix always finds a free name"),
            };
        }
    }
    let mut text = String::from("  /**\n");
    for line in &method.docs {
        if line.is_empty() {
            text.push_str("   *\n");
        } else {
            text.push_str(&format!("   * {line}\n"));
        }
    }
    if !method.docs.is_empty() {
        text.push_str("   *\n");
    }
    text.push_str(&format!("   * @nanoflow {}\n   */\n", method.name));
    if method.body.is_empty() {
        text.push_str(&format!("  async {function}(): Promise<void> {{}},\n"));
    } else {
        text.push_str(&format!("  async {function}(): Promise<void> {{\n"));
        for line in &method.body {
            text.push_str(&format!("    {line}\n"));
        }
        text.push_str("  },\n");
    }
    let mut vocabulary: BTreeSet<String> = method
        .vocabulary
        .iter()
        .map(|word| word.to_string())
        .collect();
    vocabulary.insert("nanoflowService".to_string());
    let source = match existing {
        Some(existing) => {
            let newline = if existing.contains("\r\n") {
                "\r\n"
            } else {
                "\n"
            };
            let lines: Vec<&str> = existing.lines().collect();
            let closing = lines
                .iter()
                .rposition(|line| line.trim_end() == "});")
                .ok_or_else(|| ScaffoldError::InvalidProjectSource {
                    path: path.display().to_string(),
                    reason: "its `nanoflowService(...)` does not close with `});`".to_string(),
                })?;
            let (start, end, mut words) =
                imported_words(&lines).ok_or_else(|| ScaffoldError::InvalidProjectSource {
                    path: path.display().to_string(),
                    reason: "it imports nothing from `@/mxrs/flows`".to_string(),
                })?;
            words.extend(vocabulary);
            let mut updated: Vec<String> = Vec::with_capacity(lines.len() + 12);
            updated.extend(lines[..start].iter().map(|line| line.to_string()));
            updated.extend(import_lines(&words));
            updated.extend(lines[end + 1..closing].iter().map(|line| line.to_string()));
            if lines[closing - 1].trim() != "{" && !lines[closing - 1].trim().is_empty() {
                updated.push(String::new());
            }
            updated.extend(text.lines().map(str::to_string));
            updated.extend(lines[closing..].iter().map(|line| line.to_string()));
            let mut source = updated.join(newline);
            source.push_str(newline);
            source
        }
        None => {
            let mut source = match &subject {
                Some(subject) => {
                    format!(
                        "// The {module_name} module's nanoflows about {}.\n",
                        subject.text()
                    )
                }
                None => format!("// The {module_name} module's nanoflows.\n"),
            };
            for line in import_lines(&vocabulary) {
                source.push_str(&line);
                source.push('\n');
            }
            source.push_str(&format!(
                "\nexport const {service} = nanoflowService({module_name:?}, {{\n{text}}});\n"
            ));
            source
        }
    };
    if transaction.content(&path)?.is_some() {
        transaction.write(&path, source)
    } else {
        transaction.create(&path, source)
    }
}

/// Where the import from `@/mxrs/flows` is, and what it names.
fn imported_words(lines: &[&str]) -> Option<(usize, usize, BTreeSet<String>)> {
    // The statement that ends at the flows module opens at the last
    // `import` before it; other imports are not its.
    let end = lines
        .iter()
        .position(|line| line.contains("from \"@/mxrs/flows\""))?;
    let start = (0..=end)
        .rev()
        .find(|index| lines[*index].starts_with("import "))?;
    let text = lines[start..=end].join(" ");
    let inside = text.split_once('{')?.1.split_once('}')?.0;
    let words = inside
        .split(',')
        .map(str::trim)
        .filter(|word| !word.is_empty())
        .map(str::to_string)
        .collect();
    Some((start, end, words))
}

/// The import of `words`, on one line when it fits.
fn import_lines(words: &BTreeSet<String>) -> Vec<String> {
    let one = format!(
        "import {{ {} }} from \"@/mxrs/flows\";",
        words.iter().cloned().collect::<Vec<_>>().join(", ")
    );
    if one.len() <= WIDTH {
        return vec![one];
    }
    let mut lines = vec!["import {".to_string()];
    lines.extend(words.iter().map(|word| format!("  {word},")));
    lines.push("} from \"@/mxrs/flows\";".to_string());
    lines
}

/// Names `name` in the module's `in_frontend.rs` for Rust pages: the line
/// to add to its `mxrs::frontend_flows!`, or the whole file.
pub(crate) fn marker_line(name: &str) -> String {
    format!("    nanoflow {name};")
}

/// `source` of an `in_frontend.rs` naming `name` too.
pub(crate) fn add_marker(source: &str, name: &str) -> Option<String> {
    let closing = source.rfind("\n}")?;
    let line = marker_line(name);
    if source.contains(&format!("{line}\n")) {
        return Some(source.to_string());
    }
    Some(format!(
        "{}\n{line}{}",
        &source[..closing],
        &source[closing..]
    ))
}
