//! Behavioral suite for the flow interpreter, pinned against mxrb's
//! `native.rb` semantics — including the one place this port is deliberately
//! better: a loop body with two or more activities executes completely
//! (mxrb's interpreter silently runs only the first).

use std::collections::BTreeMap;

use mxrs_bson::{Bson, Document, build_array, doc};
use mxrs_model::{Microflow, Module};
use mxrs_runtime::{
    EntityRule, MemberRight, RuntimeError, SecurityContext, SecurityPolicy, Store, StoreSchema,
};
use mxrs_runtime_flows::{FlowEngine, FlowError, FlowValue, JavaAction, Variables};

fn edge(id: &str, origin: &str, destination: &str) -> Bson {
    Bson::Document(doc! {
        "$Type": "Microflows$SequenceFlow",
        "$ID": id,
        "OriginPointer": origin,
        "DestinationPointer": destination,
        "IsErrorHandler": false,
    })
}

fn error_edge(id: &str, origin: &str, destination: &str) -> Bson {
    Bson::Document(doc! {
        "$Type": "Microflows$SequenceFlow",
        "$ID": id,
        "OriginPointer": origin,
        "DestinationPointer": destination,
        "IsErrorHandler": true,
    })
}

fn start(id: &str) -> Bson {
    Bson::Document(doc! { "$Type": "Microflows$StartEvent", "$ID": id })
}

fn end(id: &str, return_value: &str) -> Bson {
    Bson::Document(doc! {
        "$Type": "Microflows$EndEvent",
        "$ID": id,
        "ReturnValue": return_value,
    })
}

fn activity(id: &str, action: Document) -> Bson {
    Bson::Document(doc! {
        "$Type": "Microflows$ActionActivity",
        "$ID": id,
        "Action": action,
    })
}

fn flow(name: &str, objects: Vec<Bson>, edges: Vec<Bson>) -> Microflow {
    Microflow::from_bson(&doc! {
        "$Type": "Microflows$Microflow",
        "Name": name,
        "ApplyEntityAccess": false,
        "ObjectCollection": doc! {
            "$Type": "Microflows$MicroflowObjectCollection",
            "Objects": build_array(objects, 2),
        },
        "Flows": build_array(edges, 2),
    })
}

fn secured_flow(name: &str, objects: Vec<Bson>, edges: Vec<Bson>) -> Microflow {
    Microflow::from_bson(&doc! {
        "$Type": "Microflows$Microflow",
        "Name": name,
        "ApplyEntityAccess": true,
        "ObjectCollection": doc! {
            "$Type": "Microflows$MicroflowObjectCollection",
            "Objects": build_array(objects, 2),
        },
        "Flows": build_array(edges, 2),
    })
}

fn parameter(id: &str, name: &str) -> Bson {
    Bson::Document(doc! {
        "$Type": "Microflows$MicroflowParameter",
        "$ID": id,
        "Name": name,
    })
}

fn module_with(flows: Vec<Microflow>) -> Module {
    Module {
        id: String::new(),
        name: Some("App".to_string()),
        sort_index: None,
        from_app_store: false,
        app_store_guid: None,
        app_store_version: None,
        export_level: String::new(),
        domain_model: None,
        pages: Vec::new(),
        microflows: flows,
        nanoflows: Vec::new(),
        rules: Vec::new(),
        menus: Vec::new(),
        module_roles: Vec::new(),
        artifact_units: Vec::new(),
    }
}

fn store_with_order() -> Store {
    Store::new(StoreSchema::default().entity("App.Order", BTreeMap::new(), false))
}

fn call(
    engine: &FlowEngine,
    store: &mut Store,
    name: &str,
    arguments: Variables,
) -> (FlowValue, mxrs_runtime_flows::Execution) {
    engine
        .call(store, name, arguments, None)
        .unwrap_or_else(|error| panic!("{name}: {error}"))
}

#[test]
fn a_linear_flow_creates_variables_and_returns_an_expression() {
    let flows = vec![flow(
        "Sum",
        vec![
            start("s"),
            activity(
                "a1",
                doc! {
                    "$Type": "Microflows$CreateVariableAction",
                    "VariableName": "total",
                    "InitialValue": doc! { "Value": "20" },
                },
            ),
            activity(
                "a2",
                doc! {
                    "$Type": "Microflows$ChangeVariableAction",
                    "ChangeVariableName": "total",
                    "Value": doc! { "Value": "$total + 22" },
                },
            ),
            end("e", "$total"),
        ],
        vec![
            edge("f1", "s", "a1"),
            edge("f2", "a1", "a2"),
            edge("f3", "a2", "e"),
        ],
    )];
    let engine = FlowEngine::from_modules(&[module_with(flows)]);
    let mut store = store_with_order();
    let (result, _) = call(&engine, &mut store, "App.Sum", Variables::new());
    assert_eq!(result, FlowValue::Int(42));
}

#[test]
fn missing_flows_and_missing_arguments_are_named_errors() {
    let flows = vec![flow(
        "NeedsInput",
        vec![start("s"), parameter("p", "Count"), end("e", "$Count")],
        vec![edge("f1", "s", "e")],
    )];
    let engine = FlowEngine::from_modules(&[module_with(flows)]);
    let mut store = store_with_order();
    let missing = engine
        .call(&mut store, "App.Nope", Variables::new(), None)
        .unwrap_err();
    assert_eq!(missing.to_string(), "flow App.Nope not found");
    let unfilled = engine
        .call(&mut store, "App.NeedsInput", Variables::new(), None)
        .unwrap_err();
    assert_eq!(unfilled.to_string(), "missing argument Count");
    let mut arguments = Variables::new();
    arguments.insert("Count".into(), FlowValue::Int(7));
    let (result, _) = call(&engine, &mut store, "App.NeedsInput", arguments);
    assert_eq!(result, FlowValue::Int(7));
}

#[test]
fn an_exclusive_split_takes_the_matching_case_and_falls_back_to_no_case() {
    let split = Bson::Document(doc! {
        "$Type": "Microflows$ExclusiveSplit",
        "$ID": "split",
        "SplitCondition": doc! { "$Type": "Microflows$ExpressionSplitCondition", "Expression": "$flag" },
    });
    let case_edge = Bson::Document(doc! {
        "$Type": "Microflows$SequenceFlow",
        "$ID": "fy",
        "OriginPointer": "split",
        "DestinationPointer": "yes",
        "IsErrorHandler": false,
        "CaseValues": build_array(vec![Bson::Document(doc! {
            "$Type": "Microflows$EnumerationCase",
            "Value": "true",
        })], 1),
    });
    let fallback_edge = Bson::Document(doc! {
        "$Type": "Microflows$SequenceFlow",
        "$ID": "fn",
        "OriginPointer": "split",
        "DestinationPointer": "no",
        "IsErrorHandler": false,
        "CaseValues": build_array(vec![Bson::Document(doc! { "$Type": "Microflows$NoCase" })], 1),
    });
    let flows = vec![flow(
        "Branch",
        vec![
            start("s"),
            parameter("p", "flag"),
            split.clone(),
            end("yes", "'went-true'"),
            end("no", "'went-other'"),
        ],
        vec![edge("f1", "s", "split"), case_edge, fallback_edge],
    )];
    let engine = FlowEngine::from_modules(&[module_with(flows)]);
    let mut store = store_with_order();
    let mut arguments = Variables::new();
    arguments.insert("flag".into(), FlowValue::Bool(true));
    let (result, _) = call(&engine, &mut store, "App.Branch", arguments);
    assert_eq!(result, FlowValue::String("went-true".into()));
    let mut arguments = Variables::new();
    arguments.insert("flag".into(), FlowValue::Int(3));
    let (result, _) = call(&engine, &mut store, "App.Branch", arguments);
    assert_eq!(result, FlowValue::String("went-other".into()));
}

/// The regression mxrb cannot pass: a loop body with TWO activities must run
/// both on every iteration.
#[test]
fn a_multi_activity_loop_body_executes_completely_every_iteration() {
    let loop_activity = Bson::Document(doc! {
        "$Type": "Microflows$LoopedActivity",
        "$ID": "loop",
        "LoopSource": doc! {
            "$Type": "Microflows$IterableList",
            "ListVariableName": "items",
            "VariableName": "item",
        },
        "ObjectCollection": doc! {
            "$Type": "Microflows$MicroflowObjectCollection",
            "Objects": build_array(vec![
                activity("n1", doc! {
                    "$Type": "Microflows$ChangeVariableAction",
                    "ChangeVariableName": "sum",
                    "Value": doc! { "Value": "$sum + $item" },
                }),
                activity("n2", doc! {
                    "$Type": "Microflows$ChangeVariableAction",
                    "ChangeVariableName": "count",
                    "Value": doc! { "Value": "$count + 1" },
                }),
            ], 2),
        },
    });
    let flows = vec![flow(
        "LoopSum",
        vec![
            start("s"),
            parameter("p", "items"),
            activity(
                "init1",
                doc! { "$Type": "Microflows$CreateVariableAction", "VariableName": "sum", "InitialValue": doc! { "Value": "0" } },
            ),
            activity(
                "init2",
                doc! { "$Type": "Microflows$CreateVariableAction", "VariableName": "count", "InitialValue": doc! { "Value": "0" } },
            ),
            loop_activity,
            end("e", "$sum * 100 + $count"),
        ],
        vec![
            edge("f1", "s", "init1"),
            edge("f2", "init1", "init2"),
            edge("f3", "init2", "loop"),
            edge("f4", "loop", "e"),
            // The loop body's own edge lives in the DOCUMENT's flow list —
            // the layout that trips mxrb.
            edge("fb", "n1", "n2"),
        ],
    )];
    let engine = FlowEngine::from_modules(&[module_with(flows)]);
    let mut store = store_with_order();
    let mut arguments = Variables::new();
    arguments.insert(
        "items".into(),
        FlowValue::List(vec![
            FlowValue::Int(5),
            FlowValue::Int(7),
            FlowValue::Int(8),
        ]),
    );
    let (result, _) = call(&engine, &mut store, "App.LoopSum", arguments);
    // sum = 20, count = 3 — count proves the SECOND activity ran each pass.
    assert_eq!(result, FlowValue::Int(2003));
}

#[test]
fn while_loops_respect_break_events_and_iteration_budgets() {
    let loop_activity = Bson::Document(doc! {
        "$Type": "Microflows$LoopedActivity",
        "$ID": "loop",
        "LoopSource": doc! {
            "$Type": "Microflows$WhileLoopCondition",
            "WhileExpression": "true",
        },
        "ObjectCollection": doc! {
            "Objects": build_array(vec![
                activity("n1", doc! {
                    "$Type": "Microflows$ChangeVariableAction",
                    "ChangeVariableName": "i",
                    "Value": doc! { "Value": "$i + 1" },
                }),
                Bson::Document(doc! { "$Type": "Microflows$ExclusiveSplit", "$ID": "check",
                    "SplitCondition": doc! { "Expression": "$i >= 3" } }),
                Bson::Document(doc! { "$Type": "Microflows$BreakEvent", "$ID": "brk" }),
                Bson::Document(doc! { "$Type": "Microflows$ContinueEvent", "$ID": "cont" }),
            ], 2),
        },
    });
    let done_edge = Bson::Document(doc! {
        "$Type": "Microflows$SequenceFlow", "$ID": "fd", "OriginPointer": "check",
        "DestinationPointer": "brk", "IsErrorHandler": false,
        "CaseValues": build_array(vec![Bson::Document(doc! { "$Type": "Microflows$EnumerationCase", "Value": "true" })], 1),
    });
    let again_edge = Bson::Document(doc! {
        "$Type": "Microflows$SequenceFlow", "$ID": "fa", "OriginPointer": "check",
        "DestinationPointer": "cont", "IsErrorHandler": false,
        "CaseValues": build_array(vec![Bson::Document(doc! { "$Type": "Microflows$NoCase" })], 1),
    });
    let flows = vec![flow(
        "CountUp",
        vec![
            start("s"),
            activity(
                "init",
                doc! { "$Type": "Microflows$CreateVariableAction", "VariableName": "i", "InitialValue": doc! { "Value": "0" } },
            ),
            loop_activity,
            end("e", "$i"),
        ],
        vec![
            edge("f1", "s", "init"),
            edge("f2", "init", "loop"),
            edge("f3", "loop", "e"),
            edge("fb", "n1", "check"),
            done_edge,
            again_edge,
        ],
    )];
    let engine = FlowEngine::from_modules(&[module_with(flows)]);
    let mut store = store_with_order();
    let (result, _) = call(&engine, &mut store, "App.CountUp", Variables::new());
    assert_eq!(result, FlowValue::Int(3));
}

#[test]
fn create_commit_retrieve_round_trips_with_xpath_sort_and_single_object() {
    let create = |id: &str, name: &str, total: &str| {
        activity(
            id,
            doc! {
                "$Type": "Microflows$CreateObjectAction",
                "Entity": "App.Order",
                "VariableName": format!("order{id}"),
                "Commit": "Yes",
                "Items": build_array(vec![
                    Bson::Document(doc! { "Attribute": "App.Order/Name", "Value": doc! { "Value": format!("'{name}'") } }),
                    Bson::Document(doc! { "Attribute": "App.Order/Total", "Value": doc! { "Value": total } }),
                ], 2),
            },
        )
    };
    let retrieve = activity(
        "r",
        doc! {
            "$Type": "Microflows$RetrieveAction",
            "ResultVariableName": "best",
            "RetrieveSource": doc! {
                "$Type": "Microflows$DatabaseRetrieveSource",
                "Entity": "App.Order",
                "XpathConstraint": "[Total > 5]",
                "NewSortings": doc! { "Sortings": build_array(vec![
                    Bson::Document(doc! { "AttributePath": "App.Order/Total", "SortOrder": "Descending" }),
                ], 2) },
                "Range": doc! { "SingleObject": true, "LimitExpression": doc! { "Value": "" } },
            },
        },
    );
    let flows = vec![flow(
        "Best",
        vec![
            start("s"),
            create("c1", "small", "3"),
            create("c2", "mid", "8"),
            create("c3", "big", "11"),
            retrieve,
            end("e", "$best/Name"),
        ],
        vec![
            edge("f1", "s", "c1"),
            edge("f2", "c1", "c2"),
            edge("f3", "c2", "c3"),
            edge("f4", "c3", "r"),
            edge("f5", "r", "e"),
        ],
    )];
    let engine = FlowEngine::from_modules(&[module_with(flows)]);
    let mut store = store_with_order();
    let (result, _) = call(&engine, &mut store, "App.Best", Variables::new());
    assert_eq!(result, FlowValue::String("big".into()));
    // Committed objects survive the unit of work.
    assert_eq!(store.retrieve("App.Order").unwrap().len(), 3);
    assert_eq!(
        engine
            .count(&store, "App.Order", Some("[Total > 5]"))
            .unwrap(),
        2
    );
}

#[test]
fn an_error_handler_with_rollback_restores_the_store_and_binds_latest_error() {
    let failing = activity(
        "boom",
        doc! {
            "$Type": "Microflows$CreateObjectAction",
            "Entity": "App.Order",
            "VariableName": "order",
            "Commit": "Yes",
            "ErrorHandlingType": "Rollback",
            "Items": build_array(vec![
                Bson::Document(doc! { "Attribute": "Name", "Value": doc! { "Value": "$missing" } }),
            ], 2),
        },
    );
    let flows = vec![flow(
        "Recovers",
        vec![
            start("s"),
            failing,
            end("ok", "'unreached'"),
            end("handled", "$latestError"),
        ],
        vec![
            edge("f1", "s", "boom"),
            edge("f2", "boom", "ok"),
            error_edge("f3", "boom", "handled"),
        ],
    )];
    let engine = FlowEngine::from_modules(&[module_with(flows)]);
    let mut store = store_with_order();
    let (result, _) = call(&engine, &mut store, "App.Recovers", Variables::new());
    assert_eq!(
        result,
        FlowValue::String("unknown variable $missing".into())
    );
    // The rollback snapshot removed the half-created object.
    assert!(store.retrieve("App.Order").unwrap().is_empty());
}

#[test]
fn a_root_error_without_a_handler_rolls_back_the_whole_unit_of_work() {
    let flows = vec![flow(
        "Fails",
        vec![
            start("s"),
            activity(
                "c",
                doc! {
                    "$Type": "Microflows$CreateObjectAction",
                    "Entity": "App.Order",
                    "VariableName": "order",
                    "Commit": "Yes",
                },
            ),
            Bson::Document(doc! {
                "$Type": "Microflows$ErrorEvent",
                "$ID": "err",
                "ErrorExpression": "'deliberate'",
            }),
        ],
        vec![edge("f1", "s", "c"), edge("f2", "c", "err")],
    )];
    let engine = FlowEngine::from_modules(&[module_with(flows)]);
    let mut store = store_with_order();
    let error = engine
        .call(&mut store, "App.Fails", Variables::new(), None)
        .unwrap_err();
    assert_eq!(error.to_string(), "deliberate");
    assert!(store.retrieve("App.Order").unwrap().is_empty());
}

#[test]
fn microflow_calls_map_parameters_and_bind_return_values() {
    let callee = flow(
        "Double",
        vec![start("s"), parameter("p", "Value"), end("e", "$Value * 2")],
        vec![edge("f1", "s", "e")],
    );
    let caller = flow(
        "Caller",
        vec![
            start("s"),
            activity(
                "call",
                doc! {
                    "$Type": "Microflows$MicroflowCallAction",
                    "UseReturnVariable": true,
                    "ResultVariableName": "doubled",
                    "MicroflowCall": doc! {
                        "Microflow": "App.Double",
                        "ParameterMappings": build_array(vec![
                            Bson::Document(doc! { "Parameter": "App.Double.Value", "Argument": "21" }),
                        ], 2),
                    },
                },
            ),
            end("e", "$doubled"),
        ],
        vec![edge("f1", "s", "call"), edge("f2", "call", "e")],
    );
    let engine = FlowEngine::from_modules(&[module_with(vec![callee, caller])]);
    let mut store = store_with_order();
    let (result, _) = call(&engine, &mut store, "App.Caller", Variables::new());
    assert_eq!(result, FlowValue::Int(42));
}

#[test]
fn effects_and_logs_are_collected_for_client_facing_activities() {
    let flows = vec![flow(
        "Notify",
        vec![
            start("s"),
            activity(
                "log",
                doc! {
                    "$Type": "Microflows$LogMessageAction",
                    "MessageTemplate": doc! {
                        "Text": "processed {1} orders",
                        "Parameters": build_array(vec![
                            Bson::Document(doc! { "Expression": doc! { "Value": "40 + 2" } }),
                        ], 2),
                    },
                },
            ),
            activity(
                "msg",
                doc! {
                    "$Type": "Microflows$ShowMessageAction",
                    "Type": "Warning",
                    "Blocking": true,
                    "Template": doc! { "Text": "heads up" },
                },
            ),
            activity(
                "nano",
                doc! {
                    "$Type": "Microflows$NanoflowCallAction",
                    "OutputVariableName": "ignored",
                    "NanoflowCall": doc! { "Nanoflow": "App.ClientThing" },
                },
            ),
            end("e", "''"),
        ],
        vec![
            edge("f1", "s", "log"),
            edge("f2", "log", "msg"),
            edge("f3", "msg", "nano"),
            edge("f4", "nano", "e"),
        ],
    )];
    let engine = FlowEngine::from_modules(&[module_with(flows)]);
    let mut store = store_with_order();
    let (_, execution) = call(&engine, &mut store, "App.Notify", Variables::new());
    assert_eq!(execution.log, ["processed 42 orders"]);
    assert_eq!(execution.effects.len(), 2);
    assert_eq!(execution.effects[0]["type"], "show_message");
    assert_eq!(execution.effects[0]["level"], "warning");
    assert_eq!(execution.effects[0]["message"], "heads up");
    assert_eq!(execution.effects[1]["type"], "nanoflow");
    assert_eq!(execution.effects[1]["name"], "App.ClientThing");
}

#[test]
fn java_actions_require_an_explicit_registration() {
    let flows = vec![flow(
        "Uses",
        vec![
            start("s"),
            activity(
                "j",
                doc! {
                    "$Type": "Microflows$JavaActionCallAction",
                    "JavaAction": "App.Hasher",
                    "ResultVariableName": "hashed",
                    "UseReturnVariable": true,
                    "ParameterMappings": build_array(vec![
                        Bson::Document(doc! {
                            "Parameter": "App.Hasher.Input",
                            "Value": doc! {
                                "$Type": "Microflows$BasicJavaActionParameterValue",
                                "Argument": "'abc'",
                            },
                        }),
                    ], 2),
                },
            ),
            end("e", "$hashed"),
        ],
        vec![edge("f1", "s", "j"), edge("f2", "j", "e")],
    )];
    let module = module_with(flows);
    let engine = FlowEngine::from_modules(std::slice::from_ref(&module));
    let mut store = store_with_order();
    let error = engine
        .call(&mut store, "App.Uses", Variables::new(), None)
        .unwrap_err();
    assert!(
        error
            .to_string()
            .starts_with("Java Custom Action App.Hasher is not registered"),
        "{error}"
    );

    struct Upper;
    impl JavaAction for Upper {
        fn call(&self, arguments: &BTreeMap<String, FlowValue>) -> Result<FlowValue, FlowError> {
            Ok(FlowValue::String(
                arguments["Input"].mendix_string().to_uppercase(),
            ))
        }
    }
    let engine = FlowEngine::from_modules(&[module]).with_java_action("App.Hasher", Upper);
    let (result, _) = call(&engine, &mut store, "App.Uses", Variables::new());
    assert_eq!(result, FlowValue::String("ABC".into()));
}

/// mxrb resolves `$ID` parameter references through the Java action's own
/// Parameters table. That catalogue is not exposed by mxrs-model yet, so a
/// mapping keyed by identifier is refused — keying the argument map by raw
/// uuid would hand the adapter a name it can never match, silently.
#[test]
fn a_java_action_parameter_mapped_by_identifier_is_refused_not_guessed() {
    let flows = vec![flow(
        "ById",
        vec![
            start("s"),
            activity(
                "j",
                doc! {
                    "$Type": "Microflows$JavaActionCallAction",
                    "JavaAction": "App.Hasher",
                    "ResultVariableName": "hashed",
                    "UseReturnVariable": true,
                    "ParameterMappings": build_array(vec![
                        Bson::Document(doc! {
                            "Parameter": "App.Hasher.8b1f5d2e-4c3a-4f7b-9d61-0a2e5c7b3f40",
                            "Value": doc! {
                                "$Type": "Microflows$BasicJavaActionParameterValue",
                                "Argument": "'abc'",
                            },
                        }),
                    ], 2),
                },
            ),
            end("e", "$hashed"),
        ],
        vec![edge("f1", "s", "j"), edge("f2", "j", "e")],
    )];

    struct Anything;
    impl JavaAction for Anything {
        fn call(&self, _: &BTreeMap<String, FlowValue>) -> Result<FlowValue, FlowError> {
            Ok(FlowValue::Empty)
        }
    }
    let engine =
        FlowEngine::from_modules(&[module_with(flows)]).with_java_action("App.Hasher", Anything);
    let mut store = store_with_order();
    let error = engine
        .call(&mut store, "App.ById", Variables::new(), None)
        .unwrap_err();
    assert!(
        error.to_string().contains(
            "maps a parameter by identifier; name resolution from the model is not ported yet"
        ),
        "{error}"
    );
}

/// The action dispatcher strips a trailing `Action` when it is present, so
/// both spellings of a type reach the same handler. Real `.mpr` files carry
/// both, and an unmatched type is a named error, not a skipped activity.
#[test]
fn action_types_dispatch_with_or_without_the_action_suffix() {
    let with_suffix = "Microflows$CreateVariableAction";
    let without_suffix = "Microflows$CreateVariable";
    for kind in [with_suffix, without_suffix] {
        let flows = vec![flow(
            "Make",
            vec![
                start("s"),
                activity(
                    "v",
                    doc! {
                        "$Type": kind,
                        "VariableName": "answer",
                        "InitialValue": "40 + 2",
                    },
                ),
                end("e", "$answer"),
            ],
            vec![edge("f1", "s", "v"), edge("f2", "v", "e")],
        )];
        let engine = FlowEngine::from_modules(&[module_with(flows)]);
        let mut store = store_with_order();
        let (result, _) = call(&engine, &mut store, "App.Make", Variables::new());
        assert_eq!(result, FlowValue::Int(42), "{kind}");
    }
}

/// An unset request mapping means "no body", not `"null"`: mxrb passes nil
/// to its HTTP layer, which omits the body entirely. Sending the four bytes
/// `null` changes what a server sees.
#[test]
fn a_rest_call_with_an_empty_request_mapping_sends_no_body() {
    #[derive(Default)]
    struct Log {
        bodies: std::sync::Mutex<Vec<Option<String>>>,
    }
    struct Recorder(std::sync::Arc<Log>);
    impl mxrs_runtime_flows::HttpCall for Recorder {
        fn call(
            &self,
            _method: &str,
            _location: &str,
            _headers: &[(String, String)],
            body: Option<&str>,
            _timeout: Option<f64>,
        ) -> Result<mxrs_runtime_flows::HttpResponse, FlowError> {
            self.0
                .bodies
                .lock()
                .unwrap()
                .push(body.map(ToString::to_string));
            Ok(mxrs_runtime_flows::HttpResponse {
                code: 200,
                body: "pong".to_string(),
            })
        }
    }

    let rest = activity(
        "r",
        doc! {
            "$Type": "Microflows$RestCallAction",
            "HttpConfiguration": doc! {
                "HttpMethod": "POST",
                "CustomLocationTemplate": doc! { "Text": "http://localhost/ping" },
            },
            "RequestHandling": doc! { "MappingVariableName": "payload" },
            "ResultHandlingType": "String",
            "ResultHandling": doc! {
                "Bind": true,
                "ResultVariableName": "answer",
                "VariableType": doc! { "$Type": "DataTypes$StringType" },
            },
        },
    );
    let flows = vec![flow(
        "Ping",
        vec![
            start("s"),
            parameter("p", "payload"),
            rest,
            end("e", "$answer"),
        ],
        vec![edge("f1", "s", "r"), edge("f2", "r", "e")],
    )];
    let log = std::sync::Arc::new(Log::default());
    let engine = FlowEngine::from_modules(&[module_with(flows)]).with_http(Recorder(log.clone()));
    let mut store = store_with_order();

    let mut empty = Variables::new();
    empty.insert("payload".into(), FlowValue::Empty);
    let (result, _) = call(&engine, &mut store, "App.Ping", empty);
    assert_eq!(result, FlowValue::String("pong".into()));

    let mut filled = Variables::new();
    filled.insert(
        "payload".into(),
        FlowValue::Json(serde_json::json!({ "ok": true })),
    );
    call(&engine, &mut store, "App.Ping", filled);

    let bodies = log.bodies.lock().unwrap().clone();
    assert_eq!(bodies, [None, Some("{\"ok\":true}".to_string())]);
}

#[test]
fn entity_access_enforcement_denies_by_policy_and_stays_off_otherwise() {
    let create = activity(
        "c",
        doc! {
            "$Type": "Microflows$CreateObjectAction",
            "Entity": "App.Order",
            "VariableName": "order",
        },
    );
    let open_flow = flow(
        "Open",
        vec![start("s"), create.clone(), end("e", "''")],
        vec![edge("f1", "s", "c"), edge("f2", "c", "e")],
    );
    let locked_flow = secured_flow(
        "Locked",
        vec![start("s"), create, end("e", "''")],
        vec![edge("f1", "s", "c"), edge("f2", "c", "e")],
    );
    let mut policy = SecurityPolicy {
        enabled: true,
        ..SecurityPolicy::default()
    };
    policy.entities.insert(
        "App.Order".to_string(),
        vec![EntityRule {
            module_roles: ["App.Reader".to_string()].into(),
            create: false,
            delete: false,
            default_member_right: Some(MemberRight::Read),
            member_rights: BTreeMap::new(),
            xpath: String::new(),
        }],
    );
    let engine =
        FlowEngine::from_modules(&[module_with(vec![open_flow, locked_flow])]).with_policy(policy);
    let mut store = store_with_order();
    let context = SecurityContext {
        user: Some("alice".to_string()),
        user_roles: Default::default(),
        module_roles: ["App.Reader".to_string()].into(),
        variables: BTreeMap::new(),
    };
    // ApplyEntityAccess=false: the flow runs even under a context.
    engine
        .call(
            &mut store,
            "App.Open",
            Variables::new(),
            Some(context.clone()),
        )
        .unwrap();
    // ApplyEntityAccess=true: create is denied by the policy.
    let denied = engine
        .call(&mut store, "App.Locked", Variables::new(), Some(context))
        .unwrap_err();
    assert!(
        matches!(
            denied,
            FlowError::Runtime(RuntimeError::NotAuthorized { .. })
        ),
        "{denied}"
    );
    // No context at all: mxrb only enforces when one is supplied.
    engine
        .call(&mut store, "App.Locked", Variables::new(), None)
        .unwrap();
}

/// mxrb's `filter_readable`: a retrieve under entity access returns only the
/// rows some applicable rule actually covers. Before the constraint was
/// evaluated, a role named by an owner-scoped rule saw every row — the rule
/// granted the entity instead of the rows it names.
#[test]
fn an_xpath_constrained_rule_filters_retrieved_rows_per_record() {
    let retrieve = activity(
        "r",
        doc! {
            "$Type": "Microflows$RetrieveAction",
            "ResultVariableName": "orders",
            "RetrieveSource": doc! {
                "$Type": "Microflows$DatabaseRetrieveSource",
                "Entity": "App.Order",
            },
        },
    );
    let flows = vec![secured_flow(
        "Mine",
        vec![start("s"), retrieve, end("e", "$orders")],
        vec![edge("f1", "s", "r"), edge("f2", "r", "e")],
    )];
    let mut policy = SecurityPolicy {
        enabled: true,
        ..SecurityPolicy::default()
    };
    policy.entities.insert(
        "App.Order".to_string(),
        vec![EntityRule {
            module_roles: ["App.Owner".to_string()].into(),
            create: true,
            delete: true,
            default_member_right: Some(MemberRight::Read),
            member_rights: BTreeMap::new(),
            xpath: "[Owner = '[%CurrentUser%]']".to_string(),
        }],
    );
    let engine = FlowEngine::from_modules(&[module_with(flows)]).with_policy(policy);
    let mut store = store_with_order();
    for owner in ["alice", "bob", "alice"] {
        let order = store.create("App.Order").unwrap();
        store
            .set_member("App.Order", &order.id, "Owner", owner.into())
            .unwrap();
        store.commit("App.Order", &order.id).unwrap();
    }
    let context = |user: &str| SecurityContext {
        user: Some(user.to_string()),
        user_roles: Default::default(),
        module_roles: ["App.Owner".to_string()].into(),
        variables: BTreeMap::new(),
    };

    let count = |engine: &FlowEngine, store: &mut Store, user: &str| {
        let (result, _) = engine
            .call(store, "App.Mine", Variables::new(), Some(context(user)))
            .unwrap_or_else(|error| panic!("{user}: {error}"));
        match result {
            FlowValue::List(values) => values.len(),
            other => panic!("expected a list, got {other:?}"),
        }
    };
    assert_eq!(count(&engine, &mut store, "alice"), 2);
    assert_eq!(count(&engine, &mut store, "bob"), 1);
    // A caller whose roles match but who owns nothing sees nothing, rather
    // than the whole table.
    assert_eq!(count(&engine, &mut store, "carol"), 0);
}

#[test]
fn lifecycle_hooks_fire_in_the_oracle_order_and_can_reject_commits() {
    let mut order = mxrs_model::entity::Entity::from_bson(&doc! {
        "$Type": "DomainModels$Entity",
        "Name": "Order",
    });
    order.qualified_name = Some("App.Order".to_string());
    order.lifecycle = vec![
        mxrs_model::entity::LifecycleCallback {
            id: None,
            event: "before_commit".to_string(),
            handler: "Stamp".to_string(),
            pass_event_object: true,
            raise_error_on_false: false,
            raw: doc! {},
        },
        mxrs_model::entity::LifecycleCallback {
            id: None,
            event: "before_create".to_string(),
            handler: "Gate".to_string(),
            pass_event_object: true,
            raise_error_on_false: true,
            raw: doc! {},
        },
    ];
    // Stamp writes an attribute on the committed object; Gate rejects when
    // Name is 'blocked'.
    let stamp = flow(
        "Stamp",
        vec![
            start("s"),
            parameter("p", "order"),
            activity(
                "c",
                doc! {
                    "$Type": "Microflows$ChangeObjectAction",
                    "ChangeVariableName": "order",
                    "Items": build_array(vec![
                        Bson::Document(doc! { "Attribute": "Stamped", "Value": doc! { "Value": "true" } }),
                    ], 2),
                },
            ),
            end("e", "true"),
        ],
        vec![edge("f1", "s", "c"), edge("f2", "c", "e")],
    );
    let gate = flow(
        "Gate",
        vec![
            start("s"),
            parameter("p", "order"),
            end("e", "$order/Name != 'blocked'"),
        ],
        vec![edge("f1", "s", "e")],
    );
    let create = |name: &str| {
        flow(
            name,
            vec![
                start("s"),
                parameter("p", "Name"),
                activity(
                    "c",
                    doc! {
                        "$Type": "Microflows$CreateObjectAction",
                        "Entity": "App.Order",
                        "VariableName": "order",
                        "Commit": "Yes",
                        "Items": build_array(vec![
                            Bson::Document(doc! { "Attribute": "Name", "Value": doc! { "Value": "$Name" } }),
                        ], 2),
                    },
                ),
                end("e", "$order/Stamped"),
            ],
            vec![edge("f1", "s", "c"), edge("f2", "c", "e")],
        )
    };
    let mut module = module_with(vec![stamp, gate, create("Make")]);
    module.domain_model = Some(mxrs_model::DomainModel {
        id: None,
        native_type: None,
        documentation: String::new(),
        entities: vec![order],
        associations: Vec::new(),
        cross_associations: Vec::new(),
    });
    let engine = FlowEngine::from_modules(std::slice::from_ref(&module));
    let mut store = store_with_order();
    let mut arguments = Variables::new();
    arguments.insert("Name".into(), FlowValue::String("fine".into()));
    // before_commit ran before the commit and its change is visible after.
    let (result, _) = call(&engine, &mut store, "App.Make", arguments);
    assert_eq!(result, FlowValue::Bool(true));
    // before_create rejecting rolls the whole unit of work back.
    let mut arguments = Variables::new();
    arguments.insert("Name".into(), FlowValue::String("blocked".into()));
    let error = engine
        .call(&mut store, "App.Make", arguments, None)
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        "entity lifecycle App.Gate rejected App.Order"
    );
    let survivors = store.retrieve("App.Order").unwrap();
    assert_eq!(survivors.len(), 1);
    assert_eq!(
        survivors[0].members["Name"],
        serde_json::Value::String("fine".into())
    );
}

#[test]
fn list_operations_and_aggregates_follow_the_oracle_table() {
    let flows = vec![flow(
        "Lists",
        vec![
            start("s"),
            parameter("p", "items"),
            activity(
                "op",
                doc! {
                    "$Type": "Microflows$ListOperationsAction",
                    "ResultVariableName": "first",
                    "NewOperation": doc! {
                        "$Type": "Microflows$Head",
                        "ListName": "items",
                    },
                },
            ),
            activity(
                "agg",
                doc! {
                    "$Type": "Microflows$AggregateListAction",
                    "AggregateVariableName": "items",
                    "AggregateFunction": "Sum",
                    "VariableName": "total",
                },
            ),
            end("e", "$total * 10 + $first"),
        ],
        vec![
            edge("f1", "s", "op"),
            edge("f2", "op", "agg"),
            edge("f3", "agg", "e"),
        ],
    )];
    let engine = FlowEngine::from_modules(&[module_with(flows)]);
    let mut store = store_with_order();
    let mut arguments = Variables::new();
    arguments.insert(
        "items".into(),
        FlowValue::List(vec![
            FlowValue::Int(2),
            FlowValue::Int(5),
            FlowValue::Int(9),
        ]),
    );
    let (result, _) = call(&engine, &mut store, "App.Lists", arguments);
    // sum(2,5,9) = 16 → 160, head = 2 → 162.
    assert_eq!(result, FlowValue::Int(162));
}
