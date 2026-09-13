//! Structural oracle for looped activities in a real Mendix project.
//!
//! This is ignored because the customer project and the `model.mdp` produced
//! by the official compiler are deliberately not repository fixtures. Run it
//! with the same `MXRS_ACCEPTANCE_DIR` layout documented by
//! `acceptance_qrqc_spc.rs`.
//!
//! ```text
//! MXRS_ACCEPTANCE_DIR=/path/to/dir cargo test -p mxrs-compiler-flow \
//!     --test loop_oracle -- --ignored --nocapture
//! ```

use std::collections::{BTreeMap, HashMap, HashSet};

use mxrs_bson::{Bson, Document};
use mxrs_compiler_flow::FlowCompiler;
use mxrs_model::Project;

const LOOP: &str = "Microflows$LoopedActivity";

fn visit(value: &Bson, callback: &mut impl FnMut(&Document)) {
    match value {
        Bson::Document(document) => {
            callback(document);
            for (_, child) in document {
                visit(child, callback);
            }
        }
        Bson::Array(items) => {
            for child in mxrs_bson::parse_array(Some(items)).items {
                visit(&child, callback);
            }
        }
        _ => {}
    }
}

fn document_id(document: &Document) -> Option<String> {
    document.get("$ID").and_then(mxrs_bson::extract_id)
}

fn collect_loops(roots: impl IntoIterator<Item = Document>) -> HashMap<String, Document> {
    let mut loops = HashMap::new();
    for root in roots {
        visit(&Bson::Document(root), &mut |document| {
            if document.get_str("$Type").ok() == Some(LOOP)
                && let Some(id) = document_id(document)
            {
                loops.insert(id, document.clone());
            }
        });
    }
    loops
}

/// Loop ID -> (root ID, root type). Names are intentionally not retained or
/// printed: this gate verifies customer data without leaking its vocabulary.
fn loop_owners(roots: &[Document]) -> HashMap<String, (String, String)> {
    let mut owners = HashMap::new();
    for root in roots {
        let root_id = document_id(root).unwrap_or_default();
        let root_type = root.get_str("$Type").unwrap_or("<no-type>").to_string();
        visit(&Bson::Document(root.clone()), &mut |document| {
            if document.get_str("$Type").ok() == Some(LOOP)
                && let Some(id) = document_id(document)
            {
                owners.insert(id, (root_id.clone(), root_type.clone()));
            }
        });
    }
    owners
}

fn sorted_keys(document: &Document) -> String {
    let mut keys: Vec<&str> = document.keys().map(String::as_str).collect();
    keys.sort_unstable();
    keys.join(",")
}

fn child_shape(document: &Document, field: &str) -> String {
    let Ok(child) = document.get_document(field) else {
        return format!("{field}=<missing>");
    };
    format!(
        "{field}={}:{}",
        child.get_str("$Type").unwrap_or("<no-type>"),
        sorted_keys(child)
    )
}

fn shape(document: &Document) -> String {
    format!(
        "loop=[{}] {} {}",
        sorted_keys(document),
        child_shape(document, "ObjectCollection"),
        child_shape(document, "LoopSource")
    )
}

fn array_documents(document: &Document, field: &str) -> Vec<Document> {
    let items = document
        .get_array(field)
        .ok()
        .map(Vec::as_slice)
        .unwrap_or_default();
    mxrs_bson::parse_array(Some(items))
        .items
        .into_iter()
        .filter_map(|value| match value {
            Bson::Document(document) => Some(document),
            _ => None,
        })
        .collect()
}

/// Keeps every loop-specific value while reducing ordinary body nodes to
/// identity and type. Fields derived by other compiler features (retrieve
/// result type, action result variable, etc.) are deliberately outside this
/// oracle; nested loops remain fully projected and checked recursively.
fn loop_projection(loop_document: &Document) -> Document {
    let mut projected = Document::new();
    for field in ["$ID", "$Type", "LoopSource", "ErrorHandlingType"] {
        if let Some(value) = loop_document.get(field) {
            projected.insert(field, value.clone());
        }
    }
    let collection = loop_document
        .get_document("ObjectCollection")
        .cloned()
        .unwrap_or_default();
    let mut projected_collection = Document::new();
    for field in ["$ID", "$Type"] {
        if let Some(value) = collection.get(field) {
            projected_collection.insert(field, value.clone());
        }
    }
    let objects = array_documents(&collection, "Objects")
        .into_iter()
        .map(|object| {
            if object.get_str("$Type").ok() == Some(LOOP) {
                Bson::Document(loop_projection(&object))
            } else {
                let mut identity = Document::new();
                for field in ["$ID", "$Type"] {
                    if let Some(value) = object.get(field) {
                        identity.insert(field, value.clone());
                    }
                }
                Bson::Document(identity)
            }
        })
        .collect();
    projected_collection.insert("Objects", Bson::Array(objects));
    projected.insert("ObjectCollection", projected_collection);
    projected
}

fn loop_body_object_ids(loop_document: &Document) -> HashSet<String> {
    let mut ids = HashSet::new();
    let collection = loop_document
        .get_document("ObjectCollection")
        .cloned()
        .unwrap_or_default();
    for object in array_documents(&collection, "Objects") {
        if let Some(id) = document_id(&object) {
            ids.insert(id);
        }
        if object.get_str("$Type").ok() == Some(LOOP) {
            ids.extend(loop_body_object_ids(&object));
        }
    }
    ids
}

fn internal_sequence_flows(root: &Document, object_ids: &HashSet<String>) -> Document {
    let mut by_id = Document::new();
    for flow in array_documents(root, "SequenceFlows") {
        let origin = flow
            .get("OriginPointer")
            .and_then(mxrs_bson::extract_id)
            .unwrap_or_default();
        let destination = flow
            .get("DestinationPointer")
            .and_then(mxrs_bson::extract_id)
            .unwrap_or_default();
        if object_ids.contains(&origin)
            && object_ids.contains(&destination)
            && let Some(id) = document_id(&flow)
        {
            by_id.insert(id, flow);
        }
    }
    by_id
}

fn first_difference(expected: &Bson, actual: &Bson, path: &str) -> Option<String> {
    match (expected, actual) {
        (Bson::Document(expected), Bson::Document(actual)) => {
            for key in expected.keys().chain(actual.keys()) {
                let child_path = format!("{path}.{key}");
                match (expected.get(key), actual.get(key)) {
                    (Some(left), Some(right)) => {
                        if let Some(difference) = first_difference(left, right, &child_path) {
                            return Some(difference);
                        }
                    }
                    (Some(_), None) => return Some(format!("{child_path}: missing from actual")),
                    (None, Some(_)) => return Some(format!("{child_path}: unexpected in actual")),
                    (None, None) => {}
                }
            }
            None
        }
        (Bson::Array(expected), Bson::Array(actual)) => {
            if expected.len() != actual.len() {
                return Some(format!(
                    "{path}: array lengths differ ({} != {})",
                    expected.len(),
                    actual.len()
                ));
            }
            for (index, (left, right)) in expected.iter().zip(actual).enumerate() {
                if let Some(difference) = first_difference(left, right, &format!("{path}[{index}]"))
                {
                    return Some(difference);
                }
            }
            None
        }
        _ if expected == actual => None,
        _ => Some(format!("{path}: {expected:?} != {actual:?}")),
    }
}

#[test]
#[ignore]
fn rebuilds_all_real_spc_runtime_loops() {
    let Ok(base) = std::env::var("MXRS_ACCEPTANCE_DIR") else {
        eprintln!("[loop-oracle] skipped: MXRS_ACCEPTANCE_DIR is not set");
        return;
    };
    let base = std::path::Path::new(&base);
    let mpr = base.join("spc-ruby/build/JEMScc-SPC.mpr");
    let mdp = base.join("spc-ruby/build/deployment/model/model.mdp");
    if !mpr.is_file() || !mdp.is_file() {
        eprintln!(
            "[loop-oracle] skipped: expected SPC .mpr/model.mdp under {}",
            base.display()
        );
        return;
    }

    let project = Project::open(&mpr, true).unwrap();
    let units = project.all_units().unwrap();
    let editor_roots = units
        .iter()
        .filter_map(|unit| project.mpr().parse_contents(unit).ok())
        .collect::<Vec<_>>();
    let runtime_roots = mxrs_schema::read_model_package(&mdp).unwrap();
    let runtime_root_ids: HashSet<String> = runtime_roots.iter().filter_map(document_id).collect();
    let owners = loop_owners(&editor_roots);
    let editor = collect_loops(editor_roots);
    let runtime = collect_loops(runtime_roots.clone());

    let mut signatures = BTreeMap::<String, usize>::new();
    let mut omitted = Vec::new();
    for (id, source) in &editor {
        let Some(compiled) = runtime.get(id) else {
            omitted.push(id.clone());
            continue;
        };
        *signatures
            .entry(format!(
                "editor: {}\nruntime: {}",
                shape(source),
                shape(compiled)
            ))
            .or_default() += 1;
    }

    println!("editor loops: {}", editor.len());
    println!("official Runtime loops: {}", runtime.len());
    println!(
        "editor loops in roots omitted by mxbuild: {}",
        omitted.len()
    );
    for (signature, count) in signatures {
        println!("{count}x\n{signature}");
    }

    assert_eq!(runtime.len(), 118, "SPC oracle baseline changed");
    assert!(
        omitted.iter().all(|id| owners
            .get(id)
            .is_some_and(|(root_id, _)| !runtime_root_ids.contains(root_id))),
        "mxbuild omitted individual loops from a compiled root: {omitted:?}"
    );

    let modules = project.modules().unwrap();
    let module_name_by_id: HashMap<String, String> = modules
        .iter()
        .filter_map(|module| Some((module.id.clone(), module.name.clone()?)))
        .collect();
    let parent_by_id: HashMap<String, String> = units
        .iter()
        .map(|unit| (unit.unit_id.clone(), unit.container_id.clone()))
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

    let compiler = FlowCompiler::new(&project, &runtime_roots).unwrap();
    let mut rebuilt = HashMap::new();
    let mut rebuilt_roots = HashMap::new();
    for unit in &units {
        let Ok(source) = project.mpr().parse_contents(unit) else {
            continue;
        };
        let Some(root_id) = document_id(&source) else {
            continue;
        };
        if !runtime_root_ids.contains(&root_id) {
            continue;
        }
        let type_name = source.get_str("$Type").unwrap_or_default();
        if !matches!(
            type_name,
            "Microflows$Microflow" | "Microflows$Nanoflow" | "Microflows$Rule"
        ) {
            continue;
        }
        let Some(module_name) = owning_module(&unit.container_id) else {
            continue;
        };
        let compiled = compiler
            .compile_flow(&source, &module_name)
            .unwrap_or_else(|error| panic!("could not rebuild {type_name} {root_id}: {error}"));
        rebuilt.extend(collect_loops([compiled.clone()]));
        rebuilt_roots.insert(root_id, compiled);
    }

    let runtime_roots_by_id: HashMap<String, &Document> = runtime_roots
        .iter()
        .filter_map(|root| Some((document_id(root)?, root)))
        .collect();
    let mut differences = Vec::new();
    for (id, expected) in &runtime {
        let Some(actual) = rebuilt.get(id) else {
            differences.push(format!("{id}: not rebuilt"));
            continue;
        };
        if let Some(difference) = first_difference(
            &Bson::Document(loop_projection(expected)),
            &Bson::Document(loop_projection(actual)),
            "$",
        ) {
            differences.push(format!("{id}: {difference}"));
            continue;
        }
        let Some((root_id, _)) = owners.get(id) else {
            differences.push(format!("{id}: source owner not found"));
            continue;
        };
        let Some(expected_root) = runtime_roots_by_id.get(root_id) else {
            differences.push(format!("{id}: Runtime root not found"));
            continue;
        };
        let Some(actual_root) = rebuilt_roots.get(root_id) else {
            differences.push(format!("{id}: rebuilt root not found"));
            continue;
        };
        let object_ids = loop_body_object_ids(expected);
        let expected_flows = internal_sequence_flows(expected_root, &object_ids);
        let actual_flows = internal_sequence_flows(actual_root, &object_ids);
        if let Some(difference) = first_difference(
            &Bson::Document(expected_flows),
            &Bson::Document(actual_flows),
            "$.SequenceFlows",
        ) {
            differences.push(format!("{id}: {difference}"));
        }
    }
    println!(
        "rebuilt loops: {}/{} structural and connectivity matches",
        runtime.len() - differences.len(),
        runtime.len()
    );
    assert!(differences.is_empty(), "loop differences: {differences:#?}");
}
