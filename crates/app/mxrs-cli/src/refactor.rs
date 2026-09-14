//! Front end for `mxrs-refactor` — the mxrs equivalents of mxrb's `rename`,
//! `remove` and `move` commands.
//!
//! The contract kept from mxrb: the artifact is named by qualified name, the
//! command previews by default and only writes under `--apply`, and a blocked
//! removal exits nonzero so a script cannot mistake "refused" for "done".
//!
//! The `.mpr` is opened read-only for a preview and writable only for an
//! apply, so a preview cannot modify the file even if something below it
//! tried.

use std::process::ExitCode;

use mxrs_model::Project;
use mxrs_refactor::{RefactorError, plan_move, plan_remove, plan_rename};

use crate::arguments::take_flag;

pub fn run_rename(mut args: Vec<String>) -> ExitCode {
    let json = take_flag(&mut args, "--json");
    let apply = take_flag(&mut args, "--apply");
    let [path, old_name, new_name] = args.as_slice() else {
        eprintln!(
            "[mxrs] error: usage: mxrs rename <file.mpr> <old-name> <new-name> [--apply] [--json]"
        );
        return ExitCode::FAILURE;
    };
    let (old_name, new_name) = (old_name.clone(), new_name.clone());

    reported(|| {
        let mut project = Project::open(path, !apply)?;
        let plan = plan_rename(&project, &old_name, &new_name)?;
        let (target, changes, units) = (
            plan.target.clone(),
            plan.changes.clone(),
            plan.affected_units(),
        );
        if apply {
            plan.apply(&mut project)?;
        }
        if json {
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "target": target,
                    "applied": apply,
                    "units": units,
                    "changes": changes,
                }))
                .expect("a rename plan is serializable")
            );
        } else {
            for change in &changes {
                println!(
                    "{}\t{}\t{:?} => {:?}",
                    change.unit_id, change.path, change.before, change.after
                );
            }
            println!(
                "[mxrs] {}: {} change(s) in {units} unit(s)",
                if apply { "Renamed" } else { "Preview" },
                changes.len()
            );
        }
        Ok(ExitCode::SUCCESS)
    })
}

pub fn run_remove(mut args: Vec<String>) -> ExitCode {
    let json = take_flag(&mut args, "--json");
    let apply = take_flag(&mut args, "--apply");
    let [path, name] = args.as_slice() else {
        eprintln!(
            "[mxrs] error: usage: mxrs remove <file.mpr> <qualified-name> [--apply] [--json]"
        );
        return ExitCode::FAILURE;
    };
    let name = name.clone();

    reported(|| {
        let mut project = Project::open(path, !apply)?;
        let plan = plan_remove(&project, &name)?;
        let safe = plan.is_safe();
        let artifact = plan.artifact.clone();
        let incoming = plan.incoming.clone();
        let children = plan.children.len();
        // Applying an unsafe plan is an error rather than a silent skip, so
        // only a safe plan is applied and the exit code carries the rest.
        if apply && safe {
            plan.apply(&mut project)?;
        }
        if json {
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "artifact": artifact,
                    "safe": safe,
                    "applied": apply && safe,
                    "children": children,
                    "incoming": incoming,
                }))
                .expect("a removal plan is serializable")
            );
        } else {
            println!(
                "Artifact            : {} ({})",
                artifact.qualified_name, artifact.kind
            );
            println!("Incoming references : {}", incoming.len());
            for reference in &incoming {
                println!("  {}\t{}", reference.from, reference.relation);
            }
            println!("Child units         : {children}");
            println!(
                "[mxrs] {}",
                match (apply, safe) {
                    (true, true) => "Removed",
                    (_, true) => "Safe removal preview",
                    _ => "Blocked removal",
                }
            );
        }
        Ok(if safe {
            ExitCode::SUCCESS
        } else {
            ExitCode::FAILURE
        })
    })
}

pub fn run_move(mut args: Vec<String>) -> ExitCode {
    let json = take_flag(&mut args, "--json");
    let apply = take_flag(&mut args, "--apply");
    let [path, name, container] = args.as_slice() else {
        eprintln!(
            "[mxrs] error: usage: mxrs move <file.mpr> <name> <container> [--apply] [--json]"
        );
        return ExitCode::FAILURE;
    };
    let (name, container) = (name.clone(), container.clone());

    reported(|| {
        let mut project = Project::open(path, !apply)?;
        let plan = plan_move(&project, &name, &container)?;
        let artifact = plan.artifact.clone();
        let (before, after) = (plan.before_container.clone(), plan.after_container.clone());
        let already_there = plan.is_empty();
        if apply {
            plan.apply(&mut project)?;
        }
        if json {
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "artifact": artifact,
                    "before_container": before,
                    "after_container": after,
                    "applied": apply && !already_there,
                }))
                .expect("a move plan is serializable")
            );
        } else {
            println!(
                "Artifact  : {} ({})",
                artifact.qualified_name, artifact.kind
            );
            println!("Container : {before} -> {after}");
            println!(
                "[mxrs] {}",
                match (apply, already_there) {
                    (_, true) => "Already in container",
                    (true, _) => "Moved",
                    _ => "Move preview",
                }
            );
        }
        Ok(ExitCode::SUCCESS)
    })
}

/// Shared error plumbing: every refactoring reports the same way, and a
/// failure must not leave a half-printed preview looking like a result.
fn reported(run: impl FnOnce() -> Result<ExitCode, RefactorError>) -> ExitCode {
    match run() {
        Ok(code) => code,
        Err(error) => {
            eprintln!("[mxrs] error: {error}");
            ExitCode::FAILURE
        }
    }
}
