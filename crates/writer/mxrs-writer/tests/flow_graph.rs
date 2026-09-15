use mxrs_bson::{Bson, Document};
use mxrs_ir::flow::FlowReturnType;
use mxrs_ir::{Activity, FlowParameterDecl, MicroflowDecl};

fn declaration() -> mxrs_ir::ProjectDecl {
    let mut builder = mxrs_dsl::ProjectBuilder::new("11.12.1");
    builder.module("Calls", |m| {
        m.entity("Record", |_| {});
    });
    let mut project = builder.build();
    let mut flow = MicroflowDecl::new("Echo");
    flow.parameters
        .push(FlowParameterDecl::new("input", FlowReturnType::String));
    flow.return_type = Some(FlowReturnType::String);
    flow.return_expression = Some("$input".into());
    project.modules[0].microflows.push(flow);
    project
}

fn flow(mpr: &mxrs_mpr::MprFile) -> Document {
    mpr.all_units()
        .unwrap()
        .iter()
        .map(|u| mpr.parse_contents(u).unwrap())
        .find(|d| d.get_str("Name").ok() == Some("Echo"))
        .unwrap()
}

#[test]
fn unchanged_legacy_parameters_remain_exact_and_structural_edits_keep_them() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("Legacy.mpr");
    let mut project = declaration();
    mxrs_writer::write_project(&path, &project).unwrap();
    let mut mpr = mxrs_mpr::MprFile::open(&path, false).unwrap();
    let mut doc = flow(&mpr);
    let id = mxrs_bson::extract_id(doc.get("$ID").unwrap()).unwrap();
    let collection = doc.remove("MicroflowParameterCollection").unwrap();
    let parameters = mxrs_writer::flow_graph::documents(
        collection.as_document().unwrap().get("Parameters").unwrap(),
    )
    .unwrap();
    let parameter = parameters[0].clone();
    doc.get_document_mut("ObjectCollection")
        .unwrap()
        .get_array_mut("Objects")
        .unwrap()
        .push(Bson::Document(parameter.clone()));
    mpr.update_unit(&id, doc.clone()).unwrap();
    drop(mpr);
    mxrs_writer::synchronize_project(&path, &project).unwrap();
    let mpr = mxrs_mpr::MprFile::open(&path, true).unwrap();
    assert_eq!(flow(&mpr), doc);
    drop(mpr);
    project.modules[0].microflows[0]
        .activities
        .push(Activity::CreateList {
            variable: "records".into(),
            entity: "Calls.Record".into(),
        });
    mxrs_writer::synchronize_project(&path, &project).unwrap();
    let mpr = mxrs_mpr::MprFile::open(&path, true).unwrap();
    let after = flow(&mpr);
    let nodes = mxrs_writer::flow_graph::linear_nodes(&after).unwrap();
    assert_eq!(nodes.len(), 3);
    let legacy = mxrs_model::Microflow::from_bson(&after).parameters;
    assert_eq!(legacy, [parameter]);
    assert_eq!(after.get("$ID"), doc.get("$ID"));
}

#[test]
fn graph_traversal_rejects_cycles_disconnected_nodes_and_reused_identities() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("Graph.mpr");
    mxrs_writer::write_project(&path, &declaration()).unwrap();
    let mpr = mxrs_mpr::MprFile::open(&path, true).unwrap();
    let original = flow(&mpr);
    for mutation in ["cycle", "disconnected", "duplicate-id"] {
        let mut doc = original.clone();
        let nodes = mxrs_writer::flow_graph::linear_nodes(&doc).unwrap();
        let start = nodes[0].get("$ID").unwrap().clone();
        match mutation {
            "cycle" => {
                doc.get_array_mut("Flows").unwrap()[1]
                    .as_document_mut()
                    .unwrap()
                    .insert("DestinationPointer", start);
            }
            "disconnected" => {
                doc.get_array_mut("Flows").unwrap().truncate(1);
            }
            _ => {
                doc.get_array_mut("Flows").unwrap()[1]
                    .as_document_mut()
                    .unwrap()
                    .insert("$ID", start);
            }
        }
        assert!(
            mxrs_writer::flow_graph::linear_nodes(&doc).is_none(),
            "{mutation}"
        );
    }
}
