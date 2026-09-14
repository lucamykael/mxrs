//! Exercises the BSON-side pipeline (`FlowCompiler`) against a project
//! built via `mxrs-dsl`, persisted via `mxrs-writer`, and read back via
//! `mxrs-model` — the same round trip `mxrs-compiler-domain`'s own
//! `tests/end_to_end.rs` establishes. `xtask oracle-diff` can't reach this
//! pipeline stage (it only round-trips editor-shape units — see this
//! crate's own doc comments), so structural assertion against a real
//! writer-produced fixture is the correctness ceiling available here.

use mxrs_bson::Document;
use mxrs_compiler_flow::FlowCompiler;
use mxrs_dsl::ProjectBuilder;
use mxrs_expr::{MxString, TypedAttributeMarker, attribute, string};
use mxrs_ir::{AttributeMarker, EntityMarker, Ref};
use mxrs_model::Project;

struct Order;
impl EntityMarker for Order {
    const MODULE: &'static str = "Sales";
    const NAME: &'static str = "Order";
}

struct OrderNumber;
impl AttributeMarker for OrderNumber {
    type Entity = Order;
    const NAME: &'static str = "Number";
}
impl TypedAttributeMarker for OrderNumber {
    type Value = MxString;
}

fn raw_microflow(project: &Project, name: &str) -> Document {
    project
        .all_units()
        .unwrap()
        .into_iter()
        .find_map(|unit| {
            let doc = project.mpr().parse_contents(&unit).ok()?;
            let is_microflow = doc.get_str("$Type").ok() == Some("Microflows$Microflow");
            let matches_name = doc.get_str("Name").ok() == Some(name);
            (is_microflow && matches_name).then_some(doc)
        })
        .unwrap_or_else(|| panic!("microflow {name:?} not found"))
}

#[test]
fn compiles_a_real_microflow_with_create_commit_decision_and_return() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("Project.mpr");

    let mut builder = ProjectBuilder::new("11.12.1");
    builder.module("Sales", |m| {
        m.entity("Order", |e| {
            e.string("Number");
        });
        m.microflow("ACT_CreateOrder", |f| {
            let order = f.create_object(
                "order",
                Ref::<Order>::new(),
                vec![attribute::<OrderNumber>(string("A-1"))],
                false,
            );
            f.decision(
                order.attribute::<OrderNumber>().ne(string("")),
                |t| {
                    t.commit(&order);
                },
                |_otherwise| {},
            );
            f.return_value(order);
        });
    });
    mxrs_writer::write_project(&path, &builder.build()).unwrap();

    let project = Project::open(&path, true).unwrap();
    let source = raw_microflow(&project, "ACT_CreateOrder");

    let compiler = FlowCompiler::new(&project, &[]).unwrap();
    let compiled = compiler.compile_flow(&source, "Sales").unwrap();

    assert_eq!(compiled.get_str("$Type").unwrap(), "Microflows$Microflow");
    assert_eq!(
        compiled.get_str("QualifiedName").unwrap(),
        "Sales.ACT_CreateOrder"
    );
    assert_eq!(compiled.get_str("ReturnType").unwrap(), "Sales.Order");

    let flows = compiled.get_array("SequenceFlows").unwrap();
    assert!(
        !flows.is_empty(),
        "the decision/commit graph should produce sequence flows"
    );

    let objects = compiled.get_array("ObjectCollection").ok();
    // ObjectCollection isn't itself an array field on the compiled root
    // (it's a sub-document); assert on its nested Objects instead.
    let _ = objects;
    let mxrs_bson::Bson::Document(object_collection) = compiled.get("ObjectCollection").unwrap()
    else {
        panic!("expected a document")
    };
    let compiled_objects = object_collection.get_array("Objects").unwrap();
    assert!(compiled_objects.iter().any(|o| {
        let mxrs_bson::Bson::Document(d) = o else {
            return false;
        };
        d.get_str("$Type").ok() == Some("Microflows$ExclusiveSplit")
    }));
}

#[test]
fn compiling_an_unsupported_root_type_is_a_loud_error() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("Project.mpr");
    let mut builder = ProjectBuilder::new("11.12.1");
    builder.module("Sales", |m| {
        m.entity("Order", |_e| {});
    });
    mxrs_writer::write_project(&path, &builder.build()).unwrap();

    let project = Project::open(&path, true).unwrap();
    let compiler = FlowCompiler::new(&project, &[]).unwrap();
    let bogus = mxrs_bson::doc! { "$ID": "x", "$Type": "Microflows$SomethingElse" };
    let err = compiler.compile_flow(&bogus, "Sales").unwrap_err();
    assert!(matches!(
        err,
        mxrs_compiler_flow::CompilerError::UnsupportedFlowRoot { .. }
    ));
}

#[test]
fn project_index_includes_native_system_associations_without_a_counterpart_package() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("Project.mpr");
    let mut builder = ProjectBuilder::new("11.12.1");
    builder.module("Sales", |module| {
        module.entity("Order", |_| {});
    });
    mxrs_writer::write_project(&path, &builder.build()).unwrap();

    let project = Project::open(&path, true).unwrap();
    assert!(
        project
            .modules()
            .unwrap()
            .iter()
            .all(|module| module.name.as_deref() != Some("System"))
    );
    let compiler = FlowCompiler::new(&project, &[]).unwrap();
    let roles = &compiler.index().associations["System.UserRoles"];
    assert_eq!(roles.parent.as_deref(), Some("System.User"));
    assert_eq!(roles.child.as_deref(), Some("System.UserRole"));
    assert!(roles.reference_set);
    assert!(
        !compiler
            .index()
            .associations
            .contains_key("Sales.MissingAssociation")
    );
}
