use mxrs_bson::{Bson, Document};
use mxrs_ir::flow::FlowReturnType;
use mxrs_ir::{Activity, FlowParameterDecl, MicroflowDecl};

fn declaration() -> mxrs_ir::ProjectDecl {
    let mut builder = mxrs_dsl::ProjectBuilder::new("11.12.1");
    builder.module("Calls", |m| {
        m.entity("Record", |e| {
            e.string("First");
            e.string("Second");
            e.string("Third");
        });
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

#[test]
fn member_identity_follows_its_name_across_reordering_removal_and_insertion() {
    use mxrs_ir::Member;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("Members.mpr");
    let mut project = declaration();
    project.modules[0].microflows[0]
        .activities
        .push(Activity::CreateObject {
            variable: "record".into(),
            entity: "Calls.Record".into(),
            commit: false,
            members: vec![
                Member::attribute("First", "'first'"),
                Member::attribute("Second", "'second'"),
            ],
        });
    mxrs_writer::write_project(&path, &project).unwrap();
    let snapshot = || {
        let mpr = mxrs_mpr::MprFile::open(&path, true).unwrap();
        let doc = flow(&mpr);
        let nodes = mxrs_writer::flow_graph::linear_nodes(&doc).unwrap();
        nodes[1].clone()
    };
    let before = snapshot();
    let items = |node: &Document| {
        mxrs_writer::flow_graph::documents(
            node.get_document("Action").unwrap().get("Items").unwrap(),
        )
        .unwrap()
        .into_iter()
        .map(|item| {
            (
                item.get_str("Attribute").unwrap().to_string(),
                mxrs_bson::extract_id(item.get("$ID").unwrap()).unwrap(),
            )
        })
        .collect::<std::collections::BTreeMap<_, _>>()
    };
    let old = items(&before);
    let Activity::CreateObject { members, .. } =
        &mut project.modules[0].microflows[0].activities[0]
    else {
        panic!("create");
    };
    members.reverse();
    members.push(Member::attribute("Third", "'third'"));
    mxrs_writer::synchronize_project(&path, &project).unwrap();
    let reordered = snapshot();
    let current = items(&reordered);
    assert_eq!(before.get("$ID"), reordered.get("$ID"));
    assert_eq!(old["Calls.Record.First"], current["Calls.Record.First"]);
    assert_eq!(old["Calls.Record.Second"], current["Calls.Record.Second"]);
    assert!(!old.values().any(|id| id == &current["Calls.Record.Third"]));
    let Activity::CreateObject { members, .. } =
        &mut project.modules[0].microflows[0].activities[0]
    else {
        panic!("create");
    };
    members.retain(|m| m.attribute.as_deref() != Some("First"));
    mxrs_writer::synchronize_project(&path, &project).unwrap();
    let after = items(&snapshot());
    assert_eq!(after.len(), 2);
    assert_eq!(after["Calls.Record.Second"], old["Calls.Record.Second"]);
    assert_eq!(after["Calls.Record.Third"], current["Calls.Record.Third"]);
}

#[test]
fn structured_traversal_rejects_ambiguous_edges_and_container_crossings() {
    use mxrs_writer::flow_graph::{Node, structured_nodes};
    let decision = Activity::Decision {
        condition: "true".into(),
        true_branch: vec![Activity::Commit {
            variable: "a".into(),
        }],
        false_branch: vec![Activity::Commit {
            variable: "b".into(),
        }],
    };
    let (objects, edges) = mxrs_writer::flow_compiler::build_microflow_graph(
        &[
            decision.clone(),
            Activity::WhileLoop {
                condition: "false".into(),
                activities: vec![decision],
            },
        ],
        &[],
        None,
    );
    let original = mxrs_bson::doc! {
        "ObjectCollection": { "Objects": mxrs_bson::build_array(objects.into_iter().map(Bson::Document).collect(), 2) },
        "Flows": mxrs_bson::build_array(edges.into_iter().map(Bson::Document).collect(), 2),
    };
    for mutation in [
        "cross-container",
        "duplicate-case",
        "shared-branch",
        "cycle",
        "disconnected",
        "duplicate-id",
        "error-edge",
        "unknown-case",
        "duplicate-edge",
        "merge-cycle",
        "inner-parameter",
    ] {
        let mut doc = original.clone();
        let nodes = structured_nodes(&doc).unwrap();
        let Node::Decision {
            split,
            yes,
            no,
            merge,
            ..
        } = &nodes[1]
        else {
            panic!("decision")
        };
        let split_id = split.get("$ID").unwrap().clone();
        let merge_id = merge.unwrap().get("$ID").unwrap().clone();
        let Node::Simple(yes) = yes[0] else {
            panic!("yes")
        };
        let Node::Simple(no) = no[0] else {
            panic!("no")
        };
        let yes_id = yes.get("$ID").unwrap().clone();
        let no_id = no.get("$ID").unwrap().clone();
        let Node::Loop { body, node } = &nodes[2] else {
            panic!("loop")
        };
        let loop_id = node.get("$ID").unwrap().clone();
        let Node::Decision { split: inner, .. } = &body[0] else {
            panic!("inner")
        };
        let inner_id = inner.get("$ID").unwrap().clone();
        let edges = doc.get_array_mut("Flows").unwrap();
        match mutation {
            "disconnected" => edges.retain(|value| {
                value
                    .as_document()
                    .is_none_or(|edge| edge.get("DestinationPointer") != Some(&yes_id))
            }),
            "duplicate-id" => {
                edges[1].as_document_mut().unwrap().insert("$ID", split_id);
            }
            "duplicate-edge" => {
                let mut edge = edges[1].as_document().unwrap().clone();
                edge.insert("$ID", uuid::Uuid::new_v4().to_string());
                edge.remove("Line");
                edge.remove("CaseValues");
                edges.push(Bson::Document(edge));
            }
            "merge-cycle" => {
                let edge = edges
                    .iter_mut()
                    .filter_map(Bson::as_document_mut)
                    .find(|e| e.get("OriginPointer") == Some(&merge_id))
                    .unwrap();
                edge.insert("DestinationPointer", split_id);
            }
            "inner-parameter" => {
                let objects = doc
                    .get_document_mut("ObjectCollection")
                    .unwrap()
                    .get_array_mut("Objects")
                    .unwrap();
                let inner = objects
                    .iter_mut()
                    .filter_map(Bson::as_document_mut)
                    .find(|o| o.get("$ID") == Some(&loop_id))
                    .unwrap()
                    .get_document_mut("ObjectCollection")
                    .unwrap()
                    .get_array_mut("Objects")
                    .unwrap();
                inner.push(Bson::Document(mxrs_bson::doc! { "$ID": uuid::Uuid::new_v4().to_string(), "$Type": "Microflows$MicroflowParameter" }));
            }
            _ => {
                let edge = edges
                    .iter_mut()
                    .filter_map(Bson::as_document_mut)
                    .find(|e| {
                        e.get("OriginPointer") == Some(&split_id)
                            && e.get("DestinationPointer") == Some(&no_id)
                    })
                    .unwrap();
                match mutation {
                    "cross-container" => {
                        edge.insert("DestinationPointer", inner_id);
                    }
                    "shared-branch" => {
                        edge.insert("DestinationPointer", yes_id);
                    }
                    "cycle" => {
                        edge.insert("DestinationPointer", split_id);
                    }
                    "error-edge" => {
                        edge.insert("IsErrorHandler", true);
                    }
                    "duplicate-case" | "unknown-case" => {
                        edge.get_array_mut("CaseValues").unwrap()[1]
                            .as_document_mut()
                            .unwrap()
                            .insert(
                                "Value",
                                if mutation == "duplicate-case" {
                                    "true"
                                } else {
                                    "unknown"
                                },
                            );
                    }
                    _ => unreachable!(),
                }
            }
        }
        assert!(structured_nodes(&doc).is_none(), "{mutation}");
    }
}
