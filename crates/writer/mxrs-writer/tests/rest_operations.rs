//! A published REST operation is checked against the microflow it calls,
//! as Studio Pro checks it before deploying: what it would refuse, a build
//! says.

use mxrs_dsl::{ProjectBuilder, PublishedRestServiceBuilder};
use mxrs_expr::*;
use mxrs_ir::{RestOperationParameter, RestParameterType};

fn project(configure: impl FnOnce(&mut PublishedRestServiceBuilder)) -> mxrs_ir::ProjectDecl {
    let mut builder = ProjectBuilder::new("11.12.1");
    builder.module("Sales", |module| {
        module.microflow("MF_Order_Show", |flow| {
            flow.parameter::<MxString>("id", |_| {});
        });
        let mut service = PublishedRestServiceBuilder::new("OrdersApi", "api/v1");
        configure(&mut service);
        module.published_rest_service(service);
    });
    builder.build()
}

fn warnings(project: &mxrs_ir::ProjectDecl) -> Vec<String> {
    mxrs_writer::rest_operation_warnings(project, &[])
}

#[test]
fn an_operation_binding_its_microflow_is_written() {
    let project = project(|service| {
        service.resource("orders", |orders| {
            orders.get("{id}", "Sales.MF_Order_Show", |operation| {
                operation.path_parameter("id", RestParameterType::String);
            });
        });
    });
    assert_eq!(warnings(&project), Vec::<String>::new());
    // A model that says it so is written, whatever a build warns of.
    let directory = tempfile::tempdir().unwrap();
    mxrs_writer::write_project(directory.path().join("App.mpr"), &project).unwrap();
}

#[test]
fn an_operation_its_microflow_cannot_answer_is_warned_of() {
    for (configure, expected) in [
        (
            Box::new(|service: &mut PublishedRestServiceBuilder| {
                service.resource("orders", |orders| {
                    orders.get("", "Sales.MF_Missing", |_| {});
                });
            }) as Box<dyn FnOnce(&mut PublishedRestServiceBuilder)>,
            "calls \"Sales.MF_Missing\", which is no microflow of the project",
        ),
        (
            Box::new(|service: &mut PublishedRestServiceBuilder| {
                service.resource("orders", |orders| {
                    orders.get("{id}", "Sales.MF_Order_Show", |operation| {
                        operation.parameter(
                            RestOperationParameter::path("id", RestParameterType::String)
                                .bound_to("number"),
                        );
                    });
                });
            }),
            "binds its parameter \"id\" to \"number\", which Sales.MF_Order_Show does not take",
        ),
        (
            Box::new(|service: &mut PublishedRestServiceBuilder| {
                service.resource("orders", |orders| {
                    orders.get("{id}", "Sales.MF_Order_Show", |_| {});
                });
            }),
            "GET /api/v1/orders/{id} has {id} in its path and no path parameter of that name",
        ),
        (
            Box::new(|service: &mut PublishedRestServiceBuilder| {
                service.resource("orders", |orders| {
                    orders.get("", "Sales.MF_Order_Show", |operation| {
                        operation.path_parameter("id", RestParameterType::String);
                    });
                });
            }),
            "has a path parameter \"id\" its path does not hold",
        ),
        (
            Box::new(|service: &mut PublishedRestServiceBuilder| {
                service.resource("orders", |orders| {
                    orders.get("{id}", "Sales.MF_Order_Show", |operation| {
                        operation.path_parameter("id", RestParameterType::Integer);
                    });
                });
            }),
            "binds its parameter \"id\" to \"id\" of Sales.MF_Order_Show, which holds another type",
        ),
    ] {
        let warnings = warnings(&project(configure)).join("\n");
        assert!(warnings.contains(expected), "{expected}\n---\n{warnings}");
        assert!(warnings.contains("Sales.OrdersApi"), "{warnings}");
    }
}
