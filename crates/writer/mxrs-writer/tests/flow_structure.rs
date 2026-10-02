//! What the flow IR can say lowers to a graph the structured reader reads
//! back as the same flow — however the model draws the points its paths
//! join at, and whether or not the graph is built of nested blocks at all.
use mxrs_bson::{Bson, Document, extract_id};
use mxrs_ir::flow::{ErrorHandling, NativeDocument, SwitchCase};
use mxrs_ir::{Activity, MicroflowDecl};
use mxrs_writer::flow_graph::{Node, documents, rebuild_difference, structured_nodes};

fn commit(variable: &str) -> Activity {
    Activity::Commit {
        variable: variable.into(),
    }
}

fn returns(expression: &str) -> Activity {
    Activity::ReturnValue {
        expression: expression.into(),
    }
}

fn decision(condition: &str, yes: Vec<Activity>, no: Vec<Activity>) -> Activity {
    Activity::Decision {
        condition: condition.into(),
        true_branch: yes,
        false_branch: no,
    }
}

fn each(activities: Vec<Activity>) -> Activity {
    Activity::LoopOver {
        list_variable: "Orders".into(),
        iterator: "Order".into(),
        activities,
    }
}

fn handled(handling: ErrorHandling, activity: Activity, handler: Vec<Activity>) -> Activity {
    Activity::OnError {
        handling,
        activity: Box::new(activity),
        handler,
    }
}

fn case(values: &[&str], activities: Vec<Activity>) -> SwitchCase {
    SwitchCase {
        values: values.iter().map(|value| value.to_string()).collect(),
        activities,
    }
}

fn flow(activities: Vec<Activity>) -> MicroflowDecl {
    let mut declaration = MicroflowDecl::new("Flow");
    declaration.activities = activities;
    declaration
}

/// The document a model would store for `declaration`.
fn stored(declaration: &MicroflowDecl, nanoflow: bool) -> Document {
    let (objects, flows) = mxrs_writer::flow_compiler::build_flow_graph(
        &declaration.activities,
        &[],
        declaration.return_expression.as_deref(),
        nanoflow,
    );
    mxrs_bson::doc! {
        "$Type": if nanoflow { "Microflows$Nanoflow" } else { "Microflows$Microflow" },
        "Documentation": "",
        "ObjectCollection": {
            "Objects": mxrs_bson::build_array(objects.into_iter().map(Bson::Document).collect(), 3),
        },
        "Flows": mxrs_bson::build_array(flows.into_iter().map(Bson::Document).collect(), 3),
    }
}

fn kind(document: &Document) -> &str {
    document.get_str("$Type").unwrap()
}

/// The shape of what was read, for assertions.
fn shape(nodes: &[Node<'_>]) -> String {
    nodes
        .iter()
        .map(|node| match node {
            Node::Simple(document) => kind(document)
                .trim_start_matches("Microflows$")
                .trim_end_matches("Event")
                .trim_end_matches("Activity")
                .to_string(),
            Node::Decision { yes, no, .. } => format!("if({}|{})", shape(yes), shape(no)),
            Node::Switch { cases, .. } => format!(
                "switch({})",
                cases
                    .iter()
                    .map(|case| format!("{}:{}", case.values.join("+"), shape(&case.body)))
                    .collect::<Vec<_>>()
                    .join("|")
            ),
            Node::Loop { body, .. } => format!("loop({})", shape(body)),
            Node::Handled { node, handler } => {
                format!("{}!({})", shape(std::slice::from_ref(node)), shape(handler))
            }
            Node::Label(_) => "label".to_string(),
            Node::Jump(_) => "jump".to_string(),
        })
        .collect::<Vec<_>>()
        .join(",")
}

fn read(declaration: &MicroflowDecl) -> String {
    let document = stored(declaration, false);
    let nodes = structured_nodes(&document).expect("the lowered graph is read back");
    assert_eq!(rebuild_difference(&document, declaration), None);
    shape(&nodes)
}

#[test]
fn an_error_handler_ends_or_rejoins_right_after_its_activity() {
    // A handler that returns: nothing joins.
    assert_eq!(
        read(&flow(vec![
            handled(
                ErrorHandling::CustomWithoutRollback,
                commit("Order"),
                vec![commit("Log"), returns("")],
            ),
            commit("After"),
        ])),
        "Start,Action!(Action,End),Action,End"
    );
    // A handler that carries on does so with what follows the activity.
    assert_eq!(
        read(&flow(vec![
            handled(ErrorHandling::Custom, commit("Order"), vec![commit("Log")]),
            commit("After"),
        ])),
        "Start,Action!(Action),Action,End"
    );
    // An empty handler swallows the failure.
    assert_eq!(
        read(&flow(vec![
            handled(ErrorHandling::Custom, commit("Order"), vec![]),
            commit("After"),
        ])),
        "Start,Action!(),Action,End"
    );
    // A handler raising the error it was given, and one on a loop.
    assert_eq!(
        read(&flow(vec![handled(
            ErrorHandling::Custom,
            each(vec![commit("Order")]),
            vec![Activity::RaiseError],
        )])),
        "Start,loop(Action)!(Error),End"
    );
    // Inside a loop, as the last thing its body does.
    assert_eq!(
        read(&flow(vec![each(vec![handled(
            ErrorHandling::CustomWithoutRollback,
            commit("Order"),
            vec![Activity::ContinueLoop],
        )])])),
        "Start,loop(Action!(Continue)),End"
    );
}

#[test]
fn how_an_activity_fails_is_stored_where_the_model_keeps_it() {
    let declaration = flow(vec![
        handled(ErrorHandling::Continue, commit("Order"), vec![]),
        handled(
            ErrorHandling::CustomWithoutRollback,
            each(vec![]),
            vec![returns("")],
        ),
        Activity::Disabled(Box::new(commit("Skipped"))),
    ]);
    let document = stored(&declaration, false);
    let objects = documents(
        document
            .get_document("ObjectCollection")
            .unwrap()
            .get("Objects")
            .unwrap(),
    )
    .unwrap();
    let handling: Vec<_> = objects
        .iter()
        .filter_map(|object| match kind(object) {
            "Microflows$ActionActivity" => Some((
                object
                    .get_document("Action")
                    .unwrap()
                    .get_str("ErrorHandlingType")
                    .unwrap(),
                object.get_bool("Disabled").unwrap_or(false),
            )),
            "Microflows$LoopedActivity" => {
                Some((object.get_str("ErrorHandlingType").unwrap(), false))
            }
            _ => None,
        })
        .collect();
    assert_eq!(
        handling,
        [
            ("Continue", false),
            ("CustomWithoutRollBack", false),
            ("Rollback", true),
        ]
    );
    // Continuing past a failure has no handler to draw.
    assert_eq!(read(&declaration), "Start,Action,loop()!(End),Action,End");
}

#[test]
fn a_nanoflow_aborts_where_a_microflow_rolls_back() {
    let declaration = flow(vec![
        decision("$Ready", vec![commit("Order")], vec![]),
        each(vec![commit("Order")]),
    ]);
    let handling = |nanoflow: bool| {
        let mut found = Vec::new();
        fn collect(value: &Bson, found: &mut Vec<String>) {
            match value {
                Bson::Document(document) => {
                    if let Ok(handling) = document.get_str("ErrorHandlingType") {
                        found.push(handling.to_string());
                    }
                    document.values().for_each(|value| collect(value, found));
                }
                Bson::Array(items) => items.iter().for_each(|value| collect(value, found)),
                _ => {}
            }
        }
        collect(&Bson::Document(stored(&declaration, nanoflow)), &mut found);
        found.sort();
        found.dedup();
        found
    };
    assert_eq!(handling(false), ["Rollback"]);
    assert_eq!(handling(true), ["Abort"]);
    let document = stored(&declaration, true);
    assert_eq!(rebuild_difference(&document, &declaration), None);
}

#[test]
fn a_switch_has_a_branch_per_case_whatever_order_they_are_written_in() {
    let cases = vec![
        case(&["Open"], vec![commit("Order")]),
        case(&["Shipped", "Closed"], vec![returns("")]),
        case(&["(empty)"], vec![]),
    ];
    let declaration = flow(vec![
        Activity::Switch {
            expression: "$Order/Status".into(),
            cases: cases.clone(),
        },
        commit("After"),
    ]);
    // Read back by value, not by position.
    assert_eq!(
        read(&declaration),
        "Start,switch((empty):|Open:Action|Shipped+Closed:End),Action,End"
    );
    // The same cases in another order are the same switch: nothing is
    // rebuilt.
    let document = stored(&declaration, false);
    let mut reordered = cases;
    reordered.reverse();
    assert_eq!(
        rebuild_difference(
            &document,
            &flow(vec![
                Activity::Switch {
                    expression: "$Order/Status".into(),
                    cases: reordered,
                },
                commit("After"),
            ])
        ),
        None
    );
    // A case that selects something else is another flow.
    assert!(
        rebuild_difference(
            &document,
            &flow(vec![
                Activity::Switch {
                    expression: "$Order/Status".into(),
                    cases: vec![
                        case(&["Open"], vec![commit("Order")]),
                        case(&["Shipped"], vec![returns("")]),
                        case(&["(empty)"], vec![]),
                    ],
                },
                commit("After"),
            ])
        )
        .is_some()
    );
}

#[test]
fn rules_and_entities_decide_like_expressions_do() {
    let arguments = vec![("Sales.RULE_CanShip.Order".to_string(), "$Order".to_string())];
    assert_eq!(
        read(&flow(vec![Activity::RuleDecision {
            rule: "Sales.RULE_CanShip".into(),
            arguments: arguments.clone(),
            true_branch: vec![commit("Order")],
            false_branch: vec![],
        }])),
        "Start,if(Action|),End"
    );
    assert_eq!(
        read(&flow(vec![Activity::RuleSwitch {
            rule: "Sales.RULE_Status".into(),
            arguments,
            cases: vec![
                case(&["Open"], vec![commit("Order")]),
                case(&["Closed"], vec![])
            ],
        }])),
        "Start,switch(Closed:|Open:Action),End"
    );
    let declaration = flow(vec![Activity::TypeSwitch {
        variable: "Animal".into(),
        cases: vec![
            case(
                &["Zoo.Dog"],
                vec![Activity::Action(
                    NativeDocument::new("Microflows$CastAction")
                        .with("ErrorHandlingType", "Rollback")
                        .with("VariableName", "Dog"),
                )],
            ),
            case(&[""], vec![returns("")]),
        ],
    }]);
    assert_eq!(read(&declaration), "Start,switch(:|Zoo.Dog:Action,End),End");
    let document = stored(&declaration, false);
    let split = documents(
        document
            .get_document("ObjectCollection")
            .unwrap()
            .get("Objects")
            .unwrap(),
    )
    .unwrap()
    .into_iter()
    .find(|object| kind(object) == "Microflows$InheritanceSplit")
    .unwrap();
    assert_eq!(split.get_str("SplitVariableName").unwrap(), "Animal");
}

/// Removes the merge named by `pick` and sends the edges that reached it to
/// wherever it led — or nowhere, when nothing left it.
fn without_merge(document: &mut Document, pick: impl Fn(&[String]) -> String) {
    fn merges(collection: &Document, found: &mut Vec<String>) {
        for object in documents(collection.get("Objects").unwrap()).unwrap() {
            if kind(object) == "Microflows$ExclusiveMerge" {
                found.push(extract_id(object.get("$ID").unwrap()).unwrap());
            }
            if let Ok(inner) = object.get_document("ObjectCollection") {
                merges(inner, found);
            }
        }
    }
    fn remove(collection: &mut Document, target: &str) {
        let objects = collection.get_array_mut("Objects").unwrap();
        objects.retain(|object| {
            object
                .as_document()
                .is_none_or(|object| extract_id(object.get("$ID").unwrap()).unwrap() != target)
        });
        for object in objects.iter_mut().filter_map(Bson::as_document_mut) {
            if let Ok(inner) = object.get_document_mut("ObjectCollection") {
                remove(inner, target);
            }
        }
    }
    let mut found = Vec::new();
    merges(
        document.get_document("ObjectCollection").unwrap(),
        &mut found,
    );
    let target = pick(&found);
    let pointer = |edge: &Document, field: &str| extract_id(edge.get(field).unwrap()).unwrap();
    let flows = document.get_array_mut("Flows").unwrap();
    let next = flows
        .iter()
        .filter_map(Bson::as_document)
        .find(|edge| pointer(edge, "OriginPointer") == target)
        .map(|edge| edge.get("DestinationPointer").unwrap().clone());
    flows.retain(|edge| {
        edge.as_document().is_none_or(|edge| {
            pointer(edge, "OriginPointer") != target
                && (next.is_some() || pointer(edge, "DestinationPointer") != target)
        })
    });
    if let Some(next) = next {
        for edge in flows.iter_mut().filter_map(Bson::as_document_mut) {
            if pointer(edge, "DestinationPointer") == target {
                edge.insert("DestinationPointer", next.clone());
            }
        }
    }
    remove(
        document.get_document_mut("ObjectCollection").unwrap(),
        &target,
    );
}

#[test]
fn a_join_is_the_same_join_however_the_model_draws_it() {
    // A decision that closes a loop body: its branches may meet at a merge
    // nothing leaves, or simply stop where the iteration does.
    let declaration = flow(vec![each(vec![
        commit("First"),
        decision(
            "$Ready",
            vec![commit("Order")],
            vec![Activity::ContinueLoop],
        ),
    ])]);
    let mut document = stored(&declaration, false);
    let drawn = shape(&structured_nodes(&document).unwrap());
    without_merge(&mut document, |merges| merges[0].clone());
    assert_eq!(shape(&structured_nodes(&document).unwrap()), drawn);
    assert_eq!(rebuild_difference(&document, &declaration), None);

    // Nested decisions: the rebuild closes each with a merge of its own, a
    // model may send every branch to one.
    let declaration = flow(vec![
        decision(
            "$A",
            vec![
                commit("First"),
                decision("$B", vec![commit("Second"), returns("")], vec![]),
            ],
            vec![],
        ),
        commit("After"),
    ]);
    let mut document = stored(&declaration, false);
    let drawn = shape(&structured_nodes(&document).unwrap());
    assert_eq!(drawn, "Start,if(Action,if(Action,End|)|),Action,End");
    // The inner merge is the one whose edge leads to another merge.
    let inner = {
        let objects = documents(
            document
                .get_document("ObjectCollection")
                .unwrap()
                .get("Objects")
                .unwrap(),
        )
        .unwrap();
        let merges: Vec<String> = objects
            .iter()
            .filter(|object| kind(object) == "Microflows$ExclusiveMerge")
            .map(|object| extract_id(object.get("$ID").unwrap()).unwrap())
            .collect();
        documents(document.get("Flows").unwrap())
            .unwrap()
            .into_iter()
            .find_map(|edge| {
                let from = extract_id(edge.get("OriginPointer").unwrap()).unwrap();
                let to = extract_id(edge.get("DestinationPointer").unwrap()).unwrap();
                (merges.contains(&from) && merges.contains(&to)).then_some(from)
            })
            .expect("one merge leads to the other")
    };
    without_merge(&mut document, |_| inner.clone());
    assert_eq!(shape(&structured_nodes(&document).unwrap()), drawn);
    assert_eq!(rebuild_difference(&document, &declaration), None);
}

#[test]
fn a_flow_that_goes_back_is_read_with_the_point_it_goes_back_to() {
    // Try, and on failure try again: the handler's path returns to before
    // the activity it answers for.
    let declaration = flow(vec![
        commit("Before"),
        Activity::Label("again".into()),
        handled(
            ErrorHandling::CustomWithoutRollback,
            commit("Order"),
            vec![decision(
                "$Retry",
                vec![commit("Wait"), Activity::Jump("again".into())],
                vec![returns("")],
            )],
        ),
        commit("After"),
    ]);
    assert_eq!(
        read(&declaration),
        "Start,Action,label,Action!(if(Action,jump|),End),Action,End"
    );
    // A jump that opens a branch is that branch's own entering edge.
    let declaration = flow(vec![
        Activity::Label("again".into()),
        commit("Order"),
        decision("$Retry", vec![Activity::Jump("again".into())], vec![]),
    ]);
    assert_eq!(read(&declaration), "Start,label,Action,if(jump|),End");
    // No name is stored: another name for the same point is the same flow.
    let document = stored(&declaration, false);
    assert_eq!(
        rebuild_difference(
            &document,
            &flow(vec![
                Activity::Label("retry".into()),
                commit("Order"),
                decision("$Retry", vec![Activity::Jump("retry".into())], vec![]),
            ])
        ),
        None
    );
}

#[test]
fn a_graph_no_block_structure_fits_is_read_with_every_join_named() {
    // One branch runs into the middle of another: no nesting of blocks says
    // that, so the point they meet at is named and jumped to.
    let declaration = flow(vec![decision(
        "$A",
        vec![commit("First"), Activity::Jump("shared".into())],
        vec![decision(
            "$B",
            vec![commit("Second")],
            vec![Activity::Label("shared".into()), commit("Third")],
        )],
    )]);
    let document = stored(&declaration, false);
    let nodes = structured_nodes(&document).expect("any connected graph has a reading");
    let read = shape(&nodes);
    assert!(read.contains("label") && read.contains("jump"), "{read}");
    assert_eq!(rebuild_difference(&document, &declaration), None);
    // Disconnected objects still have none.
    let mut broken = document.clone();
    let flows = broken.get_array_mut("Flows").unwrap();
    let last = flows.len() - 1;
    flows.remove(last);
    assert!(structured_nodes(&broken).is_none());
}

#[test]
fn a_declaration_the_model_cannot_hold_is_refused() {
    let entities = std::collections::HashSet::new();
    let check = |activities: Vec<Activity>| {
        mxrs_writer::validate_flow_declaration("Sales", &flow(activities), false, &entities)
            .map_err(|error| error.to_string())
    };
    assert!(
        check(vec![
            Activity::Label("again".into()),
            commit("Order"),
            decision("$Retry", vec![Activity::Jump("again".into())], vec![]),
        ])
        .is_ok()
    );
    for (activities, reason) in [
        (
            vec![commit("Order"), Activity::Jump("nowhere".into())],
            "no label of that name",
        ),
        (
            vec![
                Activity::Label("again".into()),
                Activity::Label("again".into()),
            ],
            "two labels",
        ),
        // A jump cannot leave the loop it is in.
        (
            vec![
                Activity::Label("again".into()),
                each(vec![commit("Order"), Activity::Jump("again".into())]),
            ],
            "no label of that name",
        ),
        (
            vec![handled(
                ErrorHandling::Custom,
                decision("$A", vec![], vec![]),
                vec![],
            )],
            "only an action or a loop",
        ),
        (
            vec![handled(
                ErrorHandling::Continue,
                commit("Order"),
                vec![commit("Log")],
            )],
            "no handler to run",
        ),
        (
            vec![Activity::Switch {
                expression: "$Status".into(),
                cases: vec![case(&["Open"], vec![]), case(&["Open"], vec![])],
            }],
            "its own values",
        ),
        // A label's name is the flow's: one retry copied into a second loop
        // would otherwise have its jump land in the first.
        (
            vec![
                each(vec![
                    commit("Order"),
                    Activity::Label("again".into()),
                    decision("$Retry", vec![Activity::Jump("again".into())], vec![]),
                ]),
                each(vec![
                    commit("Line"),
                    Activity::Label("again".into()),
                    decision("$Retry", vec![Activity::Jump("again".into())], vec![]),
                ]),
            ],
            "two labels",
        ),
        // A loop is entered at the activity nothing leads to.
        (
            vec![each(vec![
                Activity::Label("again".into()),
                commit("Order"),
                decision("$Retry", vec![Activity::Jump("again".into())], vec![]),
            ])],
            "cannot open with a label",
        ),
        (
            vec![Activity::Switch {
                expression: "$Status".into(),
                cases: vec![],
            }],
            "at least one case",
        ),
        (
            vec![Activity::Disabled(Box::new(decision("$A", vec![], vec![])))],
            "only an action can be disabled",
        ),
    ] {
        let error = check(activities).unwrap_err();
        assert!(error.contains(reason), "{reason}: {error}");
    }
}

#[test]
fn a_guard_is_followed_by_the_rest_of_the_flow_not_wrapped_around_it() {
    let rest = vec![
        commit("First"),
        each(vec![commit("Order")]),
        commit("Second"),
        commit("Third"),
    ];
    // Drawn with or without a merge after the guard, the reading is flat.
    let mut nested = rest.clone();
    nested.push(returns("true"));
    for declaration in [
        {
            let mut activities = vec![decision("$Invalid", vec![returns("false")], vec![])];
            activities.extend(rest.clone());
            let mut declaration = flow(activities);
            declaration.return_expression = Some("true".into());
            declaration
        },
        flow(vec![decision("$Invalid", vec![returns("false")], nested)]),
    ] {
        assert_eq!(
            read(&declaration),
            "Start,if(End|),Action,loop(Action),Action,Action,End"
        );
    }
    // Inside a loop, what does not skip the iteration is the rest of it.
    assert_eq!(
        read(&flow(vec![each(vec![decision(
            "$Skip",
            vec![Activity::ContinueLoop],
            vec![commit("First"), commit("Second")],
        )])])),
        "Start,loop(if(Continue|),Action,Action),End"
    );
    // A branch that only ends is the flow ending.
    assert_eq!(
        read(&flow(vec![decision(
            "$A",
            vec![commit("First"), returns("1")],
            vec![returns("2")],
        )])),
        "Start,if(Action,End|),End"
    );
    // Two branches of the same weight stay side by side.
    assert_eq!(
        read(&flow(vec![decision(
            "$A",
            vec![commit("First"), returns("1")],
            vec![commit("Second"), returns("2")],
        )])),
        "Start,if(Action,End|Action,End)"
    );
}

#[test]
fn the_handler_of_a_whole_flow_jumps_only_to_its_labels() {
    let entities = std::collections::HashSet::new();
    let mut declaration = flow(vec![commit("Order")]);
    declaration.rescue_activities = vec![commit("Log"), Activity::Jump("nowhere".into())];
    let error = mxrs_writer::validate_flow_declaration("Sales", &declaration, false, &entities)
        .unwrap_err()
        .to_string();
    assert!(error.contains("no label of that name"), "{error}");
}

#[test]
fn a_label_after_every_path_has_ended_is_still_where_its_jumps_go() {
    // Both branches jump: nothing runs on into `done`, which only the jump
    // reaches.
    let declaration = flow(vec![
        Activity::Label("again".into()),
        commit("Order"),
        decision(
            "$Done",
            vec![Activity::Jump("done".into())],
            vec![Activity::Jump("again".into())],
        ),
        Activity::Label("done".into()),
        commit("After"),
    ]);
    let entities = std::collections::HashSet::new();
    mxrs_writer::validate_flow_declaration("Sales", &declaration, false, &entities).unwrap();
    let document = stored(&declaration, false);
    for edge in documents(document.get("Flows").unwrap()).unwrap() {
        let to = extract_id(edge.get("DestinationPointer").unwrap()).unwrap();
        assert!(!to.starts_with("label:"), "a jump left unresolved: {to}");
    }
    // Read back, the point only one jump reaches is where that jump goes on.
    let read = shape(&structured_nodes(&document).expect("the graph is read back"));
    assert_eq!(read, "Start,label,Action,if(Action,End|jump)");
    assert_eq!(rebuild_difference(&document, &declaration), None);
}
