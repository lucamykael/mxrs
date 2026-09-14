//! Acceptance pass against real production .mpr files (QRQC/SPC — real
//! customer Mendix 11.12.1 apps, not synthetic fixtures). Committed as an
//! `#[ignore]`d test parameterized by `MXRS_ACCEPTANCE_DIR` rather than a
//! hardcoded private path, so it survives across sessions/machines instead
//! of being silently lost as an untracked file (as it was through several
//! prior sessions — see `decisions/mxrs-rust-rewrite-plan.md`'s update-D
//! notes in this project's ai-memory). The referenced `.mpr`/`.mdp` files
//! themselves are still not committed (real customer data) — this only
//! commits the harness that consumes them.
//!
//! Run manually with:
//! ```text
//! MXRS_ACCEPTANCE_DIR=/path/to/dir cargo test -p mxrs-compiler-flow \
//!     --test acceptance_qrqc_spc -- --ignored --nocapture
//! ```
//! Expected directory layout under `MXRS_ACCEPTANCE_DIR`:
//! ```text
//! qrqc-ruby/build/eQRQC.mpr
//! spc-ruby/build/JEMScc-SPC.mpr
//! spc-ruby/build/deployment/model/model.mdp
//! ```
//! Default runs ignore this private-corpus test. Explicit `--ignored` runs
//! require every input and fail on missing evidence or compilation gaps.
//! Both projects are reported before asserting full compilation, so a QRQC
//! gap cannot hide SPC's results. This checks compilation, not runtime parity.

use std::collections::BTreeMap;

use mxrs_bson::Document;
use mxrs_compiler_flow::FlowCompiler;
use mxrs_model::Project;

fn run_against(path: &str, label: &str, model_package: Option<&str>) -> Vec<String> {
    let project = Project::open(path, true)
        .unwrap_or_else(|error| panic!("[{label}] cannot open private corpus {path}: {error}"));
    let existing_documents = model_package
        .map(|p| mxrs_schema::read_model_package(p).unwrap())
        .unwrap_or_default();
    let compiler = FlowCompiler::new(&project, &existing_documents).unwrap();
    let units = project.all_units().unwrap();

    let mut flow_ok = 0usize;
    let mut flow_err: BTreeMap<String, usize> = BTreeMap::new();
    let mut flow_total = 0usize;
    let mut code_action_ok = 0usize;
    let mut code_action_err: BTreeMap<String, usize> = BTreeMap::new();
    let mut code_action_total = 0usize;

    let modules = project.modules().unwrap();
    let module_name_by_id: std::collections::HashMap<String, String> = modules
        .iter()
        .filter_map(|m| Some((m.id.clone(), m.name.clone()?)))
        .collect();
    let parent_by_id: std::collections::HashMap<String, String> = units
        .iter()
        .map(|u| (u.unit_id.clone(), u.container_id.clone()))
        .collect();
    let owning_module = |container_id: &str| -> Option<String> {
        let mut current = container_id.to_string();
        for _ in 0..64 {
            if let Some(name) = module_name_by_id.get(&current) {
                return Some(name.clone());
            }
            let parent = parent_by_id.get(&current)?;
            if parent == &current {
                return None;
            }
            current = parent.clone();
        }
        None
    };

    for unit in &units {
        let doc: Document = project.mpr().parse_contents(unit).unwrap_or_else(|error| {
            panic!(
                "[{label}] cannot parse corpus unit {}: {error}",
                unit.unit_id
            )
        });
        let type_name = doc
            .get_str("$Type")
            .expect("corpus unit must have a model type");
        if !matches!(
            type_name,
            "Microflows$Microflow"
                | "Microflows$Nanoflow"
                | "Microflows$Rule"
                | "JavaActions$JavaAction"
                | "JavaScriptActions$JavaScriptAction"
        ) {
            continue;
        }
        let module_name = owning_module(&unit.container_id).unwrap_or_else(|| {
            panic!(
                "[{label}] corpus {type_name} unit {} has no resolvable owner module",
                unit.unit_id
            )
        });
        match type_name {
            "Microflows$Microflow" | "Microflows$Nanoflow" | "Microflows$Rule" => {
                flow_total += 1;
                match compiler.compile_flow(&doc, &module_name) {
                    Ok(_) => flow_ok += 1,
                    Err(e) => *flow_err.entry(format!("{e}")).or_default() += 1,
                }
            }
            "JavaActions$JavaAction" | "JavaScriptActions$JavaScriptAction" => {
                code_action_total += 1;
                match compiler.compile_code_action(&doc, Some(&module_name)) {
                    Ok(_) => code_action_ok += 1,
                    Err(e) => *code_action_err.entry(format!("{e}")).or_default() += 1,
                }
            }
            _ => {}
        }
    }

    println!(
        "=== {label} (Mendix {:?}) ===",
        project.mendix_version().ok().flatten()
    );
    println!("flows: {flow_ok}/{flow_total} ok");
    for (err, count) in &flow_err {
        println!("  {count:>4}x {err}");
    }
    println!("code actions: {code_action_ok}/{code_action_total} ok");
    for (err, count) in &code_action_err {
        println!("  {count:>4}x {err}");
    }
    println!(
        "diagnostics: {} unconfigured write(s)",
        compiler.diagnostics().len()
    );

    // Nanoflow JS compilation pass (separate compiler, own unsupported list).
    let index = compiler.index();
    let project_root = std::path::Path::new(path).parent();
    let mut nanoflow_compiler =
        mxrs_compiler_flow::nanoflow::NanoflowCompiler::new(index, project_root);
    let names: Vec<String> = index.nanoflows.keys().cloned().collect();
    let mut nano_ok = 0usize;
    for name in &names {
        if nanoflow_compiler.reference(name).is_some() {
            nano_ok += 1;
        }
    }
    println!("nanoflows: {nano_ok}/{} produced a JS program", names.len());
    let mut unsupported_counts: BTreeMap<String, usize> = BTreeMap::new();
    for u in nanoflow_compiler.unsupported() {
        *unsupported_counts.entry(u.node_type.clone()).or_default() += 1;
        println!("[{label}] unsupported {} in {}", u.node_type, u.flow);
    }
    for (kind, count) in &unsupported_counts {
        println!("  {count:>4}x unsupported {kind}");
    }
    let mut failures = Vec::new();
    if flow_total == 0 || code_action_total == 0 || names.is_empty() {
        failures.push(format!("[{label}] incomplete corpus inventory: flows={flow_total}, code actions={code_action_total}, nanoflows={}", names.len()));
    }
    if !flow_err.is_empty() {
        failures.push(format!(
            "[{label}] {} flow(s) failed compilation: {flow_err:#?}",
            flow_total - flow_ok
        ));
    }
    if !code_action_err.is_empty() {
        failures.push(format!(
            "[{label}] {} code action(s) failed compilation: {code_action_err:#?}",
            code_action_total - code_action_ok
        ));
    }
    if nano_ok != names.len() || !unsupported_counts.is_empty() {
        failures.push(format!(
            "[{label}] {nano_ok}/{} nanoflows compiled; unsupported nodes: {unsupported_counts:#?}",
            names.len()
        ));
    }
    failures
}

#[test]
#[ignore = "requires MXRS_ACCEPTANCE_DIR with complete QRQC/SPC private corpus"]
fn acceptance_pass_against_real_projects() {
    let base = std::env::var("MXRS_ACCEPTANCE_DIR")
        .expect("set MXRS_ACCEPTANCE_DIR to the complete private QRQC/SPC corpus root before explicitly running this ignored test");
    let base = std::path::Path::new(&base);
    assert!(
        base.is_dir(),
        "MXRS_ACCEPTANCE_DIR is not a directory: {}",
        base.display()
    );
    for required in [
        "qrqc-ruby/build/eQRQC.mpr",
        "spc-ruby/build/JEMScc-SPC.mpr",
        "spc-ruby/build/deployment/model/model.mdp",
    ] {
        assert!(
            base.join(required).is_file(),
            "missing private corpus input: {}",
            base.join(required).display()
        );
    }

    let mut failures = run_against(
        base.join("qrqc-ruby/build/eQRQC.mpr").to_str().unwrap(),
        "QRQC",
        None,
    );
    let spc_mpr = base.join("spc-ruby/build/JEMScc-SPC.mpr");
    let spc_mdp = base.join("spc-ruby/build/deployment/model/model.mdp");
    failures.extend(run_against(
        spc_mpr.to_str().unwrap(),
        "SPC",
        spc_mdp.to_str(),
    ));
    assert!(
        failures.is_empty(),
        "private corpus compilation is incomplete:\n{}",
        failures.join("\n")
    );
}
