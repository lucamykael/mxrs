//! Exercises `project! {}` through the real pipeline: macro expansion →
//! `mxrs_ir::ProjectDecl` → `mxrs_writer::write_project` → read back via
//! `mxrs_model::Project` — proving the macro's output is genuinely
//! equivalent to hand-written `mxrs-dsl` builder calls, not just that it
//! parses.

use mxrs_macros::project;
use mxrs_model::Project;

/// Hand-written marker types an association's target path resolves
/// against — same rationale as `mxrs-writer`'s test suite: these tests
/// don't need a manifest/build.rs, just something implementing
/// `EntityMarker`. `mxrs-typegen`'s own crate proves the codegen path.
#[allow(dead_code, non_snake_case, non_camel_case_types)]
mod markers {
    pub mod Sales {
        pub struct Customer;
        impl mxrs_ir::EntityMarker for Customer {
            const MODULE: &'static str = "Sales";
            const NAME: &'static str = "Customer";
        }
        pub struct ACT_Notify;
        impl mxrs_ir::MicroflowMarker for ACT_Notify {
            const MODULE: &'static str = "Sales";
            const NAME: &'static str = "ACT_Notify";
        }
        pub struct ACT_LogFailure;
        impl mxrs_ir::MicroflowMarker for ACT_LogFailure {
            const MODULE: &'static str = "Sales";
            const NAME: &'static str = "ACT_LogFailure";
        }
    }
    pub mod CRM {
        pub struct Account;
        impl mxrs_ir::EntityMarker for Account {
            const MODULE: &'static str = "CRM";
            const NAME: &'static str = "Account";
        }
    }
}

#[test]
fn expands_to_a_project_decl_writable_and_readable_like_hand_written_dsl() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("Macro.mpr");

    let definition = project! {
        "11.12.1",
        module Sales {
            entity Customer {
                string Name;
            }
            entity Order {
                string Number = "A-0000";
                decimal Total;
                association Order_Customer -> Sales::Customer as Reference;
            }
        }
    };

    mxrs_writer::write_project(&path, &definition).unwrap();

    let read = Project::open(&path, true).unwrap();
    let modules = read.modules().unwrap();
    assert_eq!(modules.len(), 1);
    let sales = &modules[0];
    assert_eq!(sales.name.as_deref(), Some("Sales"));

    let entities = sales.entities();
    assert_eq!(entities.len(), 2);
    let order = entities
        .iter()
        .find(|e| e.name.as_deref() == Some("Order"))
        .unwrap();
    let number = order
        .attributes
        .iter()
        .find(|a| a.name.as_deref() == Some("Number"))
        .unwrap();
    assert_eq!(number.default_value.as_deref(), Some("A-0000"));

    let associations = sales.associations();
    assert_eq!(associations.len(), 1);
    assert_eq!(associations[0].name.as_deref(), Some("Order_Customer"));
    assert!(!associations[0].is_cross_module());
    let customer = entities
        .iter()
        .find(|e| e.name.as_deref() == Some("Customer"))
        .unwrap();
    assert_eq!(
        associations[0].to_entity_id.as_deref(),
        customer.id.as_deref()
    );
}

#[test]
fn supports_every_attribute_kind_including_enumerations() {
    let definition = project! {
        "11.12.1",
        module Demo {
            entity Record {
                string StringValue;
                integer IntegerValue;
                long LongValue;
                float FloatValue;
                decimal DecimalValue;
                boolean BooleanValue;
                datetime DateTimeValue;
                autonumber AutoNumberValue;
                hash_string HashStringValue;
                binary BinaryValue;
                enumeration EnumValue("Demo.State") = "Open";
            }
        }
    };

    let attributes = &definition.modules[0].entities[0].attributes;
    assert_eq!(attributes.len(), 11);
    assert_eq!(attributes[3].attribute_type, mxrs_ir::AttributeType::Float);
    assert_eq!(
        attributes[8].attribute_type,
        mxrs_ir::AttributeType::HashString
    );
    assert_eq!(attributes[9].attribute_type, mxrs_ir::AttributeType::Binary);
    assert_eq!(
        attributes[10].attribute_type,
        mxrs_ir::AttributeType::Enumeration
    );
    assert_eq!(attributes[10].enumeration.as_deref(), Some("Demo.State"));
    assert_eq!(attributes[10].default_value.as_deref(), Some("Open"));
}

#[test]
fn supports_a_cross_module_association_target() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("CrossModule.mpr");

    let definition = project! {
        "11.12.1",
        module Sales {
            entity Order {
                association Order_Account -> CRM::Account as Reference;
            }
        },
        module CRM {
            entity Account {
                string Name;
            }
        }
    };

    mxrs_writer::write_project(&path, &definition).unwrap();

    let read = Project::open(&path, true).unwrap();
    let modules = read.modules().unwrap();
    let sales = modules
        .iter()
        .find(|m| m.name.as_deref() == Some("Sales"))
        .unwrap();
    let associations = sales.associations();
    assert_eq!(associations.len(), 1);
    assert!(associations[0].is_cross_module());
    assert_eq!(associations[0].to_entity_id.as_deref(), Some("CRM.Account"));
}

#[test]
fn supports_a_reference_set_association() {
    let definition = project! {
        "11.12.1",
        module Sales {
            entity Customer {}
            entity Order {
                association Order_Customers -> Sales::Customer as ReferenceSet;
            }
        }
    };
    let sales = &definition.modules[0];
    let order = sales.entities.iter().find(|e| e.name == "Order").unwrap();
    assert_eq!(
        order.associations[0].association_type,
        mxrs_ir::AssociationType::ReferenceSet
    );
    assert_eq!(order.associations[0].target, "Sales.Customer");
}

#[test]
fn self_hosted_markers_resolve_same_module_targets() {
    let definition = project! {
        "11.12.1",
        module Sales {
            entity Customer {}
            entity Order {
                association Order_Customer -> Sales::Customer as Reference;
            }
        }
    };
    let sales = &definition.modules[0];
    let order = sales.entities.iter().find(|e| e.name == "Order").unwrap();
    assert_eq!(order.associations[0].target, "Sales.Customer");
}

#[test]
fn supports_entity_documentation_and_persistable() {
    let definition = project! {
        "11.12.1",
        module Sales {
            entity Order {
                documentation "A customer order";
                persistable false;
                string Number;
            }
        }
    };
    let order = &definition.modules[0].entities[0];
    assert_eq!(order.documentation, "A customer order");
    assert!(!order.persistable);
}

#[test]
fn entity_structure_and_lifecycle_are_typed_and_round_trip() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("EntitySurface.mpr");
    let definition = project! {
        "11.12.1",
        module Sales {
            entity Order {
                persistable true;
                system_members {
                    owner true;
                    created_date true;
                }
                string Number;
                index {
                    attribute Number;
                    system CreatedDate ascending false;
                    include_offline true;
                }
                before_commit Sales::ACT_Audit {
                    pass_event_object false;
                    raise_error_on_false true;
                }
            }
            entity SpecialOrder {
                generalizes Sales::Order;
            }
            microflow ACT_Audit {
                return mxrs_expr::boolean(true);
            }
        }
    };

    mxrs_writer::write_project(&path, &definition).unwrap();
    let project = Project::open(&path, true).unwrap();
    let sales = project.modules().unwrap().remove(0);
    let order = sales
        .entities()
        .iter()
        .find(|entity| entity.name.as_deref() == Some("Order"))
        .unwrap();
    assert!(order.system_members.owner);
    assert!(order.system_members.created_date);
    assert_eq!(order.indexes.len(), 1);
    assert!(order.indexes[0].include_offline);
    assert_eq!(order.indexes[0].members.len(), 2);
    assert!(!order.indexes[0].members[1].ascending);
    assert_eq!(order.lifecycle.len(), 1);
    assert_eq!(order.lifecycle[0].event, "before_commit");
    assert_eq!(order.lifecycle[0].handler, "Sales.ACT_Audit");
    assert!(!order.lifecycle[0].pass_event_object);

    let special = sales
        .entities()
        .iter()
        .find(|entity| entity.name.as_deref() == Some("SpecialOrder"))
        .unwrap();
    assert_eq!(
        special.generalization_target().as_deref(),
        Some("Sales.Order")
    );
}

#[test]
fn entity_image_access_and_oql_view_are_typed_and_round_trip() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("EntityPresentation.mpr");
    let definition = project! {
        "11.12.1",
        module Sales {
            role User "Can read reports";
            oql_view_source OrderSource "SELECT Number FROM Sales.Order";
            entity Customer {}
            entity Order {
                image "Sales.OrderIcon";
                string Number;
                association Order_Customer -> Sales::Customer as Reference;
                access_rule ["User"] {
                    documentation "Visible orders";
                    default_rights None;
                    attribute Number ReadOnly;
                    association Order_Customer ReadWrite;
                    xpath "[Number != empty]";
                    xpath_caption "Orders with a number";
                }
            }
            entity OrderReport {
                oql_view Sales::OrderSource;
                string Number;
            }
        }
    };

    mxrs_writer::write_project(&path, &definition).unwrap();
    let project = Project::open(&path, true).unwrap();
    let sales = project.modules().unwrap().remove(0);
    let order = sales
        .entities()
        .iter()
        .find(|entity| entity.name.as_deref() == Some("Order"))
        .unwrap();
    assert_eq!(order.image.as_deref(), Some("Sales.OrderIcon"));
    assert_eq!(order.access_rules.len(), 1);
    assert_eq!(order.access_rules[0].roles, ["Sales.User"]);
    assert_eq!(order.access_rules[0].members.len(), 2);
    assert_eq!(
        order.access_rules[0].xpath_caption.as_deref(),
        Some("Orders with a number")
    );

    let report = sales
        .entities()
        .iter()
        .find(|entity| entity.name.as_deref() == Some("OrderReport"))
        .unwrap();
    assert!(report.oql_view());
    assert!(!report.persistable);
    assert_eq!(
        report.oql_source_document().as_deref(),
        Some("Sales.OrderSource")
    );
    assert_eq!(
        report.attributes[0]
            .raw_value_doc
            .as_ref()
            .unwrap()
            .get_str("$Type")
            .unwrap(),
        "DomainModels$OqlViewValue"
    );
    let source = sales
        .artifact_units
        .iter()
        .find(|document| {
            document.get_str("$Type").ok() == Some("DomainModels$ViewEntitySourceDocument")
        })
        .unwrap();
    assert_eq!(source.get_str("Name").unwrap(), "OrderSource");
    assert_eq!(
        source.get_str("Oql").unwrap(),
        "SELECT Number FROM Sales.Order"
    );
}

#[test]
fn supports_a_module_level_microflow_with_a_return_statement() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("Microflow.mpr");

    let definition = project! {
        "11.12.1",
        module Sales {
            entity Order {
                string Number;
            }
            microflow ACT_GetConstant {
                return mxrs_expr::integer(42);
            }
        }
    };

    mxrs_writer::write_project(&path, &definition).unwrap();

    let read = Project::open(&path, true).unwrap();
    let sales = read
        .modules()
        .unwrap()
        .into_iter()
        .find(|m| m.name.as_deref() == Some("Sales"))
        .unwrap();
    assert_eq!(sales.microflows.len(), 1);
    assert_eq!(sales.microflows[0].name.as_deref(), Some("ACT_GetConstant"));
}

#[test]
fn supports_a_module_level_nanoflow_with_a_stable_native_kind() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("Nanoflow.mpr");

    let definition = project! {
        "11.12.1",
        module Sales {
            nanoflow NF_GetConstant {
                return mxrs_expr::integer(42);
            }
        }
    };

    mxrs_writer::write_project(&path, &definition).unwrap();

    let read = Project::open(&path, true).unwrap();
    let sales = read
        .modules()
        .unwrap()
        .into_iter()
        .find(|m| m.name.as_deref() == Some("Sales"))
        .unwrap();
    assert!(sales.microflows.is_empty());
    assert_eq!(sales.nanoflows.len(), 1);
    assert_eq!(sales.nanoflows[0].name.as_deref(), Some("NF_GetConstant"));
}

#[test]
fn a_declared_microflow_is_available_as_a_self_hosted_call_marker() {
    let definition = project! {
        "11.12.1",
        module Sales {
            microflow ACT_Target {
                return mxrs_expr::integer(1);
            }
            microflow ACT_Caller {
                call Sales::ACT_Target;
            }
        }
    };

    assert_eq!(definition.modules[0].microflows.len(), 2);
}

#[test]
fn supports_the_full_widened_microflow_grammar() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("Microflow.mpr");

    let definition = project! {
        "11.12.1",
        module Sales {
            entity Order {
                string Number;
                decimal Total;
            }
            microflow ACT_Notify { parameter order_number: string; }
            microflow ACT_ProcessOrder {
                create order = Sales::Order {
                    Number = "A-1";
                } commit;
                change order {
                    Total = 100.0;
                };
                if order.total().gt(0.0) {
                    commit order;
                    call markers::Sales::ACT_Notify (order_number: order.number()) -> notified;
                } else {
                    delete order;
                }
                return order;
            }
        }
    };

    mxrs_writer::write_project(&path, &definition).unwrap();

    let read = Project::open(&path, true).unwrap();
    let sales = read
        .modules()
        .unwrap()
        .into_iter()
        .find(|m| m.name.as_deref() == Some("Sales"))
        .unwrap();
    assert_eq!(sales.microflows.len(), 2);
    let mf = sales
        .microflows
        .iter()
        .find(|flow| flow.name.as_deref() == Some("ACT_ProcessOrder"))
        .unwrap();
    assert_eq!(mf.name.as_deref(), Some("ACT_ProcessOrder"));

    let flow_types: Vec<String> = mf
        .objects
        .iter()
        .filter_map(|o| o.get_str("$Type").ok().map(String::from))
        .collect();
    assert!(flow_types.contains(&"Microflows$ActionActivity".to_string()));
    assert!(flow_types.contains(&"Microflows$ExclusiveSplit".to_string()));
}

#[test]
fn supports_an_association_member_in_a_create_statement() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("Microflow.mpr");

    let definition = project! {
        "11.12.1",
        module Sales {
            entity Customer {
                string Name;
            }
            entity Order {
                string Number;
                association ToCustomer -> Sales::Customer as Reference;
            }
            microflow ACT_CreateOrder {
                create customer = Sales::Customer {};
                create order = Sales::Order {
                    Number = "A-1";
                    assoc ToCustomer = customer;
                } commit;
                return order;
            }
        }
    };

    mxrs_writer::write_project(&path, &definition).unwrap();

    let read = Project::open(&path, true).unwrap();
    let sales = read
        .modules()
        .unwrap()
        .into_iter()
        .find(|m| m.name.as_deref() == Some("Sales"))
        .unwrap();
    assert_eq!(sales.microflows.len(), 1);
    assert!(!sales.microflows[0].objects.is_empty());
}

#[test]
fn supports_typed_loops_and_a_rescue_branch() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("ControlFlow.mpr");

    let definition = project! {
        "11.12.1",
        module Sales {
            entity Order { boolean Processed; }
            microflow ACT_LogFailure {}
            microflow ACT_ProcessSafely {
                create order = Sales::Order { Processed = false; };
                create_list orders = Sales::Order;
                for current in orders {
                    change current { Processed = true; };
                    break;
                }
                while mxrs_expr::boolean(false) {
                    continue;
                }
                commit order;
                rescue {
                    call markers::Sales::ACT_LogFailure;
                }
                return order;
            }
        }
    };

    mxrs_writer::write_project(&path, &definition).unwrap();
    let read = Project::open(&path, true).unwrap();
    let modules = read.modules().unwrap();
    let flow = modules[0]
        .microflows
        .iter()
        .find(|flow| flow.name.as_deref() == Some("ACT_ProcessSafely"))
        .unwrap();
    let loops: Vec<_> = flow
        .objects
        .iter()
        .filter(|object| object.get_str("$Type").ok() == Some("Microflows$LoopedActivity"))
        .collect();
    assert_eq!(loops.len(), 2);
    assert!(
        flow.flows
            .iter()
            .any(|edge| edge.get_bool("IsErrorHandler").ok() == Some(true))
    );
    let commit = flow
        .objects
        .iter()
        .find(|object| {
            object
                .get_document("Action")
                .and_then(|action| action.get_str("$Type"))
                .ok()
                == Some("Microflows$CommitAction")
        })
        .unwrap();
    assert_eq!(
        commit
            .get_document("Action")
            .unwrap()
            .get_str("ErrorHandlingType")
            .unwrap(),
        "CustomWithoutRollBack"
    );
}

#[test]
fn parameters_bind_typed_scalars_objects_and_lists_through_calls_and_returns() {
    use mxrs_bson::{Bson, parse_array};
    let definition = project! {
        "11.12.1",
        module Sales {
            entity Order { string Number; }
            microflow Target {
                parameter message: string { documentation "Text"; required true; default_value mxrs_expr::string("Hello"); }
                parameter order: object<Sales::Order>;
                parameter orders: list<Sales::Order>;
                change order { Number = message.clone(); };
                for current in orders { change current { Number = message.clone(); }; }
                if message.clone().eq("stop") { commit order; } else {}
                return orders;
            }
            microflow Caller {
                parameter message: string;
                parameter order: object<Sales::Order>;
                parameter orders: list<Sales::Order>;
                call Sales::Target (orders: &orders, message: &message, order: &order);
                rescue { call Sales::Target (message: message, order: order, orders: orders); }
            }
            nanoflow Client {
                parameter order: object<Sales::Order>;
                return order;
            }
        }
    };
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("Parameters.mpr");
    mxrs_writer::write_project(&path, &definition).unwrap();
    let project = Project::open(&path, true).unwrap();
    let modules = project.modules().unwrap();
    let target = modules[0]
        .microflows
        .iter()
        .find(|flow| flow.name.as_deref() == Some("Target"))
        .unwrap();
    assert_eq!(target.parameters.len(), 3);
    assert_eq!(
        target.parameters[0].get_str("Documentation").unwrap(),
        "Text"
    );
    assert_eq!(
        target
            .return_type_document
            .as_ref()
            .unwrap()
            .get_str("$Type")
            .unwrap(),
        "DataTypes$ListType"
    );
    let change = target
        .objects
        .iter()
        .filter_map(|o| o.get_document("Action").ok())
        .find(|action| action.get_str("$Type").ok() == Some("Microflows$ChangeAction"))
        .unwrap();
    let members = parse_array(change.get_array("Items").ok().map(Vec::as_slice));
    assert!(
        members
            .items
            .iter()
            .filter_map(Bson::as_document)
            .any(|member| member.get_str("Value").ok() == Some("$message"))
    );
    let caller = modules[0]
        .microflows
        .iter()
        .find(|flow| flow.name.as_deref() == Some("Caller"))
        .unwrap();
    let calls: Vec<_> = caller
        .objects
        .iter()
        .filter_map(|o| o.get_document("Action").ok())
        .filter(|a| a.get_str("$Type").ok() == Some("Microflows$MicroflowCallAction"))
        .collect();
    assert_eq!(calls.len(), 2);
    for call in calls {
        let mappings = parse_array(
            call.get_document("MicroflowCall")
                .unwrap()
                .get_array("ParameterMappings")
                .ok()
                .map(Vec::as_slice),
        );
        assert_eq!(mappings.items.len(), 3);
        assert!(
            mappings
                .items
                .iter()
                .filter_map(Bson::as_document)
                .any(
                    |m| m.get_str("Parameter").ok() == Some("Sales.Target.message")
                        && m.get_str("Argument").ok() == Some("$message")
                )
        );
    }
    assert_eq!(modules[0].nanoflows[0].parameters.len(), 1);
}
