//! Acceptance pass against an explicitly supplied, unversioned `.mpr` corpus.
//! The harness is generic by design: project names, directory layouts and
//! corpus-specific baselines never become repository metadata.
//!
//! Run manually with:
//! ```text
//! MXRS_ACCEPTANCE_PROJECTS=/path/a.mpr:/path/b.mpr \
//! MXRS_ACCEPTANCE_MODEL_PACKAGES=/path/a.mdp:/path/b.mdp \
//! cargo test -p mxrs-compiler-flow --test acceptance_corpus -- --ignored --nocapture
//! ```
//! Both variables are platform path lists. Model packages are optional and
//! combined into the compiler's read-only counterpart catalog. Default runs
//! ignore this acceptance test; explicit runs fail on missing evidence or any
//! compilation gap. This checks compilation, not runtime parity.

use std::collections::BTreeMap;

use mxrs_bson::Document;
use mxrs_compiler_flow::FlowCompiler;
use mxrs_model::Project;

fn corpus_paths(variable: &str, required: bool) -> Vec<std::path::PathBuf> {
    let Some(value) = std::env::var_os(variable) else {
        assert!(
            !required,
            "set {variable} before explicitly running this ignored test"
        );
        return Vec::new();
    };
    let paths = std::env::split_paths(&value).collect::<Vec<_>>();
    assert!(!paths.is_empty(), "{variable} contains no paths");
    for path in &paths {
        assert!(
            path.is_file(),
            "{variable} input is not a file: {}",
            path.display()
        );
    }
    paths
}

fn run_against(
    path: &std::path::Path,
    label: &str,
    existing_documents: &[Document],
) -> Vec<String> {
    let project = Project::open(path, true)
        .unwrap_or_else(|error| panic!("[{label}] cannot open corpus {}: {error}", path.display()));
    let compiler = FlowCompiler::new(&project, existing_documents).unwrap();
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
    let project_root = path.parent();
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
#[ignore = "requires MXRS_ACCEPTANCE_PROJECTS with an authorized local corpus"]
fn acceptance_pass_against_configured_projects() {
    let projects = corpus_paths("MXRS_ACCEPTANCE_PROJECTS", true);
    let existing_documents = corpus_paths("MXRS_ACCEPTANCE_MODEL_PACKAGES", false)
        .into_iter()
        .flat_map(|path| {
            mxrs_schema::read_model_package(&path).unwrap_or_else(|error| {
                panic!("cannot read model package {}: {error}", path.display())
            })
        })
        .collect::<Vec<_>>();
    let mut failures = Vec::new();
    for (index, project) in projects.iter().enumerate() {
        failures.extend(run_against(
            project,
            &format!("case-{}", index + 1),
            &existing_documents,
        ));
    }
    assert!(
        failures.is_empty(),
        "acceptance corpus compilation is incomplete:\n{}",
        failures.join("\n")
    );
}
