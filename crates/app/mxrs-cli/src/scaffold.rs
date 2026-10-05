//! Front end for `mxrs-scaffold`'s artifact generators, the registry, and
//! project inspection — the `mxrs` equivalents of mxrb's `Scaffold::CLI`
//! (`lib/mxrb/scaffold/cli.rb`), its `when 'scaffold'`/`when 'module'` cases,
//! and `when 'project'` (`bin/mxrb`).
//!
//! The contract kept from mxrb: an explicit action word before the name
//! (`new`, or `init` for `security`), `--target` defaulting to the working
//! directory, `--dry-run` previewing without writing, `--json` rendering the
//! same result as a document, absolute paths in both renderings, and the
//! `create`/`would create`/`update` line prefixes.
//!
//! The one deliberate difference in the human rendering is the closing hint:
//! mxrb points at `bundle exec mxrb generate project.rb`, mxrs at
//! [`BUILD_HINT`], because that is the command that actually turns these
//! declarations into an `.mpr` here.
//!
//! No model logic lives in this module: every decision about what a scaffold
//! writes belongs to `mxrs-scaffold`, and this file only parses arguments and
//! renders results.

use mxrs_scaffold::{
    ArtifactKind, ArtifactScaffold, PageChain, ProjectInspection, SCAFFOLD_COMMANDS,
    ScaffoldOutcome, inspect_project, page_templates, registry, scaffold_artifact,
};

use crate::arguments::{take_flag, take_value, take_values};

const BUILD_HINT: &str = "cargo mxrs build";

/// Argument grammar of one scaffold command, kept in `mxrs-scaffold` so the
/// generator catalog, `mxrs scaffold list`, and this parser cannot drift.
pub fn usage(kind: ArtifactKind) -> String {
    let command = command(kind);
    // `page` carries both its generator options and a second form for the
    // template catalog, the way mxrb's own help states them on one line.
    let (page_options, page_forms) = if kind == ArtifactKind::Page {
        (
            " [--template NAME] [--chain CHAIN] [--role Module.Role]",
            " | templates [--json]",
        )
    } else if kind == ArtifactKind::DemoUser {
        (" [--entity Module.Entity] [--role ROLE]", "")
    } else if kind == ArtifactKind::Design {
        (
            "",
            " | scan <file.mpr> [--json] | migrate <file.mpr> <literal> <token> [--apply] [--json]",
        )
    } else if kind == ArtifactKind::Module {
        (
            "",
            " | search [query] --registry SOURCE [--json] | add <name|directory> --registry SOURCE [--target DIR] [--json]",
        )
    } else {
        ("", "")
    };
    let argument = if command.argument.is_empty() {
        String::new()
    } else {
        format!(" {}", command.argument)
    };
    format!(
        "{} {}{argument}{page_options} [--target DIR] [--dry-run] [--json]{page_forms}",
        command.name, command.action
    )
}

fn command(kind: ArtifactKind) -> &'static mxrs_scaffold::ScaffoldCommand {
    SCAFFOLD_COMMANDS
        .iter()
        .find(|command| command.kind == kind)
        .expect("every artifact kind is catalogued")
}

/// Runs one artifact generator end to end. `arguments` is what followed the
/// command name, so the action word is still the first element.
pub fn generate(kind: ArtifactKind, mut arguments: Vec<String>) -> Result<(), String> {
    let json = take_flag(&mut arguments, "--json");
    let dry_run = take_flag(&mut arguments, "--dry-run");
    let target = take_value(&mut arguments, "--target").unwrap_or_else(|| ".".to_string());
    let roles = take_values(&mut arguments, "--role");
    let template = take_value(&mut arguments, "--template");
    let chain = take_value(&mut arguments, "--chain");
    let entity = take_value(&mut arguments, "--entity");
    let expected = command(kind).action;
    // MXRB's `new` is optional for demo-user (`mxrb demo-user manager` works).
    if kind == ArtifactKind::DemoUser
        && arguments.len() == 1
        && arguments[0] != expected
        && !arguments[0].starts_with('-')
    {
        arguments.insert(0, expected.to_string());
    }
    // `mxrs page templates` is the catalog, not a generator: it takes no name
    // and none of the generator options.
    if kind == ArtifactKind::Page
        && arguments.first().is_some_and(|word| word == "templates")
        && arguments.len() == 1
        && !dry_run
        && roles.is_empty()
        && template.is_none()
        && chain.is_none()
        && entity.is_none()
    {
        print_page_templates(json);
        return Ok(());
    }
    // `design init` names no artifact: the whole theme kit is the target.
    if kind == ArtifactKind::Design {
        if arguments.as_slice() != [expected.to_string()]
            || !roles.is_empty()
            || template.is_some()
            || chain.is_some()
            || entity.is_some()
        {
            return Err(format!("usage: mxrs {}", usage(kind)));
        }
        let outcome =
            scaffold_artifact(&ArtifactScaffold::new(kind, "project", target).dry_run(dry_run))
                .map_err(|error| error.to_string())?;
        render(&outcome, json);
        return Ok(());
    }
    let page_only = template.is_some() || chain.is_some();
    let roles_allowed = matches!(kind, ArtifactKind::Page | ArtifactKind::DemoUser);
    let [action, name] = arguments.as_slice() else {
        return Err(format!("usage: mxrs {}", usage(kind)));
    };
    if action != expected
        || (page_only && kind != ArtifactKind::Page)
        || (!roles.is_empty() && !roles_allowed)
        || (entity.is_some() && kind != ArtifactKind::DemoUser)
    {
        return Err(format!("usage: mxrs {}", usage(kind)));
    }
    let chain = chain
        .map(|value| PageChain::parse(&value))
        .transpose()
        .map_err(|error| error.to_string())?;
    let outcome = scaffold_artifact(
        &ArtifactScaffold::new(kind, name, target)
            .dry_run(dry_run)
            .page_roles(roles)
            .page_template(template)
            .page_chain(chain)
            .demo_entity(entity),
    )
    .map_err(|error| error.to_string())?;
    render(&outcome, json);
    Ok(())
}

fn print_page_templates(json: bool) {
    if json {
        let payload: Vec<_> = page_templates::grouped()
            .into_iter()
            .map(|(category, entries)| {
                serde_json::json!({
                    "category": category,
                    "templates": entries
                        .iter()
                        .map(|entry| serde_json::json!({
                            "name": entry.name,
                            "description": entry.description,
                            "data_backed": entry.data_backed,
                        }))
                        .collect::<Vec<_>>(),
                })
            })
            .collect();
        println!(
            "{}",
            serde_json::to_string_pretty(&payload).expect("the template catalog is serializable")
        );
    } else {
        println!("{}", page_templates::tree());
    }
}

fn render(outcome: &ScaffoldOutcome, json: bool) {
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "kind": outcome.kind,
                "name": outcome.name,
                "dry_run": outcome.dry_run,
                "files": paths(&outcome.files),
                "updated": paths(&outcome.updated),
                "notes": outcome.notes,
            }))
            .expect("a scaffold outcome is serializable")
        );
        return;
    }
    let prefix = if outcome.dry_run {
        "would create"
    } else {
        "create"
    };
    for file in &outcome.files {
        println!("  {prefix}  {}", file.display());
    }
    for file in &outcome.updated {
        println!("  update  {}", file.display());
    }
    for note in &outcome.notes {
        println!("  note    {note}");
    }
    println!("\nDone. Run:\n  {BUILD_HINT}");
}

fn paths(paths: &[std::path::PathBuf]) -> Vec<String> {
    paths
        .iter()
        .map(|path| path.display().to_string())
        .collect()
}

pub const REGISTRY_USAGE: &str = "usage: mxrs scaffold <list|destroy> [<kind:name>] [--target DIR]";

/// `mxrs scaffold list` prints every generator and where it writes, then the
/// scaffolds this project has registered; `destroy` removes one.
pub fn registry_command(mut arguments: Vec<String>) -> Result<(), String> {
    let target = take_value(&mut arguments, "--target").unwrap_or_else(|| ".".to_string());
    match arguments.as_slice() {
        [action] if action == "list" => {
            println!("command\tdestination");
            for command in SCAFFOLD_COMMANDS {
                println!("{}\t{}", command.name, command.destination);
            }
            let registered = registry::entries(&target).map_err(|error| error.to_string())?;
            if !registered.is_empty() {
                println!("\nregistered scaffold\tfiles");
                for entry in registered {
                    println!("{}\t{}", entry.key, entry.files.len());
                }
            }
            Ok(())
        }
        [action, key] if action == "destroy" => {
            let removal = registry::destroy(&target, key).map_err(|error| error.to_string())?;
            for file in removal.files {
                println!("  remove  {}", file.display());
            }
            Ok(())
        }
        _ => Err(REGISTRY_USAGE.to_string()),
    }
}

pub const PROJECT_USAGE: &str = "usage: mxrs project inspect [DIR] [--json]";

pub fn project_command(mut arguments: Vec<String>) -> Result<(), String> {
    take_flag(&mut arguments, "--no-progress");
    let json = take_flag(&mut arguments, "--json");
    let target = match arguments.as_slice() {
        [action] if action == "inspect" => ".".to_string(),
        [action, directory] if action == "inspect" => directory.clone(),
        _ => return Err(PROJECT_USAGE.to_string()),
    };
    let inspection = inspect_project(&target).map_err(|error| error.to_string())?;
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&document(&inspection))
                .expect("a project inspection is serializable")
        );
    } else {
        for (key, value) in fields(&inspection) {
            println!("{key}: {value}");
        }
    }
    Ok(())
}

fn document(inspection: &ProjectInspection) -> serde_json::Value {
    serde_json::json!({
        "root": inspection.root.display().to_string(),
        "manifest": inspection.manifest,
        "domain_module": inspection.domain_module,
        "layout": inspection.layout.as_str(),
        "declared_version": inspection.declared_version,
        "modules": inspection.modules,
        "mprs": paths(&inspection.mprs),
        "registered_scaffolds": inspection.registered_scaffolds,
    })
}

/// Mirrors mxrb's `payload.each { puts "#{key}: #{Array(value).join(', ')}" }`,
/// so a list renders as one comma-separated line and an absent version renders
/// as an empty value rather than a guessed one.
fn fields(inspection: &ProjectInspection) -> Vec<(&'static str, String)> {
    vec![
        ("root", inspection.root.display().to_string()),
        ("manifest", inspection.manifest.to_string()),
        ("domain_module", inspection.domain_module.to_string()),
        ("layout", inspection.layout.to_string()),
        (
            "declared_version",
            inspection.declared_version.clone().unwrap_or_default(),
        ),
        ("modules", inspection.modules.join(", ")),
        ("mprs", paths(&inspection.mprs).join(", ")),
        (
            "registered_scaffolds",
            inspection.registered_scaffolds.join(", "),
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_catalogued_generator_has_a_usage_line_naming_its_action() {
        for command in SCAFFOLD_COMMANDS {
            let usage = usage(command.kind);
            assert!(usage.starts_with(&format!("{} {} ", command.name, command.action)));
            // Every generator ends on the shared option tail; `page` alone
            // continues with a second form for its template catalog.
            let generator_form = usage
                .split_once(" | ")
                .map_or(usage.as_str(), |(generator, _)| generator);
            assert!(generator_form.ends_with("[--target DIR] [--dry-run] [--json]"));
            assert_eq!(
                usage.contains(" | templates [--json]"),
                command.kind == ArtifactKind::Page
            );
            assert_eq!(
                usage.contains("--role"),
                matches!(command.kind, ArtifactKind::Page | ArtifactKind::DemoUser)
            );
            assert_eq!(
                usage.contains("--entity"),
                command.kind == ArtifactKind::DemoUser
            );
        }
    }

    #[test]
    fn a_missing_or_wrong_action_word_is_a_usage_error_not_a_scaffold() {
        for arguments in [
            vec![],
            vec!["new"],
            vec!["Sales.Order"],
            vec!["init", "Sales.Order"],
            vec!["new", "Sales.Order", "extra"],
        ] {
            let arguments = arguments.iter().map(ToString::to_string).collect();
            assert!(
                generate(ArtifactKind::Entity, arguments)
                    .unwrap_err()
                    .contains("usage: mxrs entity new")
            );
        }
        assert!(
            generate(
                ArtifactKind::Entity,
                ["new", "Sales.Order", "--role", "Sales.User"]
                    .map(str::to_string)
                    .to_vec()
            )
            .unwrap_err()
            .contains("usage: mxrs entity new")
        );
    }

    #[test]
    fn registry_and_project_subcommands_reject_unknown_actions() {
        for arguments in [
            vec![],
            vec!["remove"],
            vec!["list", "extra"],
            vec!["destroy"],
        ] {
            let arguments = arguments.iter().map(ToString::to_string).collect();
            assert_eq!(registry_command(arguments).unwrap_err(), REGISTRY_USAGE);
        }
        for arguments in [vec![], vec!["show"], vec!["inspect", "a", "b"]] {
            let arguments = arguments.iter().map(ToString::to_string).collect();
            assert_eq!(project_command(arguments).unwrap_err(), PROJECT_USAGE);
        }
    }

    #[test]
    fn an_inspection_renders_the_same_facts_as_text_and_as_json() {
        let inspection = ProjectInspection {
            root: std::path::PathBuf::from("/projects/orders"),
            manifest: true,
            domain_module: true,
            layout: mxrs_scaffold::lifecycle::ProjectLayout::Layered,
            declared_version: None,
            modules: vec!["sales".into(), "billing".into()],
            mprs: vec![std::path::PathBuf::from(
                "/projects/orders/build/Orders.mpr",
            )],
            registered_scaffolds: vec!["module:Sales".into()],
        };
        let fields = fields(&inspection);
        let document = document(&inspection);
        assert_eq!(fields[0].1, "/projects/orders");
        assert_eq!(fields[3].1, "layered");
        assert_eq!(fields[4].1, "");
        assert_eq!(fields[5].1, "sales, billing");
        assert!(document["declared_version"].is_null());
        assert_eq!(document["layout"], "layered");
        assert_eq!(document["modules"][1], "billing");
        assert_eq!(fields.len(), document.as_object().unwrap().len());
    }
}
