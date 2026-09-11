//! `xtask mxbuild-oracle` — runs the real, official Mendix compiler
//! (`mxbuild`) against an app directory as a second, independent oracle
//! alongside `mxrb` (D3 in `decisions/mxrs-rust-rewrite-plan.md` in this
//! project's ai-memory). Dev-only, like every other `xtask` command: never
//! a runtime dependency of anything mxrs ships, and never invoked by
//! `mxrs-cli` itself.
//!
//! # What this unlocks
//!
//! `mxrb` can only oracle the *editor-shape* (`.mpr`) side of things —
//! `xtask oracle-diff` already covers that. Nothing in this workspace could
//! previously check mxrs's compiler-path crates (`mxrs-compiler-domain`/
//! `-flow`/`-widgets`) against Runtime-shape ground truth, because
//! producing that ground truth means running the actual Mendix compiler.
//! `mxbuild` does exactly that: it reads a full app directory and writes a
//! Runtime-shape `deployment/model/model.mdp` (ordered BSON documents,
//! `mxrs_schema::read_model_package` already reads this format) —
//! independent of the model, this is authoritative Mendix output, not a
//! guess.
//!
//! # A real, load-bearing limitation — read before assuming success or failure means what it usually does
//!
//! `mxbuild --target=package`'s *overall* exit code also depends on later
//! steps this command doesn't care about (compiling `javasource/**/*.java`
//! against the project's own Java dependencies, which mxrs has nothing to
//! do with and can genuinely fail for reasons unrelated to model
//! correctness — verified directly against a real customer app in this
//! session: `deployment/model/model.mdp` was written correctly even though
//! the overall build failed later on pre-existing Java compile errors in
//! that app's own custom code). So this command's success criterion is
//! **"did `deployment/model/model.mdp` get written"**, checked directly,
//! not `mxbuild`'s process exit code — an all-or-nothing gate on the exit
//! code would produce false negatives on real projects with unrelated Java
//! issues, which is worse than useless for a model-shape oracle.
//!
//! **Also confirmed, and still unresolved as of this writing**: pointing
//! this at one of `xtask/fixtures/*` (an `mxrb generate`-produced synthetic
//! app, not a real Studio-Pro-authored one) currently fails *before*
//! `model.mdp` is ever written, with an opaque
//! `System.NullReferenceException` inside `mxbuild`'s closed-source
//! `DeploymentProcessBuilder.ExecuteDeploymentWorkForPhase` during its
//! "Checking for errors" step — with no visibility into `mxbuild`'s source,
//! the specific field/document this trips on is not yet identified. A real
//! Studio-Pro-authored app directory (verified against a real customer
//! app's original, unconverted project folder) does **not** hit this and
//! proceeds normally through model processing. Flagging this loudly rather
//! than silently working around it: **this command is proven against real
//! app directories, not yet against mxrs's own synthetic fixtures** — do
//! not assume it exercises `xtask/fixtures/*` until that gap is closed.
//!
//! # Usage
//! ```text
//! cargo run -p xtask -- mxbuild-oracle <app_dir>
//! ```
//! `<app_dir>` must contain a `.mpr` directly inside it (a real Studio Pro
//! project folder, or any directory shaped like one). Environment
//! overrides, mirroring `oracle_diff`'s `MXRB_HOME` convention:
//! - `MXBUILD_PATH` — path to the `mxbuild` executable (default:
//!   `~/.local/share/mendix/11.12.1/modeler/mxbuild`, this machine's local
//!   Mendix 11.12.1 install).
//! - `MXBUILD_JAVA_HOME` — a JDK 21 home `mxbuild` requires explicitly
//!   (default: this machine's `mise`-installed Zulu 21).

use std::path::{Path, PathBuf};
use std::process::Command;

use mxrs_bson::Document;

fn mxbuild_path() -> PathBuf {
    std::env::var("MXBUILD_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            PathBuf::from(std::env::var("HOME").unwrap_or_default())
                .join(".local/share/mendix/11.12.1/modeler/mxbuild")
        })
}

fn java_home() -> PathBuf {
    std::env::var("MXBUILD_JAVA_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            PathBuf::from(std::env::var("HOME").unwrap_or_default())
                .join(".local/share/mise/installs/java/zulu-21.50.19.0")
        })
}

pub fn mxbuild_oracle(app_dir: &Path) -> Result<(), String> {
    let app_dir = std::path::absolute(app_dir).map_err(|e| e.to_string())?;
    let mpr_path = find_mpr_file(&app_dir).ok_or("no .mpr file found directly inside app_dir")?;
    let mxbuild = mxbuild_path();
    if !mxbuild.is_file() {
        return Err(format!(
            "mxbuild not found at {} (override with MXBUILD_PATH)",
            mxbuild.display()
        ));
    }
    let java_home = java_home();
    let java_exe = java_home.join("bin/java");

    println!(
        "[xtask] mxbuild-oracle: running {} on {}",
        mxbuild.display(),
        mpr_path.display()
    );
    let output = Command::new(&mxbuild)
        .arg("--target=package")
        .arg("-o")
        .arg(app_dir.join("deployment").join("out.mda"))
        .arg(format!("--java-home={}", java_home.display()))
        .arg(format!("--java-exe-path={}", java_exe.display()))
        .arg(&mpr_path)
        .current_dir(&app_dir)
        .output()
        .map_err(|e| format!("failed to spawn mxbuild: {e}"))?;

    let model_mdp = app_dir.join("deployment/model/model.mdp");
    if !model_mdp.is_file() {
        let stdout = String::from_utf8_lossy(&output.stdout);
        let tail: String = stdout
            .lines()
            .rev()
            .take(20)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect::<Vec<_>>()
            .join("\n");
        return Err(format!(
            "mxbuild did not produce deployment/model/model.mdp (exit status: {}); last output lines:\n{tail}",
            output.status
        ));
    }

    let documents = mxrs_schema::read_model_package(&model_mdp).map_err(|e| e.to_string())?;
    report(&documents);

    if !output.status.success() {
        println!(
            "[xtask] mxbuild-oracle: NOTE - mxbuild's own exit status was non-zero ({}), but model.mdp was written successfully — this is expected when the failure is in a later, model-independent step (e.g. javasource compilation). See this file's module doc for why exit code alone isn't the success signal here.",
            output.status
        );
    }
    println!("[xtask] PASS");
    Ok(())
}

fn report(documents: &[Document]) {
    let mut counts: std::collections::BTreeMap<String, usize> = std::collections::BTreeMap::new();
    for doc in documents {
        let type_name = doc
            .get_str("$Type")
            .map(str::to_string)
            .unwrap_or_else(|_| "<untyped>".to_string());
        *counts.entry(type_name).or_default() += 1;
    }
    println!(
        "[xtask] mxbuild-oracle: {} Runtime document(s), {} distinct $Type(s)",
        documents.len(),
        counts.len()
    );
    for (type_name, count) in counts.iter().filter(|(_, c)| **c >= 5) {
        println!("  {count:>5}x {type_name}");
    }
}

fn find_mpr_file(dir: &Path) -> Option<PathBuf> {
    std::fs::read_dir(dir)
        .ok()?
        .filter_map(Result::ok)
        .map(|e| e.path())
        .find(|p| p.extension().and_then(|e| e.to_str()) == Some("mpr"))
}
